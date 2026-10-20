# Development guide

## System dependencies

You need the GTK 4, libadwaita and poppler-glib development files, plus a C toolchain, `pkg-config` and `glib-compile-resources` (it comes with the GLib development files; `build.rs` uses it to bundle the icons and stylesheet in `data/`).

| Distro | Command |
|---|---|
| Arch / CachyOS | `sudo pacman -S --needed base-devel gtk4 libadwaita poppler-glib libseccomp glycin` |
| Fedora | `sudo dnf install gcc gtk4-devel libadwaita-devel poppler-glib-devel libseccomp-devel glycin-loaders` |
| Debian / Ubuntu | `sudo apt install build-essential libgtk-4-dev libadwaita-1-dev libpoppler-glib-dev libseccomp-dev glycin-loaders` |

Minimum versions, checked at build time: GTK ≥ 4.16 (the `gnome_47` feature in `Cargo.toml`), libadwaita ≥ 1.8 (`v1_8`, for `adw::ShortcutsDialog`), poppler-glib ≥ 25.06 (`v25_6`), libseccomp (for glycin's sandbox), Rust ≥ 1.93 (glycin 4). Older distributions (Debian 13, Ubuntu 24.04) do not have them; build the Flatpak there.

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

## Scripting and the control interface

A running Lumen offers `io.github.imanolgo.Lumen.Control1` at `/io/github/imanolgo/Lumen/Control`
on the session bus (`src/app/control.rs`). Every method returns JSON, most of them the state
afterwards (file, kind, page, page count, size, zoom, unsaved changes, undo/redo, markup tool).
Work that finishes later (opening, saving, page changes, form fields, exports, searching) answers
when it has finished, or with an error such as `io.github.imanolgo.Lumen.Error.Busy`.

```sh
ctl() {   # call a method: ctl Method ARG…
    gdbus call --session --dest io.github.imanolgo.Lumen \
        --object-path /io/github/imanolgo/Lumen/Control \
        --method io.github.imanolgo.Lumen.Control1."$@"
}
ctl Open "['/tmp/form.pdf']" "{}"
ctl ListFields                              # each field's id, kind, value and area
ctl FillField 65537 "<'Ada Lovelace'>"      # an id from ListFields
ctl Annotate 0 '{"type": "rect", "x": 50, "y": 60, "w": 120, "h": 40, "color": "#e01b24"}'
ctl Save /tmp/filled.pdf "{}"
```

| Method | Arguments | Notes |
|---|---|---|
| `GetState` | | |
| `Open` | `as paths`, `a{sv}` | `discard` (b) to drop unsaved edits, `password` (s), `add` (b) to add to the files being browsed |
| `GotoPage` | `u page` | 0-based |
| `ListPages`, `ListFields` | | sizes and areas in points from the page's top-left |
| `FillField` | `i id`, `v value` | `b` for checkboxes and radio buttons, `s` for text, `u` for a list item |
| `ListAnnotations` | `u page` | in the form `Annotate` takes, plus `id` |
| `Annotate` | `u page`, `s json` | `type`: rect, ellipse (`x y w h`), line, arrow (`x1 y1 x2 y2`), pen (`points`), ink (`strokes`), text (`x y w h text size`), highlight (`rects`); optional `color`, `fill`, `width` |
| `DeleteAnnotation` | `u page`, `t id` | |
| `EditImage` | `s operation`, `a{sv}` | rotate-right, rotate-left, flip-horizontally, flip-vertically, crop (`x y w h`), resize (`w h`), adjust (`exposure contrast saturation`) |
| `ChangePages` | `s operation`, `au pages`, `a{sv}` | rotate (`degrees`), delete, move (`before`), insert (`at`, `path`) |
| `Undo`, `Redo` | | |
| `Save` | `s path`, `a{sv}` | empty path: the open file; `overwrite` (b) to replace a file, `drop-password` (b), `quality` (u, JPEG) |
| `Export` | `s kind`, `s path`, `a{sv}` | pages (`pages`) and png for PDFs, pdf for pictures, print; `overwrite` (b) |
| `Search` | `s text` | answers with every hit once the search has finished |
| `SetMarkup` | `a{sv}` | `open` (b), `tool` (s), `color` (s), `size` (d) |
| `Screenshot` | `s path` | the window as a PNG |

Nothing destructive happens implicitly, and no method waits on a dialog: replacing unsaved edits
needs `discard`, writing over any file `overwrite`, and saving a rearranged protected PDF without
its password `drop-password`. A protected PDF opened without the right `password` is an error.
Pictures use pixels and PDFs points, both from the top-left of the page. The menu actions are on
the bus too, as GTK's `org.gtk.Actions` at `/io/github/imanolgo/Lumen/window/1`.

### End-to-end tests

`tests/control.rs` starts Lumen on its own D-Bus session bus and Broadway display, with throwaway
settings, and drives it only through the control interface: opening, forms, annotations, page
changes, picture edits, passwords, search and export, each checked through what Lumen reports and
saves. They need `dbus-daemon` and `gtk4-broadwayd` (part of GTK) and are skipped without them:

```sh
cargo test --test control
```

### Screenshots

`build-aux/screenshot.sh` runs Lumen headless on a Broadway display and a private session bus (so it
never reaches a Lumen you have open), applies menu actions and control calls, and saves a
screenshot. The screenshots in `data/screenshots/` were made with it, from synthetic files:

```sh
cargo build --release
SCHEME=prefer-dark build-aux/screenshot.sh data/screenshots/image-crop.png crop -- Mountains.png
build-aux/screenshot.sh shot.png markup "ctl:SetMarkup|{'tool': <'arrow'>}" -- report.pdf
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

## Picture decoding

`src/image/decode.rs` decodes pictures with glycin 4 (`glycin-external`), which runs the loaders
installed under `/usr/share/glycin-loaders/2+/` (or the GNOME runtime's, in the Flatpak) in
bubblewrap sandboxes. `apply_transformations` is off: Lumen applies the EXIF orientation itself, so
lossless rotation and editing can change it. Animations are decoded up front (capped at 2000 frames
or 512 MB, beyond which only the first frame is shown) and played by the canvas. Without any loaders
installed, Lumen falls back to `gdk::Texture::from_filename` and logs a warning. Editing still
decodes with the `image` crate (pure Rust) for the formats it can write back.
