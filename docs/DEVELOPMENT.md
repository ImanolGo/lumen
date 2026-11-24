# Development guide

## What you need

Rust 1.92 or newer (hayro, the PDF reader, needs it). Nothing else: the app is pure Rust and needs no system libraries to
build, other than what iced's windowing and GPU backends find at run time (a Wayland or X11
session and a Vulkan or OpenGL driver; without a GPU it falls back to software drawing).

The CI image also installs `libxkbcommon-dev`, `libwayland-dev` and `pkg-config` for the windowing
crates. Optional AVIF and HEIC support (`--features heif`) needs the `libheif` development
files and its codecs.

## Everyday commands

```sh
cargo run --release -- ~/Pictures/Trip        # open a folder
cargo run --release -- report.pdf photo.jpg   # open files
RUST_LOG=info cargo run --release -- file.pdf # timing logs

make test      # cargo test --locked
make lint      # cargo fmt --check, and clippy with warnings as errors
make install PREFIX=$HOME/.local
```

CI runs the same checks and more (see below), so run `make test` and `make lint` before you
send a change. Check lint by its exit status: `cargo clippy --all-targets --locked -- -D warnings && echo OK`.

## Tests

| Suite | What it covers | How to run |
|---|---|---|
| Unit tests (`src/**`) | Models, geometry, PDF reading and writing, IPP encoding, decoders, trash, settings, i18n, and `src/app/tests.rs`: the app's message flows with no window | `cargo test` |
| `tests/sandbox.rs` | Every picture format through the real sandboxed worker, damaged files, and that opening files, sockets, programs and file writes are all stopped | `cargo test` (runs by default) |
| `tests/control.rs` | The real app, driven over D-Bus on a private bus: open, browse, go to page, zoom, search, passwords | `LUMEN_CONTROL_TESTS=1 cargo test --test control -- --test-threads=1` (opens real windows, so it is opt-in) |

`cargo test cups_live -- --ignored` asks the local CUPS server to validate a job (IPP
Validate-Job); it never prints anything.

Test files: `tests/fixtures/` holds small PDFs (a form, a password-protected file whose password is
`secret`, a five-page document with searchable text) and pictures in every format.
`tests/fixtures/pictures/*` were made with ImageMagick, `gradient.jxl` with `cjxl`, `gradient.jp2` with
`opj_compress` and `gradient.avif`/`.heic` with `heif-enc`.

## Continuous integration

`.github/workflows/ci.yml` runs on every push and pull request:

| Job | What it does |
|---|---|
| `check` | `cargo fmt --check`, clippy with warnings as errors, `cargo test` |
| `msrv` | `cargo check` with the Rust version in `Cargo.toml`'s `rust-version` |
| `build` | A release build, `--version` and `--help` as a smoke test, kept as an artifact for 14 days |
| `packaging` | Validates the desktop entry and the AppStream metadata, checks the Flatpak sources against `Cargo.lock`, builds the `.deb` and checks what is in it |
| `aur` | Checks that each `.SRCINFO` matches its PKGBUILD, in an Arch container |
| `audit` | Known security advisories for the dependencies (`cargo audit`) |
| `gui` | Runs the control tests against the real app in a headless Wayland session (software drawing); allowed to fail while it is new |

## Verifying changes to the interface

Unit and flow tests cover the logic, but how something looks and feels needs the real app. If
you cannot send real mouse or keyboard events (for example `/dev/uinput` is not available), do this:

