# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Floating zoom controls (zoom out, zoom level, zoom in) and page navigation for PDFs at the bottom of the view.
- Document properties for PDFs: title, author, subject, pages, page size, creation date, producer and PDF version.
- PDF thumbnails in the file sidebar.
- An "Open…" button on the start page, and the app icon there.
- `win.screenshot-to` action and `build-aux/screenshot.sh` for headless screenshots; window actions are also on D-Bus under Broadway.
- Undo, Redo, Save and Save As in the Pages menu, as in the Edit menu for pictures.
- PNG renditions of the app icon (16 to 512 px) and a README with screenshots, install instructions and usage.
- `CONTRIBUTING.md` and this changelog.

### Changed
- Refreshed app icon and symbolic icon; new symbolic icons for the markup tools and the Edit menu, bundled in the binary.
- Simpler header bar: Open and Recent Files are one split button, the sidebar toggle sits on the left, Print moved to the main menu and the page number moved to the floating controls.
- The markup toolbar uses icons instead of text labels.
- The sidebars slide in and out, PDF pages cast a soft shadow, and the loading spinner is libadwaita's.
- Escape leaves fullscreen and the slideshow when nothing else (a tool, a selection) wants it.

### Fixed
- Keys without modifiers (Space, Backspace, arrows, Home, End, Page Up/Down) no longer trigger shortcuts while typing in the page number, the search bar or a dialog, or while a menu is open. Backspace in the page entry used to go to the previous file.
- The Edit menu button showed a missing icon (`edit-symbolic` is not in the Adwaita theme).
- The sidebar button was hidden when a single PDF was open, although the page sidebar was there.
- The file sidebar stopped showing thumbnails after the folder was listed: rows were tracked by position, and a recycled row could be dropped as soon as it was bound.
- The app icon was missing from the About dialog and the start page when running from the source tree.
- The documented minimum versions were wrong: the build needs libadwaita 1.8 and poppler-glib 25.06. The PKGBUILD now says so too.
- An error message that was not translated.

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

[Unreleased]: https://github.com/ImanolGo/lumen/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ImanolGo/lumen/releases/tag/v0.1.0
