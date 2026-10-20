# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/ImanolGo/lumen/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/ImanolGo/lumen/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ImanolGo/lumen/releases/tag/v0.1.0
