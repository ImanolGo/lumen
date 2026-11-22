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

- **Has no C code in the document path.** Pictures are decoded by [`image`](https://github.com/image-rs/image) and [`resvg`](https://github.com/linebender/resvg). PDFs are rendered by [`hayro`](https://github.com/LaurenzV/hayro), which forbids `unsafe` code. The app itself is `#![forbid(unsafe_code)]`.
- **Has no GTK runtime requirements.** It is a single binary that talks to the desktop only through XDG portals (D-Bus), so it behaves the same on GNOME, KDE, Niri, Sway, Hyprland and others.
- **Draws on the GPU** through wgpu, with a CPU fallback (tiny-skia).

## Features (0.1)

- **Pictures:** PNG, JPEG, GIF (first frame), WebP, BMP, TIFF, ICO, QOI, TGA, the PNM family, and SVG. EXIF orientation is honoured.
- **PDFs:** continuous scrolling. Only the pages near the view are rendered, and that happens on a pool of background threads that does the newest request first. Pages re-render sharply at every zoom level and on HiDPI screens.
- **Folders:** open a folder, or open one file and flip through its neighbours. You can also drop files and folders onto the window. A thumbnail strip at the bottom loads lazily; camera JPEGs use their embedded EXIF thumbnail.
- **Printing:** uses the desktop's native print dialog through the XDG print portal. PDFs are sent as they are. Pictures are fitted to the paper and margins you choose and sent as a one-page PDF at up to 300 dpi.
- **Light and dark mode:** follows the system colour scheme and switches live when you change it.
- **Zoom and pan:** best fit, fit width (PDF), actual size, Ctrl + scroll wheel, and drag to pan. View-only rotation for pictures.
- **Fast flipping:** the next and previous pictures are preloaded, and recently viewed ones are kept decoded.
- **Robust:** decoding runs under allocation limits, and a panic while decoding or rendering a hostile file becomes an error message instead of a crash.

Editing, markup, forms and signing from the original Lumen are not ported yet.

## Keyboard

| Keys | Action |
|---|---|
| Ctrl+O / Ctrl+Shift+O | Open files / open a folder |
| Ctrl+P | Print |
| ← → (Space / Backspace for pictures) | Next / previous file |
| Home / End | First / last file (first / last page in a PDF) |
| Page Up / Page Down | Previous / next page (PDF) |
| ↑ ↓ | Scroll |
| + − 0 1 | Zoom in, zoom out, best fit, actual size |
| W | Fit width (PDF) |
| R / Shift+R | Rotate right / left (pictures) |
| T | Show or hide the thumbnail strip |
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
| `src/raster.rs` | Picture decoding, SVG rendering and thumbnails |
| `src/library.rs` | Which files to browse: folders, natural sort order |
| `src/print.rs` | Printing through the portal; turns pictures into PDFs |
| `src/portal.rs` | Native file and folder choosers |
| `src/style.rs`, `src/icons.rs` | Light and dark palettes, widget styles, symbolic icons |

Run `make test` and `make lint` before sending changes.

## License

GPL-3.0-or-later, like the original Lumen.
