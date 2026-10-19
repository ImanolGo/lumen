# Lumen

A fast, Preview-like image and PDF viewer for Linux, written in Rust with GTK 4 and libadwaita.

> **Status: early development.** Phase 0 (foundations) is done. See the [roadmap](docs/PLAN.md).

## Goals

- **Fast:** opens instantly, never freezes the UI, handles huge images and 500-page PDFs.
- **One app** for pictures *and* PDFs.
- **Light editing:** rotate, flip, crop, resize, with lossless JPEG rotation.
- **Markup:** shapes, arrows, text, freehand and highlights on images and PDFs. PDF annotations stay editable in other readers.
- **Print** anything with the native GTK print dialog.
- **Native:** follows the GNOME HIG, works on any desktop, and gets distributed via Flatpak.

## Features

| | Feature | Phase |
|---|---|---|
| ✅ | Open dialog, CLI argument, image/PDF detection | 0 |
| ✅ | Image viewing: zoom, pan, folder browsing, EXIF orientation | 1 |
| ⬜ | PDF viewing: continuous scroll, thumbnails, outline, search, text selection | 2 |
| ⬜ | Printing | 3 |
| ⬜ | Rotate / flip / crop / resize, undo, save & export | 4 |
| ⬜ | Annotations on images and PDFs | 5 |
| ⬜ | PDF page management: reorder, rotate, delete, merge | 6 |
| ⬜ | Preferences, Flatpak, AUR | 7 |

## Building

Install the system libraries (see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for other distros):

```sh
# Arch / CachyOS
sudo pacman -S --needed base-devel gtk4 libadwaita poppler-glib
```

Then:

```sh
cargo run                       # empty window, Ctrl+O to open
cargo run -- path/to/file.pdf   # open a file directly
```

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+O` | Open |
| `Ctrl+W` | Close window |
| `←` / `→`, `Backspace` / `Space` | Previous / next file in the folder |
| `F9` | Toggle the thumbnail sidebar |
| `Alt+Enter` | Toggle the properties panel |
| `Page Up` / `Page Down`, `Home` / `End` | Previous / next / first / last PDF page |
| `Ctrl+G` | Go to page |
| `Ctrl+=` / `Ctrl+-` | Zoom in / out |
| `Ctrl+0` / `Ctrl+1` | Best fit / 100 % |
| `Ctrl`+scroll | Zoom around the cursor |

## Documentation

- [docs/PLAN.md](docs/PLAN.md): the step-by-step development plan
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): stack, module layout, data flow, coordinate spaces
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): setup, debugging and references

## License

[GPL-3.0-or-later](LICENSE). Lumen links against Poppler, which is GPL-licensed.
