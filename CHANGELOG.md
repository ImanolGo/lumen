# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.9.0] - 2026-11-26

### Added
- More ways to select part of a picture, chosen in the editing tools: a rectangle, freehand (lasso), or by colour (click, and drag away to take in more of it). A selection can be inverted (Ctrl+Shift+I), and its pixels deleted (Delete), which makes them transparent; undo brings them back. Copy takes only what is selected, with the rest transparent.

### Changed
- Lumen (Rust/iced) is now Lumen: the repository is ImanolGo/lumen, the command `lumen`, the app ID `io.github.imanolgo.Lumen`, and on Linux the settings and state are in `~/.config/lumen` and `~/.local/state/lumen` (moved from `lumen-rs` the first time). The AUR packages are `lumen-viewer` and `lumen-viewer-git`, as `lumen` there is another program; release files are called `lumen-…`. The first Lumen, written with GTK, is now [lumen-gtk](https://github.com/ImanolGo/lumen-gtk).

## [0.8.0] - 2026-11-26

### Added
- macOS: several windows. A file opened from Finder has a window of its own; one that is open already is brought forward. Open in New Window and ⌘-click work as on the other systems. Nobody has tried this on a Mac yet.
- Close Window (Ctrl+W); Ctrl+Q quits, on macOS all windows, after asking about what is not saved in any of them.
- The control interface has `OpenWindow`.

## [0.7.0] - 2026-11-25

### Added
- Linux and Windows: Open in New Window, in the File menu and a sidebar file's right-click menu; Ctrl+click in the sidebar or in Open Recent does the same.

### Changed
- A window that opens on a picture is made to fit it: at its own size or smaller, at least 720×480 and at most 80 % of the screen. It stays that size while browsing, and where the system sizes windows itself it is left alone. PDFs and folders open at the remembered size, as before.

## [0.6.0] - 2026-11-25

### Changed
- The header has what is done most in its middle: the zoom, rotate and crop for pictures, Markup, Edit or Organise by name; Undo, Discard and Save appear there when something has changed.
- Tools are in one row under the header, where they always are, with their names where there is room. They float over the page no more. Colours, Resize and Light and colour open under their own buttons.
- Crop can be started from the header, without the other editing tools; the File menu has Discard Changes.
- A file opened by itself is shown by itself: a picture without the sidebar, a PDF with its contents or pages. The arrows go through the pictures around a picture, and never leave a PDF. "Show Folder" (T) brings the folder's list, which is then what is browsed; several files or a folder are a list from the start.
- Dropping a file on an open one adds that file, not its whole folder.
- The preference to show the sidebar when Lumen starts is gone.

### Fixed
- A picture counts as opened (for the control interface and the progress line) only once it is on screen.

## [0.5.0] - 2026-11-25

### Added
- The sidebar can be made wider or narrower by dragging its edge; its thumbnails grow with it.
- The contents' entries can be folded.
- The percentage in the zoom control opens a menu: best fit, fit width, 50 %, 100 %, 200 %.
- Menus, dialogs and messages slide into place, and what is behind a dialog dims, in 120 ms; a Reduce motion preference turns this off, with the easing of the zoom and the fading of the controls.
- A file that cannot be opened offers to open another one; HEIC and AVIF pictures are said not to be in this build.

### Changed
- Tools and the Find bar no longer cover the page: it starts, and pictures are fitted, below them.
- While a file loads the one before it stays in view, with a line under the header if it takes long, instead of a blank view with "Loading…".
- Long titles in the contents and names in the sidebar end with "…".
- Find says "No results" in red. Hints wait half a second.
- The markup button is only there when there is something to mark up, and the empty window's shortcut is the system's own.

### Fixed
- In a small window the sidebar starts at the file on screen.
- In the smallest window the Find bar, the crop bar and the edit panels are no longer cut off, and messages do not cover the zoom control.

## [0.4.0] - 2026-11-25

### Added
- Quitting with changes offers to save them, when they are all in the file on screen.
- Lumen's print dialog takes a range of pages and the orientation.
- Properties: the values can be selected and the path copied; dates are in local time.
- Lumen's file chooser: type to jump to a name, arrow keys to move.
- A right-click menu on the picture or page, on selected text, on an annotation (delete, bring to front) and on a file in the sidebar.
- Open Recent in the ☰ menu; Ctrl+, opens the preferences; a Mac's menu bar has every command.

### Changed
- Dialogs share one layout: the buttons are always in view however small the window, never over the header, and in the order each system has them. Enter does what the dialog is for, except throw something away; keys no longer reach what is behind a dialog. Export is one form; Preferences are regrouped; shortcuts are drawn as keys.
- The ☰ menu is short: eleven rows, with View, Export and File leading to more. It hangs from its button, stays inside the window and scrolls when it must. On Windows and macOS the preferences are called Settings.
- The command palette names each command's group.
- The keys, the menus, the palette, the list of shortcuts and its documentation come from one table of commands. A plain letter no longer answers while Alt is held.
- One set of controls: inputs, drop-downs, check boxes, sliders and scrollbars look the same everywhere, buttons that cannot be pressed are neutral in every colour, and secondary text, icons and buttons are easier to read in both themes (a test checks the contrast). The file on screen is marked in the sidebar by its ring alone.

### Fixed
- What a button is for no longer stays on screen over the menu it opened, or over anything else a click or a key opens.

## [0.3.0] - 2026-11-25

### Added
- Drag over a picture to select a part of it: copy it (Ctrl+C), crop to it (Ctrl+K), or select everything (Ctrl+A). A preference makes dragging move the picture, as before; the middle button and the wheel always do.
- Discard: unsaved markup, form values and edits can be thrown away with a button beside Save, or with Escape once nothing else is open. It asks first.
- The language can be chosen in Preferences.

### Changed
- On Linux the header is the window's title bar: drag it to move the window, double-click to maximise, right-click for the system's menu, and drag the edges to resize. GNOME and KDE no longer draw a second title bar above it. Minimise and maximise buttons are a preference, off by default. On macOS the header is inside the title bar; on Windows it is a toolbar under the system's.

### Fixed
- Zooming no longer flickers: the picture or page and its position always change together, and a sharper page takes the old one's place only once it is on the GPU (animations and edits too).
- The pen no longer draws a line from the top of the page when the view is scrolled (nor does the crop frame jump), and what it draws is smooth.
- In a narrow window the tools wrap onto more lines: Save, Undo and the rest are no longer cut off.
- On Windows the window has Lumen's icon; on Windows and macOS it never opens larger than the screen.
- With the software renderer (`ICED_BACKEND=tiny-skia`) pages no longer turn grey and menus black, and everything is much faster: shadows are left out there.

## [0.2.0] - 2026-11-25

### Added
- Lumen runs on macOS and Windows, with each system's own file dialogs, trash (with Undo on Windows), clipboard, file watching, folders and language. Printing goes through Lumen's dialog and CUPS on macOS, and through the program that prints PDFs on Windows. The decoder there is a separate process that is not shut in. Nobody has tried these builds by hand yet.
- On macOS, shortcuts are shown with ⌘, the menu bar has Lumen's commands (its Quit asks about unsaved changes), Ctrl+⌘F is fullscreen, and files opened from Finder open in the running window.
- The control interface also works over standard input and output (`LUMEN_CONTROL=stdio`), on every system.
- Packages for macOS (a `.dmg` for Apple silicon and Intel) and Windows (a `.zip`), not signed.

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

[Unreleased]: https://github.com/ImanolGo/lumen/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/ImanolGo/lumen/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/ImanolGo/lumen/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/ImanolGo/lumen/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/ImanolGo/lumen/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/ImanolGo/lumen/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/ImanolGo/lumen/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/ImanolGo/lumen/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ImanolGo/lumen/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/ImanolGo/lumen/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ImanolGo/lumen-rs/releases/tag/v0.1.0
