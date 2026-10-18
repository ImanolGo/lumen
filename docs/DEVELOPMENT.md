# Development guide

## System dependencies

You need the GTK 4, libadwaita and poppler-glib development files (plus a C toolchain and `pkg-config`).

| Distro | Command |
|---|---|
| Arch / CachyOS | `sudo pacman -S --needed base-devel gtk4 libadwaita poppler-glib` |
| Fedora | `sudo dnf install gcc gtk4-devel libadwaita-devel poppler-glib-devel` |
| Debian / Ubuntu (24.04+) | `sudo apt install build-essential libgtk-4-dev libadwaita-1-dev libpoppler-glib-dev` |

Minimum versions: GTK ≥ 4.16, libadwaita ≥ 1.6 (the `gnome_47` feature in `Cargo.toml`), Rust ≥ 1.92.

Check them with:
```sh
pkg-config --modversion gtk4 libadwaita-1 poppler-glib
```

## Everyday commands

```sh
cargo run                         # empty window
cargo run -- ~/Pictures/x.jpg     # open a file
RUST_LOG=lumen=debug cargo run    # verbose logs
cargo fmt && cargo clippy --all-targets && cargo test   # before every commit
```

## Debugging GTK

- **GTK Inspector:** `GTK_DEBUG=interactive cargo run`, or press `Ctrl+Shift+D` once it's enabled:
  `gsettings set org.gtk.Settings.Debug enable-inspector-keybinding true`.
  It lets you inspect widgets, live-edit CSS and see the render nodes (great for checking your canvas snapshot).
- **Fake HiDPI:** `GDK_SCALE=2 cargo run`.
- **Renderer:** `GSK_RENDERER=ngl|vulkan|cairo cargo run` to rule out GPU issues.
- **Critical warnings as panics:** `G_DEBUG=fatal-criticals cargo run` under `rust-gdb` to get a backtrace.

## Install the desktop file locally (step 1.4)

```sh
cargo build --release
install -Dm755 target/release/lumen ~/.local/bin/lumen
install -Dm644 data/io.github.imanolgo.Lumen.desktop ~/.local/share/applications/io.github.imanolgo.Lumen.desktop
update-desktop-database ~/.local/share/applications
# optional: make it the default for PDFs
xdg-mime default io.github.imanolgo.Lumen.desktop application/pdf
```

## Sample files

Keep test files in `samples/` (git-ignored). Useful ones to collect:
- a 50+ MP JPEG, a phone photo with EXIF rotation, a transparent PNG, an animated GIF, an SVG;
- a 500+ page PDF, a PDF with an outline, an encrypted PDF, a PDF with existing annotations, a landscape/mixed-orientation PDF.

## References

- Relm4 book: <https://relm4.org/book/stable/>
- gtk4-rs book: <https://gtk-rs.org/gtk4-rs/stable/latest/book/>
- libadwaita widget gallery: <https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/widgets.html>
- poppler-rs docs: <https://world.pages.gitlab.gnome.org/Rust/poppler-rs/>
- **Loupe** (GNOME image viewer, Rust + GTK4) — best reference for the canvas: <https://gitlab.gnome.org/GNOME/loupe>
- **Papers** (GNOME document viewer) — reference for the PDF view: <https://gitlab.gnome.org/GNOME/papers>
- GNOME HIG: <https://developer.gnome.org/hig/>