1. Put the logic under unit tests, and the message flow under `src/app/tests.rs` (`App::new`, then `App::update` with the messages a click or key would send).
2. Temporarily add **demo hooks** in `App::new`, between `// DEMO-HOOK-START` and `// DEMO-HOOK-END`, that send messages on a timer, chosen by `LUMEN_DEMO=…`. Because markup, forms and crops are all driven by messages (`MarkupMsg::Press/Drag/Release`, `FormMsg`, `EditMsg`), a hook can draw and fill things in as a person would. **Remove them before committing**: `grep -c DEMO src/app.rs` must print 0.
3. Take a screenshot of the window. On niri:

   ```sh
   wl-paste -n > /tmp/clip.bak                      # keep the clipboard
   (env LUMEN_DEMO=x ./target/release/lumen-rs FILE &); sleep 3
   niri msg action screenshot-window -d false; sleep 0.5
   wl-paste -t image/png > shot.png
   pkill -x lumen-rs                                # exact name, never `pkill -f`
   wl-copy < /tmp/clip.bak
   ```

4. Say plainly what you could not check with real input.

Useful switches: `LUMEN_THEME=light|dark`, `LUMEN_NO_PORTAL=1` (the built-in file chooser and
print dialog), `LUMEN_NO_SANDBOX=1`, `LUMEN_CONTROL=1` (the D-Bus interface), `RUST_LOG=info`.

### Screenshots for the README

The screenshots in `data/screenshots/` are of sample files drawn by `tools/screenshots/make_samples.py`
(so they can be published), taken from the real app with scripted input on a 1200×800 window. The
scenes, the capture script and the steps to reproduce them are in
[`tools/screenshots/`](../tools/screenshots/README.md).

### The control interface

With `LUMEN_CONTROL=1`, Lumen offers `io.github.imanolgo.LumenRs.Control1` at
`/io/github/imanolgo/LumenRs/Control` on the session bus. It is off by default, because anything
on the bus could otherwise open files in your viewer.

| Method | Does |
|---|---|
| `GetState()` | JSON: `kind` (none, loading, picture, pdf, locked, failed), `file`, `index`, `count`, `page`, `pages`, `zoom`, `night`, `unsaved`, `search`, `busy` |
| `Open(paths, password)` | Opens files or folders; poll `GetState` until `busy` is false |
| `Next()`, `Previous()`, `First()`, `Last()` | Move among the files |
| `GotoPage(page)` | Pages count from 1 |
| `Zoom(how)` | `in`, `out`, `fit`, `width`, `actual` |
| `Search(text)` | Opens the search and runs it |
| `Quit()` | Exits |

Every method answers with the state afterwards, or an error that says why.

## Benchmarks

`tools/bench.py [runs]` starts the GTK Lumen and lumen-rs cold on the same files (from
`~/.local/bin`), reads their timing logs, samples memory and CPU, and prints JSON. Set
`BENCH_JPEG=/path/to/photo.jpg` to include a picture. Results are in [STATUS.md](STATUS.md).

## Translations

English is the source. Wrap every visible string in `tr("…")` (or `trf!("… {} …", arg)` when it has
values to fill in), then add its Spanish to `po/es.po`; `cargo test` fails with the list of strings
that have no entry. To add a language, add `po/<code>.po` and one line to `CATALOGUES` in
`src/i18n.rs`. The `.po` files are embedded in the binary; there is no `msgfmt` step.

## Adding a picture format

1. Add its extensions to `RASTER` in `src/library.rs`, and its MIME types to `data/*.desktop` and the filter in `src/portal.rs`.
2. Decode it from bytes in `src/raster.rs` (`special()` and `decode()`), so it also runs inside the sandbox.
3. Add a small fixture to `tests/fixtures/` and a test in `src/raster.rs`; `tests/sandbox.rs` then picks it up from `tests/fixtures/pictures/`.
4. If it needs a new system call, the sandbox kills the worker: run the new test through the sandbox (`cargo test --test sandbox`) and add the call to `allowed_syscalls()` in `src/sandbox.rs` only if the decoder really needs it. `gdb -batch` with `catch syscall` on `--decode-worker load EXT --unsandboxed` lists what a decoder uses.

## Icons

The app icon is in `data/icons/` (an SVG and PNGs from 16 to 512 px). The toolbar icons are inline
SVG in `src/icons.rs`; they are 24×24, stroked, and take the theme's text colour.

## Printing

Do not send a real print job from a test or a demo. The live CUPS test only validates.
