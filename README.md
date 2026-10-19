<p align="center">
  <img src="data/icons/hicolor/256x256/apps/io.github.imanolgo.Lumen.png" alt="Lumen icon" width="128" height="128">
</p>

<h1 align="center">Lumen</h1>

<p align="center">
  A fast image and PDF viewer for Linux, with the light editing and markup you use every day.<br>
  Written in Rust with GTK 4 and libadwaita.
</p>

<p align="center">
  <a href="https://github.com/ImanolGo/lumen/actions/workflows/ci.yml"><img src="https://github.com/ImanolGo/lumen/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue" alt="License: GPL-3.0-or-later"></a>
</p>

<p align="center">
  <img src="data/screenshots/image-markup.png" alt="Marking up a picture in Lumen" width="640">
</p>

## Features

- **Fast.** Pictures and pages load in the background, so the UI never freezes. A 50 MP JPEG is on screen in about half a second and a 500-page PDF opens in about 50 ms (release build, measured on a 2020 laptop).
- **Pictures and PDFs in one app.** Browse a whole folder with thumbnails, zoom and pan smoothly, honour EXIF orientation, run a slideshow or go fullscreen.
- **Light editing.** Rotate, flip, crop, resize and adjust colour, with undo and redo. Rotating a JPEG does not recompress it.
- **Markup.** Rectangles, ellipses, arrows, lines, pen strokes, text and highlights on pictures and PDFs. PDF annotations are real annotations, so other readers can still edit them.
- **PDF reading.** Continuous scroll, page thumbnails, table of contents, search, text selection and copy, clickable links, password-protected files and document properties.
- **PDF page management.** Rotate, delete, reorder and insert pages, merge files, export pages as a PDF or PNG.
- **Printing** through the native GTK print dialog.
- **Native.** Follows the GNOME HIG and works on any desktop. Preferences, keyboard shortcuts dialog and accessible names are included. Translations use gettext; Spanish ships today.

| Crop with draggable handles | Annotate a PDF |
|---|---|
| <img src="data/screenshots/image-crop.png" alt="Cropping a picture" width="380"> | <img src="data/screenshots/pdf-annotations.png" alt="Annotating a PDF with page thumbnails" width="380"> |

## Installing

Lumen is at version 0.1.0 and is not on Flathub or the AUR yet. For now, install it from source or from a release.

### From a release

Each tagged release on the [releases page](https://github.com/ImanolGo/lumen/releases) has an x86_64 tarball and a Flatpak bundle.

```sh
sudo tar -C / -xzf lumen-vX.Y.Z-x86_64-linux.tar.gz   # installs under /usr
# or the Flatpak bundle
flatpak install --user lumen.flatpak
```

The tarball needs GTK 4.16+, libadwaita 1.8+ and poppler-glib 25.06+ on the system. If your distribution is older, use the Flatpak.

### Arch Linux

A `PKGBUILD` is in [`build-aux/arch/`](build-aux/arch/):

```sh
cd build-aux/arch && makepkg -si
```

### From source

You need a C toolchain, Rust 1.92 or newer, GTK ≥ 4.16, libadwaita ≥ 1.8, poppler-glib ≥ 25.06, the GLib tools (`glib-compile-resources`) and gettext. The build checks these versions.

| Distro | Command |
|---|---|
| Arch / CachyOS | `sudo pacman -S --needed base-devel rust gtk4 libadwaita poppler-glib gettext` |
| Fedora | `sudo dnf install gcc cargo gtk4-devel libadwaita-devel poppler-glib-devel gettext` |
| Debian / Ubuntu | `sudo apt install build-essential cargo libgtk-4-dev libadwaita-1-dev libpoppler-glib-dev gettext` |

These are GNOME 49-era libraries, so distributions released before late 2025 (for example Debian 13 and Ubuntu 24.04) are too old; use the Flatpak there. Check what you have with `pkg-config --modversion gtk4 libadwaita-1 poppler-glib`, and use [rustup](https://rustup.rs) if your Rust is older than 1.92.

```sh
git clone https://github.com/ImanolGo/lumen.git
cd lumen
make
sudo make install                    # to /usr
make install PREFIX=$HOME/.local     # or just for your user
```

To try it without installing:

```sh
cargo run --release -- path/to/file.pdf
```

Remove an installed copy with `make uninstall` (use the same `PREFIX`).

## Usage

```sh
lumen                      # empty window, Ctrl+O to open
lumen photo.jpg report.pdf # open files directly
```

Lumen registers itself for PDF, PNG, JPEG, GIF, WebP, BMP, TIFF, SVG, AVIF and HEIF. All of them can be viewed (SVG, AVIF and HEIF need the matching gdk-pixbuf loaders); JPEG, PNG, WebP, GIF, BMP and TIFF can also be edited and marked up. Edits are saved in place for JPEG, PNG and WebP, and as a copy for the others. Animated GIFs show their first frame. To make it the default PDF viewer:

```sh
xdg-mime default io.github.imanolgo.Lumen.desktop application/pdf
```

Press `Ctrl+?` in the app for the full, searchable list of shortcuts. The main ones:

| Shortcut | Action |
|---|---|
| `Ctrl+O` | Open |
| `←` / `→` | Previous / next file in the folder |
| `Page Up` / `Page Down`, `Ctrl+G` | Previous / next PDF page, go to page |
| `Ctrl+=` / `Ctrl+-`, `Ctrl+0` / `Ctrl+1` | Zoom in / out, best fit / 100 % |
| `Ctrl+R` / `Ctrl+Shift+R` | Rotate right / left |
| `Ctrl+K` | Crop (`Enter` applies, `Esc` cancels) |
| `Ctrl+Shift+A` | Markup toolbar |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / redo |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save As |
| `Ctrl+F` | Find in PDF |
| `Ctrl+P` | Print |
| `F9` | Toggle the thumbnail sidebar |
| `F11` / `F5`, `Esc` | Fullscreen / slideshow, leave them |
| `Ctrl+,` | Preferences |

### Files Lumen writes

| Path | Contents |
|---|---|
| `~/.config/lumen/settings.toml` | Preferences |
| `~/.local/share/lumen/state.toml` | Window size, sidebar state, last page per PDF |

Lumen only overwrites a file you opened when you choose Save. Edits and page changes are kept as working copies until then.

## Contributing

Bug reports and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow, and the docs below for how the code is organised.

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): stack, module layout, data flow, coordinate spaces
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): setup, debugging, icons, scripting and screenshots, translations
- [docs/PLAN.md](docs/PLAN.md): the development plan and what is still open
- [CHANGELOG.md](CHANGELOG.md)

## License

[GPL-3.0-or-later](LICENSE). Lumen links against Poppler, which is GPL-licensed.
