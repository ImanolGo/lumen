# Closing the gap with the GTK Lumen

A plan for a fresh session. Read `CLAUDE.md` first (rules, how to verify without
real input), then work through the packages below **in order**, one at a time:
each ends with tests, lint, README, a commit, a push, a green CI run and
`make install PREFIX=$HOME/.local`. Tick the status table at the end as you go,
in the same commit.

## Where we are (23 Nov 2026, commit 4a52aa1)

lumen-rs covers roughly 80 % of the GTK Lumen 0.4.2: viewing, folders, PDF reading
(contents, search, selection, links, column-aware reading order, passwords),
markup, forms, signatures, picture editing (turn, mirror, crop, resize, colour),
page organising, printing (portal or direct CUPS), built-in file chooser, pinch zoom.

### Benchmarks (`tools/bench.py`, medians of 5 cold starts, this laptop)

| | GTK Lumen | lumen-rs |
|---|---|---|
| First view, PDF 42 p / 477 p | 312 / 397 ms | **165 / 190 ms** |
| First view, JPEG 2256×1820 / PNG 5120×2160 | 357 / 440 ms | **203 / 286 ms** |
| Window built | **~125 ms** | ~140 ms |
| CPU to load: PDFs / pictures | 450–610 / **380–470 ms** | **~265** / 480–930 ms |
| Idle CPU over 3 s after loading | 60–350 ms | **0** (but 290 ms in the 5K folder) |
| RSS: PDF / JPEG / PNG 5K | 125–163 / **96 / 118 MB** | **118** / 139 / 457–502 MB |
| Binary / shared libraries | **13 MB** / 145 | 33 MB / **5** |
| Tests | **143** + D-Bus control suite | 84 |

(The GTK Lumen decodes pictures in separate glycin processes, so its picture RSS
does not count the decoders.)

### What the GTK Lumen has and lumen-rs lacks

Formats (HEIC, AVIF, JPEG XL, JPEG 2000, OpenEXR), animation (GIF, APNG, WebP),
sandboxed decoding, lossless JPEG rotation and saving in place, page thumbnails in
the sidebar, go to page, night mode, document/picture properties, export pages as
PNG/PDF, slideshow, preferences, show in folder / open with / move to trash,
translations (es), accessibility names, a D-Bus control interface, packages
(Flatpak, .deb, AUR), more tests.

## Decisions to ask the user about first

Ask these at the start (one `AskUserQuestion` call), because they change the work:

1. **AVIF and HEIC** need C decoders (dav1d, libheif); everything else can stay
   pure Rust. Allow them as an optional cargo feature, or skip them?
2. **Sandboxed decoding** (package 8): worth the extra process per decode?
3. **Go to page shortcut**: the GTK Lumen uses Ctrl+G for "go to page"; lumen-rs
   uses Ctrl+G for "next search match". Which wins?
4. **Saving edited pictures in place** (the GTK Lumen saves JPEG/PNG/WebP in place;
   lumen-rs always saves a copy). Keep copy-only, or save in place with Ctrl+S and
   copy with Ctrl+Shift+S?

Also ask about the open bug in package 1 if the user has not answered yet.

## Packages

### 1. Open bugs and memory budget

- **Two-finger vertical scrolling** does not work for the user (horizontal does). Not
  reproduced; no input injection here. Ask: where (PDF, zoomed picture, sidebar,
  organiser), what happens instead (nothing, zoom), and does a mouse wheel work there.
  Already ruled out: iced `Stack` levitation (the floating layers report no
  interaction outside their buttons). Already fixed: modifiers are now tracked from
  all events, so a stuck Ctrl can no longer turn vertical scrolling into zoom.
  Suspects to check: a nested scrollable capturing vertical wheel events, the
  `pannable` mouse_area, `Message::Wheel` (`src/app.rs`).
