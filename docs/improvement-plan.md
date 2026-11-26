# Plan: fixes, macOS and Windows, interface, features

Written 24 November 2026 against commit `f9edcd8` (version 0.1.0). It follows
[parity-plan.md](parity-plan.md), which is finished. Read `CLAUDE.md` first: its rules and its
way of verifying interface changes still hold.

The plan has four parts, to be done in this order:

1. **Fix what is wrong** (packages 1–4): defects found by reading the code.
2. **Run on macOS and Windows** (packages 5–11).
3. **Interface** (packages 12–17): one look that fits all three systems.
4. **Features** (package 18): what a viewer and editor like this still lacks, and what macOS Preview has.

Part 3b (packages 20 to 23, agreed on 25 November 2026) rethinks what opens and where the tools are; it comes **after package 17 and before package 18**, whatever the numbers say.

A fifth part (package 19) confines the decoder on macOS and Windows, after the first builds there exist.

Part 2 comes before part 3 because the interface work depends on how each system draws its
window and names its keys.

## How to work through it

- One package at a time, in order. Each ends with `make test`, `make lint` (by exit status), the
  docs updated (`FEATURES.md`, `ARCHITECTURE.md`, `CHANGELOG.md`), a commit on `main`, a push, a
  green CI run (`gh run list -L1`) and `make install PREFIX=$HOME/.local`.
