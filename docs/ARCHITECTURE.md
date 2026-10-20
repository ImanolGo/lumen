# Architecture

## Stack

| Layer | Choice | Why |
|---|---|---|
| UI toolkit | GTK 4 + libadwaita | Native Linux look, HiDPI, print dialog, portals, accessibility |
| App structure | [Relm4](https://relm4.org) | Elm-style `Model → view → Msg → update`; avoids `Rc<RefCell>` spaghetti |
| Custom drawing | GTK widget subclass + `GtkSnapshot` (GPU) / Cairo (CPU, export, print) | |
| Images | glycin (sandboxed decoding) → `gdk::Texture`; `image` crate for edits and encoding | Decoders never run inside Lumen |
| PDF | `poppler-rs` (render, text, search, annotations), `lopdf` (page restructuring) | |
| Printing | `gtk::PrintOperation` → Cairo | Poppler renders straight into the print context |

> **License note:** Poppler is GPL, so Lumen is **GPL-3.0-or-later**. To go
> permissive later you'd swap in `pdfium-render` (BSD) and bundle PDFium.

## Module layout

```
src/
├── main.rs              entry point, logging, resources, GApplication
├── app.rs               root Relm4 component: window, header bar, actions, message routing
├── app/                 the parts of the window's behaviour, one file each
│   ├── edit.rs          edit stack, crop/adjust modes, saving, guarding unsaved work
│   ├── pages.rs         PDF page operations, working copies, exports
│   ├── markup.rs        markup toolbar state and history
│   ├── forms.rs         filling in PDF forms
│   ├── signature.rs     the signature pad and menu
│   ├── prefs.rs         preferences and About dialogs
│   ├── shortcuts.rs     the shortcuts dialog
│   └── control.rs       the D-Bus control interface (scripts and end-to-end tests)
├── document.rs          file-type detection, open/probe
├── folder.rs            natural ordering, listing folders
├── geometry.rs          widget ↔ layout ↔ page-area transforms (pure, tested)
├── fsutil.rs            replacing files without losing permissions, symlinks or hard links
├── i18n.rs              gettext helpers
├── settings.rs          preferences and remembered state (TOML)
├── signatures.rs        saved signatures
├── thumbnails.rs        background file thumbnails and the desktop thumbnail cache
├── print.rs             GtkPrintOperation
├── save.rs              writing edited pictures (lossless JPEG rotation, ICC profile kept)
├── widgets/             LumenCanvas (zoom, pan, pages, markup, crop), sidebars, properties
├── image/               ImageDocument, glycin decoding (sandboxed), EXIF metadata
├── edit/                EditOp, EditStack (undo/redo), crop, colour adjustment
├── annotate/            pure-Rust annotation model, editor state machine, Cairo renderer
└── pdf/                 Poppler access: renderer, search, selection, links, forms,
                         annotations (Poppler C API), page operations and composing (lopdf)
```

`geometry.rs` holds the tested arithmetic between widget pixels, layout points and areas inside a
page (`View`); the canvas owns the zoom and scroll position it is built from.

## Data flow

```
 user input ──► Widget/controller ──► AppMsg ──► App::update ──► model mutates
                                                     │
                                                     ├─► spawn worker (decode, render, save)
                                                     │         └──► AppMsg::Loaded(..) back
                                                     ▼
                                              #[watch] view refresh
```

- **The main thread only does UI.** Decoding, PDF rendering and saving run on workers and report back via messages. Every request carries a ticket or generation number, and results from older requests are dropped.
- **Poppler is not thread-safe.** Each document has a few worker threads (page renderer, thumbnail renderer, search, selection, links), each with its own `poppler::Document` opened on its first job. All Poppler calls take one process-wide lock (`pdf::poppler_lock`).
- **Pictures are decoded in glycin's sandbox**, for viewing, thumbnails, editing and saving alike (`image::decode`). Only metadata (EXIF, colour profile) is read in-process.
- **Documents are immutable plus a history.** What you see = original + edit stack + annotation layer. A picture's edits live in `EditStack`. A PDF's page changes and form fills each write a *working copy* of the file (in the cache folder), and the history of (working copy, annotations) states is the undo history; the original is only replaced on save.
- **Shared state** between the canvas and the markup code (`annotate::Store`) is the one place that uses `Rc<RefCell<…>>`, because the canvas draws it while the window edits it.
- **Scripts and tests** drive the real app through the `io.github.imanolgo.Lumen.Control1` D-Bus interface (`app/control.rs`, `tests/control.rs`).

## Coordinate spaces

| Space | Units | Origin |
|---|---|---|
| Widget | logical px | top-left of the canvas |
| Document (image) | image px | top-left of the image |
| Page (PDF) | points (1/72") | top-left in Poppler's API (*bottom*-left in raw PDF) |

All conversions live in `geometry.rs` and are unit-tested. Annotations are always stored in document or page space, never in widget space.
