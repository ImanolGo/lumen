# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Security
- A decoder that panics, or whose answer is cut short, is an error for that picture; it is no longer decoded again outside the sandbox. A worker says when it has shut itself in, so one that was taken over cannot claim there is no sandbox.
- Animation frames are decoded in the sandbox too, under the same limits as still pictures. Animations with very large frames (over about 5 megapixels) show their first frame only.
- The decoding limit covers the pixels themselves (a small PNG could ask for 10 GB), and the worker's address space is capped at 8 GB.
- An SVG can no longer pull in other picture files from disk.
- The sandbox allows fewer system calls: no new processes, reading only the picture, no `newfstatat`, signals only to itself.
- The worker still starts after a package upgrade replaced the binary while Lumen was running.
- PDF pages of absurd size, and annotations and form fields that share one huge array, no longer exhaust the memory.

### Fixed
- Printing a picture whose decoded size is refused no longer waits for ever.
- A PDF page that cannot be rendered says so instead of being asked for again without end.
- A file name that is not UTF-8 no longer crashes Lumen at start.

## [0.1.0] - 2026-11-24

First release. Lumen (Rust/iced) is a rewrite of the GTK Lumen without GTK, poppler or any C library in the document path.

### Added
- Pictures: PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA, PNM, JPEG XL, JPEG 2000, OpenEXR and SVG, with EXIF orientation; animated GIF, APNG and WebP. AVIF and HEIC through the optional `heif` feature (libheif).
- Pictures are decoded in a sandboxed child process (Landlock and seccomp).
- PDFs: continuous scroll, page thumbnails, table of contents, search, text selection with column-aware reading order, links, password-protected files, document properties, night mode, go to page.
- Markup on pictures and PDFs, saved into PDFs as real annotations; PDF forms (text, check boxes, radio buttons, drop-downs); handwritten signatures.
- Picture editing: turn, mirror, crop, resize, exposure, contrast and saturation, with undo; lossless JPEG turning; Ctrl+S saves JPEG, PNG and WebP in place.
- PDF page organiser: reorder, turn, delete, insert, combine; export pages as PNG (72, 150 or 300 dpi) or as a new PDF; export a picture as a PDF.
- Printing through the desktop portal or straight to CUPS; Lumen's own file chooser and print dialog for desktops without portals.
- Folders with big thumbnails in a sidebar, drag and drop, preloading of neighbours, slideshow, fullscreen, pinch zoom on touchpads.
- Show in folder, open with, move to the trash with undo.
- Preferences (theme, slideshow time, PDF fit, sidebar, controls, cache and preload sizes, portals, sandbox) kept in `~/.config/lumen-rs/settings.toml`.
- Spanish translation.
- Search can match case (**Aa**) and whole words (**[ab]**); Space and Shift+Space scroll a screen in a PDF; Alt+Enter opens Properties; with the markup bar open, V H P R O L A T pick the tool; a preference makes ← and → wrap around at the ends of the list.
- Alt+Left and Alt+Right go back and forward after following a link in a PDF. A picture or PDF that another program changes is reloaded (a PDF at the same place); it waits on inotify, so an idle window costs no CPU.
- The control interface also fills in forms, draws annotations, edits pictures, saves and exports (`GetForm`, `SetField`, `SetMarkup`, `Annotate`, `GetAnnotations`, `Edit`, `Save`, `SaveCopy`, `Export`); its `busy` now covers saving, exporting and pictures loading behind the one on screen.
- Ctrl+C on a picture copies it to the clipboard (Wayland data-control; not available on GNOME or X11).
- Thumbnails use and add to the desktop's thumbnail cache (`~/.cache/thumbnails/large`), shared with the file manager; a preference turns it off.
- Lumen asks for the integrated GPU by default (it restarts itself once with `WGPU_POWER_PREF=low`; set that variable or `LUMEN_NO_REEXEC=1` to opt out): faster start and less memory on laptops with two GPUs.
- A command palette (Ctrl+Shift+P) finds any menu command, and zoom, next/previous file and page, by name.
- Deleting pages in the organiser shows a toast with an Undo button.
- Recent files on the empty window; the window's size and each PDF's last page are remembered (`~/.local/state/lumen-rs/state.toml`).
- A D-Bus control interface (off unless `LUMEN_CONTROL=1`) and end-to-end tests that drive the real app through it.
- Packaging files: AUR PKGBUILDs, a `.deb` through `cargo-deb`, a Flatpak manifest, AppStream metadata, a release workflow. None is published yet.
- A comparison of PDF rendering with poppler (`tools/fidelity.py`, `docs/FIDELITY.md`).
- CI: format, lint, tests, minimum Rust version, release build, packaging checks, AUR checks, dependency audit and a headless GUI job.

[Unreleased]: https://github.com/ImanolGo/lumen-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ImanolGo/lumen-rs/releases/tag/v0.1.0