- **Preload memory budget** (`preload_neighbours`, `PICTURE_CACHE`, `request_thumbs`
  in `src/app.rs`): skip decoding neighbours whose pixel count is above a limit
  (say 24 MP, or a total budget of ~256 MB for the cache), drop `Picture` bytes once
  the GPU handle exists if possible, and decode sidebar thumbnails with
  `image::DynamicImage::thumbnail` from a reduced decode (JPEG: use the decoder's
  scale-down via `image` `JpegDecoder` with `set_limits`/DCT scaling if available,
  or the EXIF thumbnail, which is already used).
  **Done when:** PNG 5K folder RSS < 200 MB and idle CPU returns to 0 within 5 s;
  `tools/bench.py` shows no regression elsewhere.

### 2. PDF reading parity

- **Page thumbnails** as a third sidebar tab ("Pages"), virtualised like the file list,
  rendered at small scale through `pdf::Document::render`; click goes to the page,
  the current page is ringed (`style::thumb_frame`).
- **Go to page**: a small field (shortcut per decision 3), also by clicking "Page x of y"
  in the header.
- **Night mode** for PDFs: invert lightness of rendered pages (keep hue), in the ☰ menu
  and remembered per session; pictures inside pages may stay inverted (as in the GTK
  Lumen) — check what it does in `~/repos/lumen/src` (grep `night`).
- **Properties** dialog (☰ menu): file name, size, path, dates; PDF title, author,
  subject, producer, creator, PDF version, page count and size, encryption, form fields;
  picture: dimensions, format, colour profile, EXIF summary (camera, lens, exposure,
  date, GPS). hayro exposes metadata (`Pdf::metadata`); EXIF via `kamadak-exif`.
**Done when:** each works on the test PDFs, with unit tests for metadata reading.

### 3. Export

- Export the current page (or selected pages in the organiser) as **PNG** at a chosen
  resolution (72/150/300 dpi) — render through hayro at that scale.
- Export selected pages as a **new PDF** (`pages::compose` with a sub-plan).
- Export a **picture as PDF** (reuse `print::picture_to_pdf`).
- "Merge" = organiser + Insert, already there; add "Combine Files…" to the ☰ menu that
  opens the organiser and the insert chooser.
All through the save chooser (`App::ask_for_files` with a new `Purpose`).

### 4. File actions

- **Show in folder**: D-Bus `org.freedesktop.FileManager1.ShowItems` (zbus is already a
  dependency through ashpd), fallback `xdg-open` on the folder.
- **Open with…**: OpenURI portal with `ask(true)`; no fallback needed.
- **Move to trash**: portal `Trash`, fallback the freedesktop trash spec in Rust
  (`~/.local/share/Trash/files` + `.trashinfo`), then show the next file, with an
  "Undo" toast that restores it.

### 5. Slideshow

Fullscreen, next file every N s (default 5, from preferences), pause with Space,
leave with Esc; skips PDFs or shows their first page (check the GTK Lumen).

### 6. Pictures

- **Animation**: play GIF, APNG and animated WebP with `image`'s `AnimationDecoder`
  (`into_frames`); frames on a timer subscription; cap decoded frames by memory
  (decode on the fly past the cap); pause while editing/markup; the first frame is
  what edits apply to.
- **JPEG XL** with `jxl-oxide` (pure Rust); **JPEG 2000** with `hayro-jpeg2000`
  (already in the tree); **OpenEXR** with `image`'s `exr` feature (pure Rust, tone-map
  to 8 bit). Add to `library::Kind`, the file chooser filter, the `.desktop` MIME list.
- **AVIF/HEIC** only per decision 1 (optional feature, off by default).
- **Lossless JPEG rotation**: when only turns/mirrors were applied to a JPEG, write the
  original bytes with the EXIF orientation tag changed (rewrite the APP1 segment;
  `kamadak-exif` reads it, `src/metadata.rs` writes EXIF) instead of re-encoding.
- **Save in place** per decision 4 (atomic write via `pdf::write_atomic`, keep EXIF).
**Done when:** test files for each format decode (add tiny fixtures generated in tests),
animation plays in a demo-hook screenshot sequence, a rotated JPEG's pixel data is
byte-identical to the original.

