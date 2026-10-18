# Architecture

## Stack

| Layer | Choice | Why |
|---|---|---|
| UI toolkit | GTK 4 + libadwaita | Native Linux look, HiDPI, print dialog, portals, accessibility |
| App structure | [Relm4](https://relm4.org) | Elm-style `Model → view → Msg → update`; avoids `Rc<RefCell>` spaghetti |
| Custom drawing | GTK widget subclass + `GtkSnapshot` (GPU) / Cairo (CPU, export, print) | |
| Images | `gdk::Texture` (Phase 1), `image` crate for edits (Phase 4), optional `glycin` | |
| PDF | `poppler-rs` (render, text, search, annotations), `lopdf` (page restructuring) | |
| Printing | `gtk::PrintOperation` → Cairo | Poppler renders straight into the print context |

> **License note:** Poppler is GPL, so Lumen is **GPL-3.0-or-later**. To go
> permissive later you'd swap in `pdfium-render` (BSD) and bundle PDFium.

## Target module layout

Modules marked *(Phase N)* don't exist yet; they're created by that phase of [PLAN.md](PLAN.md).

```
src/
├── main.rs              entry point, logging, RelmApp
├── app.rs               root component: window, header bar, actions, routing
├── document.rs          file-type detection, open/probe
├── geometry.rs          (Phase 1) widget ↔ document ↔ page coordinate transforms
├── widgets/
│   └── canvas.rs        (Phase 1) LumenCanvas: zoom/pan/draw, shared by images and PDF pages
├── image/               (Phase 1) ImageDocument, folder browsing, loading
├── pdf/                 (Phase 2) PdfDocument, render worker, page cache, search
├── print.rs             (Phase 3)
├── edit/                (Phase 4) EditOp, EditStack (undo/redo), save/export
└── annotate/            (Phase 5)
    ├── model.rs         pure Rust: shapes, hit-testing, no GTK
    ├── render.rs        draw an AnnotationLayer on a cairo::Context
    ├── tools.rs         tool state machine
    └── pdf_io.rs        model ⇄ poppler annotations
```

## Data flow

```
 user input ──► Widget/controller ──► AppMsg ──► App::update ──► model mutates
                                                     │
                                                     ├─► spawn worker (decode, render, save)
                                                     │         └──► AppMsg::Loaded(..) back
                                                     ▼
                                              #[watch] view refresh
```

- **The main thread only does UI.** Decoding, PDF rendering and saving run on workers and report back via messages (Relm4 commands).
- **Poppler is not thread-safe.** A single render-worker thread owns the `poppler::Document` and takes requests over a channel.
- **Documents are immutable plus an edit stack.** What you see = original + `EditStack` ops + annotation layer. Saving is the only time bytes are written.

## Coordinate spaces

| Space | Units | Origin |
|---|---|---|
| Widget | logical px | top-left of the canvas |
| Document (image) | image px | top-left of the image |
| Page (PDF) | points (1/72") | top-left in Poppler's API (*bottom*-left in raw PDF) |

All conversions live in `geometry.rs` and are unit-tested. Annotations are always stored in document or page space, never in widget space.
