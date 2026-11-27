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

- **Pictures:** PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA, the PNM family, JPEG XL, JPEG 2000, OpenEXR (linear light, clamped and given the sRGB curve) and SVG, all decoded in pure Rust. EXIF orientation is honoured, and so is a colour profile (Adobe RGB, Display P3…): the colours are turned into sRGB, so wide-gamut photos look as they should (not yet for JPEG XL, JPEG 2000, HEIC and AVIF).
- **AVIF and HEIC (optional):** `cargo build --release --features heif` adds them through the system's libheif (needs `libheif` and its AVIF/HEIC codecs installed). That is the only C code Lumen can reach, so it is off by default: the default build is pure Rust. Add `image/heic;image/heif;image/avif;` to the `.desktop` file's `MimeType` when you build it that way.
- **Animation:** animated GIF, APNG and WebP play, each frame for its own delay (a delay of 10 ms or less shows for 100 ms, as browsers do). Frames are decoded in the sandboxed worker, a few ahead, and the first pass is kept while it fits in 96 MB; longer animations keep streaming from the file. Frames over about 5 megapixels are not played: the first one is shown, and a notice says so. The first frame is what editing and markup work on, so the animation pauses (and shows it) while they are open.
- **PDFs:** continuous scrolling, a table of contents, search, and clickable links. Only the pages near the view are rendered, and that happens on a pool of background threads that does the newest request first. Pages re-render sharply at every zoom level and on HiDPI screens.
- **Recent files and memory:** the empty window lists the last five files you opened (click one to open it; **Clear** forgets them all; files that no longer exist or cannot be opened drop out). A PDF opens at the page you left it at, and the window comes back at the size it had when you closed it (on desktops that let applications choose; a tiling compositor decides for itself).
- **Reloading:** while a picture or PDF is open Lumen waits for the system (inotify) to say another program wrote to it, replaced it or touched it; nothing is polled, so an idle window costs no CPU (where inotify is not available it looks every 3 seconds instead). It then compares the file's size and modification time with how it was when it was opened. If it changed and you have nothing unsaved, it is loaded again (a PDF returns to the place you were reading, its thumbnail is redrawn) with a short notice; with unsaved changes it only says so, and keeps them. Lumen's own saves do not count as changes.
- **Search:** Ctrl+F finds text across the whole document (case-insensitive unless you turn on **Aa**; **[ab]** matches whole words only), highlights every match and jumps between them. It starts from the page you are on, results appear while it scans, and each page's text is read only once. The 477-page Ghostscript manual is fully indexed in about 0.7 s.
- **Reading order:** pages set in columns of running text (two, three or more, also on turned pages) are read column by column for copying and search, even when the file draws them line by line across. Everything else (tables, code, forms, diagrams) keeps the order the file has, which is usually the right one.
- **Selecting a part of a picture:** drag over it. The editing tools choose how: a rectangle (drag the handles or the inside to adjust it), freehand, or by colour (click a colour, and drag away to take in more of it, as for cutting out a background). Ctrl+C copies what is selected (the rest of its rectangle transparent), Ctrl+K crops to it, Ctrl+Shift+I inverts it, Delete makes it transparent (undo brings it back; a JPEG is saved with white there), Ctrl+A selects everything, Esc clears it. Preferences can make dragging move the picture instead (the wheel and the middle button always do).
- **Copying a picture:** Ctrl+C (or ☰ → Copy Picture) puts the picture on the clipboard as a PNG, with its edits (crop, colour) but without unsaved markup. It uses the Wayland data-control protocol through the pure-Rust `wl-clipboard-rs` crate, so it works on compositors that offer it (niri, Sway, Hyprland, KDE) and says so when it cannot (GNOME, X11 sessions).
- **An area of a page:** Select an Area (right-click on a page) makes a drag draw a rectangle on the page instead of moving the view; Copy puts that part on the clipboard as a picture, at 300 dpi (at most 4096 pixels across). Esc clears it.
- **Text selection:** drag over text to select it (Shift+click extends, double-click picks a word), Ctrl+C copies, Ctrl+A selects the whole document. Like other Linux apps, the selection also goes to the primary selection for middle-click paste. Dragging anywhere that is not text still pans.
- **Pages sidebar:** a third tab, next to Files and Contents, shows a thumbnail of every page (drawn only for the rows on screen); click one to go there, and the page you are on is ringed and followed as you scroll.
- **Go to page:** Ctrl+G (or a click on "Page x of y" in the header) asks for a page number, or for its label in a PDF whose pages have their own names ("iv", "A-3"); the header shows them too ("Page iv (6 of 300)").
- **Night mode:** Ctrl+Shift+N (or ☰ → Night Mode) turns paper dark and text light while pictures in the page keep their colours (the GTK Lumen's colour matrix); page thumbnails follow. It is remembered for the session.
- **Export:** ☰ → Export Pages… (or the export button in the page organiser) writes the selected pages, or the page in view, as PNG images at 72, 150 or 300 dpi (several pages go into a folder as `name-01.png`, …; the file records its resolution; turned pages come out turned) or as a new PDF (unsaved markup and form values included; a protected PDF stays protected). ☰ → Export as PDF… turns a picture into a PDF as big as the picture, without margins. ☰ → Combine Files… opens the page organiser and asks for the PDFs to add.
- **File actions:** ☰ → Show in Folder (the file manager selects it, over D-Bus, or `xdg-open` on the folder), Open With… (the portal's app chooser) and Move to Trash, and Rename… (F2): it never replaces another file, even one that appears meanwhile, and the file keeps its place in the list and among the recent files (a file with unsaved changes is saved or discarded first). Trashing follows the freedesktop Trash spec (`~/.local/share/Trash`, or `.Trash/<uid>` or `.Trash-<uid>` at the top of another disk, with a `.trashinfo` note, so file managers can restore it too), shows the next file and offers Undo in the toast for eight seconds. A file with unsaved changes is only moved after asking, and Undo brings those back as well. A file that cannot be renamed into a trash is reported, never copied and deleted. There is no keyboard shortcut for the trash, so a stray key in a text box cannot send the file there.
- **Slideshow:** F5 (or ☰ → Slideshow) goes fullscreen and shows each picture for five seconds, PDFs page by page, then the next file; Space pauses and resumes, Esc leaves, and it ends after the last file (the window goes back to how it was). Flipping by hand restarts the count. It will not start with unsaved changes and stops if any appear. The interval becomes a preference with the preferences dialog.
- **Preferences:** ☰ → Preferences… changes, at once and saved to `~/.config/lumen/settings.toml`: the theme (system, light, dark), the zoom control and arrows over pictures, how a PDF is fitted when it opens, PDF night mode, the slideshow time (2 to 20 s), whether ← and → wrap around at the ends of the list, whether to share thumbnails with the desktop's cache, how many megabytes of decoded pictures are kept and up to how many megapixels neighbours are decoded ahead (or none), reduced motion, and whether to use the desktop's file and print dialogs. The file is read leniently: a missing key, an unknown key or a bad value keeps its default and never stops Lumen from starting. `LUMEN_THEME` and `LUMEN_NO_PORTAL` in the environment win over the file.
- **Sandboxed decoding:** raster pictures (everything but SVG, PDF and, in the optional build, AVIF/HEIC) are decoded by `lumen --decode-worker`, a fresh process per picture that gets the open file as its standard input, shuts itself in (Landlock: no file system access at all; seccomp: about forty allowed system calls, anything else kills it, `read` only from the picture, `write` only to its two output descriptors, threads but no processes; and at most 8 GB of address space) and only then reads and decodes. The pixels come back over a pipe. A decoder that crashes, is stopped by the sandbox or hangs for two minutes costs one picture and an error message. If the sandbox cannot start Lumen decodes in-process and logs it once; a worker that was shut in and then fails in any way never falls back, so a hostile file never runs unsandboxed. A worker starts and exits in about 2 ms, a small picture costs about 4 ms in all, and the pixels of a very large picture (44 MB for 11 megapixels) take another 16 to 20 ms to come back over the pipe; `tests/sandbox.rs` runs every format through the real binary and checks that opening files, sockets, running programs and writing files are all stopped. Turn it off in Preferences or with `LUMEN_NO_SANDBOX=1`. **On macOS and Windows** the decoder is a separate process too, but not shut in: a picture that crashes or hangs it costs only that picture, but one that takes it over could reach your files.
- **Control interface (off by default):** with `LUMEN_CONTROL=1` in its environment, Lumen offers `io.github.imanolgo.Lumen.Control1` on the session bus (`/io/github/imanolgo/Lumen/Control`) for scripts and tests: `GetState` (JSON: what is open, the file's place among its neighbours, page, zoom, search results, whether it is busy), `Open(paths, password)`, `Next`, `Previous`, `First`, `Last`, `GotoPage(page)`, `Zoom(in|out|fit|width|actual)`, `Search(text)` and `Quit`, and for scripts that fill in forms (`GetForm`, `SetField`), annotate (`SetMarkup`, `Annotate`, `GetAnnotations`), edit pictures (`Edit`), save (`Save`, `SaveCopy`) and export (`Export`); [DEVELOPMENT.md](DEVELOPMENT.md) lists them all. It is off by default because anything on the session bus could otherwise open files in your viewer. `LUMEN_CONTROL=stdio` offers the same calls as lines of JSON on standard input and output, on every system, and only to the process that started Lumen. `tests/control.rs` drives the real app through it (`LUMEN_CONTROL_TESTS=1 cargo test --test control`; it opens real windows, so it is not part of `make test`).
- **Spanish:** Lumen speaks Spanish when `LANGUAGE`, or else the first of `LC_ALL`, `LC_MESSAGES` and `LANG` that is set, says so (like gettext). English is the source language: every UI string goes through `tr()` / `trf!()` (`src/i18n.rs`) and is looked up in `po/<language>.po`, which is embedded in the binary, so there is no `msgfmt` step and nothing to install. About a fifth of `po/es.po` is reused from the GTK Lumen; a test fails if any string in the source lacks a Spanish entry, so new text cannot be forgotten. To add a language, add a `.po` file and one line to `CATALOGUES`.
- **Properties:** ☰ → Properties… shows the file (name, folder, size, dates in UTC) and, for a PDF, its title, author, producer, version, page count and size, protection and form fields; for a picture its format, size, colour profile and EXIF summary (camera, lens, exposure, date, location).
- **Table of contents:** the PDF's outline appears in the sidebar, follows you as you scroll, and jumps to the exact spot of each section. Entries with others under them can be folded.
- **Links:** links inside the document jump to their target. Web and mail links open in your browser or mail app through the desktop portal; other kinds of link (files, programs) are ignored for safety.
- **Markup:** press M (or the pen button) to draw on a picture or a PDF page: highlighter, pen, sketch (what you draw by hand becomes the line, arrow, ellipse or rectangle it looks like), rectangles, ellipses, lines, arrows and text boxes, and from the Shapes menu rounded rectangles, stars and polygons (3 to 12 points or sides), speech bubbles and double arrows; any outline or line can be solid or dashed. Eight colours, any line width. Select, move, resize, recolour and delete what you drew, with undo and redo. On a PDF the highlighter marks the text you drag over.
- **Real PDF annotations:** markup is saved into the PDF as standard annotations (highlight, ink, square, circle, polygon, line, free text; dashed borders and two-headed lines too), each with its own appearance, so every PDF viewer shows it. Saving appends an incremental update: the original bytes are kept as they were. Annotations of those kinds that are already in a PDF, from Lumen or another app, can be edited too; comments attached to them are kept.
- **Password-protected PDFs:** Lumen asks for the password and opens the file; the password stays in memory for the session only, so flipping back does not ask again. Markup and form values can be saved into it and it stays protected (what is added is encrypted the same way). Printing sends an unlocked copy, since the print system could not open the file otherwise. In the file list a protected PDF shows a lock until it is unlocked, then its real thumbnail.
- **Forms:** fill in PDF forms: text fields (single and multi-line, passwords, length limits), check boxes, radio buttons and drop-downs. Ctrl+S saves the values into the PDF, each with a matching appearance, so other viewers and printers show the filled-in form. Read-only fields and script buttons are left alone.
- **Signatures:** draw your signature once and Lumen keeps it (in `~/.local/share/lumen/signatures.toml`, shared with the GTK Lumen). Sign from the toolbar, or click a signature field, and the signature is placed as ink you can move and resize; it is saved like other markup. This is a handwritten signature, as on paper, not a cryptographic (certificate) one.
- **Saving pictures:** Ctrl+S saves an edited or marked-up JPEG, PNG or WebP over its own file (atomically, keeping its permissions); Ctrl+Shift+S, or any other format (GIF, animations, JPEG XL, …), saves a copy (PNG, JPEG, WebP, TIFF or BMP). A JPEG that was only turned or mirrored is saved **losslessly**: its pixel data is kept byte for byte and only the EXIF orientation tag changes (files without EXIF get a small block, and files whose EXIF has no orientation tag get the tag). Printing prints what you see, unsaved markup included. If another program changed the file since it was opened, saving asks first: overwrite it, or save a copy. Markup you have not saved yet is kept while you look at other files, and quitting asks first.
- **Picture editing:** press E (or the sliders button) to turn, mirror, crop (free, the picture's own shape, square, 4:3, 3:2, 16:9) and resize (in pixels or by percentage, keeping the proportions if you like), straighten (a slider up to 45°, seen at once, keeping the largest upright part), and to change exposure, contrast and saturation, with undo and redo. Editing is non-destructive: the picture as opened is kept, the result is worked out in the background, and markup follows every turn, mirror, crop and resize. Save a copy (Ctrl+S) as PNG, JPEG, WebP, TIFF or BMP; unsaved edits are kept while you look at other files.
- **Metadata is kept:** copies saved as JPEG, PNG or WebP keep the original's EXIF data (camera, lens, exposure, date, GPS location) and its colour profile. The orientation is reset (the pixels are upright already), the size is updated, and the old embedded preview and maker notes are left out. TIFF keeps the colour profile only; BMP keeps neither.
- **Page organiser:** press G (or the grid button) on a PDF to see every page as a thumbnail. Drag pages to reorder them (Ctrl/Shift+click picks several), turn them, delete them (a toast offers **Undo** for eight seconds), or insert the pages of other PDFs, with undo. Saving rewrites the PDF with the new order; outline and links follow their pages, unsaved markup and form values are written too, and a password-protected PDF stays protected.
- **Folders:** open a folder, or several files, to browse them as a list. One picture opens by itself and the arrows flip through the pictures around it; one PDF opens by itself with its contents or pages beside it. T (Show Folder) shows the folder's list, and then all of it is browsed. You can also drop files and folders onto the window. A sidebar on the left shows the files as big thumbnails with their names underneath, loaded lazily (only the rows on screen are built, so huge folders stay fast); camera JPEGs use their embedded EXIF thumbnail, other JPEGs are decoded at a fraction of their size, and PNGs are shrunk while they are read, so big folders stay light on memory. The top of the list sorts the files by name (numbers in order), date modified, size or kind, or the other way round; the order is remembered. Drag the sidebar's edge to make it wider or narrower (160 to 360 pixels, remembered).
- **Printing:** uses the desktop's native print dialog through the XDG print portal. PDFs are sent as they are. Pictures are fitted to the paper and margins you choose and sent as a one-page PDF at up to 300 dpi.
- **A window that fits:** a window that opens on one picture takes the size the picture needs (within 80 % of the screen), once; PDFs, folders and the empty window have the size you left them at.
- **Windows:** on Linux and Windows every file opened from the file manager has a window of its own, and Open in New Window (or Ctrl+click in the sidebar or the recent files) opens another; each is a process of its own. On macOS all windows are one program, a file that is open already is brought forward, and quitting asks about the unsaved changes of every window first.
- **A tidy header:** as in the GTK Lumen, the header has the sidebar and open buttons and the file's name; in the middle the zoom, rotate and crop (pictures), Markup, and Edit or Organise; and at the end Find and a short ☰ menu. Save, Discard and Undo show there when something has changed. The menu is short (open, save, print, and View, Export and File leading to more), each command with its shortcut. The right button opens a menu on a picture, a page, selected text, an annotation or a file in the sidebar; the command palette (Ctrl+Shift+P) finds everything.
- **A calm interface:** one surface for the header, sidebar and canvas; the tools of what you are doing are in one row under the header, with their names where there is room, and their details open under their buttons; Lumen's amber marks only what is active. Set in Inter, bundled.
- **Light and dark mode:** follows the system colour scheme and switches live when you change it (`LUMEN_THEME=light` or `dark` overrides it).
- **Zoom and pan:** best fit, fit width (PDF), actual size, Ctrl + scroll wheel (around the pointer), pinch on a touchpad (Wayland; see `vendor/README.md`), and drag to pan. Zoom changes ease in. Moving the pointer over the picture fades in a zoom control at the bottom and previous/next arrows at the sides, as in the GTK Lumen; they fade out again when the pointer rests. The percentage opens a menu of zooms. While a file loads, the one before it stays in view.
- **Fast flipping:** the next and previous pictures are preloaded, and recently viewed ones are kept decoded, within a memory budget (neighbours over 4 megapixels are not decoded ahead, and decoding runs on a small fixed set of threads).
- **Robust:** decoding runs under allocation limits, and a panic while decoding or rendering a hostile file becomes an error message instead of a crash.

## Keyboard

Single letters of the tools work while the markup bar is open. On a Mac, ⌘ stands for Ctrl.

<!-- keyboard -->
| | Keys | Action |
|---|---|---|
| File | Ctrl+O | Open Files… |
|  | Ctrl+Shift+O | Open Folder… |
|  | Ctrl+S | Save |
|  | Ctrl+Shift+S | Save a Copy… |
|  | Ctrl+P | Print… |
|  | F2 | Rename… |
|  | Alt+Enter | Properties… |
|  | Ctrl+, | Preferences… |
|  | Ctrl+Q | Quit |
|  | Ctrl+W | Close Window |
| Edit | Ctrl+Z | Undo |
|  | Ctrl+Shift+Z, Ctrl+Y | Redo |
|  | Ctrl+C | Copy |
|  | Ctrl+A | Select All |
|  | Ctrl+Shift+I | Invert Selection |
|  | Ctrl+K | Crop to Selection |
|  | Delete, Backspace | Delete Selection |
|  | ← → ↑ ↓ | Move the selected annotation (Shift: further) |
|  | Enter | Apply the crop |
|  | Esc | Cancel, close, and finally discard what is not saved |
| View | +, Ctrl++ | Zoom In |
|  | -, Ctrl+- | Zoom Out |
|  | 0, Ctrl+0 | Best Fit |
|  | W | Fit Width |
|  | 1, Ctrl+1 | Actual Size |
|  | F, F11 | Fullscreen |
|  | F5 | Slideshow |
|  | T | Sidebar |
|  | Ctrl+Shift+N | Night Mode |
|  | Ctrl+scroll, pinch | Zoom in / out |
|  | Space, Esc | Pause / leave the slideshow |
|  | Esc | Leave fullscreen |
| Go | Ctrl+F | Find… |
|  | F3 | Next Match |
|  | Shift+F3 | Previous Match |
|  | Ctrl+G | Go to Page… |
|  | Alt+← | Back |
|  | Alt+→ | Forward |
|  | Page Down | Next Page |
|  | Page Up | Previous Page |
|  | Home | First Page |
|  | End | Last Page |
|  | Space | Scroll Down a Screen |
|  | Shift+Space | Scroll Up a Screen |
|  | →, Space, Page Down | Next File |
|  | ←, Backspace, Page Up | Previous File |
|  | Home | First File |
|  | End | Last File |
|  | ↑ ↓ | Scroll |
|  | Enter, Shift+Enter | Next / previous match, in the Find box |
| Tools | V | Select |
|  | H | Highlighter |
|  | P | Pen |
|  | S | Sketch |
|  | R | Rectangle |
|  | O | Ellipse |
|  | L | Line |
|  | A | Arrow |
|  | T | Text |
|  | M | Markup |
|  | E | Edit Picture |
|  | G | Organise Pages |
|  | R | Rotate Right |
|  | Shift+R | Rotate Left |
|  | Ctrl+←, Ctrl+↑ | Move Pages Earlier |
|  | Ctrl+→, Ctrl+↓ | Move Pages Later |
|  | Shift+click | Extend the selection |
| Help | Ctrl+Shift+P | Command Palette |
<!-- /keyboard -->

## Accessibility

Lumen does not work with screen readers yet. iced 0.14, the toolkit it is built on, does not expose an accessibility tree (AT-SPI), so assistive technology sees one unlabelled window; this is worth re-checking whenever iced gains support. What Lumen does today: every icon button has a tooltip naming it (with its shortcut), nearly everything has a keyboard shortcut (see the table above and ☰ → Keyboard Shortcuts), the interface follows the system light or dark setting, and text is set in Inter at sizes that scale with the window's scale factor. Markup drawing and cropping need a pointer. Form check boxes and radio buttons sit on the page and take their meaning from the text around them, so they have no tooltip.

## Files Lumen reads and writes

| Path | Contents |
|---|---|
| `~/.config/lumen/settings.toml` (or under `$XDG_CONFIG_HOME`) | Preferences, written when you change them in the app |
| `~/.cache/thumbnails/large/` (or under `$XDG_CACHE_HOME`) | The desktop's thumbnail cache, shared with the file manager: Lumen shows a thumbnail another program made (if the file has not changed since) and adds the ones it makes (256 px PNGs named after the MD5 of the file's address, private to you). Turn it off in Preferences. PDFs are not cached, since drawing a page is as fast as reading one |
| `~/.local/state/lumen/state.toml` (or under `$XDG_STATE_HOME`) | The window's size, the last 30 files opened, and the page, zoom and place each was left at. Written a moment after you open a file and when Lumen quits; "Clear" in the recent list empties it. Safe to delete |
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
| `WGPU_POWER_PREF=high` / `LUMEN_NO_REEXEC=1` | Lumen starts itself once more with `WGPU_POWER_PREF=low` so the integrated GPU is used (faster start, less memory); either variable keeps the default (discrete GPU) |
| `LUMEN_CONTROL=1` or `stdio` | Offers the control interface over D-Bus (Linux), or over standard input and output (see [DEVELOPMENT.md](DEVELOPMENT.md)) |
| `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG` | The language (English or Spanish) |
| `RUST_LOG=info` | Timing logs: `window built`, `opened load_ms`, `first view on screen` |
