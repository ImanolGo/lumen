# Status

Where Lumen stands, as of 26 November 2026 (version 0.12.0). It is a
ground-up rewrite of the [GTK Lumen](https://github.com/ImanolGo/lumen-gtk) 0.4.2, which it replaces, and
[parity-plan.md](parity-plan.md) is the plan that closes the gap.

## In one paragraph

Everything on the GTK Lumen's feature list works in some form, except the items under
[Not there yet](#not-there-yet). It opens files about as fast as the GTK Lumen, with less CPU and none while idle, uses no C library in
the document path, and decodes pictures in a sandbox. Version 0.12.0 is tagged and published on
GitHub; it is in no distribution's packages, has been used by no one but its author, and a number of things have only been checked in tests and screenshots, not by
a person clicking (see [What has not been checked](#what-has-not-been-checked)).

## Numbers

`tools/bench.py`, medians of 3 cold starts on 26 November 2026, on a 2020 laptop (i7-10710U, Wayland, Intel UHD;
a 6000×4000 JPEG, the other files as named there). Version 0.8.0 against the GTK Lumen 0.4.

| | GTK Lumen 0.4 | Lumen 0.8 |
|---|---|---|
| Window on screen | **~112 ms** | ~300 ms |
| First view, PDF 42 p / 477 p | 304 / 386 ms | 310 / **368 ms** |
| First view, JPEG 24 MP / PNG 5120×2160 | **647 / 433 ms** | 769 / 456 ms |
| CPU time to the first view | 420–560 ms | **280–340 ms** |
| Idle CPU over 3 s after loading | 60–150 ms | **0–20 ms** |
| Memory (RSS) | 165–210 MB | 160–238 MB |

- Most of Lumen's time to the first view is the GPU renderer starting (the window takes about 300 ms where GTK's takes 110); with the software renderer (`ICED_BACKEND=tiny-skia`) the window takes about 40 ms. Earlier in development Lumen measured 148 / 171 ms for the PDFs on this laptop, before an NVIDIA driver and kernel upgrade; since then it starts itself once more with `WGPU_POWER_PREF=low` so as not to wake the discrete GPU (`LUMEN_NO_REEXEC=1` keeps it: 345 / 367 ms and 236 / 242 MB for the PDFs).
- Sandboxing costs about 0–1 ms for the sandbox itself and about 4 ms for a small picture. The pixels of a very large picture take 16–20 ms to come back over a pipe, so the overhead target of 15 ms holds up to roughly 4–5 megapixels and is missed by the biggest pictures (+17…30 ms at 11 MP).
- A folder of 5K pictures: 149 MB, down from 458 MB before the cache budget, streamed thumbnails and the sandbox.
- Watching the open file for changes costs nothing while idle (it waits on inotify). A first version that looked at the file every 1.5 s raised the idle CPU of a PDF from 0 to 30 ms per 3 s, which is why it does not.

## What works

PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA, PNM, JPEG XL, JPEG 2000, OpenEXR and SVG, with animation; PDFs
with outline, page thumbnails, search, selection, links, forms, passwords, markup saved as real
annotations, signatures, the page organiser, export to PNG and PDF, night mode, properties, printing
(portal or direct CUPS), slideshow, preferences, light and dark themes, Spanish, trash and show in
folder, lossless JPEG turning, save in place, a sandboxed decoder, a D-Bus control interface.
[FEATURES.md](FEATURES.md) has the detail.

## Not there yet

Things the GTK Lumen has that lumen does not (checked against the GTK Lumen's changelog and our
code):

| Feature | GTK Lumen |
|---|---|
| Copy the picture on GNOME and X11 (it needs the Wayland data-control protocol, which GNOME's compositor lacks) | yes (GTK's clipboard) |
| Screen-reader support (accessible names) | yes (GTK) |
| HEIC and AVIF without a C library | via glycin (C-free here only with the optional `heif` feature, which links libheif) |
| Camera RAW | no (neither has it) |

## What has not been checked

- **Real input.** There is no way to send real mouse, keyboard or touchpad events where this was developed, so interaction was checked by sending the app the messages those events would produce, plus screenshots. Keyboard shortcuts, dragging, pinch zoom and the touchpad scroll have not been exercised by a person in the latest builds.
- **The desktop's own dialogs.** The file and print portals, Show in Folder and Open With were not run (they would open windows on the author's desktop); only the built-in fallbacks and the trash were.
- **Very large animations** beyond the 96 MB cache, and the home trash (`~/.local/share/Trash`) in the app, were covered by unit tests only. The other-disk trash was tried by hand.
- **AVIF and HEIC** were tested only with libheif 1.23 on one machine; CI does not build that feature.
- **Packaging.** The `-git` PKGBUILD was built once with `makepkg` and the `.deb` was built and inspected; the release workflow ran for v0.1.0. The AUR upload, the Flatpak build and the released `lumen` PKGBUILD have never been run.
- **PDF rendering fidelity** was measured against poppler on 132 pages of real documents and a feature test ([FIDELITY.md](FIDELITY.md)): close, with heavier hairlines and slightly heavier text. The corpus is mostly text documents; scanned pages, JBIG2 and JPEG 2000 images, CJK fonts and exotic transparency were not covered.
- **The Spanish** in dialogs other than the menu and Preferences has not been looked at on screen, and was written by the same author as the code.
- **The headless GUI test job** in CI (the control tests on a headless Wayland compositor with the software renderer) passed on every run so far, so a failure now blocks. Its software rendering is not the GPU path people use.

## macOS and Windows

The author went through the checklist below by hand on a MacBook Pro and on Windows 11 (in a virtual
machine) with 0.2.0 on 25 November 2026, and the last two rows with the build that became 0.3.0. It brought up the flicker while
zooming, the pen's vertical lines, the missing way to discard markup, Windows' generic window icon and a
window larger than the screen, all were fixed in 0.3.0, and the author confirmed the fixes. What CI does on a real machine of
each, on every push: the unit tests; a file moved to the real trash (and back, on Windows); noticing
changes to a file; and the app itself with a window (software rendering), driven over standard
input: opening and browsing files, searching, passwords, filling in and saving a form, annotating
and saving a PDF, editing and saving a picture, exporting, reloading a changed file. The release
workflow builds the `.dmg` and the `.zip` and starts the program with `--version`.

The checklist for a person (package 11 of [improvement-plan.md](improvement-plan.md)), as reported by the author:

| To check | macOS | Windows |
|---|---|---|
| Open a picture, a folder and a PDF from the dialog, and by dropping | tried | tried |
| Open from the file manager (double click, Open With), with Lumen closed and running | tried | tried |
| Arrows; zoom with the wheel and the touchpad; fullscreen and back | tried | tried |
| Markup and Ctrl/⌘+S on a PDF, reopened in another program | tried | tried |
| Crop a JPEG and save; fill `tests/fixtures/form.pdf`; unlock `encrypted.pdf` (`secret`) | tried | tried |
| Trash, and undo (Windows) | tried | tried |
| Copy a picture and paste it elsewhere | tried | tried |
| Print to a PDF printer | tried | tried |
| Switch the system between light and dark; a second screen with another scale | tried | tried |
| The menu bar, ⌘ labels and ⌘Q with unsaved changes (macOS) | tried | |
| Spanish (the language is in Preferences from 0.3 on) | tried | tried |

## Known issues

- Pictures with a very large side are shrunk to 16 384 px for display.
- An earlier commit added a binary by mistake; it was removed from the history, but GitHub may still serve the old commits by their hashes until it cleans up.