### 7. Preferences

`~/.config/lumen-rs/settings.toml` (serde + toml are dependencies): theme
(system/light/dark), slideshow interval, default PDF fit, sidebar open by default,
floating controls on/off, portals on/off (same as `LUMEN_NO_PORTAL`), preload budget.
A dialog from the ☰ menu in the existing dialog style (`src/app/menu.rs`).

### 8. Sandboxed decoding (per decision 2)

Decode pictures in a child process (`lumen-rs --decode-worker`), restricted with
`landlock` (filesystem: read the one file) and `seccompiler` (syscalls), pixels back
over a pipe or memfd. Fall back to in-process decoding if the sandbox cannot start.
**Done when:** overhead < 15 ms per picture (measure with `tools/bench.py`), a crashing
decoder only produces an error message.

### 9. Product finish

- **Translations**: wrap UI strings in a `tr!` macro backed by a pure-Rust gettext
  reader (`gettext` crate, reads `.mo`), port `~/repos/lumen/po/es.po` where strings
  match; English stays the source.
- **Accessibility**: iced 0.14 has no screen reader support; note it in the README and
  re-check when iced gains it. Make sure every icon button has a tooltip label.
- **D-Bus control interface** (zbus): `io.github.imanolgo.LumenRs` with Open, Next,
  Previous, Zoom, GoToPage, State; port the scenarios of `~/repos/lumen/tests/control.rs`
  as integration tests (they also give real end-to-end UI tests).
- **Packages**: AUR `lumen-rs` and `lumen-rs-git` PKGBUILDs (`build-aux/aur/`), a GitHub
  release workflow building an x86_64 tarball, `.deb` via `cargo-deb`, a Flatpak manifest
  (`flatpak-cargo-generator` for sources). Ask before publishing anything to the AUR.
- **Tests**: bring the count past the GTK Lumen's (143), especially app-level message
  flows (`App::new` + `App::update` without a window).

### 10. PDF rendering fidelity

Compare hayro with poppler on a corpus (the Ghostscript PDFs, `tests/fixtures`,
pdf.js test PDFs if downloaded into a scratch folder): render page images with
`pdftoppm` and with lumen-rs (`pdf::Document::render`, a small ignored test that
writes PNGs), compute a pixel difference per page, and list the worst pages with
screenshots. Fix what is ours; report upstream to hayro what is not.

## Status

| # | Package | Status |
|---|---|---|
| 1 | Open bugs and memory budget | done (bench re-run: PDFs unchanged, 5K PNG first view 286 → 242 ms, RSS 457–502 → 148 MB); scroll bug confirmed fixed by the user; cache byte budget, preload limit 4 MP, fixed worker pool, streamed PNG and DCT-scaled JPEG thumbnails (5K folder 458 → 149 MB) |
| 2 | PDF reading parity | done: Pages tab, go to page (Ctrl+G), night mode (Ctrl+Shift+N, session only until package 7), Properties dialog |
| 3 | Export | done: PNG at 72/150/300 dpi, selected pages as PDF, picture as PDF, Combine Files… |
| 4 | File actions | done: show in folder, open with, move to trash with Undo (spec trash in Rust; no portal trash, since it cannot undo); no Ctrl+Delete shortcut on purpose |
| 5 | Slideshow | done: F5, 5 s fixed until package 7 makes it a preference; PDFs advance page by page |
| 6 | Pictures (animation, formats, lossless JPEG) | partly: JPEG XL, JPEG 2000, OpenEXR and GIF/APNG/WebP animation done; AVIF/HEIC (optional C feature), lossless JPEG rotation and save in place to do |
| 7 | Preferences | todo |
| 8 | Sandboxed decoding | todo |
| 9 | Product finish | todo |
| 10 | PDF rendering fidelity | todo |

After all packages: re-run `tools/bench.py`, update the tables at the top, and write
the user a short comparison of what changed.
