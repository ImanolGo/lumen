# Lumen — Development Plan

A step-by-step roadmap from an empty window to a Preview.app-class viewer for
Linux. Every step is small enough to finish in one sitting (1–4 h), ends with
something you can **see working**, and should be its own commit (or PR).

**How to use this file**

- Work top to bottom. Phases build on each other; steps inside a phase mostly do too.
- Tick the box (`- [x]`) in the same commit that finishes the step.
- Each step has **Goal → Tasks → Hints → Done when**. "Done when" is your
  acceptance test; don't move on until it passes.
- 🧠 marks a concept worth reading up on before you start the step.
- Effort: 🟢 small · 🟡 medium · 🟠 large.

---

## Phase 0 — Foundations ✅

- [x] **0.1 Project skeleton** — Cargo project, GPL-3.0-or-later, Relm4 + libadwaita window.
- [x] **0.2 Open flow** — `Ctrl+O` file dialog (filters for images and PDFs), file path from CLI.
- [x] **0.3 Stack check** — `document::probe` decodes images with GDK and PDFs with Poppler, shows size / page count.
- [x] **0.4 Docs** — README, this plan, architecture notes, dev guide.

**Before Phase 1:** read the [Relm4 book](https://relm4.org/book/stable/) chapters
*Simple app*, *Components* and *Factory*, and skim
[GUI development with Rust and GTK 4](https://gtk-rs.org/gtk4-rs/stable/latest/book/)
(especially *GObject concepts* and *Subclassing*). You'll need subclassing in 1.2.

---

## Phase 1 — Image viewer

Goal of the phase: Lumen replaces your current image viewer for everyday use.

### 1.1 ✅ Show the image 🟢
**Goal:** opening a picture displays it, fitted to the window.
- Tasks
  - Add `src/image/mod.rs` with an `ImageDocument { path, texture: gdk::Texture }`.
  - Replace the `adw::StatusPage` placeholder with a `gtk::Stack`: page `"empty"` (status page) and page `"image"`.
  - Show the texture in a `gtk::Picture` with `ContentFit::Contain`.
- Hints: `gdk::Texture::from_filename`, `gtk::Picture::set_paintable`. Keep the status page for the empty state and for errors.
- **Done when:** `cargo run -- photo.jpg` shows the photo; resizing the window rescales it.

### 1.2 ✅ Custom canvas widget with zoom 🟠 🧠 GObject subclassing, `GtkSnapshot`
**Goal:** zoom in and out around the cursor.
`gtk::Picture` can't zoom, so you build the widget that *everything* later (PDF pages, annotations) will draw on.
- Tasks
  - Create `src/widgets/canvas.rs`: a `gtk::Widget` subclass `LumenCanvas` holding a texture, `zoom: f64` and `offset: (f64, f64)`.
  - Override `snapshot()`: `snapshot.translate` + `snapshot.scale` + `snapshot.append_texture`.
  - Override `measure()` so the widget reports the zoomed size.
  - Add a "Best Fit" mode (zoom computed from the allocation) and a "manual" mode.
  - Ctrl+scroll zooms (`gtk::EventControllerScroll`), keeping the point under the cursor fixed.
  - Shortcuts: `Ctrl+=` / `Ctrl+-` zoom, `Ctrl+0` best fit, `Ctrl+1` 100 %.
- Hints: use `gsk::ScalingFilter::Trilinear` via `snapshot.append_scaled_texture` for nice downscaling and `Nearest` above 200 % (pixel-peeping).
  Loupe's `src/widgets/image/` is the best reference implementation anywhere.
- **Done when:** you can zoom smoothly to 800 % and back; the point under the cursor does not jump.

### 1.3 ✅ Pan and scroll 🟡 🧠 `gtk::Scrollable`
**Goal:** move around a zoomed image.
- Tasks
  - Implement `gtk::Scrollable` on the canvas (h/v adjustments) and put it in a `gtk::ScrolledWindow`.
  - Drag to pan with `gtk::GestureDrag`; change the cursor to `grab`/`grabbing`.
  - Touchpad pinch zoom with `gtk::GestureZoom`.
- **Done when:** scrollbars, wheel, touchpad two-finger scroll, drag and pinch all behave like Loupe or Firefox.

### 1.4 ✅ Open files from anywhere 🟡
**Goal:** Lumen is a proper desktop citizen.
- Tasks
  - Switch to `gio::ApplicationFlags::HANDLES_OPEN` and handle `connect_open` (remove the manual `args_os` parsing in `main.rs`).
  - Drag-and-drop a file onto the window (`gtk::DropTarget` for `gdk::FileList`).
  - Install `data/io.github.imanolgo.Lumen.desktop` locally (see DEVELOPMENT.md) with `MimeType=` so "Open With → Lumen" works.
  - A recent-files menu (`gtk::RecentManager`).
- **Done when:** double-clicking a PNG in Nautilus or Dolphin (with Lumen chosen) opens it; dropping a file works.

### 1.5 ✅ Load off the main thread 🟡 🧠 async in GTK
**Goal:** no UI freeze on a 50 MP JPEG.
- Tasks
  - Decode with `gio::spawn_blocking` (or `relm4::spawn` + a `oneshot` channel) and send the result back as a message.
  - Show an `adw::Spinner` while loading, and cancel stale loads if the user opens another file.
- Hints: Relm4 *Commands* (`Component::update_cmd`) are designed for exactly this.
- **Done when:** opening a huge image keeps the window responsive.

### 1.6 ✅ Browse the folder 🟡
**Goal:** ←/→ go to the previous or next image in the same directory.
- Tasks
  - `ImageList` sorted naturally (`photo2` < `photo10`; there's the `natord` crate), filtered to supported MIME types.
  - Overlay prev/next buttons (`gtk::Overlay`, appear on hover), Space/Backspace as alternatives.
  - Preload the neighbours for instant switching.
- **Done when:** holding → flips through 200 photos without stutter.

### 1.7 ✅ Correct orientation and metadata 🟢
- Tasks
  - Read EXIF orientation (`kamadak-exif` crate) and apply it at display time.
  - Properties side panel (`adw::OverlaySplitView`): dimensions, file size, camera, date, GPS.
- **Done when:** phone photos are never sideways; `Alt+Enter` toggles properties.

### 1.8 (Optional) Switch to glycin 🟡
Install `glycin` + `glycin-loaders` and use `glycin::Loader`. This gives sandboxed decoding, HEIC/AVIF/JXL/RAW and animated images, and it's what Loupe uses. You could do this later in Phase 7 instead.

---

## Phase 2 — PDF viewer ✅

### 2.1 ✅ Render one page 🟢 🧠 Cairo surfaces
**Goal:** show page 1 of a PDF.
- Tasks
  - `src/pdf/mod.rs`: `PdfDocument` wrapping `poppler::Document`.
  - Render: create a `cairo::ImageSurface` at `page_size × scale × device_scale_factor`, `cr.scale(..)`, `page.render(&cr)`, then turn it into a `gdk::MemoryTexture`.
  - Paint a white background first (PDF pages are transparent).
- Hints: `widget.scale_factor()` for HiDPI. Reuse `LumenCanvas` to display the texture.
- **Done when:** a PDF page shows crisp text on both 1× and 2× displays.

### 2.2 ✅ Navigate pages 🟢
- Tasks: page Up/Down, Home/End, a "Page N of M" entry in the header bar, `Ctrl+G` "go to page".
- **Done when:** you can jump to any page of a 300-page PDF.

### 2.3 ✅ Continuous scroll with lazy rendering 🟠 🧠 virtualised lists, caching
**Goal:** scroll through the whole document like a real reader.
- Tasks
  - Lay out all pages vertically (sizes are known without rendering: `page.size()`).
  - Render only the visible pages ± 1, on a worker thread. Poppler documents are **not** thread-safe, so either one render thread owns the document, or each thread opens its own.
  - LRU texture cache bounded by memory (`lru` crate).
  - While zooming, show a scaled low-res texture, then re-render at the final zoom.
- **Done when:** scrolling a 500-page PDF is smooth and memory stays flat.

### 2.4 ✅ Thumbnails sidebar 🟡
- Tasks: `adw::OverlaySplitView` + `gtk::ListView` (virtualised) with small renders; click to jump; highlight the current page.
- **Done when:** `F9` toggles the sidebar and thumbnails appear lazily.

### 2.5 ✅ Outline (table of contents) 🟢
- Tasks: `poppler::IndexIter` → `gtk::TreeListModel`; clicking jumps to the destination.

### 2.6 ✅ Text search 🟡
- Tasks: `Ctrl+F` search bar; `page.find_text_with_options`; highlight the matches (draw rectangles in the canvas snapshot); Enter/Shift+Enter step through them. Search in the background, page by page.
- **Done when:** searching a word highlights every hit across the document.

### 2.7 ✅ Text selection and copy 🟠
- Tasks: map widget coordinates to page coordinates (write and unit-test this transform!), `page.selected_region` / `page.selected_text`, draw the selection, `Ctrl+C` to the clipboard.
- **Done when:** you can select a paragraph and paste it elsewhere.

### 2.8 ✅ Links and passwords 🟢
- Tasks: clickable internal and external links (`page.link_mapping`), a password dialog for encrypted PDFs.

---

## Phase 3 — Printing ✅

### 3.1 ✅ Print a PDF 🟡 🧠 `GtkPrintOperation`
- Tasks
  - `src/print.rs`: `gtk::PrintOperation`, `set_n_pages`, and in `connect_draw_page` call `poppler_page.render_for_printing(&ctx.cairo_context())`.
  - Respect the page orientation per page (`connect_request_page_setup`).
  - Remember the print settings for the session.
- **Done when:** `Ctrl+P` prints a multi-page PDF correctly, including "Print to File → PDF".

### 3.2 ✅ Print an image 🟢
- Tasks: one page; scale to fit within the printable area keeping the aspect ratio; centre; auto-rotate landscape images. Add a "Fit to page / Actual size" option via `connect_create_custom_widget`.
- **Done when:** a landscape photo fills an A4 or Letter page nicely.
- **Note:** under the desktop portal (the default on GNOME/KDE/Niri) GTK cannot show a custom "Scaling" tab in the print dialog, so the same choice lives in the print button's menu. The tab still appears where the portal is not used.

---

## Phase 4 — Basic image editing ✅

### 4.1 ✅ Edit pipeline and undo 🟡 🧠 command pattern
**Goal:** the foundation for every edit.
- Tasks
  - `src/edit/mod.rs`: `enum EditOp { Rotate(Rotation), FlipH, FlipV, Crop(Rect), Resize(w,h) }`.
  - `EditStack { ops: Vec<EditOp>, cursor }` with undo/redo; the displayed image = original + ops applied.
  - Ops are applied to an `image::DynamicImage` (add the `image` crate), off the main thread.
  - Unit-test the op composition (e.g. 4× rotate = identity).
- **Done when:** `Ctrl+Z` / `Ctrl+Shift+Z` work for any sequence of the ops below.

### 4.2 ✅ Rotate and flip 🟢
- Tasks: `Ctrl+R` / `Ctrl+Shift+R`, flip in the menu. Display-only until saved.

### 4.3 ✅ Save, Save As, Export 🟡
- Tasks
  - `Ctrl+S` writes back (asking before overwriting); `Ctrl+Shift+S` Save As with a format choice (PNG/JPEG/WebP), plus a JPEG quality setting.
  - **Lossless JPEG rotation:** for rotate-only edits, update the EXIF orientation tag instead of re-encoding (`little_exif` crate) so quality is never lost.
  - Preserve EXIF on re-encode where possible.
  - Warn on close with unsaved changes (`adw::AlertDialog`).
- **Done when:** rotating a JPEG and saving gives a byte-identical image body (only the tag changed).

### 4.4 ✅ Crop 🟠
- Tasks: a crop mode with draggable handles drawn on the canvas, aspect presets (free, 1:1, 4:3, 16:9), Enter to apply, Esc to cancel.

### 4.5 ✅ Resize and adjust 🟡
- Tasks: a resize dialog (keep aspect, px/%); optionally exposure/contrast/saturation sliders with a live preview (do this on the GPU via a `gsk::GLShader`, or with the CPU plus a downscaled preview).

---

## Phase 5 — Annotations (the "Markup toolbar") ✅

This is the heart of "Preview-like". **Design the model first, UI second.**

### 5.1 ✅ Annotation model 🟡 🧠 retained-mode scene graph
- Tasks
  - `src/annotate/model.rs`, which is **pure Rust with no GTK**, fully unit-testable:
    ```rust
    enum Shape { Rect, Ellipse, Line, Arrow, Freehand(Vec<Point>), Text(String), Highlight(Vec<Rect>) }
    struct Annotation { id, shape, bounds, style: Style /* stroke, fill, width, font */ }
    struct AnnotationLayer { items: Vec<Annotation> }  // in *document* coordinates
    ```
  - Hit-testing (`layer.hit(point, tolerance)`), bounds, move and resize.
  - Integrate with the `EditStack` from 4.1 so annotation changes are undoable too.
- **Done when:** model tests cover create, move, resize, hit-test and undo.

### 5.2 ✅ Render the layer 🟢
- Tasks: draw annotations in the canvas `snapshot()` *after* the content, transforming document coordinates to widget coordinates. One renderer function that takes a `cairo::Context` lets you reuse it for screen, flattening (5.6) and printing.

### 5.3 ✅ Tools and toolbar 🟠
- Tasks: a markup toolbar (`Ctrl+Shift+A`) with Select, Rectangle, Ellipse, Arrow, Line, Pen, Text and Highlight tools; a colour picker (`gtk::ColorDialogButton`) and a stroke width; a tool state machine driven by `GestureDrag`.
- **Done when:** you can draw every shape type on an image.

### 5.4 ✅ Select and edit 🟠
- Tasks: click to select, handles to resize, drag to move, Delete to remove, double-click a text annotation to edit it in place (an overlaid `gtk::Entry` / `gtk::TextView`), arrow keys to nudge.

### 5.5 ✅ Annotate PDFs 🟡
- Tasks: one `AnnotationLayer` per page, in page coordinates (PDF points; watch out: PDF y goes *up*, so unit-test the transform).

### 5.6 ✅ Save annotations 🟠
- **Images:** flatten. Render the image plus the layer into a `cairo::ImageSurface` at full resolution and encode it.
- **PDFs:** write *real* annotations so other viewers can edit them:
  `poppler::AnnotSquare`, `AnnotCircle`, `AnnotLine`, `AnnotInk`, `AnnotFreeText`, `AnnotTextMarkup::new_highlight` → `page.add_annot` → `document.save(uri)`.
  Arrows: `AnnotLine` with line endings, or ink. If poppler lacks something, fall back to `lopdf`.
- Also *load* existing PDF annotations into your model (`page.annot_mapping`) so they're editable.
- **Done when:** annotate a PDF in Lumen, open it in Okular or Firefox, and the annotations are there and editable.
- **Notes:** Poppler has no line-ending API, so lines and arrows are written as *ink* annotations (tagged in their contents so Lumen reads them back as lines and arrows). Annotations are written with Poppler's C API from `src/pdf/annots.rs`; `lopdf` was not needed.

---

## Phase 6 — PDF page management

### 6.1 Rotate, delete and reorder pages 🟠
- Tasks: thumbnail sidebar gets multi-select, drag-to-reorder, rotate and delete. Keep the edits as a page-ops list and apply them on save with `lopdf` (Poppler can't restructure documents).
### 6.2 Merge and insert 🟡
- Tasks: drag a PDF or image onto the sidebar to insert its pages; "Insert page from file"; images become pages.
### 6.3 Export 🟢
- Tasks: export selected pages as a new PDF, export a page as PNG, "Export as PDF" for images.

---

## Phase 7 — Polish and distribution

- [ ] **7.1 Preferences** — `adw::PreferencesDialog` + `gio::Settings` (GSchema in `data/`): default zoom, background colour, smooth zoom.
- [ ] **7.2 Remember state** — window size, sidebar visibility, last page per PDF.
- [ ] **7.3 Fullscreen and slideshow** — `F11`, auto-advance timer, hide the cursor.
- [ ] **7.4 Accessibility and keyboard** — `gtk::ShortcutsWindow` (`Ctrl+?`), accessible labels, a full keyboard pass.
- [ ] **7.5 i18n** — `gettext-rs`, `po/` directory.
- [ ] **7.6 Icon and metainfo** — app icon (SVG), `metainfo.xml`, screenshots.
- [ ] **7.7 Flatpak** — manifest in `build-aux/`, `flatpak-cargo-generator` for offline builds, test via GNOME Builder.
- [ ] **7.8 Packaging** — AUR `PKGBUILD` (you're on CachyOS), release tags, GitHub Actions building release artifacts.
- [ ] **7.9 Performance pass** — `cargo flamegraph`, startup time < 150 ms, test with huge files.

## Stretch goals

- Signatures (draw once, store, stamp onto PDFs).
- PDF form filling (`poppler::FormField`).
- OCR with Tesseract to make scanned PDFs searchable.
- Copy text from images (OCR).
- Compare two images side by side.
- Colour management (ICC profiles).

---

## Cross-cutting rules (apply at every step)

1. **Keep logic out of widgets.** Geometry, edit ops and the annotation model live in plain Rust modules with unit tests. Widgets just draw and forward events.
2. **Never block the main thread.** Anything that touches disk or decodes runs on a worker and returns a message.
3. **One coordinate-transform module** (`src/geometry.rs`) for widget ↔ document ↔ PDF space. Most "weird bugs" in viewers come from here.
4. **Commit per step** with a message like `feat(image): zoom around cursor (1.2)`.
5. Run `cargo fmt && cargo clippy --all-targets && cargo test` before every commit (CI enforces it).
