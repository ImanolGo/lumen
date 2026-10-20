# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed
- The document menu in the header (Edit for a picture, Pages for a PDF) no longer closes by itself a few seconds after it opens. Its model was reassigned on every view update, and GTK builds a new popover each time, so the open menu was thrown away.

## [0.4.0] - 2026-10-20

### Added
- Opening a single picture lets you browse the other pictures in its folder (a preference, on by default). Several files, a folder or a PDF open exactly as before.
- After following a link or an outline entry in a PDF, Alt+Left goes back to where you were and Alt+Right forward again, and a toast has a Back button.
- The open file is shown again when another program changes it on disk (handy for documents being rebuilt); with unsaved edits, a toast offers to reload instead.
- Ctrl+Delete (or the main menu) moves the file on screen to the trash and goes on to the next one; the toast's Undo puts it back.
- Ctrl+C on a picture copies it (and the file, when it has no unsaved edits). The main menu gains Show in Folder and Open With.
- PDF view options in the main menu: fit the page width or a whole page, and a night mode (Ctrl+Shift+N) that inverts the lightness of pages and keeps their hues.
- The search bar has Match Case and Whole Words toggles.
- A command palette (Ctrl+Shift+P): type to find any command and run it.
- The empty window shows your recent files with their thumbnails.
- While the markup toolbar is open, V, R, O, A, L, P, T and H choose the tool (shown in the tooltips).
- In a PDF, Space and Shift+Space scroll a screen (and carry on to the next or previous file at the end). Home and End go to the first and last file when a picture is shown. Shift+Left and Shift+Right move a selected annotation in bigger steps.
- A "Wrap around" preference: ←/→ continue past the last or first file.
- Deleting pages shows "1 page deleted · Undo" at the bottom of the window; saving and exporting confirm the same way.

### Changed
- One accent colour: the icon's amber replaces the system blue for the selected page, the active tool, suggested actions and switches.
- A slimmer header bar: one menu for what can be done to the document (page changes for a PDF, picture edits for a picture), and Properties moved into the main menu (Alt+Enter still works).
- Editing, saving, "Export as PDF" and inserting a picture as a page decode through glycin's sandbox, like viewing does. With glycin installed, AVIF, HEIF, JPEG XL and similar pictures can now be edited and saved as a copy.
- The shortcuts dialog is built from the same table as the registered shortcuts, so they cannot drift apart.

### Faster
- Moving to a picture that is still being decoded shows its thumbnail, scaled up, which fades into the real picture. PDF pages still being drawn show their sidebar thumbnail instead of blank paper.
- Thumbnails use the desktop's thumbnail cache (`~/.cache/thumbnails`): ones the file manager made are reused, and the ones Lumen makes are saved there.
- Animated pictures show their first frame at once; the other frames are decoded in the background.
- Dragging the colour-adjust sliders on a large picture renders one preview at a time, spread over all cores.
- Filling in a form field or undoing no longer flashes the pages white.
- Searching waits for a short pause in typing. The PDF workers for search, selection, links and thumbnails open the document only when first used.
- A JPEG inserted into or exported as a PDF is embedded as it is, instead of as large uncompressed pixels.
- Working copies of an edited PDF live in the cache folder instead of `/tmp` (often memory), and the ones undo can no longer reach are deleted.
- Pictures you only flip past no longer fill the desktop's recent-files list, and the remembered state is written at most every two seconds.

### Fixed
- Saving a picture or PDF no longer replaces the files you are browsing with just the saved one; Save As adds the copy after the current file.
- Saving over a file keeps its permissions and follows a symlink or hard link instead of replacing it with a separate file.
- Saving an edited JPEG, PNG or WebP keeps its embedded colour profile, so wide-gamut photos no longer shift colour.
- A colour-adjust preview that failed to render no longer undoes the last edit you made.
- Ctrl+G brings back the faded page controls before focusing the page box, and the box is no longer reset while you type in it.

## [0.3.1] - 2026-10-20

