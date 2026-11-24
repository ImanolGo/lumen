# Status

Where Lumen (Rust/iced) stands, as of 23 November 2026 (version 0.1.0, not released). It is a
ground-up rewrite of the [GTK Lumen](https://github.com/ImanolGo/lumen) 0.4.2, and
[parity-plan.md](parity-plan.md) is the plan that closes the gap.

## In one paragraph

Everything on the GTK Lumen's feature list works in some form, except the items under
[Not there yet](#not-there-yet). It is faster to open files than the GTK Lumen, uses no C library in
the document path, and decodes pictures in a sandbox. It has not been released, packaged or used by
anyone but its author, and a number of things have only been checked in tests and screenshots, not by
a person clicking (see [What has not been checked](#what-has-not-been-checked)).

## Numbers

`tools/bench.py`, medians of 5 cold starts on a 2020 laptop (i7-10710U, Wayland), with the
sandbox on. The JPEG is 5120×2160, the PNG 5120×2160.

| | GTK Lumen 0.4.2 | lumen-rs |
|---|---|---|
| First view, PDF 42 p / 477 p | 261 / 327 ms | **148 / 171 ms** |
| First view, JPEG / PNG (5120×2160) | 571 / 395 ms | **440 / 357 ms** |
| Memory after loading: PDF 42 p / 477 p | 125 / 159 MB | **121 / 120 MB** |
| Memory after loading: JPEG / PNG | **118 / 118 MB** | 144 / 133 MB |
| Idle CPU over 3 s after loading, PDFs | 70–80 ms | **0** |
| Idle CPU over 3 s after loading, pictures | 130–150 ms | **30 ms** |
| Binary size / shared libraries | **13 MB** / 145 | 36 MB / **5** |
| Tests that always run | 143 | **157** (154 unit + 3 sandbox) |

Notes:

- The GTK Lumen decodes pictures in glycin's separate processes, so its picture memory does not count the decoders; lumen-rs's sandbox worker is also a separate process whose memory is not counted either.
- Sandboxing costs about 0–1 ms for the sandbox itself and about 4 ms for a small picture. The pixels of a very large picture take 16–20 ms to come back over a pipe, so the overhead target of 15 ms holds up to roughly 4–5 megapixels and is missed by the biggest pictures (+17…30 ms at 11 MP).
- A folder of 5K pictures: 149 MB, down from 458 MB before the cache budget, streamed thumbnails and the sandbox.

## What works

PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA, PNM, JPEG XL, JPEG 2000, OpenEXR and SVG, with animation; PDFs
with outline, page thumbnails, search, selection, links, forms, passwords, markup saved as real
annotations, signatures, the page organiser, export to PNG and PDF, night mode, properties, printing
(portal or direct CUPS), slideshow, preferences, light and dark themes, Spanish, trash and show in
folder, lossless JPEG turning, save in place, a sandboxed decoder, a D-Bus control interface.
[FEATURES.md](FEATURES.md) has the detail.

## Not there yet

Things the GTK Lumen has that lumen-rs does not (checked against the GTK Lumen's changelog and our
code):

| Feature | GTK Lumen |
|---|---|
| Back and forward after following a PDF link (Alt+Left / Alt+Right) | yes |
| Reload when another program changes the open file | yes |
| Copy the picture to the clipboard (Ctrl+C on a picture) | yes |
| Search: match case, whole words | yes |
| Command palette (Ctrl+Shift+P) | yes |
| Recent files on the empty window | yes |
| Letter keys for the markup tools (V, R, O, A, L, P, T, H) | yes |
| Space and Shift+Space scroll a screen in a PDF | yes |
| "Wrap around" preference for ← / → | yes |
| The desktop's thumbnail cache (`~/.cache/thumbnails`) | yes |
| Remembering the window size and the last page of each PDF | yes |
| An undo toast after deleting pages | yes |
| Alt+Enter for Properties | yes |
| Screen-reader support (accessible names) | yes (GTK) |
| A control interface covering forms, annotation, editing and export | yes (this one covers opening, browsing, pages, zoom and search) |
| HEIC and AVIF without a C library | via glycin (C-free here only with the optional `heif` feature, which links libheif) |
| Camera RAW | no (neither has it) |

## What has not been checked

- **Real input.** There is no way to send real mouse, keyboard or touchpad events where this was developed, so interaction was checked by sending the app the messages those events would produce, plus screenshots. Keyboard shortcuts, dragging, pinch zoom and the touchpad scroll have not been exercised by a person in the latest builds.
- **The desktop's own dialogs.** The file and print portals, Show in Folder and Open With were not run (they would open windows on the author's desktop); only the built-in fallbacks and the trash were.
- **Very large animations** beyond the 96 MB cache, and the home trash (`~/.local/share/Trash`) in the app, were covered by unit tests only. The other-disk trash was tried by hand.
- **AVIF and HEIC** were tested only with libheif 1.23 on one machine; CI does not build that feature.
- **Packaging.** The `-git` PKGBUILD was built once with `makepkg` and the `.deb` was built and inspected; the release workflow, the AUR upload, the Flatpak build and the released `lumen-rs` PKGBUILD (it needs a tag) have never been run.
- **PDF rendering fidelity** was measured against poppler on 132 pages of real documents and a feature test ([FIDELITY.md](FIDELITY.md)): close, with heavier hairlines and slightly heavier text. The corpus is mostly text documents; scanned pages, JBIG2 and JPEG 2000 images, CJK fonts and exotic transparency were not covered.
- **The Spanish** in dialogs other than the menu and Preferences has not been looked at on screen, and was written by the same author as the code.
- **The headless GUI test job** in CI is new and may need adjusting.

## Known issues

- Pictures with a very large side are shrunk to 16 384 px for display.
- An earlier commit added a binary by mistake; it was removed from the history, but GitHub may still serve the old commits by their hashes until it cleans up.
