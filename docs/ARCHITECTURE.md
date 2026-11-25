# Architecture

## Stack

| Layer | Choice | Why |
|---|---|---|
| UI toolkit | [iced](https://iced.rs) 0.14 (wgpu, with a tiny-skia CPU fallback) | Pure Rust, GPU drawing, runs on any Wayland or X11 desktop without GTK or Qt |
| App structure | iced's Elm architecture: `App` state, `Message`, `update`, `view` | One place decides what a click or a key does; easy to test without a window |
| Pictures | [`image`](https://github.com/image-rs/image), `jxl-oxide`, `hayro-jpeg2000`, `png`, `jpeg-decoder`, [`resvg`](https://github.com/linebender/resvg) | All pure Rust. JPEG XL, JPEG 2000 and OpenEXR are decoded here too; AVIF and HEIC need the optional `heif` feature (libheif, C) |
| PDF reading | [`hayro`](https://github.com/LaurenzV/hayro) | Pure Rust, `forbid(unsafe_code)`, renders on a pool of background threads |
| PDF writing | [`lopdf`](https://github.com/J-F-Liu/lopdf), `subsetter` | Annotations, form values, page restructuring, incremental updates |
| Sandbox | `landlock`, `seccompiler` | Pictures are decoded in a locked-down child process |
| Desktop | [`ashpd`](https://github.com/bilelmoussaoui/ashpd) / `zbus` (XDG portals, D-Bus) | File and print dialogs, opening links, the control interface |
| Printing without a portal | A small IPP client over CUPS's socket (`src/cups.rs`) | No libcups |

The app itself is `#![forbid(unsafe_code)]`. `vendor/` holds patched copies of three
dependencies (touchpad pinch); see `vendor/README.md`.

## Rules the code follows

- **No C in the document path.** New dependencies must be pure Rust. The one exception, libheif for AVIF and HEIC, is an optional cargo feature that is off by default.
- **Only `src/platform/` names a system.** The rest of the code calls it, and asks `platform::caps()` what to show.
- **Never block the interface.** Decoding, rendering, searching, saving and exporting run on worker threads or in the decoder process; the UI thread only reacts to messages.
- **Logic lives outside the widgets.** Geometry, edit operations, the annotation model, page plans and PDF reading and writing are plain modules with unit tests. The `app/` files turn them into messages and views.
- **One look.** Design tokens are in `src/style.rs`: one surface, floating panels, the amber accent only for what is active, Inter bundled. New UI uses `icon_button`, `tip` and floats its tools as islands.
- **Every visible string goes through `tr()`** (or `trf!()` when it has `{}` holes), and every one needs an entry in `po/es.po`; a test enforces it.

## Data flow

```
 file ──► library (browse the folder) ──► App::show ──► raster::load ──► sandbox worker ──► Picture ──► image::Handle ──► view
                                                    └─► pdf::Document ──► render pool ──► pages ──► slots ──► view
 clicks, keys, timers ──► Message ──► App::update ──► new state (+ a Task for the slow part) ──► view
```

- **Pictures.** `raster::load` asks `sandbox::run` to decode in `lumen-rs --decode-worker`. The parent hands the open file to the worker as its standard input; the worker shuts itself in (Landlock, then seccomp) before reading it, and writes the pixels back over a pipe. The same worker makes thumbnails and print copies, and streams the frames of animations. It sends one byte once it is shut in; only a worker that ends without it, with the "no sandbox" status, means the sandbox is unavailable. Only then do the decoders run in-process; any other failure is an error for that picture. SVG and PDF are not sandboxed: they need fonts and file access, and they are memory-safe Rust.
- **PDFs.** `pdf::Document` parses the file once per render thread. Only the pages near the view are rendered, newest request first, and re-rendered at every zoom level. Text, links and the outline are read lazily (`structure.rs`) for search and selection.
- **Editing.** A picture keeps the original and an `Edits` value (turns, mirror, crop, resize, colour); the shown picture is computed in the background. Markup is kept in picture or page coordinates and follows every edit. Saving flattens it (pictures) or writes real annotations (PDFs).
- **Saving a PDF** appends an incremental update, so the original bytes stay as they were: `pdf::Document` keeps the bytes it was opened from, and changes are only ever applied to those. Page changes rewrite the file (`pages.rs`), carrying markup, form values and a password along.
- **Coordinate spaces.** Page points (1/72 inch, origin top left, after the page's own rotation) for PDFs; picture pixels for pictures; logical pixels on screen. `view.zoom` converts between them, and the overlay canvas places annotations with it.
- **State.** Everything the user can see is a field of `App` (`src/app.rs`); preferences are in `settings::Settings` and are written when changed.

## Layout

### Core

| File | What it does |
|---|---|
| `src/main.rs` | Entry point; the decoder worker mode (`--decode-worker`) starts here before anything else |
| `src/app.rs` | State, messages, update and view |
| `src/style.rs`, `src/icons.rs` | Design tokens, widget styles, symbolic icons |
| `src/settings.rs`, `src/app/prefs.rs` | Preferences: the settings file, and the dialog |
| `src/app/commands.rs` | The command palette (Ctrl+Shift+P): the menus' commands, filtered by name |
| `src/clipboard.rs` | Pictures as PNGs for the clipboard |
| `src/thumbcache.rs` | A shared thumbnail cache (freedesktop standard): read, write, MD5 naming |
| `src/fsio.rs` | Writing files all at once, private folders and files |
| `src/state.rs` | What Lumen remembers between runs: window size, recent files, each PDF's page |
| `src/i18n.rs`, `po/` | Translations: the `.po` reader, `tr()`, the catalogues |
| `assets/fonts` | Inter (Latin), under the SIL Open Font License (`OFL.txt`) |

### Pictures

| File | What it does |
|---|---|
| `src/raster.rs` | Picture decoding (every format), SVG rendering, thumbnails |
| `src/sandbox.rs` | The sandboxed decoder process, and the parent side that runs it |
| `src/animation.rs`, `src/app/anim.rs` | Animated pictures: decoding the frames (in the worker), and playing them |
| `src/heif.rs` | AVIF and HEIC through libheif (only with `--features heif`) |
| `src/library.rs` | Which files to browse: folders, natural sort order |
| `src/edit/` | Picture edits: turns, mirrors, crop geometry, resizing, colour adjustments, and how each moves markup |
| `src/app/edit.rs` | The editing bar, crop overlay and colour panel |
| `src/lossless.rs` | Lossless JPEG turning (the EXIF orientation tag) |
| `src/metadata.rs` | Carrying EXIF data and colour profiles into saved copies |

### PDFs

| File | What it does |
|---|---|
| `src/pdf.rs` | PDF documents and their render-thread pool |
| `src/structure.rs` | Outline, links and page text (reading order, search) |
| `src/forms.rs`, `src/app/forms.rs` | Form fields: reading, writing values and their appearances; the inputs over the page |
| `src/pages.rs`, `src/app/pages.rs` | Page plans (reorder, turn, delete, insert), writing them, and the page organiser |
| `src/app/pagelist.rs` | The Pages sidebar tab and the go-to-page box |
| `src/redact.rs` | Taking removed pages out of a PDF for good, with everything that leads to them |
| `src/merge.rs` | What inserted pages bring along: fields, named places, outline |
| `src/textfont.rs` | Fonts for text written into PDFs: Helvetica, or an embedded part of a system font |
| `src/properties.rs` | What the Properties dialog shows |
| `src/export.rs`, `src/app/export.rs` | Exporting pages as PNG or PDF, pictures as PDF; the Export dialog |

### Markup and signatures

| File | What it does |
|---|---|
| `src/annotate/` | The annotation model and editor, the shared drawing description, PDF annotations in and out, pictures with markup |
| `src/app/markup.rs`, `src/app/palette.rs`, `src/app/overlay.rs` | The markup bar, colour and size menu, and the canvas that draws annotations over a page and takes the pointer |
| `src/signatures.rs`, `src/app/signature.rs` | Saved signatures; the signature menu, drawing pad and placing |

### Window and desktop

| File | What it does |
|---|---|
| `src/app/menu.rs` | The header's menus, and the keyboard shortcuts, properties and about dialogs |
| `src/app/slideshow.rs` | The slideshow |
| `src/app/chooser.rs`, `src/app/printing.rs` | Lumen's own file chooser and print dialog, for desktops without a portal |
| `src/platform/mod.rs` | The operating system behind the same functions everywhere (`dirs`, `dialogs`, `shell`, `clipboard`, `watch`, `trash`, `print`, `confine`, `startup`, `thumbs`, `control`, `fs`), and `caps()`: what this system can do |
| `src/platform/linux/` | Linux: XDG folders, portals, freedesktop trash and thumbnails, inotify, Wayland clipboard, CUPS socket, Landlock and seccomp, D-Bus |
| `src/platform/desktop/` | macOS and Windows: what they share (`mod.rs`: `directories`, `rfd`, `arboard`, `notify`, `sys-locale`) and what only one does (`macos.rs`, `windows.rs`: `trash`, opening links, printing) |
| `src/platform/unix_fs.rs`, `unix_print.rs` | Private files, and CUPS's socket (Linux and macOS) |
| `src/print.rs`, `src/cups.rs` | Turning pictures into PDFs to print; printing straight to CUPS (IPP) |
| `src/control.rs`, `src/app/control.rs` | The control interface on the session bus, and running its commands |

### Tests and tools

| File | What it does |
|---|---|
| `src/app/tests.rs` | App message flows, driven without a window |
| `tests/sandbox.rs` | Every format through the real sandboxed worker, and the escape attempts |
| `tests/control.rs` | The real app, driven over D-Bus (opt-in: it opens windows) |
| `tests/fixtures/` | Small PDFs and pictures the tests use |
| `tools/bench.py` | Cold-start benchmark against the GTK Lumen |
| `tools/screenshots/` | Makes the sample files for screenshots |
| `tools/update-flatpak-sources.py`, `tools/check-flatpak-sources.py` | Rebuilds / checks the Flatpak crate list against `Cargo.lock` |
| `tools/fidelity.py`, `tools/make_feature_pdf.py` | Compares PDF rendering with poppler; makes a feature-test PDF |

### Packaging

| File | What it does |
|---|---|
| `data/` | Desktop entry, AppStream metadata, icons |
| `build-aux/aur/`, `build-aux/flatpak/` | PKGBUILDs and the Flatpak manifest |
| `.github/workflows/` | CI and the release workflow |