### Fixed
- SVG drawings stay sharp when zoomed in: the part in view is rendered again at the zoom level (in glycin's sandbox) instead of the decoded pixels being enlarged.
- The orientation glycin reads from a picture (HEIF and other formats Lumen's own EXIF reader does not cover) is used again; pictures without any orientation tag counted as rotated internally.

## [0.3.0] - 2026-10-20

### Added
- A D-Bus control interface, `io.github.imanolgo.Lumen.Control1`, for scripts and tests: open, navigate, fill in forms, annotate, edit pictures, change pages, undo, save, export, search and take screenshots, with JSON answers. Nothing destructive happens without an explicit flag, and no call waits on a dialog. See docs/DEVELOPMENT.md.
- End-to-end tests that drive the real app through that interface.

### Fixed
- After saving a PDF, Lumen asked whether to save the changes it had just saved.

### Removed
- The ad-hoc `win.*` actions for tests (`save-to`, `markup-draw`, `page-op` and so on); the control interface does what they did.

## [0.2.0] - 2026-10-20

### Added
- Pictures are decoded with glycin, in sandboxed processes, so a malicious file cannot reach the system. This adds HEIC, AVIF, JPEG XL, JPEG 2000, OpenEXR, QOI and more, and animated GIF, APNG and WebP now play. File thumbnails are decoded the same way. Without glycin's loaders, GTK's own decoders are used as before.
- PDF forms can be filled in: text fields, checkboxes, radio buttons and lists. Each change is one undo step, and the saved file shows the answers in any reader.
- Signatures: draw one once, keep it, and stamp it onto PDFs and pictures, or into a form's signature field. On a PDF it is one ink annotation that other readers show.

### Changed
- Rust 1.93 or newer is needed (glycin 4), and libseccomp to build.
- Reloading a PDF with the same pages (a form filled in, an undo) keeps the zoom and scroll position.
- In text annotations and multi-line form fields, `Enter` finishes and `Shift+Enter` starts a new line (it was `Ctrl+Enter` to finish).

## [0.1.0] - 2026-10-19

First release.

### Added
- Image viewing with zoom, pan and EXIF orientation. Lumen shows the files you open (or every picture and PDF in a folder you open), and files or folders dropped onto the window are added to them.
- PDF viewing with continuous scroll, thumbnails, outline, search, text selection, links and password support.
- Rotate, flip, crop, resize and colour adjustment with undo; lossless JPEG rotation.
- Markup on pictures and PDFs: shapes, arrows, lines, pen, text and highlights, in Lumen's amber and yellow by default, with eight swatches, a custom colour and one size slider in a popover.
- PDF page management: reorder, rotate, delete, insert, merge and export.
- Printing, fullscreen and slideshow.
- Preferences, remembered window state, keyboard shortcuts dialog and accessible names.
- Spanish translation.
- Desktop file, metainfo, app and symbolic icons, Makefile install, Flatpak manifest, Arch `PKGBUILD` and GitHub Actions for CI and releases.
- PDF document properties, PDF thumbnails in the file sidebar, and floating zoom and page controls.
- Editing of GIF, BMP and TIFF pictures (saved as a copy in JPEG, PNG or WebP).
- A warning before saving a rearranged password-protected PDF, which can only be written without its password.
- `win.screenshot-to` and `build-aux/screenshot.sh` for headless screenshots.
- Packages: Flatpak, `.deb` for Ubuntu 26.04+ and Debian testing (`make deb`), and `lumen` and `lumen-git` PKGBUILDs for the AUR. The release workflow builds the Flatpak, the `.deb` and a tarball, and can be run by hand without publishing.
- README, `CONTRIBUTING.md`, `docs/RELEASING.md` and this changelog.

[Unreleased]: https://github.com/ImanolGo/lumen/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/ImanolGo/lumen/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/ImanolGo/lumen/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/ImanolGo/lumen/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ImanolGo/lumen/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ImanolGo/lumen/releases/tag/v0.1.0
