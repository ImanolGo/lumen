<p align="center">
  <img src="data/icons/scalable/apps/io.github.imanolgo.LumenRs.svg" alt="Lumen icon" width="112" height="112">
</p>

<h1 align="center">Lumen (Rust/iced)</h1>

<p align="center">
  A fast, safe viewer for pictures and PDFs, written in pure Rust with <a href="https://iced.rs">iced</a>.<br>
  A ground-up rewrite of <a href="https://github.com/ImanolGo/lumen">Lumen</a> without GTK, poppler or any other C library in the document path.
</p>

## Why a rewrite

The original Lumen is built on GTK 4, libadwaita and poppler. This edition:

- **Has no C code in the document path.** Pictures are decoded by [`image`](https://github.com/image-rs/image) and [`resvg`](https://github.com/linebender/resvg). PDFs are rendered by [`hayro`](https://github.com/LaurenzV/hayro), which forbids `unsafe` code, and markup is written into them with [`lopdf`](https://github.com/J-F-Liu/lopdf). The app itself is `#![forbid(unsafe_code)]`.
- **Has no GTK runtime requirements.** It is a single binary that talks to the desktop only through XDG portals (D-Bus), so it behaves the same on GNOME, KDE, Niri, Sway, Hyprland and others.
- **Draws on the GPU** through wgpu, with a CPU fallback (tiny-skia).

## Features (0.1)

- **Pictures:** PNG, JPEG, GIF (first frame), WebP, BMP, TIFF, ICO, QOI, TGA, the PNM family, and SVG. EXIF orientation is honoured.
- **PDFs:** continuous scrolling, a table of contents, search, and clickable links. Only the pages near the view are rendered, and that happens on a pool of background threads that does the newest request first. Pages re-render sharply at every zoom level and on HiDPI screens.
- **Search:** Ctrl+F finds text across the whole document (case-insensitive), highlights every match and jumps between them. It starts from the page you are on, results appear while it scans, and each page's text is read only once. The 477-page Ghostscript manual is fully indexed in about 0.7 s.
- **Text selection:** drag over text to select it (Shift+click extends, double-click picks a word), Ctrl+C copies, Ctrl+A selects the whole document. Like other Linux apps, the selection also goes to the primary selection for middle-click paste. Dragging anywhere that is not text still pans.
- **Table of contents:** the PDF's outline appears in the sidebar, follows you as you scroll, and jumps to the exact spot of each section.
- **Links:** links inside the document jump to their target. Web and mail links open in your browser or mail app through the desktop portal; other kinds of link (files, programs) are ignored for safety.
- **Markup:** press M (or the pen button) to draw on a picture or a PDF page: highlighter, pen, rectangles, ellipses, lines, arrows and text boxes, in eight colours and three line widths. Select, move, resize, recolour and delete what you drew, with undo and redo. On a PDF the highlighter marks the text you drag over.
- **Real PDF annotations:** markup is saved into the PDF as standard annotations (highlight, ink, square, circle, line, free text), each with its own appearance, so every PDF viewer shows it. Saving appends an incremental update: the original bytes are kept as they were. Annotations of those kinds that are already in a PDF, from Lumen or another app, can be edited too; comments attached to them are kept.
- **Pictures with markup** are saved as a copy (PNG, JPEG, WebP, TIFF or BMP), never over the original unless you choose it. Printing prints what you see, unsaved markup included. Markup you have not saved yet is kept while you look at other files, and quitting asks first.
- **Folders:** open a folder, or open one file and flip through its neighbours. You can also drop files and folders onto the window. A sidebar on the left lists the files with lazily loaded thumbnails (only the rows on screen are built, so huge folders stay fast); camera JPEGs use their embedded EXIF thumbnail.
- **Printing:** uses the desktop's native print dialog through the XDG print portal. PDFs are sent as they are. Pictures are fitted to the paper and margins you choose and sent as a one-page PDF at up to 300 dpi.
- **Light and dark mode:** follows the system colour scheme and switches live when you change it.
- **Zoom and pan:** best fit, fit width (PDF), actual size, Ctrl + scroll wheel, and drag to pan. View-only rotation for pictures.
- **Fast flipping:** the next and previous pictures are preloaded, and recently viewed ones are kept decoded.
- **Robust:** decoding runs under allocation limits, and a panic while decoding or rendering a hostile file becomes an error message instead of a crash.

Picture editing (crop, adjust), page reordering, forms, signing and password-protected PDFs from the original Lumen are not ported yet.

## Keyboard

| Keys | Action |
|---|---|
| Ctrl+O / Ctrl+Shift+O | Open files / open a folder |
| Ctrl+P | Print |
| M | Markup on or off |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo markup |
| Delete, arrow keys | Delete / nudge the selected annotation (Shift moves further) |
| Ctrl+S / Ctrl+Shift+S | Save markup into the PDF / save a copy (a picture always saves a copy) |
| Ctrl+C / Ctrl+A | Copy the selected text / select all text (PDF) |
| Ctrl+F | Find in the PDF (Enter / Shift+Enter or F3 / Shift+F3 for next / previous, Esc to close) |
| ← → (Space / Backspace for pictures) | Next / previous file |
| Home / End | First / last file (first / last page in a PDF) |
| Page Up / Page Down | Previous / next page (PDF) |
| ↑ ↓ | Scroll |
| + − 0 1 | Zoom in, zoom out, best fit, actual size |
| W | Fit width (PDF) |
| R / Shift+R | Rotate right / left (pictures) |
| T | Show or hide the sidebar (files, and the PDF's contents) |
| F / F11, Esc | Fullscreen, leave fullscreen |
| Ctrl+Q / Ctrl+W | Quit |

## Building

You need Rust 1.88 or newer. At run time you also need an XDG desktop portal with a file chooser and print backend: `xdg-desktop-portal-gtk` or `xdg-desktop-portal-gnome` (`-kde` works as well). The rest is pure Rust.

```sh
git clone https://github.com/ImanolGo/lumen-rs.git
cd lumen-rs
make                                  # cargo build --release
sudo make install                     # to /usr/local
make install PREFIX=$HOME/.local      # or just for your user
```

To try it without installing:

```sh
cargo run --release -- ~/Pictures/Trip
cargo run --release -- report.pdf photo.jpg
```

Make it the default PDF viewer:

```sh
xdg-mime default io.github.imanolgo.LumenRs.desktop application/pdf
```

## Layout

| File | What it does |
|---|---|
| `src/app.rs` | State, messages, update and view (iced's Elm architecture) |
| `src/pdf.rs` | PDF documents and their render-thread pool (hayro) |
| `src/structure.rs` | Outline, links and page text (for search) |
| `src/annotate/` | Markup: the annotation model and editor, the shared drawing description, PDF annotations in and out (lopdf), pictures with markup |
| `src/app/markup.rs`, `src/app/overlay.rs` | The markup bar and its messages; the canvas that draws annotations over a page and takes the pointer |
| `src/raster.rs` | Picture decoding, SVG rendering and thumbnails |
| `src/library.rs` | Which files to browse: folders, natural sort order |
| `src/print.rs` | Printing through the portal; turns pictures into PDFs |
| `src/portal.rs` | Native file and folder choosers |
| `src/style.rs`, `src/icons.rs` | Light and dark palettes, widget styles, symbolic icons |

Run `make test` and `make lint` before sending changes.

## Timings

Like the GTK Lumen, timings are logged at `info` level: `open requested`, `window built`, `opened load_ms=…` and `first view on screen ms=…` (for a PDF, when its first page is drawn). Run `lumen-rs file.pdf` from a terminal to see them; `RUST_LOG=warn` silences them.

## License

GPL-3.0-or-later, like the original Lumen.
