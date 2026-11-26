# Working on lumen

A picture and PDF viewer/editor in pure Rust with iced 0.14; the successor of the GTK
Lumen (`~/repos/lumen-gtk`, also on GitHub as ImanolGo/lumen-gtk, no longer developed). `README.md` is the short
version; `docs/FEATURES.md` and `docs/ARCHITECTURE.md` have the features and the source
layout, `docs/STATUS.md` says what is missing, and `docs/parity-plan.md` is the plan for
what is left to do. Read the plan and the docs you need before starting.

## Rules

- **Pure Rust, no C in the document path.** `#![forbid(unsafe_code)]` in the app;
  new dependencies must be pure Rust. Adding a C library (dav1d, libheif, …) is the
  user's decision: ask first.
- `vendor/` holds patched copies of winit, iced_core and iced_winit (touchpad pinch);
  see `vendor/README.md`. Do not edit them for anything else.
- Look and feel lives in `src/style.rs` (design tokens: one surface, floating panels,
  amber accent only for what is active, Inter via `style::FONT`/`style::font(..)`).
  New UI uses those styles, `icon_button`, `tip`, and puts tools in the row under the header (`App::tool_row`).
- Things the user asked to keep like the GTK Lumen: big sidebar thumbnails with the
  name underneath, fading zoom pill and previous/next arrows, the header layout with
  the ☰ menu, the markup style menu (palette, custom colour, size slider).
- Keep the user's private files out of commits and out of anything sent elsewhere.

## Build, check, ship

```sh
cargo build --release
make test                      # cargo test
make lint                      # cargo fmt --check + clippy -D warnings (must be clean)
make install PREFIX=$HOME/.local
```

Check lint by exit status (`cargo clippy --all-targets --locked -- -D warnings && echo OK`),
not by grepping its output. Commit on `main`, push to `origin`, then check CI with
`gh run list -L1`. Commit messages end with:

    Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

Update `docs/FEATURES.md` (features, keyboard table), `docs/ARCHITECTURE.md` (layout table) and
`CHANGELOG.md` with every feature; keep `README.md` short.

## Verifying UI changes (no input injection here)

There is no way to send real mouse, keyboard or touchpad events (no ydotool,
`/dev/uinput` is root-only). Verify like this:

1. **Unit tests** for logic (models, geometry, PDF reading/writing, IPP encoding …).
2. **Demo hooks**: temporarily add code in `App::new` that sends messages on a timer,
   between `// DEMO-HOOK-START` and `// DEMO-HOOK-END`, driven by an env var
   (`LUMEN_DEMO=…`). Remove them before committing: `grep -c DEMO src/app.rs` must be 0.
3. **Screenshots** of the window (niri):
   ```sh
   wl-paste -n > /tmp/clip.bak                       # keep the user's clipboard
   (env LUMEN_DEMO=x ./target/release/lumen FILE &) ; sleep 3
   niri msg action screenshot-window -d false; sleep 0.5
   wl-paste -t image/png > shot.png
   pkill -x lumen                                 # never `pkill -f` (kills the shell)
   wl-copy < /tmp/clip.bak
   ```
   The shell is zsh: to pass several `VAR=x` words from one variable use `env ${=VARS}`.
4. Tell the user plainly what could not be checked with real input.

Useful switches: `LUMEN_THEME=light|dark`, `LUMEN_NO_PORTAL=1` (built-in file chooser and
CUPS print dialog), `RUST_LOG=info` (timing logs: `window built`, `opened load_ms`,
`first view on screen`). `tools/bench.py` compares cold starts with the GTK Lumen.

Test files: `/usr/share/doc/ghostscript/*.pdf` (42 and 477 pages),
`/usr/share/wallpapers/cachyos-wallpapers/` (big PNGs), `tests/fixtures/form.pdf`,
`tests/fixtures/encrypted.pdf` (password `secret`). Printing: the live CUPS test uses
IPP Validate-Job only (`cargo test cups_live -- --ignored`); never send a real print
job without asking.
