# Features

Everything Lumen does, in detail. The [README](../README.md) has the short version, and
[STATUS.md](STATUS.md) says what is still missing compared with the GTK Lumen.

## Contents

- [What it does](#what-it-does)
- [Keyboard](#keyboard)
- [Accessibility](#accessibility)
- [Files Lumen reads and writes](#files-lumen-reads-and-writes)
- [Environment variables](#environment-variables)

## What it does

- **Pictures:** PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA, the PNM family, JPEG XL, JPEG 2000, OpenEXR (linear light, clamped and given the sRGB curve) and SVG, all decoded in pure Rust. EXIF orientation is honoured.
- **AVIF and HEIC (optional):** `cargo build --release --features heif` adds them through the system's libheif (needs `libheif` and its AVIF/HEIC codecs installed). That is the only C code Lumen can reach, so it is off by default: the default build is pure Rust. Add `image/heic;image/heif;image/avif;` to the `.desktop` file's `MimeType` when you build it that way.
- **Animation:** animated GIF, APNG and WebP play, each frame for its own delay (a delay of 10 ms or less shows for 100 ms, as browsers do). Frames are decoded on a thread of their own and the first pass is kept while it fits in 96 MB; longer animations keep streaming from the file. The first frame is what editing and markup work on, so the animation pauses (and shows it) while they are open.
- **PDFs:** continuous scrolling, a table of contents, search, and clickable links. Only the pages near the view are rendered, and that happens on a pool of background threads that does the newest request first. Pages re-render sharply at every zoom level and on HiDPI screens.
- **Recent files and memory:** the empty window lists the last five files you opened (click one to open it; **Clear** forgets them all; files that no longer exist or cannot be opened drop out). A PDF opens at the page you left it at, and the window comes back at the size it had when you closed it (on desktops that let applications choose; a tiling compositor decides for itself).
- **Reloading:** while a picture or PDF is open Lumen looks at its size and modification time every 1.5 seconds. If another program changed it and you have nothing unsaved, it is loaded again (a PDF returns to the place you were reading, its thumbnail is redrawn) with a short notice; with unsaved changes it only says so, and keeps them. Lumen's own saves do not count as changes.
- **Search:** Ctrl+F finds text across the whole document (case-insensitive unless you turn on **Aa**; **[ab]** matches whole words only), highlights every match and jumps between them. It starts from the page you are on, results appear while it scans, and each page's text is read only once. The 477-page Ghostscript manual is fully indexed in about 0.7 s.
- **Reading order:** pages set in columns of running text (two, three or more, also on turned pages) are read column by column for copying and search, even when the file draws them line by line across. Everything else (tables, code, forms, diagrams) keeps the order the file has, which is usually the right one.
- **Copying a picture:** Ctrl+C (or ☰ → Copy Picture) puts the picture on the clipboard as a PNG, with its edits (crop, colour) but without unsaved markup. It uses the Wayland data-control protocol through the pure-Rust `wl-clipboard-rs` crate, so it works on compositors that offer it (niri, Sway, Hyprland, KDE) and says so when it cannot (GNOME, X11 sessions).
- **Text selection:** drag over text to select it (Shift+click extends, double-click picks a word), Ctrl+C copies, Ctrl+A selects the whole document. Like other Linux apps, the selection also goes to the primary selection for middle-click paste. Dragging anywhere that is not text still pans.
- **Pages sidebar:** a third tab, next to Files and Contents, shows a thumbnail of every page (drawn only for the rows on screen); click one to go there, and the page you are on is ringed and followed as you scroll.
- **Go to page:** Ctrl+G (or a click on "Page x of y" in the header) asks for a page number.
- **Night mode:** Ctrl+Shift+N (or ☰ → Night Mode) turns paper dark and text light while pictures in the page keep their colours (the GTK Lumen's colour matrix); page thumbnails follow. It is remembered for the session.
- **Export:** ☰ → Export Pages… (or the export button in the page organiser) writes the selected pages, or the page in view, as PNG images at 72, 150 or 300 dpi (several pages go into a folder as `name-01.png`, …; the file records its resolution; turned pages come out turned) or as a new PDF (unsaved markup and form values included; a protected PDF stays protected). ☰ → Export as PDF… turns a picture into a PDF as big as the picture, without margins. ☰ → Combine Files… opens the page organiser and asks for the PDFs to add.
- **File actions:** ☰ → Show in Folder (the file manager selects it, over D-Bus, or `xdg-open` on the folder), Open With… (the portal's app chooser) and Move to Trash. Trashing follows the freedesktop Trash spec (`~/.local/share/Trash`, or `.Trash-<uid>` at the top of another disk, with a `.trashinfo` note, so file managers can restore it too), shows the next file and offers Undo in the toast for eight seconds. A file that cannot be renamed into a trash is reported, never copied and deleted. There is no keyboard shortcut for the trash, so a stray key in a text box cannot send the file there.
- **Slideshow:** F5 (or ☰ → Slideshow) goes fullscreen and shows each picture for five seconds, PDFs page by page, then the next file; Space pauses and resumes, Esc leaves, and it ends after the last file (the window goes back to how it was). Flipping by hand restarts the count. It will not start with unsaved changes and stops if any appear. The interval becomes a preference with the preferences dialog.
- **Preferences:** ☰ → Preferences… changes, at once and saved to `~/.config/lumen-rs/settings.toml`: the theme (system, light, dark), whether the sidebar is shown at start, the zoom control and arrows over pictures, how a PDF is fitted when it opens, PDF night mode, the slideshow time (2 to 20 s), whether ← and → wrap around at the ends of the list, whether to share thumbnails with the desktop's cache, how many megabytes of decoded pictures are kept and up to how many megapixels neighbours are decoded ahead (or none), and whether to use the desktop's file and print dialogs. The file is read leniently: a missing key, an unknown key or a bad value keeps its default and never stops Lumen from starting. `LUMEN_THEME` and `LUMEN_NO_PORTAL` in the environment win over the file.
- **Sandboxed decoding:** raster pictures (everything but SVG, PDF and, in the optional build, AVIF/HEIC) are decoded by `lumen-rs --decode-worker`, a fresh process per picture that gets the open file as its standard input, shuts itself in (Landlock: no file system access at all; seccomp: about forty allowed system calls, anything else kills it, and `write` only to its two output descriptors) and only then reads and decodes. The pixels come back over a pipe. A decoder that crashes, is stopped by the sandbox or hangs for two minutes costs one picture and an error message. If the sandbox cannot start Lumen decodes in-process and logs it once; if the sandbox itself stops a decoder it does not fall back, so a hostile file never runs unsandboxed. A worker starts and exits in about 2 ms, a small picture costs about 4 ms in all, and the pixels of a very large picture (44 MB for 11 megapixels) take another 16 to 20 ms to come back over the pipe; `tests/sandbox.rs` runs every format through the real binary and checks that opening files, sockets, running programs and writing files are all stopped. Turn it off in Preferences or with `LUMEN_NO_SANDBOX=1`.
- **Control interface (off by default):** with `LUMEN_CONTROL=1` in its environment, Lumen offers `io.github.imanolgo.LumenRs.Control1` on the session bus (`/io/github/imanolgo/LumenRs/Control`) for scripts and tests: `GetState` (JSON: what is open, the file's place among its neighbours, page, zoom, search results, whether it is busy), `Open(paths, password)`, `Next`, `Previous`, `First`, `Last`, `GotoPage(page)`, `Zoom(in|out|fit|width|actual)`, `Search(text)` and `Quit`. It is off by default because anything on the session bus could otherwise open files in your viewer. `tests/control.rs` drives the real app through it (`LUMEN_CONTROL_TESTS=1 cargo test --test control`; it opens real windows, so it is not part of `make test`).
- **Spanish:** Lumen speaks Spanish when `LC_ALL`, `LC_MESSAGES`, `LANGUAGE` or `LANG` says so (like gettext). English is the source language: every UI string goes through `tr()` / `trf!()` (`src/i18n.rs`) and is looked up in `po/<language>.po`, which is embedded in the binary, so there is no `msgfmt` step and nothing to install. About a fifth of `po/es.po` is reused from the GTK Lumen; a test fails if any string in the source lacks a Spanish entry, so new text cannot be forgotten. To add a language, add a `.po` file and one line to `CATALOGUES`.
- **Properties:** ☰ → Properties… shows the file (name, folder, size, dates in UTC) and, for a PDF, its title, author, producer, version, page count and size, protection and form fields; for a picture its format, size, colour profile and EXIF summary (camera, lens, exposure, date, location).
- **Table of contents:** the PDF's outline appears in the sidebar, follows you as you scroll, and jumps to the exact spot of each section.
- **Links:** links inside the document jump to their target. Web and mail links open in your browser or mail app through the desktop portal; other kinds of link (files, programs) are ignored for safety.
- **Markup:** press M (or the pen button) to draw on a picture or a PDF page: highlighter, pen, rectangles, ellipses, lines, arrows and text boxes, in eight colours and three line widths. Select, move, resize, recolour and delete what you drew, with undo and redo. On a PDF the highlighter marks the text you drag over.
- **Real PDF annotations:** markup is saved into the PDF as standard annotations (highlight, ink, square, circle, line, free text), each with its own appearance, so every PDF viewer shows it. Saving appends an incremental update: the original bytes are kept as they were. Annotations of those kinds that are already in a PDF, from Lumen or another app, can be edited too; comments attached to them are kept.
- **Password-protected PDFs:** Lumen asks for the password and opens the file; the password stays in memory for the session only, so flipping back does not ask again. Markup and form values can be saved into it and it stays protected (what is added is encrypted the same way). Printing sends an unlocked copy, since the print system could not open the file otherwise. In the file list a protected PDF shows a lock until it is unlocked, then its real thumbnail.
- **Forms:** fill in PDF forms: text fields (single and multi-line, passwords, length limits), check boxes, radio buttons and drop-downs. Ctrl+S saves the values into the PDF, each with a matching appearance, so other viewers and printers show the filled-in form. Read-only fields and script buttons are left alone.
- **Signatures:** draw your signature once and Lumen keeps it (in `~/.local/share/lumen/signatures.toml`, shared with the GTK Lumen). Sign from the toolbar, or click a signature field, and the signature is placed as ink you can move and resize; it is saved like other markup. This is a handwritten signature, as on paper, not a cryptographic (certificate) one.
- **Saving pictures:** Ctrl+S saves an edited or marked-up JPEG, PNG or WebP over its own file (atomically, keeping its permissions); Ctrl+Shift+S, or any other format (GIF, animations, JPEG XL, …), saves a copy (PNG, JPEG, WebP, TIFF or BMP). A JPEG that was only turned or mirrored is saved **losslessly**: its pixel data is kept byte for byte and only the EXIF orientation tag changes (files without EXIF get a small block; files whose EXIF has no orientation tag are re-encoded instead). Printing prints what you see, unsaved markup included. Markup you have not saved yet is kept while you look at other files, and quitting asks first.
- **Picture editing:** press E (or the sliders button) to turn, mirror, crop (free, the picture's own shape, square, 4:3, 3:2, 16:9) and resize (in pixels or by percentage, keeping the proportions if you like), and to change exposure, contrast and saturation, with undo and redo. Editing is non-destructive: the picture as opened is kept, the result is worked out in the background, and markup follows every turn, mirror, crop and resize. Save a copy (Ctrl+S) as PNG, JPEG, WebP, TIFF or BMP; unsaved edits are kept while you look at other files.
- **Metadata is kept:** copies saved as JPEG, PNG or WebP keep the original's EXIF data (camera, lens, exposure, date, GPS location) and its colour profile. The orientation is reset (the pixels are upright already), the size is updated, and the old embedded preview and maker notes are left out. TIFF keeps the colour profile only; BMP keeps neither.
- **Page organiser:** press G (or the grid button) on a PDF to see every page as a thumbnail. Drag pages to reorder them (Ctrl/Shift+click picks several), turn them, delete them (a toast offers **Undo** for eight seconds), or insert the pages of other PDFs, with undo. Saving rewrites the PDF with the new order; outline and links follow their pages, unsaved markup and form values are written too, and a password-protected PDF stays protected.
- **Folders:** open a folder, or open one file and flip through its neighbours. You can also drop files and folders onto the window. A sidebar on the left shows the files as big thumbnails with their names underneath, loaded lazily (only the rows on screen are built, so huge folders stay fast); camera JPEGs use their embedded EXIF thumbnail, other JPEGs are decoded at a fraction of their size, and PNGs are shrunk while they are read, so big folders stay light on memory.
- **Printing:** uses the desktop's native print dialog through the XDG print portal. PDFs are sent as they are. Pictures are fitted to the paper and margins you choose and sent as a one-page PDF at up to 300 dpi.
- **A tidy header:** as in the GTK Lumen, the header has the sidebar and open buttons, the file's name, the markup and edit buttons, and a ☰ menu with everything else (print, fit, turn, fullscreen, sign, save, keyboard shortcuts, about), each with its shortcut.
- **A calm interface:** one surface for the header, sidebar and canvas; tools float as islands over the view (the markup island shows colours and sizes only while you draw); Lumen's amber marks only what is active. Set in Inter, bundled.
- **Light and dark mode:** follows the system colour scheme and switches live when you change it (`LUMEN_THEME=light` or `dark` overrides it).
- **Zoom and pan:** best fit, fit width (PDF), actual size, Ctrl + scroll wheel (around the pointer), pinch on a touchpad (Wayland; see `vendor/README.md`), and drag to pan. Zoom changes ease in. Moving the pointer over the picture fades in a zoom control at the bottom and previous/next arrows at the sides, as in the GTK Lumen; they fade out again when the pointer rests.
- **Fast flipping:** the next and previous pictures are preloaded, and recently viewed ones are kept decoded, within a memory budget (neighbours over 4 megapixels are not decoded ahead, and decoding runs on a small fixed set of threads).
- **Robust:** decoding runs under allocation limits, and a panic while decoding or rendering a hostile file becomes an error message instead of a crash.

## Keyboard

| Keys | Action |
|---|---|
| Ctrl+O / Ctrl+Shift+O | Open files / open a folder |
| Ctrl+P | Print |
| M | Markup on or off |
| V H P R O L A T | With the markup bar open: select, highlight, pen, rectangle, ellipse, line, arrow, text |
| Alt+Enter | Properties |
| Ctrl+Shift+P | Command palette: type a few words, Up / Down to choose, Enter to run, Esc to close |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo markup |
| Delete, arrow keys | Delete / nudge the selected annotation (Shift moves further) |
| Ctrl+S / Ctrl+Shift+S | Save markup and form values into the PDF, or an edited JPEG/PNG/WebP over its file / save a copy |
| Ctrl+C / Ctrl+A | Copy the selected text / select all text (PDF); Ctrl+C on a picture copies the picture |
| Ctrl+F | Find in the PDF (Enter / Shift+Enter or F3 / Shift+F3 for next / previous, Esc to close) |
| Ctrl+G | Go to page (PDF) |
| F5 / Space / Esc | Start or stop the slideshow / pause it / leave it |
| Ctrl+Shift+N | Night mode (PDF) |
| ← → (Space / Backspace for pictures) | Next / previous file |
| Home / End | First / last file (first / last page in a PDF) |
| Page Up / Page Down | Previous / next page (PDF) |
| Space / Shift+Space | Scroll a screen down / up (PDF) |
| Alt+← / Alt+→ | Back / forward after following a link or a contents entry (PDF) |
| ↑ ↓ | Scroll |
| + − 0 1 | Zoom in, zoom out, best fit, actual size |
| W | Fit width (PDF) |
| R / Shift+R | Turn right / left (pictures; selected pages in the organiser) |
| E | Edit the picture (Enter applies a crop, Esc cancels it) |
| G | Organise the PDF's pages (Del deletes, Ctrl+← / → moves, Ctrl+A selects all) |
| T | Show or hide the sidebar (files, the PDF's contents and pages) |
| F / F11, Esc | Fullscreen, leave fullscreen |
| Ctrl+Q / Ctrl+W | Quit |

## Accessibility

Lumen does not work with screen readers yet. iced 0.14, the toolkit it is built on, does not expose an accessibility tree (AT-SPI), so assistive technology sees one unlabelled window; this is worth re-checking whenever iced gains support. What Lumen does today: every icon button has a tooltip naming it (with its shortcut), nearly everything has a keyboard shortcut (see the table above and ☰ → Keyboard Shortcuts), the interface follows the system light or dark setting, and text is set in Inter at sizes that scale with the window's scale factor. Markup drawing and cropping need a pointer. Form check boxes and radio buttons sit on the page and take their meaning from the text around them, so they have no tooltip.

## Files Lumen reads and writes

| Path | Contents |
|---|---|
| `~/.config/lumen-rs/settings.toml` (or under `$XDG_CONFIG_HOME`) | Preferences, written when you change them in the app |
| `~/.cache/thumbnails/large/` (or under `$XDG_CACHE_HOME`) | The desktop's thumbnail cache, shared with the file manager: Lumen shows a thumbnail another program made (if the file has not changed since) and adds the ones it makes (256 px PNGs named after the MD5 of the file's address, private to you). Turn it off in Preferences. PDFs are not cached, since drawing a page is as fast as reading one |
| `~/.local/state/lumen-rs/state.toml` (or under `$XDG_STATE_HOME`) | The window's size, the last 30 files opened and the page each PDF was left at. Written a moment after you open a file and when Lumen quits; "Clear" in the recent list empties it. Safe to delete |
| `~/.local/share/lumen/signatures.toml` (or under `$XDG_DATA_HOME`) | Your saved signatures (up to eight), as strokes; shared with the GTK Lumen |
| `~/.local/share/Trash/` (or `.Trash-<uid>` at the top of another disk) | Files moved to the trash, in the freedesktop format |

Lumen only overwrites a file you opened when you choose Save. Edits, markup and page changes
are kept in memory until then (and while you look at other files), and quitting asks first.

## Environment variables

| Variable | Effect |
|---|---|
| `LUMEN_THEME=light\|dark` | Overrides the theme (and the preference) |
| `LUMEN_NO_PORTAL=1` | Uses Lumen's own file chooser and print dialog even when a desktop portal is there |
| `LUMEN_NO_SANDBOX=1` | Decodes pictures in the main process instead of the sandboxed worker |
| `LUMEN_CONTROL=1` | Offers the D-Bus control interface (see [DEVELOPMENT.md](DEVELOPMENT.md)) |
| `LC_ALL`, `LC_MESSAGES`, `LANGUAGE`, `LANG` | The language (English or Spanish) |
| `RUST_LOG=info` | Timing logs: `window built`, `opened load_ms`, `first view on screen` |