- Tick the [status table](#status) in the same commit.
- Every fix gets a test that fails before it and passes after. Where a finding below says
  **suspected**, first write the test that shows it; if it cannot be reproduced, note that in
  the status table and move on.
- A change to `Cargo.lock` needs `python3 tools/update-flatpak-sources.py` (CI checks it), and new
  crates must build with the Rust version in `Cargo.toml` (`rust-version`, the `msrv` job).
- Line numbers are from `f9edcd8`; search for the function name if they have moved.

- **A release before each part** (asked for on 25 November 2026): once a part is done and CI is green,
  release it as in [RELEASING.md](RELEASING.md) before starting the next one. Also whenever the
  user confirms by hand that a batch of fixes works: that commit, not later untried work.

## Where things stand

- Linux only. `make test` passes (182 unit tests, 3 sandbox tests; the 10 control tests are
  skipped without `LUMEN_CONTROL_TESTS=1`), and `make lint` is clean.
- **What stops a Windows or macOS build today**, measured with
  `cargo check --locked --keep-going --target <triple>` from Linux:

  | Target | Dependencies that do not compile | Everything else |
  |---|---|---|
  | `x86_64-pc-windows-msvc` | `landlock`, `seccompiler`, `ashpd`, `wayland-sys` (through `wl-clipboard-rs`) | compiles, including iced, wgpu, hayro, lopdf, zbus and the vendored winit |
  | `aarch64-apple-darwin` | `landlock`, `seccompiler` | compiles |

  Lumen's own code was not reached (cargo stops at the failed dependencies). The places that
  will not compile are listed in [package 5](#5-a-platform-layer-linux-only-no-change-in-behaviour).
- How this was reviewed: the picture, sandbox, desktop and PDF modules were read in full; the
  interface layer (`src/app.rs`, `src/app/*`) only in parts (the full review of it did not
  finish). Package 4 ends with reading the rest.

## Decisions to ask the user about first

Ask these in one `AskUserQuestion` call before starting part 2. Part 1 needs none of them.

1. **System crates on macOS and Windows.** File dialogs, the trash, the clipboard and file
   watching need each system's own API. The usual crates (`rfd`, `trash`, `arboard`, `notify`)
   are Rust, with no C library, but they call the system through `unsafe` bindings of their own.
   Lumen itself stays `#![forbid(unsafe_code)]`. Recommended: allow them, on macOS and Windows only.
2. **No sandbox on macOS and Windows at first.** Landlock and seccomp are Linux only.
   Recommended: ship the first builds there with the separate decoder process but without
   confinement (a crash or a hang still costs only one picture), say so plainly in Preferences
   and the docs, and add confinement afterwards as [package 19](#19-confine-the-decoder-on-macos-and-windows).
   That package has a decision of its own: on Windows it needs a few lines of `unsafe`, kept
   in a small separate crate.
3. **The window frame.** Recommended, one per system:
   macOS: Lumen's header merged into the title bar, with the system's traffic lights;
   Windows: the system title bar, with Lumen's header under it as a toolbar;
   Linux: no system frame, Lumen's header is the title bar (drag, double-click, close), which
   also removes the second title bar GNOME and KDE draw today. Details in [package 12](#12-window-frame-per-system).
4. **`vendor/` may need one more patch.** On macOS a file opened from Finder reaches a running
   app as an event, not as an argument, and the vendored winit 0.30.13 has no such event (no
   `openURLs` in `vendor/winit/src/platform_impl/macos`). Without it "Open With → Lumen" opens an
   empty window. `CLAUDE.md` says not to edit `vendor/` for anything but pinch, so ask.
5. **Signing.** Unsigned builds work but macOS Gatekeeper and Windows SmartScreen warn. Signing
   needs an Apple Developer account (99 USD a year) and a Windows certificate. Recommended: ship
   unsigned first and document how to open it.

**Answered on 25 November 2026:** 1 allowed (on macOS and Windows only); 2 and 5 the first builds are
unconfined and unsigned, and say so; 3 one frame per system, as above; 4 the patch is allowed.

---

# Part 1: fix what is wrong

Confidence tags: **confirmed** means the path was traced in the code (and in the dependency
where it matters); **suspected** means one step was not checked, and says which.

## 1. The decoder must fail closed and stay within limits

The sandbox's promise is that a hostile picture never runs outside it. Four paths break that.

| # | Where | What is wrong | Fix |
|---|---|---|---|
| 1.1 | `src/sandbox.rs:157-166` (`run_worker`) | **Fails open.** A decoder that panics exits with status 101, which is read as "sandbox unavailable", so `run()` decodes the same file in the app. The same for a cut-off reply with exit 0. Confirmed. | Only `EXIT_NO_SANDBOX` means unavailable; every other status is `Failure::Failed`. Add a `selftest-panic` worker mode and a test in `tests/sandbox.rs` that it yields an error and no in-process decode. |
| 1.2 | `src/animation.rs:114-133`, started from `src/app.rs:2035` | **Animations are decoded in the app**, with `Limits::no_limits()`: every frame after the first of a GIF, APNG or WebP is parsed unsandboxed, and a GIF with a 16 000 × 16 000 canvas takes several GB (3 frames queued, each 1 GB). Confirmed. | Short term: set the same limits as `raster.rs`, and do not animate when one frame × 5 is over the 96 MB budget (show the first frame, say so in a notice). Then move the frame decoder into the worker (`--decode-worker frames`, frames streamed over the pipe with the existing length-prefixed framing). |
| 1.3 | `src/raster.rs:290-294` (`decode`) | **The 1 GiB limit does not cover the pixel buffer.** `into_decoder()` + `DynamicImage::from_decoder()` skips the check that `ImageReader::decode()` makes (image 0.25.10), so a small 50 000 × 50 000 PNG allocates about 10 GB. Confirmed. JPEG XL and JPEG 2000 working memory (`:191`, `:225`): suspected, their internals were not read. | Check `decoder.total_bytes()` against the limit before `from_decoder`. Add a test with a generated huge-header PNG. For the worker, set an address-space limit if a safe API allows it (`rustix::process::setrlimit` needs the `process` feature). |
| 1.4 | `src/raster.rs:299-310` (`svg_options`) | **An SVG can read other files.** With `resources_dir: None` usvg still loads `<image href="/absolute/path">` (usvg 0.50 `parser/image.rs:86-114`), in the app process. The comment says it cannot. Confirmed. | Set `image_href_resolver` so that only `data:` URLs are decoded; test with an SVG that points at a fixture. |
| 1.5 | `src/sandbox.rs:94`, `src/main.rs:72` | After a package upgrade replaces the binary, `current_exe()` points at a deleted file, the worker cannot start and every picture is decoded unsandboxed for the rest of the session. Confirmed in our code; suspected on what std returns. | On Linux spawn `/proc/self/exe`. |
| 1.6 | `src/sandbox.rs:125-138` | When the parent rejects a reply (size over `MAX_BYTES`) it waits for the worker while the worker is blocked writing to the pipe: the print job never returns. Reachable by printing a picture over about 335 MP. Confirmed. | Kill the child (or drop `stdout`) before `wait()` on every error path. |
| 1.7 | `src/sandbox.rs:300-343` (`allowed_syscalls`) | The allow-list is wider than its comments say: `clone`/`clone3` unrestricted (new processes are possible, and a grandchild holding the pipe hangs the parent), `tgkill` to any process, `read` on any descriptor, `newfstatat` on any path, `prctl` unrestricted; `--unsandboxed` (`:238`) is honoured in release builds. Confirmed by reading; exploiting it needs code execution in the worker. | Add argument filters (`clone` with `CLONE_THREAD` only and refuse `clone3` with `ENOSYS` so libc falls back; `tgkill` to the worker's own pid; `read` on fd 0), try dropping `newfstatat` and `prctl` and run `cargo test --test sandbox` for every format; compile `--unsandboxed` only with `cfg(debug_assertions)`. |
| 1.8 | `src/pdf.rs:373` | `.max(0.01)` defeats the `MAX_SIDE` cap: a page with `/MediaBox [0 0 6553500 6553500]` renders at 65 535 × 65 535 px (17 GB) and the allocation failure aborts the app. Confirmed. | After the clamp, refuse to render when either side is still over `MAX_SIDE` (show the page as an error placeholder). |
| 1.9 | `src/annotate/pdf.rs:35-36, 88-91, 164-174`; `src/forms.rs:197-215, 254-256, 292` | Per-item caps do not bound memory when many items share one big array: 5 000 ink annotations pointing at one 100 000-number stroke, or 5 000 fields sharing one 10 000-entry `/Opt`, take GB. Forms are read on every open. Confirmed. | A budget for the whole page (points) and the whole form (options), counted across items; stop reading and add a notice when it runs out. |
| 1.10 | `src/main.rs:81` | `std::env::args()` panics on an argument that is not UTF-8 (a Latin-1 file name), before anything else. Confirmed. | Use `args_os()` for the `--decode-worker` check, as line 94 does. |

**Done when:** each row has a test; `tests/sandbox.rs` covers the panic and the oversized-reply
cases; the animation and the over-limit fixtures are generated in the tests, not committed.

## 2. Saving must not lose or expose data

| # | Where | What is wrong | Fix |
|---|---|---|---|
| 2.1 | `src/pdf.rs:233` (`with_changes`), `src/app.rs:2494-2498` | **Saving after another program rewrote the PDF corrupts it.** The toast says "saving will overwrite it", but the save re-reads the file from disk and applies object ids from the version that was opened: annotations with those ids are removed and form values are written into whatever objects now have them. Confirmed. | Apply changes to the bytes the document was opened from (keep them in `pdf::Document`), never to a fresh read. When the file changed on disk, Ctrl+S asks: overwrite with my version, or save a copy. |
| 2.2 | `src/app/markup.rs:179-183, 975-978` (`print_copy`) | **A decrypted copy of a protected PDF stays in `/tmp`.** It is written to `/tmp/lumen-<pid>/<name>` with default permissions and never deleted. Confirmed (no `remove_file` for it anywhere). | Make it like `print.rs:193-207`: a private directory (0700), `create_new`, deleted in `Message::Printed` and on exit. |
| 2.3 | `src/pdf.rs:424-442` (`write_atomic`), `src/annotate/flatten.rs:98-157` | Two copies of the atomic write, both weak. Pictures are **not fsynced** before the rename (a power cut after "Saved" can leave a short file where the original was). Both use one fixed temp name (two saves collide), follow a symlink planted at it, replace a symlinked target with a regular file, and set permissions only after writing. Confirmed. A failed save never truncates the original; that part is sound. | One `fsio::write_atomic` used by both: resolve a symlinked target first, temp file with a random suffix and `create_new`, permissions at creation, `sync_all`, rename, then sync the directory (Unix). |
| 2.4 | `src/pdf.rs:225-229`, `src/app/pages.rs:406-408` | The file is replaced before the new bytes are checked to open again. Suspected failing case: lopdf's recovery of a broken xref leaves out `/Prev`. | Parse the new bytes with hayro first; write only if that works. |
| 2.5 | `Message::Trash` (`src/app.rs:1515-1560`) | Moving a file with unsaved markup or edits to the trash drops them (`stash.remove`) without asking, and Undo does not bring them back. Confirmed. | Ask first when that file has unsaved changes; keep the stash until the Undo toast has gone. |
| 2.6 | `src/signatures.rs:82-116` | A `signatures.toml` that does not parse loads as empty and the next change writes that back, deleting every signature; two windows (or the GTK Lumen) overwrite each other. Confirmed. | On a parse error keep the file (rename to `.bak`) and tell the user; read and merge before every write. |
| 2.7 | `src/state.rs:45-82`, `src/settings.rs:156-164` | One recent file with a non-UTF-8 name empties the whole state file (`to_toml` returns `""`). Entries on a disk that is not mounted are dropped, with their remembered page. Instances overwrite each other. Confirmed. | Skip what cannot be written instead of writing nothing; keep unreachable entries (hide them in the list only); merge with the file on disk before saving; unique temp name. |
| 2.8 | `src/metadata.rs:81-84` | The source's ICC profile is attached to the saved RGB copy whatever its colour space (CMYK, grey), so colour-managed viewers show it wrong. Confirmed by reading. | Carry the profile over only when its colour-space field (bytes 16..20) is `RGB `. |
| 2.9 | `src/pages.rs:167` (`compose`) | **Deleted pages stay in the file** when the outline, links, name tables or the structure tree still point at them: their content can be recovered from the saved or exported PDF. Confirmed. | In `compose`, drop outline entries, named destinations, link annotations and structure elements that target removed pages, then prune. Test: export page 1 of `tests/fixtures/pages.pdf` and check page 3's text is not in the bytes. |

**Done when:** a test for each; the atomic-write tests cover a symlinked target and two saves at once.

## 3. PDF writing: keep what other programs wrote

| # | Where | What is wrong | Fix |
|---|---|---|---|
| 3.1 | `src/annotate/pdf.rs:300-336, 390-395` | Saving a page rewrites **every** annotation on it from Lumen's simpler model, not only those the user changed: other programs' annotations lose flags, dates, dashed and cloud borders, line endings, callouts, popups and their original appearance. Confirmed. | Track which imported annotations were edited; leave the others' objects untouched. For edited ones, start from the original dictionary and change only what Lumen models. |
| 3.2 | `src/forms.rs:384-422` | The text-field appearance ignores `/MK`: on a rotated page the text comes out sideways in other viewers, and the field's border and background vanish once a value is saved. Confirmed by reading. | Honour `/MK /R` (appearance matrix), `/BG` and `/BC`. |
| 3.3 | `src/annotate/pdf.rs:572-592`, `src/forms.rs:404` | Text outside a partial WinAnsi table is written as `?` in appearances ("Škoda", "œuvre", Cyrillic, CJK). The values themselves are right. Confirmed. | Complete the WinAnsi table; for text beyond it, embed a font subset (pure Rust: `subsetter` + a bundled font) or, as a first step, set `/NeedAppearances true` and write no appearance. |
| 3.4 | `src/forms.rs:220, 304-305, 343, 355, 365` | A check box whose on-state name is not UTF-8 (`/S#ED`) is saved as `/{non-ascii key}`, a state that does not exist. Confirmed. | Keep state names as bytes. |
| 3.5 | `src/forms.rs:390-408` | Multi-line fields are not word-wrapped in the saved appearance; comb fields are not handled; widths assume half an em per character. Confirmed. | Wrap with the standard font's real widths (the AFM table of Helvetica); handle comb. |
| 3.6 | `src/pages.rs:114-122, 141-165` | Inserting another PDF drops its `/AcroForm` (its fields can no longer be filled), its named destinations and its outline. Confirmed. | Merge `/Fields`, `/DR` and `/DA`; carry the names the inserted pages use. |
| 3.7 | `src/annotate/pdf.rs:68`, `src/structure.rs:106, 249`, `src/forms.rs:191, 263` | A `null` or dangling entry in `/Annots` or `/Fields` ends the reading: every later link, annotation and field is ignored. Confirmed. | Iterate with `raw_iter` and skip what is not a dictionary, as `forms.rs:175` does. |
| 3.8 | `src/pages.rs:97-103` | `/Rotate 90.0` (a real number) reads as 0, so reorganising resets the page's turn. Confirmed. | Accept numbers of either kind. |
| 3.9 | `src/structure.rs:183-186` | Outline entries and links that target a turned page land at its top or bottom. Confirmed by the maths, not run. | Transform the destination point with the page's matrix. |
| 3.10 | `src/export.rs:46-50` | Exporting a large page as PNG at 300 dpi is silently clamped to 8192 px but still says 300 dpi in the file. Confirmed. | Write the resolution actually used and say so in the notice. |
| 3.11 | `src/print.rs:143` | Export as PDF crops small pictures (a 1000 × 50 px banner loses its top quarter) because of a 36 pt minimum box. Confirmed by arithmetic. | Apply the minimum to the page, not the box, or drop it for export. |

**Done when:** round-trip tests for 3.1 (a fixture with a foreign annotation, made in the test
with `pdf-writer`), 3.4, 3.7 and 3.8; `tests/control.rs` still passes.

## 4. Smaller defects and things that disagree

Pictures and files:

- `src/raster.rs:425-446`: embedded EXIF previews are **upscaled** to 256 px and stored in the shared thumbnail cache with the preview's size as the picture's size; small pictures (`:92`) are upscaled too. Never enlarge; record the real size.
- `src/raster.rs:370-423`: the streamed PNG thumbnail ignores the PNG's EXIF orientation; the full view applies it.
- `src/library.rs:64-76`: paths are canonicalised before their kind is read, so a symlink whose target has no picture extension (git-annex) does not open, and otherwise the target's folder is browsed; the trash (`src/trash.rs:24`) then moves the target and leaves the link. Keep the path as given (make it absolute only).
- `src/watch.rs:39-43`: deleting or renaming the open file is not noticed (no `DELETE`, `MOVED_FROM`, `DELETE_SELF`); a queue overflow is ignored; `CREATE` triggers a reload of a half-written file.
- `src/lossless.rs:83`: a JPEG whose EXIF has no orientation tag is re-encoded at quality 92 without a word. Add the tag, or say that it was re-encoded.
- `src/animation.rs:103`, `src/app/anim.rs:84-93`: the last frame of each pass shows for twice its time; the loop count is never read.
- `src/trash.rs`: shells out to `date` (`:131-136`) — compute local time in Rust; `$topdir/.Trash/$uid` is never tried; restore is check-then-rename (`:104-110`).
- `src/thumbcache.rs:47-48`: the modification time is read when the thumbnail is written, not when the picture was decoded.
- `src/raster.rs:317-322`: a very large SVG loses its proportions (each side clamped alone) and `pw * ph * 4` overflows `u32`.

PDF:

- `src/pdf.rs:348-352`: the render caches (hayro's glyph cache has no eviction) grow with every zoom level, per thread. Mechanism confirmed, size not measured: measure by zooming through 30 levels of the 477-page manual, then reset the caches past a budget.
- `src/pages.rs:118, 167`: lopdf's renumbering and pruning are quadratic; a PDF with a few hundred thousand objects stalls on "Saving…". Measure first.
- `src/structure.rs:422-678`: search does not normalise text (ligatures such as U+FB01, decomposed accents, words hyphenated across lines). Suspected on how often hayro returns ligature characters.
- `src/properties.rs:59, 111-120`: Properties parses the whole file with lopdf only to learn if it is encrypted, and decodes strings differently from `structure::decode_text` (quotes and dashes in titles vanish).
- `src/cups.rs:271-277`: a chunk size near `usize::MAX` panics the reader (caught, low severity); `CUPS_SERVER` is ignored.

Text and translations:

- `src/i18n.rs:54-72`: the order is not gettext's. `LANGUAGE=en LANG=es_ES.UTF-8` gives Spanish; it must give English. `LANGUAGE` first, then the first set of `LC_ALL`, `LC_MESSAGES`, `LANG` decides alone.
- Strings that never reach `tr()`: the portal dialogs (`src/portal.rs:35-36, 41, 62, 82`), the quit dialog (`src/app/markup.rs:1258`), the password prompt (`src/app.rs:3517`), "already exists" (`src/app/chooser.rs:583`), "of N" (`src/app/pagelist.rs:179`), "pixels" (`src/app.rs:2986`). The guard test only sees `tr(`/`trf!(`; extend it to flag a `text(format!(…))` or a string literal with words passed to `text(`.
- The portal's file filter omits `.svgz` and `.j2k`, which `library::Kind` accepts.

Docs and repository:

- `data/icons/OME/.local/share/icons/hicolor/…` (nine tracked files) is a copy of the icons made by a mistyped `$HOME`. Delete it.
- `docs/STATUS.md` says "It has not been released" and "the release workflow … never been run", but `v0.1.0` is tagged and published on GitHub. `docs/RELEASING.md` says the same of the workflow.
- The in-app shortcuts list (`src/app/menu.rs:47`) says "Ctrl+S: Save (a copy of a picture)"; it saves JPEG, PNG and WebP in place.
- `docs/ARCHITECTURE.md` lists `tools/check-flatpak-sources.py` twice.
- `CLAUDE.md` asks for a `Co-Authored-By` line naming one model; leave that to the user.

**Finish the review.** The interface layer was only read in parts. Read `src/app.rs` and
`src/app/*.rs` in full, looking for: results of background work applied after the user moved on
(most carry `generation`; check `Thumb`, `Preloaded`, `PropertiesRead`, `Printed`, the export
and save results), ways past the unsaved-changes question (open, drop, reload, slideshow),
shortcuts that fire while a text box has focus (`Escape` and `Delete` are taken in
`event::listen_with` at `src/app.rs:865-877` whatever has focus), and per-file maps that are never
trimmed (`thumbs`, `stash*`, `passwords`). Add what is found to this package.

**Done when:** each item is fixed with a test, or measured and written down as not worth fixing.

---

# Part 2: macOS and Windows

The aim: `cargo build --release` works on Linux, macOS and Windows from the same source; each
system gets its own file dialogs, trash, clipboard and file watching; CI builds and tests all
three; a release publishes a package for each.

## 5. A platform layer (Linux only, no change in behaviour)

Everything that talks to the system moves behind one module, so that the rest of the code
never names a system. Do this on Linux alone first; nothing the user sees may change.

Create `src/platform/` with `mod.rs` choosing an implementation by `cfg(target_os)`:

| Module | Functions (the same on every system) | Linux code that moves in |
|---|---|---|
| `platform::dirs` | `config()`, `state()`, `data()`, `cache()`, `home()`, `places()` (the file chooser's shortcuts) | the XDG lookups repeated in `settings.rs:85`, `state.rs:37`, `signatures.rs:74`, `thumbcache.rs:19`, `trash.rs:39`, `app/chooser.rs:159, 284`, `app.rs:2736` |
| `platform::dialogs` | `choose(folder)`, `save_as(suggested)` | `portal.rs:30, 76` |
| `platform::shell` | `open_uri`, `show_in_folder`, `open_with` | `portal.rs:101-180` |
| `platform::clipboard` | `copy_png` | `clipboard.rs:25-70` (keep `encode_png` where it is) |
| `platform::watch` | `changes(path)` | `watch.rs` |
| `platform::trash` | `trash(path)`, `restore(item)` | `trash.rs` |
| `platform::print` | `print(path)`, `printers()`, `print_direct(…)` | `print.rs:44-100`, `cups.rs` connection (`:218`) |
| `platform::confine` | `enter()` (called by the worker), `explain(status)` | `sandbox.rs:283-365` and the signal reading at `:142-152` |
| `platform::startup` | `before_window()` | `main.rs:67-76` (the re-exec for the integrated GPU) |
| `platform::thumbs` | `shared_cache()` | `thumbcache.rs:18-25` and its Unix permissions (`:46`) |
| `platform::control` | `requests()` | `control.rs:230-240` (the D-Bus service) |
| `platform::caps()` | a struct of booleans the interface reads: `sandbox`, `trash_undo`, `open_with`, `direct_print`, `shared_thumbnails`, `primary_selection`, `own_title_bar` | new |

Also: `print.rs:195-199` (private temp file) and the `PermissionsExt` in the atomic write go
through small helpers in `platform::fs`.

**Done when:** on Linux the tests, lint and `tools/bench.py` are unchanged, and this prints nothing:

```sh
grep -rnE 'std::os::(unix|fd)|rustix|ashpd|zbus|landlock|seccompiler|wl_clipboard|libc::|XDG_|"/proc|"/run/|"/var/' src --include='*.rs' | grep -v '^src/platform/' | grep -v 'mod tests' 
```

(Test modules may keep Unix paths; gate those tests with `#[cfg(unix)]`.)

## 6. Make it compile everywhere

1. In `Cargo.toml`, move the Linux-only crates under a target table and add an empty one for the others:

   ```toml
   [target.'cfg(target_os = "linux")'.dependencies]
   ashpd = { … }            # as today
   zbus = { … }
   landlock = "0.4"
   seccompiler = "0.5"
   libc = "0.2"
   rustix = { … }
   wl-clipboard-rs = "0.9.5"
   ```

   `iced`'s features stay as they are: `x11`, `wayland` and `linux-theme-detection` are ignored on other systems (the Windows check above got past iced with them).
2. Add `src/platform/macos.rs` and `src/platform/windows.rs` (or one `other.rs` to begin with) where every function returns an error such as `Err("not available on this system yet")` and `caps()` is all `false`. No new crates yet.
3. Type-check all three from Linux. The two extra targets are already installed on the author's laptop (`rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin` elsewhere); no SDK or linker is needed for `cargo check`:

   ```sh
   cargo check --locked --all-targets
   cargo check --locked --target x86_64-pc-windows-msvc
   cargo check --locked --target aarch64-apple-darwin
   ```

   Add these as `make check-all`. Fix what they report in Lumen's own code. Expect: `ExitStatusExt::signal` (`sandbox.rs:142`), `CommandExt::exec` (`main.rs:68`), `UnixStream` (`cups.rs:8`), `OsStrExt::as_bytes` (`trash.rs`, `thumbcache.rs:135`, `portal.rs:132, 204`, `watch.rs:7`), `MetadataExt`/`PermissionsExt`/`OpenOptionsExt`.
4. Add CI jobs on real machines, since type-checking is not linking or running:

   ```yaml
   portable:
     strategy:
       fail-fast: false
       matrix:
         os: [macos-latest, windows-latest]
     runs-on: ${{ matrix.os }}
     steps:
       - uses: actions/checkout@v4
       - uses: dtolnay/rust-toolchain@stable
         with: { components: clippy }
       - uses: Swatinem/rust-cache@v2
       - run: cargo clippy --all-targets --locked -- -D warnings
       - run: cargo test --locked
       - run: cargo build --release --locked
       - run: ./target/release/lumen --version
   ```

   Tests that are about Linux (the sandbox escapes in `tests/sandbox.rs`, the trash spec, the thumbnail cache's permissions, CUPS) get `#[cfg(target_os = "linux")]`; the rest must pass everywhere. Watch for tests that build paths with `/tmp` or `/`.

**Done when:** the three `cargo check` lines pass locally and the `portable` job is green. At
this point the app starts on macOS and Windows and shows files given on the command line and
dropped on it; dialogs, trash, clipboard, printing and reloading say "not available".

## 7. Each system's own services

Add the crates for macOS and Windows only, so the Linux build does not change:

```toml
[target.'cfg(any(target_os = "macos", target_os = "windows"))'.dependencies]
rfd = { version = "…", default-features = false }
arboard = { version = "…", default-features = false, features = ["image-data"] }
trash = "…"
notify = { version = "…", default-features = false }
directories = "…"
sys-locale = "…"
```

Take the newest versions that build with the `rust-version` in `Cargo.toml` (`cargo add`, then
the `msrv` job). All are Rust without C libraries; check with `cargo tree --target <triple>`
that none brings a `-sys` crate that compiles C, and that `cargo audit` stays clean.

| Service | macOS | Windows | Notes |
|---|---|---|---|
| Folders | `directories::ProjectDirs` (`~/Library/Application Support/…`, `~/Library/Caches/…`) | the same (`%APPDATA%`, `%LOCALAPPDATA%`) | Signatures: the shared `lumen/signatures.toml` location is a Linux matter; elsewhere use the app's data folder. |
| File dialogs | `rfd::AsyncFileDialog` | the same | `LUMEN_NO_PORTAL` and the "use the desktop's dialogs" preference become Linux only (`caps`). Lumen's own chooser stays reachable for tests; give `places()` the drives on Windows. |
| Links | `open` (the command) | `cmd /C start "" <url>`, or the `open` crate | Keep `structure::is_safe_uri` as the gate. |
| Show in folder | `open -R <path>` | `explorer /select,<path>` | |
| Open with | hide the item (`caps().open_with = false`) | `rundll32 shell32.dll,OpenAs_RunDLL <path>` | |
| Trash | `trash::delete` | `trash::delete` | Undo: the `trash` crate can restore on Windows, not on macOS. Set `caps().trash_undo` and show the Undo button only when true. |
| Copy picture | `arboard::Clipboard::set_image` | the same | **Also try `arboard` on Linux afterwards:** it supports X11, which would close the "copy on X11" gap in `STATUS.md` (GNOME on Wayland to be tested; not promised). |
| Primary selection | none | none | `caps().primary_selection = false`; `Message::Copied(_, true)` does nothing. |
| File changes | `notify` (FSEvents) | `notify` (ReadDirectoryChangesW) | Watch the folder, filter by name, 150 ms settle, as `watch.rs` does. Check idle CPU stays at 0. |
| Language | `sys_locale::get_locale()` after the environment variables | the same | Apps started from Finder or Explorer have no `LANG`. |
| Theme | iced's `system::theme()` (winit reports it) | the same | Nothing to do; verify it switches live. |
| Decoder process | the same worker, not confined | the same; start it with `creation_flags(CREATE_NO_WINDOW)` (a safe std API) so no console flashes | Per decision 2; confinement comes in package 19. Preferences shows "Decode pictures in a separate process" instead of "sandbox" when `caps().sandbox` is false; `FEATURES.md` says what that does and does not protect against. |
| Integrated GPU | no re-exec | no re-exec | `main.rs`'s restart is Linux only. Measure start-up on a two-GPU Windows laptop before doing anything about it. |
| Printing | Lumen's print dialog over CUPS: add the macOS socket path (expected `/private/var/run/cupsd`; verify on a Mac) to `cups.rs:13` | write the print-ready PDF to a private temp file and hand it to the system's print verb (`powershell -Command "Start-Process -FilePath <pdf> -Verb Print"`); if no program prints PDFs, open it and say so | Windows has no print dialog reachable from safe Rust; this is the honest first version. Never send a real job from a test. |
| Thumbnail cache | Lumen's own folder under the cache directory, same PNG format | the same | The freedesktop shared cache is Linux only; the preference's label changes with `caps().shared_thumbnails`. |
| Control interface | see package 9 | see package 9 | No session bus there. |

Windows details to handle in the shared code:

- `std::fs::canonicalize` returns `\\?\C:\…` paths, which then show in titles and break `explorer /select`. Package 4 already removes the canonicalising in `library.rs`; use `std::path::absolute` instead.
- Replacing a file that another program holds open fails; the save error must say so ("close it in the other program").
- `file_name`-based temp names starting with a dot are not hidden on Windows; that is fine.
- `shorten_home` (`app.rs:2735`) uses `HOME`: take it from `platform::dirs::home()`.

**Done when:** on each system, by hand (see package 11's checklist): open through the dialog,
save a copy, trash and (Windows) undo, copy a picture and paste it elsewhere, edit the open
file in another program and see it reload, print to a PDF printer.

## 8. Keys, menus and file opening per system

- **Shortcut names.** The handlers already use `modifiers.command()` (Cmd on macOS), but every label says "Ctrl": 87 strings in `src/app*` and `po/es.po` such as `tr("Find (Ctrl+F)")`, the menu items (`menu.rs:118-212`) and the shortcuts table (`menu.rs:36-93`). Add `platform::keys::show("Ctrl+Shift+S")`, giving `⇧⌘S` on macOS and the text unchanged elsewhere; change the strings to `trf!("Find ({})", keys::show("Ctrl+F"))` and update `po/es.po`.
- `Message::Wheel` (`app.rs:943`) tests `control()`; keep that on every system (a Windows touchpad pinch arrives as Ctrl + wheel, and macOS sends real pinch events, which the vendored iced already passes on).
- macOS conventions: Cmd+W closes the window (it does: `"q" | "w"`), Ctrl+Cmd+F for fullscreen next to F and F11, Backspace deletes the selected annotation (`markup.rs:1038` does).
- **macOS Quit must ask.** winit builds a default application menu whose Quit ends the app directly, so unsaved changes would be lost without the question (expected from winit's code; verify on a Mac). Replace the default menu with Lumen's own (below) whose Quit sends `Message::Quit`.
- **macOS menu bar.** Mac users expect File, Edit, View, Window and Help in the system bar. Build it with the `muda` crate from the same command table as the ☰ menu (package 14), and hide the ☰ button there. If `muda` cannot be attached without touching `vendor/iced_winit`, stop and ask (decision 4).
- **Files opened from Finder** (double-click, Open With, drop on the Dock icon) arrive as an event. Needs the winit patch of decision 4: handle `application:openURLs:` in `vendor/winit/src/platform_impl/macos/app_delegate.rs`, pass it through `vendor/iced_winit` as a window event, and treat it like `Message::Chosen`. Record the patch in `vendor/patches/` and `vendor/README.md`.
- **Windows:** a second double-click starts a second Lumen, as on Linux. Fine for now.

**Done when:** on macOS the labels show ⌘, Cmd+Q asks with unsaved markup, and a PDF
double-clicked in Finder opens in the running app.

## 9. Tests that drive the app on every system

`tests/control.rs` is the only end-to-end test and it needs D-Bus. Give the control interface a
second transport that works anywhere: with `LUMEN_CONTROL=stdio`, Lumen reads one JSON command
per line on standard input and writes one JSON answer per line on standard output (the same
commands and answers as the D-Bus methods; `src/app/control.rs` already works on a `Command`
value, so only `platform::control::requests()` grows). It is also safer than the bus: only the
process that started Lumen can reach it.

Then make the test harness (`tests/control.rs:24-120`) speak through a small trait with two
implementations, D-Bus (Linux, as today) and stdio (everywhere), and run the stdio one in the
`portable` job with `ICED_BACKEND=tiny-skia`. Whether a window can open on the macOS and Windows
runners is to be found out; if it cannot on one, leave that job to the unit tests and say so in
`DEVELOPMENT.md`.

**Done when:** the ten control scenarios pass over stdio on Linux, and on the other systems
where a window can open.

## 10. Packages and releases

| | Linux (as today) | macOS | Windows |
|---|---|---|---|
| Build | `cargo build --release` on `ubuntu-latest` | on `macos-latest`: build `aarch64-apple-darwin` and `x86_64-apple-darwin`, join with `lipo -create` | on `windows-latest`: `x86_64-pc-windows-msvc` |
| App identity | `.desktop`, AppStream, icons | `Lumen.app`: `Info.plist` (bundle id `io.github.imanolgo.Lumen`, `CFBundleDocumentTypes` for PDF and the picture types, `NSHighResolutionCapable`), `AppIcon.icns` made from `assets/app-icon.svg` | `#![windows_subsystem = "windows"]` (no console window), an `.ico` and version info embedded by `build.rs` with the `winresource` crate (`[target.'cfg(windows)'.build-dependencies]`) |
| Package | tarball, `.deb`, AUR, Flatpak | `.dmg` (use `cargo-packager`, or `hdiutil` in the workflow) | `.zip` of the `.exe` first; then an installer (`cargo-packager`'s NSIS, or `cargo-wix`) that registers "Open with" for the file types |
| Signing | none | per decision 5: Developer ID + notarisation (`xcrun notarytool`), or unsigned with a note on right-click → Open | per decision 5 |

- With `windows_subsystem = "windows"`, `--help` and `--version` print nothing in a terminal. Keep them working when output is piped (they do) and document it; the decoder worker uses pipes and is not affected.
- `release.yml` becomes a matrix of three jobs that upload to one release; `SHA256SUMS` covers all files.
- `Cargo.toml`'s description says "for Linux"; so do `README.md` and the metainfo. Update them, and add a "Systems" table to `README.md`: what works where (from `platform::caps()`).

**Building for another system from Linux** (what is possible, for local work):

```sh
# Type-check everything (fast, no SDK): the daily check
make check-all

# A Windows .exe from Linux, GNU toolchain
yay -S mingw-w64-gcc
rustup target add x86_64-pc-windows-gnu
cargo build --release --locked --target x86_64-pc-windows-gnu
#   → target/x86_64-pc-windows-gnu/release/lumen.exe  (try it: ICED_BACKEND=tiny-skia wine …)

# A Windows .exe from Linux, MSVC toolchain (what CI ships)
cargo install cargo-xwin        # downloads Microsoft's CRT and SDK; read their licence
cargo xwin build --release --locked --target x86_64-pc-windows-msvc
```

These two were not run while writing this plan; the first step of package 10 is to try them.
A macOS binary is not built from Linux: it needs Apple's SDK, whose licence ties it to Apple
hardware. macOS builds come from the `macos-latest` runner (`gh run download` fetches the
artifact), and `cargo check --target aarch64-apple-darwin` covers the daily work.

**Done when:** pushing a tag publishes a tarball and `.deb`, a `.dmg` and a Windows `.zip`, and
each was downloaded and started once on its system.

## 11. Check by hand, and write it down

Nobody can click on macOS or Windows from the development laptop. For each system, someone
with that machine runs this list and the result goes into `docs/STATUS.md` under a new
"macOS" and "Windows" heading, with the date and what was not tried:

open a picture, a folder and a PDF from the dialog and by dropping; arrows, zoom with the wheel
and the touchpad; fullscreen and back; markup and Ctrl/Cmd+S on a PDF, reopen it elsewhere;
crop a JPEG and save; fill `tests/fixtures/form.pdf`; unlock `encrypted.pdf` (`secret`); trash
and undo; copy a picture and paste it; print to a PDF printer; switch the system between light
and dark; a second screen with another scale factor; Spanish.

Update `ARCHITECTURE.md` (the platform layer, the stack table's "Desktop" row), `FEATURES.md`
(paths per system in "Files Lumen reads and writes", environment variables that are Linux
only), `DEVELOPMENT.md` (`make check-all`, the stdio control transport).

---

# Part 3: interface

The aim: Lumen should look like one careful, quiet program on Linux, macOS and Windows.
"Minimal" here means the picture or page is what you see, and controls appear where and when
they are needed. "Functional" means nothing is cut off, hidden behind something else, or
labelled with a key that the system does not have.

## What was looked at, and what it showed

The window was screenshotted at 1200 × 800 in both themes, in 19 states: the seven published
screenshots in `data/screenshots/`, and twelve taken for this plan with a temporary demo hook
(empty window, ☰ menu on a PDF and on a picture, Preferences, Find, command palette, Keyboard
Shortcuts, Properties, Export, the edit bar, Lumen's own file chooser and its print dialog).
Not looked at: the signature pad, the colour menu, the resize and colour panels, the quit
question, the slideshow, any window smaller than 1200 × 800, and anything on macOS or Windows.

**What already works and stays.** One surface for header, sidebar and canvas. Tools as floating
islands. Amber only for what is active. Inter. Big sidebar thumbnails with the name underneath.
The fading zoom pill and arrows. The ☰ menu. The markup style menu. Stroked 18 px icons. The
thin scrollbar in the main view.

**Things that are broken on screen** (each is fixed in the package named):

| # | Seen | Where | Package |
|---|---|---|---|
| U1 | The ☰ menu on a PDF is taller than an 800 px window: "Preferences" is cut in half, "Keyboard Shortcuts" and "About" cannot be reached. It does not scroll. The window may be as small as 360 px. | `menu.rs:216-254` | 14 |
| U2 | Keyboard Shortcuts: a thick lavender scrollbar is drawn over the keys column ("Ctrl+", "Home / En", "Alt+Ente" are cut), the Close button is pushed out of the card, and the card runs over the header. | `menu.rs:263-288`, `:345` | 15 |
| U3 | A tool island covers the first lines of a fitted page (`data/screenshots/pdf-forms.png`). | `app.rs:2763-2775` | 16 |
| U4 | Lumen's file chooser: the first row is cut by the top edge of the list, and file names are drawn in muted grey, as if they could not be chosen. | `app/chooser.rs` | 15 |
| U5 | Contents sidebar: long titles are cut in the middle of a letter, with no ellipsis ("PDL Color Definitions an"). A tooltip has the full title. | `app.rs:3245-3250` | 16 |
| U6 | A disabled main button keeps its amber fill at low strength: "Save" in the edit bar is a muddy brown in the dark theme, "Open" in the chooser is pink in the light one. Neither reads as a disabled button. | `style.rs:176-193` | 13 |
| U7 | Preferences: "Wrap around at the first and last file" and "Share thumbnails with the desktop's cache" sit under SECURITY; the note about the sandbox is under the thumbnails switch; two notes explain environment variables to someone who opened a dialog. | `app/prefs.rs` | 15 |

**The default look of the toolkit shows through.** `style.rs` styles buttons, panels and the main
scrollbar, but several widgets are used as iced draws them, and they do not match:

| Widget | Where it shows | How it looks now |
|---|---|---|
| Text input | Find, command palette, go to page, password, copies, the chooser's name field | Square corners, grey fill, a hard amber outline |
| Drop-down | Printer and paper in the print dialog | Lavender-grey fill, a black triangle |
| Check box | The chooser ("Hidden files"), the print dialog ("both sides") | Pale blue-grey box; Preferences has an amber one |
| Scrollbar | Inside dialogs (Keyboard Shortcuts) | Thick and lavender; the main view's is thin and neutral |
| List selection | The command palette's highlighted row | A brown-amber bar (amber is meant for what is active, not for where the cursor is) |

**Dialogs do not agree with each other.** Export has no main action: its three dpi buttons act at
once and the only button is "Close". Print has Cancel and an amber Print. Preferences has "Reset
to defaults" and "Close". Properties and Shortcuts have "Close". Widths are 360, 420, 440, 460.
The print dialog's subtitle ("Printing directly, without the desktop's print dialog") describes
the implementation, not the task.

**Three signals for one thing.** The file on screen has, in the sidebar, an amber ring, a wash
behind it and a bold name. One is enough.

## 12. Window frame per system

Today the header has its own × and the window keeps whatever frame the system adds: GNOME
draws a second title bar above it (winit's `wayland-csd-adwaita`), KDE and X11 add theirs, and
the header cannot be dragged. Per decision 3:

```
Linux (no system frame; the header is the title bar)
┌──────────────────────────────────────────────────────────────────────┐
│ [▯] [🗀▾]            report.pdf                    [⌕] [✎] [▦] [☰] [×] │
│                    Page 3 of 42                                      │
├─────────┬────────────────────────────────────────────────────────────┤

macOS (the header inside the title bar; the system menu bar has the commands, so no ☰)
┌──────────────────────────────────────────────────────────────────────┐
│ ● ● ●  [▯]  report.pdf                             [⌕] [✎] [▦] [🗀▾]  │
│             Page 3 of 42                                             │
├─────────┬────────────────────────────────────────────────────────────┤

Windows (the system title bar stays; the header is a toolbar under it)
┌──────────────────────────────────────────────────────────────────────┐
│ ▣ report.pdf — Lumen                                       –  ☐  ✕  │
├──────────────────────────────────────────────────────────────────────┤
│ [▯] [🗀▾]            Page 3 of 42                   [⌕] [✎] [▦] [☰]    │
├─────────┬────────────────────────────────────────────────────────────┤
```

- **Linux:** `decorations: false`. Dragging the header's empty parts moves the window (`window::drag`), double-click toggles maximise (`window::toggle_maximize`), right-click opens the system menu (`window::show_system_menu`), and 6 px zones at the edges and corners resize (`window::drag_resize`). Keep ×. Minimise and maximise buttons are useless on a tiling compositor and Lumen cannot tell where it runs, so make them a preference, off by default.
- **macOS:** `titlebar_transparent`, `fullsize_content_view` and `title_hidden` (`iced::window::settings::PlatformSpecific`), no ×, 78 px left free for the traffic lights, the title left-aligned after the sidebar button as Preview does, the header draggable.
- **Windows:** the system title bar stays, so snap layouts, the system menu and resizing keep working. The header loses × and the file name (the title bar shows it); its centre keeps "Page x of y" or "3 of 40".
- In fullscreen every system hides the header, as now.

Put the choice in `platform::caps()` (`own_title_bar`, `close_button`, `title_in_header`,
`title_centered`, `leading_inset`) and build the header from those, so `toolbar()`
(`app.rs:2877-3000`) has no `cfg` in it. With no frame on Linux the compositor rounds the
corners or not; do not draw a fake window shadow.

**Done when:** screenshots on niri; and by the checklist of package 11 on GNOME (one title bar,
not two), macOS and Windows. At the minimum window size (520 × 360) the header's buttons do
not overlap the title on any of them.

## 13. A complete set of controls, and tokens for size and space

The fix for "the toolkit shows through" is that view code never builds a bare widget.

1. **Tokens.** `style.rs` has tokens for colour and corners but none for type or spacing: the views use 11 text sizes and dozens of paddings written as numbers (`size(13)` 50 times, `size(12)` 36, `size(14)` 30). Add:

   | Token | Values |
   |---|---|
   | Type | `CAPTION 11`, `SMALL 12`, `BODY 13`, `TITLE 15`, `HEADING 18`, `DISPLAY 24` (the empty window only). Fold 14 into 13 and 16 into 15. |
   | Space | `4, 8, 12, 16, 24` |
   | Control height | `32` (buttons, inputs, drop-downs, menu rows), `28` compact (segments, chips) |
   | Corners | `RADIUS 8` for controls, `PANEL_RADIUS 12` for panels, as now; inputs and drop-downs use `RADIUS` too |

2. **Controls.** A new `src/ui.rs` (helpers that return widgets already styled) and the matching styles in `style.rs`:

   | Helper | Look |
   |---|---|
   | `ui::input` | 32 px high, `RADIUS`, the faint wash as fill, no border at rest, a 1.5 px amber ring when focused. One variant without any frame for the palette and Find, where the panel is the frame. |
   | `ui::select` | The same box as the input, with the `icons::down()` chevron |
   | `ui::check` | The amber box Preferences has, everywhere |
   | `ui::segmented` | What Preferences uses for Theme (the existing `style::segment`) |
   | `ui::slider` | A thin track with a round thumb; amber only on the filled part |
   | `ui::scroll` | `style::scrollbar` on every `scrollable`, with room beside the content so it never covers it |
   | `ui::list_row` | A neutral wash for the highlighted row (palette, chooser, menu) |
   | Buttons | `primary` (one per surface), `secondary`, `quiet` (today's `tool`), and a new `danger` for "Discard" and "Delete". **Disabled:** every kind becomes the neutral wash with 40 % text; the amber fill goes away (fixes U6). |

3. Replace the bare `text_input(`, `pick_list(`, `checkbox(`, `scrollable(` and `slider(` in `src/app.rs` and `src/app/*.rs` (32 uses in 11 files), and the numbers with tokens. The PDF form fields keep their own styles (`style::form_*`): they belong to the page, not to Lumen.
4. Move the styles written inline into `style.rs`: the key cap (`app.rs:3412`), the drop overlay (`app.rs:3538`), the separator (`app.rs:2746`), the floating glyph colours (`app.rs:3570, 3590, 3625`).
5. **Contrast test.** A unit test computes the contrast of each text and icon token on its background in both themes (4.5:1 for text, 3:1 for icons and for the edges of inputs). Muted icons (45 %) and disabled text (35–40 %) are the likely failures; raise them until the test passes.
6. One signal for the file on screen in the sidebar: the amber ring. Drop the wash and the bold name (`style::thumb`, `app.rs` file list).

**Done when:** these print nothing, and the screenshot set of package 17 shows no lavender, no
square input and no thick scrollbar:

```sh
grep -nE '\.size\([0-9]' src/app.rs src/app/*.rs
grep -nE '\b(text_input|pick_list|checkbox|slider)\(' src/app.rs src/app/*.rs | grep -v 'ui::'
```

## 14. Commands and menus

The same facts are written in four places that have drifted: the key handler
(`app.rs:1756-1827` and the `*_key` functions), the ☰ menu (`menu.rs:112-213`), the shortcuts
dialog (`menu.rs:36-93`) and `docs/FEATURES.md`. The palette (`app/commands.rs`) already builds
a list of `(name, shortcut, Message)`.

- **One table.** `commands::all(&App) -> Vec<Command>` with: id, name, group (File, Edit, View, Go, Tools, Help), shortcut as data (key and modifiers), whether it can be done now, and its `Message`. The key handler looks the pressed key up in it; the ☰ menu, the macOS menu bar (package 8), the palette and the shortcuts dialog are drawn from it; a test compares it with the keyboard table in `FEATURES.md`. Key names come from `platform::keys::show` (package 8).
- **A menu that fits (U1).** Today it is one column of up to 27 rows. New shape, at most twelve rows, and it scrolls when the window is shorter than it:

  ```
  Open…                 Ctrl+O        View ▸     Best Fit · Fit Width · Actual Size
  Open Recent ▸                                  Fullscreen · Slideshow · Sidebar
  ──────────────────────────────                 Night Mode
  Save                  Ctrl+S        Export ▸   Export Pages… · Export as PDF…
  Save a Copy…          Ctrl+Shift+S             Combine Files…
  Print…                Ctrl+P        File ▸     Show in Folder · Open With…
  ──────────────────────────────                 Move to Trash · Properties…
  View ▸
  Export ▸
  File ▸
  ──────────────────────────────
  Settings…             Ctrl+,
  Keyboard Shortcuts
  About Lumen
  ```

  Find, Markup, Edit, Organise and Sign have buttons in the header or the island; leave them out of the menu (the palette still finds them). Call it "Settings" on Windows and macOS and "Preferences" on Linux, and give it Ctrl+, (⌘, on macOS).
- **Anchor menus to their button** instead of the fixed offsets (`Padding { left: 44.0 }`, `right: 46.0` at `menu.rs:219-220`), which are wrong as soon as the header changes (package 12).
- **A context menu.** There is no right-click handling anywhere. On the picture or page: Copy, Best Fit, Actual Size, Rotate, Show in Folder, Properties. On a sidebar thumbnail: Show in Folder, Move to Trash. On an annotation: Delete, Bring to Front. On selected text: Copy, Highlight.
- **The palette** gets the frameless input with a search icon, the neutral highlighted row, and the group name (File, View, …) in muted text beside each command.

**Done when:** adding a command in one place makes it appear in the menu, the palette and the
shortcuts dialog with the right key name on each system; the menu is fully reachable in a
520 × 360 window (screenshot); the docs test passes.

## 15. Dialogs

One helper, `ui::dialog(title, body, actions)`, used by every dialog:

- Two widths (360 for questions, 480 for forms and lists). The title, then the body, which scrolls by itself when it is too tall; the buttons stay at the bottom and are always visible (fixes U2). The card never covers the header and never exceeds the window.
- Buttons: one `primary` for what the dialog is for, `secondary` for Cancel, `danger` when the action discards something. A dialog that only shows something (Properties, Shortcuts, About) has one "Close". Enter runs the main action, Esc cancels, the first field has focus.
- Order: main action on the right on Linux and macOS; on Windows the main action comes first and Cancel last (`platform::caps().confirm_first`).

Then, dialog by dialog:

| Dialog | Change |
|---|---|
| Keyboard Shortcuts | Through the helper (U2). Two columns when the window is wide enough. Keys drawn as key caps (the style of the empty window's "Ctrl O"), from the command table. |
| Preferences | Regroup (U7): **General** (theme, sidebar at start, zoom control and arrows, wrap around), **PDF** (fit, night mode), **Slideshow**, **Performance** (decoded pictures kept, neighbours decoded ahead), **Privacy and safety** (sandbox with its note, shared thumbnails), **System** (the desktop's dialogs; Linux only). Remove the notes about environment variables (they are in `FEATURES.md`). Rows that do not apply on a system are hidden by `caps()`. |
| Export | A form with a main action: Format (PNG / PDF, segmented), Resolution (72 / 150 / 300, only for PNG), a line saying which pages, then Cancel and **Export…**. |
| Print (Lumen's own) | Remove the subtitle. Add page range and orientation if `cups.rs` can send them (`page-ranges`, `orientation-requested`); the controls from package 13 fix the rest. |
| File chooser (Lumen's own) | Fix the cut first row and the grey names (U4); folders before files; the name field and "Hidden files" through `ui::`; type-ahead to jump to a name. On macOS and Windows it is never shown (package 7). |
| Properties | Dates in local time, not UTC. A "Copy path" button. Values can be selected. |
| Quit with unsaved changes | Not looked at. Three buttons: **Save**, `danger` Discard, Cancel; its text goes through `tr()` (package 4). |
| Password | The input through `ui::input`; the title through `tr()` (package 4). |

**Done when:** every dialog in the screenshot set of package 17 has its buttons visible at
1200 × 800 and at 520 × 360.

## 16. The main view

1. **Islands do not cover the page (U3).** While an island shows, add its height to the top padding of the scroll content in `pdf_view`, and fit a picture to the area below it in `image_view`, as the page organiser already does (`TOP_INSET`, `app/pages.rs:21`). The same for the Find bar: it sits over the top right corner of the page; give it the island's row.
2. **Find.** The frameless input; "3 of 17" beside it once there are results, "No results" in the danger colour when there are none; the **Aa** and **[ab]** switches keep their tooltips.
3. **Contents (U5).** End a long title with an ellipsis (`text::Wrapping::None` plus iced's ellipsis if 0.14 has it; otherwise shorten the string to the measured width). Entries with children get a disclosure arrow and can be folded.
4. **Sidebar width.** It is fixed at 196 px (`SIDEBAR_W`). A drag handle on its edge (160 to 360 px), remembered in `state.toml`; thumbnails scale with it.
5. **Loading.** `Doc::Loading` is the word "Loading…" (`app.rs:3005`). Keep the previous picture on screen until the next is ready, and show a 2 px progress line under the header after 150 ms.
6. **Zoom.** The percentage in the pill sets actual size when clicked. Make it open a small menu: Best Fit, Fit Width, 50 %, 100 %, 200 %.
7. **The empty window.** The key cap says "Ctrl O": take it from `platform::keys::show`. The pen button in the header does nothing without a file: hide it there.
8. **Errors a person can act on.** `Doc::Failed` shows the decoder's words. Add an "Open another file" button and, for a format this build leaves out (HEIC, AVIF), say so.
9. **Motion.** Menus, dialogs and toasts appear at once while zoom eases. Give them one 120 ms fade, using the animation `App` already has for the controls; a "Reduce motion" preference turns all of it off.
10. **Tooltips** appear at once on every hover (`tip`, `app.rs:2708`). Delay them about 500 ms if iced 0.14's tooltip has a delay; otherwise leave them.

**Done when:** each point was checked with a before and an after screenshot, in both themes.

## 17. A screenshot set that is kept

The defects in this part were found by looking, and nothing stops them coming back. Make the
looking repeatable:

- Add the scenes used for this review to `tools/screenshots/demo.rs.txt` (`menu`, `prefs`, `keys`, `props`, `search`, `palette`, `export`, `edit`, `open`, `print`, plus `quit`, `sign`, `colour`, `resize`, `adjust`, `slideshow`), each a message sent 1.8 s after start.
- A script `tools/screenshots/review.sh` takes every scene in light and dark, at 1200 × 800 and at 520 × 360, into a scratch folder. It must check that Lumen's window has the focus before it resizes or captures anything (`niri msg focused-window`): otherwise it acts on whatever window the user is in.
- Run it at the end of packages 12 to 16 and look at every picture. Update `data/screenshots/` when the published scenes change.

**Done when:** the script exists, `DEVELOPMENT.md` describes it, and a full run was looked at
after package 16.

---

# Part 3b: what opens, and where the tools are

Agreed with the user on 25 November 2026, after they compared Lumen with macOS Preview. Done before
package 18, whose tools need the new toolbar.

## 20. What opening a file shows

1. The folder of an opened file is still loaded, but the sidebar does not show it by itself:

   | Opened | Sidebar | ← → |
   |---|---|---|
   | One picture | hidden | previous/next **picture** of the folder |
   | One PDF | Contents, or Pages, if it has more than one page; else hidden | never another file |
   | Several files | those files | between those files |
   | A folder | the folder | all of it |

2. "Show Folder" (T, and the sidebar button) brings the list. "3 of 15" in the header is for pictures and for lists that were asked for, not for a PDF opened alone.
3. The preference "Show the sidebar when Lumen starts" is replaced by what the table says; a sidebar the person opened or closed by hand stays so while browsing.
4. Slideshow, wrap around and the arrows over the picture follow the same list. Files added by dropping make it a list that was asked for.

**Done when:** a test covers every row of the table, and dropping, reloading and Undo of the trash keep the list.

## 21. A toolbar joined to the header

1. The header always has, for what is open: zoom out, percentage (its menu), zoom in; rotate and crop (pictures); Markup; Edit (pictures) or Organise (PDFs); Find (PDFs); ☰. Undo and Save show there when something has changed.
2. Right below it, one centred second row, joined to the header, only while a tool is on. Markup, Edit, Crop, Find, Organise and a selected part swap its **contents**; it keeps its place, so the page moves once. It wraps in a narrow window.
3. Colours, width and other details open directly under the button they belong to.
4. Pencil and sliders no longer look alike: tools have their names beside the icons where there is room.
5. The floating islands go. The zoom control and arrows that fade in over the picture stay (a thing the user asked to keep); the header's zoom is what is always there.
6. At 520 px the less used buttons move into ☰; nothing is cut off.

**Done when:** the screenshot set was looked at, and the pictures in `data/screenshots/` (left as they are by package 17) are taken again.

## 22. The window's first size

1. A window that opens on a picture is sized for it once: the picture at 100 % or less with the header, at least 720 × 480, at most 80 % of the screen. It does not change while browsing, nor after the person resized it.
2. PDFs, folders and the empty window keep the remembered size; the size made for a picture is not remembered.
3. Nothing is done in a tiling window manager, when maximised, or in fullscreen.

## 23. A window for each opening

1. Opening from the file manager opens a new window; several files chosen together share one. A file that is already open has its window brought forward (where the system lets a program do so; Wayland needs an activation token).
2. Arrows, the sidebar, dropping and Ctrl+O stay in the window. "Open in New Window" in a file's right-click menu and in Open Recent, and Ctrl+click in the sidebar.
3. On macOS it is one program with several windows (iced's `daemon`): `App` becomes the state of a window, with preferences, recent files, passwords and caches shared. Linux and Windows keep a process for each opening unless sharing one proves simple. Closing the last window quits except on macOS.
4. Quitting asks about the unsaved changes of every window.

**Done when:** two windows can be edited and closed independently; the same file never opens twice; a person tried it on macOS.

---

# Part 4: features

## 18. What is missing, in order of worth

Each is its own piece of work; ask the user which to do. Verified absent by searching the source.

**Worth doing first (common needs, moderate work):**

| Feature | Notes |
|---|---|
| Underline, strike-out and squiggly text markup | The highlighter already maps a drag to text quads; these are the same quads with another subtype and appearance. |
| Sticky notes | A `/Text` annotation with a popup; the model already carries `/Contents` for imported ones. |
| Sort order for folders | By name (now), date, size, type; reversed. A menu in the Files tab; remembered. |
| Rename the open file | F2; keeps the place in the list; refuses to overwrite. |
| Straighten and free rotation | A slider in the crop tool (±45°), cropping to the largest inner rectangle. |
| Colour management | Pictures are shown without regard to their ICC profile; wide-gamut photos look oversaturated or dull. Convert to sRGB on decode with a pure-Rust engine (`moxcms`), inside the worker. |
| Page labels | Show "iv" or "A-3" (`/PageLabels`) in the header and in go-to-page. |
| Open several documents | Tabs or windows; today opening a file replaces the list. iced 0.14 supports several windows (`window::open`). |
| Remember zoom and scroll per file, not only the PDF page | `state.rs` already keeps per-file data. |

**Order agreed on 26 November 2026:** after *Rectangular selection on PDF pages*, the common ones above
(underline and strike-out and sticky notes come with *Notes, underline, strike-out* below; *Open several
documents* is done, by package 23), then the rest of the Preview list.

**From macOS Preview: to be done** (the user asked for all of them on 25 November 2026; no need to ask
again which). The rectangular selection on pictures, with Copy and Crop, is done. In this order, each
a commit of its own with tests and docs:

| Feature | Notes |
|---|---|
| More ways to select a picture's part | **Done**: lasso, by colour, invert, delete to transparent (the picture is "baked": the edits start again from it, and undo takes it back). Pasting a selection as a new picture comes with *Open the clipboard as a new picture*, which needs the same thing: a picture that is not a file yet. |
| Rectangular selection on PDF pages | **Copy: done** (Select an Area). Cropping the page to it is not: it means writing a CropBox, and markup and form fields are placed relative to the page's box, so it is a piece of work of its own; after the common features. |
| Sketch tool that recognises shapes | A roughly drawn circle, arrow or rectangle becomes a clean one |
| More shapes | Rounded rectangle, star, polygon, speech bubble; dashed and double-ended lines |
| Loupe and mask | A magnifier placed on the picture; everything but a region dimmed |
| Text style | Font, bold, italic, alignment and colour in text boxes |
| Notes, underline, strike-out | As in the table above |
| Redact | Removes the text under the box, not only covers it |
| Open the clipboard as a new picture | File → New from Clipboard |
| Contact sheet | Every open file as thumbnails over the whole window |
| Adjust Colour: levels, tint, sepia, sharpness | More sliders beside exposure, contrast and saturation |
| Bookmarks in PDFs | Marks of your own, listed in the sidebar |

**Good to have:**

- Histogram, levels and curves; white balance; sharpen. Auto-enhance.
- Batch work on a folder selection: convert, resize, rename.
- Redaction that removes the content under the box (not a black rectangle over it), for PDFs.
- Edit the PDF outline; the outline should also follow page operations.
- Attachments (embedded files): list and save them.
- Layers (optional content): show and hide.
- Two-page and book view; single-page (non-continuous) mode; presentation mode for PDFs.
- Text search inside annotations and form values; a list of all annotations in the sidebar.
- Crop PDF pages (set the CropBox); split a PDF by ranges or bookmarks; compress on export.
- OCR for scanned pages is out of reach in pure Rust today; note it and leave it.
- Verify digital signatures (`/ByteRange`); signing with a certificate is a larger, separate project.
- Camera RAW: no complete pure-Rust decoder; showing the embedded JPEG preview is cheap and covers browsing.
- Set as wallpaper; share; print several pictures per sheet.
- Auto-update check (off by default) for macOS and Windows, where there is no package manager.
- Accessibility: re-check when iced gains an accessibility tree; until then, make every action reachable by keyboard (crop and markup placement are pointer-only today) and add a high-contrast theme.
- More languages: the `.po` machinery is ready; German, French and Catalan are the natural next ones.

---

# Part 5: the sandbox on macOS and Windows

## 19. Confine the decoder on macOS and Windows

After package 7 the decoder runs in its own process on every system, but only Linux shuts it
in. This package adds that elsewhere. Nothing here was tried: there was no Mac or Windows
machine when the plan was written, so each step starts by proving the mechanism works.

What makes it tractable: the worker already needs nothing but its standard input (the open
file) and its standard output (the pixels). It opens no file and no socket itself.

**macOS, step 1: `sandbox-exec` (no `unsafe`, no signing).** Start the worker as
`/usr/bin/sandbox-exec -p '<profile>' <lumen> --decode-worker …` with a profile that denies
everything by default and allows only what a process needs to start (reading the system
libraries and the binary itself) and nothing else: no file writes, no network, no starting
programs. Apple marks the tool deprecated; it still ships and other projects rely on it. If it
is missing or fails to start, that is "sandbox unavailable" (decode in the app, log once), as
on Linux; a worker stopped by the profile is a failed picture, never a fallback (package 1.1).

**macOS, step 2: the App Sandbox entitlement (when the app is signed, package 10).** Build the
worker as its own small executable (`[[bin]] name = "lumen-decode"`) inside `Lumen.app` and
sign it with `com.apple.security.app-sandbox` and no file or network entitlements. This is
the supported mechanism. To check on a Mac: a sandboxed command-line helper is believed to
need an `Info.plist` embedded in the executable. Do **not** sandbox the whole app: it could
then read only the files the user picked, and flipping through a folder would stop working.
Keep step 1 as the fallback for unsigned builds.

(Calling `sandbox_init` from the worker with the "pure computation" profile is a third way,
closest to what Linux does, but it needs `unsafe` and is deprecated as well. Not planned.)

**Windows: AppContainer, in a small separate crate.** The real mechanisms are an AppContainer
with no capabilities (what Chrome and Firefox use), or a restricted token at low integrity,
together with a job object (memory limit, no child processes, the worker dies with Lumen).
All need Win32 calls at process creation that safe Rust's standard library cannot make; no
route without `unsafe` is known. **Ask the user first:** the proposal is a crate in the
repository (`crates/lumen-confine`) that holds only those calls behind one safe function,
`spawn_confined(exe, args, stdin) -> Child`, while the app crate stays
`#![forbid(unsafe_code)]`. If the user says no, Windows stays unconfined and the docs keep
saying so.

**For both:**

- `platform::caps().sandbox` becomes true where confinement works, and Preferences goes back to saying "sandbox" there.
- Port the escape tests in `tests/sandbox.rs` (open a file, open a socket, run a program, write a file: each must be stopped) and run every picture format through the confined worker on that system's CI runner.
- Measure the cost per picture as `STATUS.md` does for Linux (about 4 ms there); `sandbox-exec` adds a process start.
- `FEATURES.md` gets a table: what the decoder is kept from doing on each system.

**Done when:** the escape tests pass in CI on each system that claims a sandbox, and the docs
say exactly which systems those are.

---

## Status

| # | Package | Status |
|---|---|---|
| 1 | The decoder fails closed and stays within limits | Done. The worker keeps `prctl`, for naming threads and mappings only. `clone3` is answered with `ENOSYS`. JPEG XL and JPEG 2000 memory is bounded by the worker's 8 GB address-space limit; their internals were not read. Also found and fixed: the size check on the worker's answer overflowed, and a page that failed to render was requested in a loop. |
| 2 | Saving does not lose or expose data | Done. 2.4's suspected case (recovery without `/Prev`) was not reproduced; the new bytes are checked before writing anyway. Article threads are dropped whole when pages are removed. The new questions (overwrite, trash) were tested by message; the overwrite one was looked at. |
| 3 | PDF writing keeps what other programs wrote | Done. Embedded text is not shaped (Arabic and Indic scripts come out unjoined). A merged outline goes after the document's own. |
| 4 | Smaller defects, things that disagree, finish the review | Done, with what the review found added (organiser changes lost silently, late answers, Delete in text boxes, clicks through questions). Render caches measured on the 477-page manual: 311 MB and growing after 120 zoom levels before, 205 MB and flat after. Page rearranging on 200 000 objects: 0.5 s, with as many inserted 1 s (not measured before). Review: the update logic of `app.rs` and the files in `app/` were read for the points listed; the view code was not read line by line. `CLAUDE.md`'s `Co-Authored-By` line is left to the user. |
| 5 | Platform layer (Linux only) | Done. `tools/bench.py` was not run again. |
| 6 | Compiles on Linux, macOS and Windows; CI on all three | Done. The first macOS run showed that private files were not private there; fixed. |
| 7 | Each system's own services | Done in code; file watching and the trash are tested on the real systems in CI, the rest by nobody yet (package 11). Windows opens links with `url.dll`, not `cmd /C start` (a shell would read `&` in an address). `arboard` was not tried on Linux: decision 1 allows it on the other two only. |
| 8 | Keys, menus and file opening per system | Done in code; compiled, not seen on a Mac yet. Shortcut signs are applied in `tr()` rather than in each string. The ☰ button stays on macOS until package 14 gives the menu bar every command. Quit from the Dock icon, or logging out, still ends Lumen without asking: that needs `applicationShouldTerminate:` in winit, which decision 4 does not cover. |
| 9 | End-to-end tests on every system | The ten scenarios pass over stdio and D-Bus on Linux; CI runs them on macOS and Windows too. |
| 10 | Packages and releases | The workflow builds all packages. Not done: an installer for Windows, the two ways to build a Windows binary from Linux (they need system packages). |
| 11 | Checked by hand on macOS and Windows | Done by the author (see `STATUS.md`); what it found is fixed and confirmed. |
| 12 | Window frame per system | Done. Looked at on an X display without a window manager; GNOME, KDE, macOS and Windows not seen (package 11). Moving and resizing with the pointer were not tried. |
| 13 | A complete set of controls; tokens for size and space | Done: type sizes, controls, button kinds and the contrast test (3:1 for disabled text; the edge of an input at rest is not checked, it has none). Paddings are not all tokens yet. At 520×360 the Find bar, Unlock, crop chips and the edit panels are still cut or overlap: packages 15 and 16. |
| 14 | One table of commands; a menu that fits; context menu | Done. The submenus take the menu's place (with a way back) rather than fly out, so that they fit a small window. The right-click menu of a page was tried with a pointer on an X display; the Mac's menu bar was not tried by hand. |
| 15 | Dialogs | Done. Enter does not run a button that discards something. Quitting offers Save only when all that is unsaved is in the file on screen. The page range and orientation were not tried on a printer, and Windows' order of buttons was not seen. |
| 16 | The main view | Done. iced cannot fade a whole panel, so menus, dialogs and messages slide 8 px into place and only the dimming behind a dialog fades. The zoom menu, folding, dragging the sidebar's edge and the delayed hints were tried with a pointer on an X display; motion was not watched by a person. |
| 17 | A screenshot set that is kept | Done: `tools/screenshots/review.py` (on an invisible X display, so the focus of the desktop does not matter). A full run was looked at; the sidebar's first position in a small window was wrong, and is fixed. `data/screenshots/` was taken again after package 23. |
| 20 | What opening a file shows | Done. Showing the folder's list makes it what is browsed. |
| 21 | A toolbar joined to the header | Done. What is said of a selected part of a picture (Copy, Crop) stays a small island on the view: in the row it would move the picture under the pointer. Rotate, crop, a slider and a click outside its panel were tried with a pointer on an X display. `data/screenshots/` was taken again after package 23. |
| 22 | The window's first size | Done. The size is read from the picture's header, not decoded. Looked at on an X display without a window manager; not on Wayland desktops, macOS or Windows. SVGs keep the remembered size. |
| 23 | A window for each opening | Done. Linux and Windows: a process for each window; a file already open in another one is not brought forward there (Wayland gives a program no way to raise another's window, and saving asks when the file changed on disk). macOS: one process (`app/windows.rs`), which ends with its last window. `vendor/` did not have to be changed: a widget uploads pictures with its own window's renderer. Tried on Linux with `LUMEN_ONE_PROCESS=1`: the real-app tests with both renderers, and no blank frame while zooming a PDF in a second window (0 of 758; 122 of 272 with iced's own way). Passwords and caches are per window. **Not tried on a Mac.** |
| 18 | Features: the ones from macOS Preview (all of them, see the list), then those the user picks | Under way; each done feature says so in the list. |
| 19 | Confine the decoder on macOS and Windows | |
