# Development guide

## System dependencies

You need the GTK 4, libadwaita and poppler-glib development files, plus a C toolchain, `pkg-config` and `glib-compile-resources` (it comes with the GLib development files; `build.rs` uses it to bundle the icons and stylesheet in `data/`).

| Distro | Command |
|---|---|
| Arch / CachyOS | `sudo pacman -S --needed base-devel gtk4 libadwaita poppler-glib` |
| Fedora | `sudo dnf install gcc gtk4-devel libadwaita-devel poppler-glib-devel` |
| Debian / Ubuntu | `sudo apt install build-essential libgtk-4-dev libadwaita-1-dev libpoppler-glib-dev` |

Minimum versions, checked at build time: GTK ≥ 4.16 (the `gnome_47` feature in `Cargo.toml`), libadwaita ≥ 1.8 (`v1_8`, for `adw::ShortcutsDialog`), poppler-glib ≥ 25.06 (`v25_6`), Rust ≥ 1.92. Older distributions (Debian 13, Ubuntu 24.04) do not have them; build the Flatpak there.

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

## Install locally

```sh
make install PREFIX=$HOME/.local     # binary, desktop file, metainfo, icons, translations
update-desktop-database ~/.local/share/applications
# optional: make it the default for PDFs
xdg-mime default io.github.imanolgo.Lumen.desktop application/pdf
```

## Icons and stylesheet

The app icon lives in `data/icons/hicolor/` (SVG, symbolic SVG and PNG renditions from 16 to 512 px).
After changing the SVG, regenerate the PNGs:

```sh
for s in 16 32 48 64 128 256 512; do
    rsvg-convert -w $s -h $s data/icons/hicolor/scalable/apps/io.github.imanolgo.Lumen.svg \
        -o data/icons/hicolor/${s}x${s}/apps/io.github.imanolgo.Lumen.png
done
```

The toolbar icons (`data/icons/actions/lumen-*-symbolic.svg`) and `data/style.css` are compiled
into the binary by `build.rs` (see `data/lumen.gresource.xml`), so they work from `cargo run`
without installing anything. Symbolic icons must use plain fills, not strokes: GTK recolours
fills only. Add new icons to `data/lumen.gresource.xml`.

## Scripting, tests and screenshots

Every window action is on the session bus, so a running Lumen can be driven from a script:

```sh
gdbus call --session --dest io.github.imanolgo.Lumen \
    --object-path /io/github/imanolgo/Lumen/window/1 \
    --method org.gtk.Actions.Activate screenshot-to "[<'/tmp/lumen.png'>]" "{}"
```

Besides the menu actions there are scripted ones for tests: `crop-to`, `resize-to`, `adjust-to`,
`save-to`, `add-files`, `markup-draw`, `markup-text`, `markup-tool`, `markup-color`, `markup-size`, `page-op`, `export-pages-to`,
`export-png-to`, `print-to-file`, `select-text`, `search`, `password` and `screenshot-to`
(see `src/app.rs` for their arguments).

`build-aux/screenshot.sh` runs Lumen headless on a Broadway display and takes a screenshot after
applying some actions, without touching your desktop or settings. The screenshots in
`data/screenshots/` were made with it, from synthetic files (`Mountains.png`, `Quarterly report.pdf`):

```sh
cargo build --release
SCHEME=prefer-dark build-aux/screenshot.sh data/screenshots/image-crop.png crop -- Mountains.png
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

## Translations

Every text the user sees goes through `tr()` (or `tr_fmt()` for texts with `{name}` placeholders;
`n_()` marks texts in compile-time tables). Translations live in `po/`.

```sh
./build-aux/update-pot.sh                    # after adding or changing texts: update po/lumen.pot and merge into po/*.po
msginit -l de -i po/lumen.pot -o po/de.po    # start a new language (then translate the msgstr lines)
./build-aux/compile-translations.sh          # compile into target/locale
LANGUAGE=es LUMEN_LOCALEDIR=$PWD/target/locale cargo run   # try it
```

Add new source files that use `tr()` to `po/POTFILES.in`. Spanish (`es`) is complete.
