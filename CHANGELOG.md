# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed
- New app icon and symbolic icon: a fan of pages, the front one holding a photo above text with a highlighter stroke.
- Retook the screenshots.

## [0.1.0] - 2026-10-19

First release.

### Added
- Image viewing with zoom, pan, folder browsing and EXIF orientation.
- PDF viewing with continuous scroll, thumbnails, outline, search, text selection, links and password support.
- Rotate, flip, crop, resize and colour adjustment with undo; lossless JPEG rotation.
- Markup on pictures and PDFs: shapes, arrows, lines, pen, text and highlights.
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

[Unreleased]: https://github.com/ImanolGo/lumen/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ImanolGo/lumen/releases/tag/v0.1.0
