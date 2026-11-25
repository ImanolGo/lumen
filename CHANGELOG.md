# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Lumen compiles on macOS and Windows. There it shows files given on the command line or dropped on it; its dialogs, trash, clipboard, printing and file watching come next.

## [0.1.1] - 2026-11-25

### Security
- A decoder that panics, or whose answer is cut short, is an error for that picture; it is no longer decoded again outside the sandbox. A worker says when it has shut itself in, so one that was taken over cannot claim there is no sandbox.
- Animation frames are decoded in the sandbox too, under the same limits as still pictures. Animations with very large frames (over about 5 megapixels) show their first frame only.
- The decoding limit covers the pixels themselves (a small PNG could ask for 10 GB), and the worker's address space is capped at 8 GB.
- An SVG can no longer pull in other picture files from disk.
- The sandbox allows fewer system calls: no new processes, reading only the picture, no `newfstatat`, signals only to itself.
- The worker still starts after a package upgrade replaced the binary while Lumen was running.
- PDF pages of absurd size, and annotations and form fields that share one huge array, no longer exhaust the memory.

- Pages deleted in the organiser, or left out of an export, are really gone from the file: outline entries, links, named destinations, form fields and tagged text that led to them go too.
- The unlocked copy of a protected PDF made for printing is in a private folder and deleted after printing.

### Fixed
- Saving a PDF that another program rewrote no longer damages it: changes go into the version that was opened, and saving asks first (overwrite, or save a copy).
- Every save goes to the disk before it replaces the old file, follows links, and cannot collide with another save. A PDF is only written if the result opens again with all its pages.
- Moving a file with unsaved changes to the trash asks first, and Undo brings the changes back too.
- A damaged signatures file is kept (`signatures.toml.bak`) instead of written over; several windows, and the GTK Lumen, no longer overwrite each other's signatures, recent files or preferences.
- One recent file with a name that is not UTF-8 no longer empties the list; files on a disk that is not mounted are remembered.
- Annotations other programs made keep everything Lumen does not model (flags, dates, dashed and cloudy borders, line endings, pop-ups, their own look): those you did not change are not written at all, and those you did only change in what you changed.
- Saved form fields honour the turn, border and background of the field, wrap long text with real character widths, and fill comb fields cell by cell.
- Text beyond Western European letters (Škoda, œuvre, Cyrillic, Greek, CJK…) is written with a part of a system font embedded, instead of question marks.
- Check boxes whose state has a name that is not text are saved right; a null among a page's annotations or a form's fields no longer hides the rest.
- Inserting another PDF brings its form fields, named link targets and outline along.
- Reorganising keeps the turn of pages that give it as a real number; outline entries and links land at the right height on turned pages.
- Exporting a very large page as PNG records (and tells) the resolution it really has; exporting a small picture as a PDF no longer crops it.
- Thumbnails are never enlarged, know the picture's real size, and turn a PNG the way its EXIF says; a very large SVG keeps its proportions.
- A linked picture opens and is browsed where the link is, and moving it to the trash moves the link.
- Deleting or renaming the open file is noticed; a half-written file is no longer loaded.
- A JPEG whose EXIF has no orientation is turned without loss too.
- Animations show their last frame for its own time, and stop after as many rounds as they ask for.
- The trash uses an administrator's shared `.Trash` where there is one, and restoring never replaces a file.
- Search finds words with ligatures, accents written apart from their letters, and words broken across lines.
- Memory no longer grows with every zoom level of a PDF; rearranging a PDF of hundreds of thousands of objects takes a second or two.
- Curly quotes and dashes in PDF titles show in Properties, which opens faster for large PDFs.
- `CUPS_SERVER` is honoured, and a nonsensical answer from the printing service is an error.
- `LANGUAGE` decides the language before `LC_ALL`, `LC_MESSAGES` and `LANG`, as in other programs; the desktop's dialogs, the quit question and the password prompt are translated. The file dialog lists `.svgz` and `.j2k` files.
- Pages rearranged and not saved are kept when the organiser closes or another file is shown, and quitting asks about them.
- A place to save that was asked for one file is not used for another shown meanwhile; properties, thumbnails and inserted pages that arrive late do not land on another file.
- Delete in a text box no longer deletes the selected annotation as well; clicks on a question no longer reach what is under it.
- A saved picture only carries its colour profile over when that is an RGB one.
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

[Unreleased]: https://github.com/ImanolGo/lumen-rs/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/ImanolGo/lumen-rs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ImanolGo/lumen-rs/releases/tag/v0.1.0
