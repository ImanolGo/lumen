# Lumen — Review and Improvement Plan (towards 0.4)

A full review of Lumen 0.3.1, written so a coding agent can work through it one item at a time.
It sits next to [PLAN.md](PLAN.md), which covers how Lumen was built. This file covers what
should change next.

## How to use this file

- Work **phase by phase, top to bottom**. Phase A (correctness) comes first because some of the
  later work touches the same code.
- Each item has an **ID**, an effort marker (🟢 small · 🟡 medium · 🟠 large) and a
  **confidence** marker:
  - ✅ **Confirmed**: the defect is plain from reading the code (file and line given).
  - 🔍 **Verify first**: likely, but reproduce it before changing anything. If it does not
    reproduce, tick the box, write "not reproducible" under it and move on.
- Each item has **Problem → Do → Done when**. "Done when" is the acceptance test.
- **One item per commit**, with the ID in the message, for example `fix(save): keep the file list after saving (A1)`.
- Before every commit run `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`.
  `tests/control.rs` drives the real app over D-Bus on Broadway. Prefer adding an end-to-end
  test there for anything a user can see.
- Changed or new user-visible strings: run `build-aux/update-pot.sh`, and update `po/es.po`.
- Add a line to `CHANGELOG.md` under `[Unreleased]` for every user-visible change.
- Tick the box (`- [x]`) in the same commit that finishes the item.
- Keep the rules in PLAN.md: logic out of widgets, nothing blocking on the main thread, pure
  modules with unit tests.

---

## 0. Review summary

**State of the project (2026-10-20, `0b52d76`).** About 20k lines of Rust. `cargo clippy` is
clean, and 118 unit tests plus 7 end-to-end tests pass. The architecture is sound: pure models
(`annotate`, `edit`, `pageops`), background workers, and the working-copy undo design for PDFs.
Packaging (Flatpak, AUR, `.deb`) and CI are in place.

**Main risks found:**

1. **Data and file-safety bugs in saving.** Saving collapses the browsed file list. Saving
   replaces files without keeping permissions or symlinks, drops ICC colour profiles, and turns
   lossy WebP into lossless WebP.
2. **The security claim in the README is not fully true.** Viewing goes through glycin's
   sandbox, but editing, saving, "Export as PDF" and "insert picture as page" decode with the
   `image` crate inside the Lumen process.
3. **Speed bottlenecks that users will notice:**
   - Every PDF opens 5 Poppler instances, all serialised by one global lock.
   - Filling a form field or undoing reloads the whole document, and the pages flash white.
   - The colour-adjust sliders re-render the full-resolution image on every tick, in parallel
     and without cancellation.
   - Thumbnails decode the full picture and ignore the desktop's thumbnail cache.
4. **Gaps in everyday use:**
   - Opening one picture does not let you browse its folder.
   - There is no "back" after following a PDF link.
   - Files cannot be moved to the trash, and pictures cannot be copied to the clipboard.
   - PDFs do not reload when the file changes on disk.
   - Space does not scroll PDFs.
5. **Drift.** `app.rs` is 3,300 lines. ARCHITECTURE.md describes modules that do not exist.
   Some strings are not translated.

---

## Design direction: "quiet, instant, keyboard-first"

These are the rules for the features and polish below. When in doubt, choose the smaller option.

1. **Content first, no chrome at rest.** The image or page fills the window. Controls show up
   on intent (pointer movement, a key) and fade away. No new permanent toolbars.
2. **Instant before perfect.** Show *something* right away (a cached thumbnail, the previous
   render, a low-resolution page), then sharpen. Never show blank white where there was content
   a moment ago.
3. **Undo over confirm.** Reversible actions (delete pages, trash a file, clear annotations)
   happen at once and show an `adw::Toast` with **Undo**. Dialogs are only for things that
   cannot be undone (overwriting a file on disk, dropping a password).
4. **Every action has a key, and every key is discoverable.** Tooltips show the shortcut. Markup
   tools get single-letter keys while the toolbar is open. The shortcuts dialog is generated from
   the same table that registers the shortcuts, so it cannot drift.
5. **Motion is short and functional.** Animations last 120–180 ms, only show cause and effect
   (zoom, crossfade between files, sidebar slide), and follow
   `gtk-enable-animations` (the system's reduced-motion setting).
6. **One accent.** The amber and yellow from the icon are used for selection rings, the current
   search hit and the active tool. Nothing else is coloured.

---

## Phase A: Correctness and file safety (do first)

### - [x] A1 ✅ 🟢 Saving a picture collapses the browsed list to one file
- **Where:** `src/app/edit.rs:567`, which sends `AppMsg::Open(vec![dest])` after every
  successful save.
- **Problem:** You browse a folder of 200 photos, rotate one and press `Ctrl+S`. `Open` replaces
  `self.list` with just the saved file, so ←/→ and the file sidebar stop working. Save As does
  the same, and the user ends up on the copy.
- **Do:** Add an `AppMsg::Reload { path }` (or a `reload_current(dest)` helper) that decodes
  `dest` again and replaces `list[index]` in place:
  - For a save in place, the path is the same.
  - For Save As, keep showing the original and insert the copy right after it in `list`, then
    call `refresh_strip`.
  - Invalidate `cache` entries for that path.
  - Keep `control_saved_reopens()` working: the control call should still answer once the reload
    is done.
- **Done when:** a new control test opens a folder of 3 pictures, edits and saves the middle
  one, and `GetState` still reports 3 files with index 1. Save As reports 4 files.

### - [x] A2 ✅ 🟢 A failed preview render undoes a committed edit
- **Where:** `src/app/edit.rs:287–296` (`edit_rendered`, `Err` branch).
- **Problem:** While the Adjust sliders are moving, `refresh_edit` renders a *preview* that is
  not on the stack. If that render fails, the code calls `session.stack.undo()` and removes the
  last *real* edit.
- **Do:** Record whether the ticket was a preview (store `preview: bool` with the ticket in
  `EditSession`). Undo only for a non-preview op that was just pushed. For a preview, leave
  Adjust mode and show the error.
- **Done when:** a unit test on the decision logic, or a manual check that forces an error,
  shows the stack unchanged after a failed preview.

### - [x] A3 ✅ 🟡 Saving over a file loses its permissions, breaks symlinks and hard links
- **Where:** `src/save.rs:94–104`, `src/app/edit.rs:767–788` (`write_pdf`). Both write a temp
  file and `rename` it over the destination.
- **Problem:**
  - A `0600` private scan becomes `0644` (umask default).
  - A symlinked file is replaced by a regular file, and the link target never changes.
  - Hard links split.
  - Extended attributes (tags, SELinux labels) are lost.
- **Do:** Add `fn replace_file(temp, dest)` in a new `src/fsutil.rs`:
  1. Resolve `dest` with `fs::canonicalize` when it is a symlink, and write the temp file next to
     the *target*.
  2. Copy the mode from the existing file (`fs::set_permissions(temp, fs::metadata(dest)?.permissions())`).
  3. If `nlink > 1`, copy the bytes into the existing inode instead of renaming.
  4. `fsync` the temp file before the rename, and the directory after it.

  Use it from `save.rs`, `write_pdf` and `settings::save`.
- **Done when:** unit tests cover a `0600` file staying `0600`, a symlink still being a symlink
  that points at the updated file, and a hard link staying shared.

### - [x] A4 (ICC part) ✅ 🟡 Re-encoding drops the ICC profile, and lossy WebP becomes lossless
- **Where:** `src/save.rs:136–169` (`write_reencoded`), `Format::WebP` ("Lossless WebP",
  `save.rs:31`).
- **Problem:**
  - Phone photos (Display P3) and photos from Adobe RGB cameras visibly change colour after any
    crop or resize.
  - A 300 KB lossy WebP saved in place can grow to several MB.
  - In-place JPEG saves always use quality 90.
- **Do:**
  - Read the ICC profile from the source (`image::ImageDecoder::icc_profile`) and write it back
    with `JpegEncoder::set_icc_profile`. Use the PNG and WebP encoders' equivalents where the
    `image` crate supports them. If it does not, note it in the CHANGELOG.
  - Detect whether the source WebP is lossy (VP8 rather than VP8L chunk). For a lossy source,
    either save as JPEG-quality lossy WebP (if the encoder supports it) or ask once, the same way
    the JPEG quality dialog does.
  - Estimate the source JPEG's quality from its quantisation tables (or simply keep 90), and add
    a **JPEG quality** preference (default 90) instead of the hard-coded constant.
- **Done when:** a test re-encodes a JPEG fixture with an embedded ICC profile and the output
  still has the same profile bytes.
- **Status:** the ICC profile is now carried over for JPEG, PNG and WebP (test in `save.rs`). **Still open:** the `image` crate cannot write lossy WebP, so lossy sources are still saved lossless (needs another encoder, e.g. the `webp` crate); the JPEG-quality preference moves to C11.

### - [x] A5 ✅ 🟠 Editing decodes outside the sandbox (README claim is not true)
- **Where:** `image::open` in `src/app/edit.rs:249`, `src/save.rs:138`,
  `src/app/pages.rs:520`, `src/pdf/compose.rs:152`. `can_edit` (`save.rs:36`) also limits
  editing to formats the `image` crate reads.
- **Problem:** The README says "a malicious file cannot reach your system". But pressing
  `Ctrl+R` and then `Ctrl+S` on a crafted TIFF or BMP runs the `image` crate's decoder inside
  Lumen. Editing HEIC, AVIF or JXL is impossible for the same reason.
- **Do:** Add `decode::pixels(path) -> Result<DynamicImage>` in `src/image/decode.rs`:
  - Load through glycin with `apply_transformations(false)`.
  - Download the first frame's texture with `gdk::TextureDownloader`. Use `R8g8b8a8`, or
    `R16g16b16a16` when the texture format has more than 8 bits, so 16-bit PNGs keep their
    depth.
  - Wrap the result in `DynamicImage`.
  - Fall back to `image::open` only when `sandboxed()` is false.

  Use it in all four places. Then widen `can_edit` to "readable by glycin". Saving still writes
  only JPEG, PNG and WebP; other formats go through Save As, as GIF, BMP and TIFF do today.
- **Done when:** `grep -rn "image::open" src` only shows the fallback and tests. A HEIC photo
  can be rotated and saved as a copy. The README security paragraph is accurate.

### - [x] A6 ✅ 🟢 Every picture you flip past goes into the desktop-wide "Recent" list
- **Where:** `src/app.rs:1033–1034` (`apply`: `RecentManager::add_item` and
  `refresh_recent_menu` on every file shown).
- **Problem:**
  - Holding → through a folder writes hundreds of entries into
    `~/.local/share/recently-used.xbel`, which is a privacy leak into other apps' recent lists
    and pushes out the user's real documents.
  - `refresh_recent_menu` calls `item.exists()` (a `stat`) on every recent item, on the main
    thread, on every navigation.
- **Do:** Add only files the user opened on purpose (`Open`, `Add`, recents, the command line),
  not those reached by `Navigate`, `GoTo` or a slideshow. Refresh the menu lazily when the
  split button's popover opens (`gtk::MenuButton::set_create_popup_func` or the `notify::active`
  equivalent), and drop the `exists()` check or move it off the main thread.
- **Done when:** browsing 20 files adds at most one recent entry (the one opened).

### - [x] A7 ✅ 🟢 State file written on every file shown
- **Where:** `src/app.rs:1081` (`self.store.borrow().save_state()` in `apply`).
- **Problem:** A synchronous TOML write and rename on the main thread for every picture while
  browsing.
- **Do:** Mark the state dirty and save it from a 2 s debounced `glib::timeout`, and always on
  close. The last-page map is already updated in memory.
- **Done when:** no `state.toml` write happens while holding → over 100 files (check with
  `strace -e rename`), and the last page is still remembered after closing.

### - [x] A8 (fixed defensively, not reproduced headless) 🔍 🟢 The page-number entry is overwritten while typing, and `Ctrl+G` may focus a hidden entry
- **Where:**
  - `src/app.rs:1793`: `#[watch] set_text: &model.page_text()` re-applies on *every* message
    (page renders, thumbnails, `ControlsIdle`).
  - `src/app.rs:2228–2234`: `win.goto-page` focuses the entry without waking the floating
    controls, which may be hidden inside a `Revealer`.
- **Reproduce:**
  - Open a long PDF, press `Ctrl+G` after the controls have faded, and see whether the entry
    gets focus.
  - Click the entry, type slowly while pages render, and see whether the text resets.
- **Do:**
  - Use `#[track]`, or update the entry by hand only when `pdf.page` changes *and* the entry
    does not have focus.
  - `goto-page` sends a message that calls `wake_controls` and keeps the controls shown while
    the entry has focus (count focus like `controls_hovered`).
- **Done when:** `Ctrl+G`, type `120`, `Enter` works every time on a 300-page PDF.

### - [x] A9 ✅ 🟢 Keys that do the wrong thing in PDFs
- **Where:** `src/app.rs:2242–2243` (`space` and `BackSpace` are bound to next or previous
  *file*).
- **Problem:** In a PDF, Space should scroll down a screen (every reader does this). Today it
  jumps to the next file or does nothing. Home and End only work in PDFs. In the file list they
  could go to the first and last file.
- **Do:** Map Space and Shift+Space to "scroll one viewport minus 10 %" when a PDF is shown, and
  keep next and previous file for pictures. Map Home and End to the first and last file when a
  picture is shown. Make Shift+Left and Shift+Right nudge annotations by 10, the same as
  Shift+Up and Shift+Down (the shortcuts dialog already implies this).
- **Done when:** the behaviour matches the shortcuts dialog, which is generated as in E3.

### - [x] A10 ✅ 🟢 Small inconsistencies
- `document.rs:110,121–122` and `widgets/properties.rs:54`: "N pages" and "W × H px" are not
  translated. Use `ngettext` (add an `ntr` helper in `i18n.rs`) and `tr_fmt`.
- Errors shown on the error page are raw developer text ("unsupported file type: text/plain",
  "cannot read …"). Map `OpenError::Other` to a short translated sentence and put the details
  in an expandable label.
- `src/app/markup.rs:81–83`: the doc comment for `can_markup` is attached to `markup_color`.
  `SWATCHES` (`markup.rs:11`) is declared above the `use` block.
- `src/document.rs:3`: stale comment ("Phase 2 adds a real `PdfDocument`").
- The insert-pages dialog uses `add_pixbuf_formats()` (`pages.rs:229`), while Open accepts
  `image/*`. Use the same filter (after A5, any glycin format works).
- **Done when:** `rg '"[0-9a-z{} ]+ pages"' src` is empty, and Spanish shows "3 páginas".

---

## Phase B: Speed ("feels instant")

Measure before and after each item using the `info`-level timings that already exist. Record
the numbers in the commit message.

### - [ ] B1 (partly done: workers now open the document lazily, which saves the 3–4 extra parses in a normal reading session; the single prioritised worker is still open) ✅ 🟠 One Poppler worker per document, with a priority queue
- **Where:** `make_pdf_view` (`src/app.rs:970`) starts `PageRenderer` ×2, `SearchWorker`,
  `SelectionWorker` and `LinkWorker`. Each opens and parses the PDF, and every call takes the
  global `poppler_lock` (`src/pdf/mod.rs:113`).
- **Problem:** Each open or reload parses the file five times and keeps five copies in memory,
  with no parallelism because of the lock. A long search page or a thumbnail render can sit in
  front of the page the user is looking at.
- **Do:** Replace them with a single `DocWorker` thread per document that owns one
  `poppler::Document` and serves typed requests from a priority queue:
  1. visible page renders
  2. selection
  3. links
  4. read-ahead renders
  5. thumbnails
  6. search (one page per slot, so it gives way after each page)

  Keep the existing worker APIs as thin façades so `app.rs` barely changes. Keep the
  "replace pending jobs" semantics per request type. File thumbnails (`render_thumbnail`) keep
  their own short-lived instance.
- **Done when:**
  - A 500-page PDF still opens in ≤ 60 ms.
  - Searching a common word does not delay page renders (log render latency during a search).
  - RSS after open drops by about 4× the parsed-document size.

### - [x] B2 (same-layout reloads; page-operation remapping not done) ✅ 🟡 No white flash when a PDF reloads (form fill, undo, page ops)
- **Where:** `src/widgets/canvas.rs:510–547`. When `keep_view` is true, the new `PagesState`
  starts with `textures: HashMap::new()`.
- **Do:**
  - When the layout is identical, carry the old textures over as placeholders. Mark them
    stale (`PageCache` has no scale for them), so they are re-rendered but never blanked.
  - For page operations (layout changed), map old textures through the `pageops` plan, so moved
    pages keep their image.
- **Done when:** filling a checkbox on a 20-page form shows no frame with white pages. Check it
  with a Broadway screenshot sequence or by eye.

### - [x] B3 (investigated, not implemented: a live `text_set_text` on an open Poppler document does change the next render of that page, so in-place filling is possible. It would need every worker's document kept in step, a value-based undo history and a write-on-save step, which is a redesign of the working-copy model for a gain that only shows on very large forms; worth doing only if form filling feels slow in practice.) 🔍 🟠 Fill forms without reloading the whole document
- **Where:** `src/app/forms.rs:78–120`. Every field change writes a full working copy and
  reopens the PDF, and with B1 not yet done, that is 5 parses.
- **Do:** Prototype setting the value on the *live* worker document
  (`poppler::FormField::*_set_*`), re-render only that page, and keep a list of changed values.
  Write one working copy on save, or when a page operation needs a file. Undo becomes
  "set previous value". Keep the current approach only if Poppler's in-memory form updates turn
  out to be unreliable. Write down the finding either way.
- **Done when:** typing into 10 fields of a 100-page form gives no reload (no `PdfReloaded` in
  the log) and the saved file still shows all 10 values in Okular and Firefox.

### - [ ] B4 (re-measured: a page is capped at 32 MP, which is about 800 % on a 1× screen or 400 % on a 2× screen for a Letter page, so sharpness is only lost beyond that; the gain from tiles is mostly speed and memory at high zoom, so this is lower priority than first thought) ✅ 🟠 Sharp, fast PDF pages at high zoom (tiles)
- **Where:** `effective_scale` caps a page at 32 MP (`src/pdf/mod.rs:33–34,197`), so text gets
  blurry above roughly 300–400 % on HiDPI. Every render at that cap redraws the whole page.
- **Do:** Above a threshold (page wider than 2× the viewport), render only the visible region
  plus a margin as a tile (`cr.translate` and a clip before `page.render`). Draw it over a
  low-resolution full-page texture, the same way SVGs already use `set_sharp`. Generalise
  `set_sharp` into per-page tiles in `PagesState`.
- **Done when:** text at 800 % is as sharp as at 100 %, and a re-render at 800 % takes less
  than 100 ms on the dev laptop.

### - [x] B5 (cache read/write for pictures; EXIF-embedded thumbnails not done) ✅ 🟡 Thumbnails: use the desktop thumbnail cache
- **Where:** `src/thumbnails.rs`, `src/image/decode.rs:125` (`thumbnail` decodes the full
  picture and then shrinks it).
- **Problem:** A folder of 50 MP photos decodes 200 MB per thumbnail, even though Nautilus has
  already written `~/.cache/thumbnails/{normal,large}/<md5(uri)>.png`.
- **Do:**
  1. Query `thumbnail::path` and `thumbnail::is-valid` through GIO (`G_FILE_ATTRIBUTE_THUMBNAIL_PATH`)
     and load that PNG.
  2. Otherwise, for JPEGs, use the EXIF embedded thumbnail when it is at least 120 px.
  3. Otherwise decode as today, and **write** the result to `~/.cache/thumbnails/large` following
     the freedesktop spec (`Thumb::URI` and `Thumb::MTime` tEXt chunks).

  The Flatpak needs `--filesystem=xdg-cache/thumbnails` for this; add it to the manifest.
  Also give PDFs a thumbnail without glycin (today `thumbnail()` returns an error without it).
- **Done when:** opening a 500-photo folder the second time fills the visible thumbnails in
  under 200 ms.

### - [x] B6 (coalescing + parallel pixels; screen-sized proxy and base-render cache not done) ✅ 🟡 Colour adjust: preview on a screen-sized proxy, one render in flight
- **Where:** `src/app/edit.rs:217–260,367–382`, `src/edit/adjust.rs:23`.
- **Problem:** Each slider tick clones the full-resolution image, replays the *whole* edit stack
  (including Lanczos resizes) and adjusts on one core. A drag fires dozens of these at once
  through `spawn_blocking`, which can mean several GB of RAM on a 50 MP photo.
- **Do:**
  - Cache the rendered result of the committed stack (`EditSession::base_render`).
  - In Adjust mode, build a proxy once, downscaled to the viewport size × scale factor, and
    preview on that.
  - Allow one render in flight. Further ticks only store the latest value and run when the
    current render finishes (a coalescing "latest wins" slot).
  - Run the per-pixel loop in parallel (`std::thread::scope` over row chunks, so no new
    dependency).
- **Done when:** dragging a slider on a 50 MP JPEG stays above 30 fps and peak RSS stays under
  1.5× the decoded image size.

### - [x] B7 ✅ 🟢 Animated pictures: show the first frame right away
- **Where:** `src/image/decode.rs:68–89` decodes *every* frame before returning.
- **Do:** Return after the first frame with an `AnimationStream` handle. Keep decoding frames on
  the worker and send them over a channel. The canvas starts animating once frame 2 arrives.
- **Done when:** a 300-frame GIF shows its first frame as fast as a still image of the same size.

### - [x] B8 ✅ 🟢 Images inserted as PDF pages: keep the JPEG, do not inflate it
- **Where:** `src/pdf/compose.rs:150–180` converts every picture to an uncompressed ARGB Cairo
  surface. A 12 MP photo becomes a PDF page of roughly 20–40 MB.
- **Do:** For JPEG sources with normal orientation, attach the original bytes with
  `surface.set_mime_data(cairo::MIME_TYPE_JPEG, …)` so Cairo embeds them as-is (DCT). For other
  sources, keep the current path.
- **Done when:** inserting a 4 MB JPEG adds about 4 MB to the PDF (unit test on the file size).

### - [x] B9 (measured: every message through `update` and the view took under 0.5 ms while jumping around a 300-page PDF with the sidebar open, so no change is needed) 🔍 🟡 Cut the per-message view cost
- **Where:** `SimpleComponent::update` → every `#[watch]` and the whole `post_view`
  (`src/app.rs:1857`: about 25 action lookups and the swatch redraw loop) run for *every*
  message, including `PageRendered`, `ThumbRendered`, `Thumbnail` and `LinksLoaded`.
- **Do:**
  1. Measure first: count messages per second while scrolling a PDF fast with the sidebar open,
     and time `update` + `update_view` (`tracing` spans at `debug`).
  2. If the time is more than 1 ms a frame, switch `App` to `Component` with `#[tracker::track]`,
     or route the hot texture messages straight to the widgets they touch (canvas, strips)
     without going through the full view.
  3. In `post_view`, set action states only when they change: cache the last `[bool; N]`.
- **Done when:** a profile of fast scrolling shows less than 5 % of main-thread time in
  `update_view`.

### - [x] B10 (work folder + pruning done; preload direction and history cap not) ✅ 🟢 Smarter preloading and working-copy hygiene
- Preload in the direction of travel: 2 ahead and 1 behind (`PRELOAD_RADIUS`, `app.rs:61`).
- Working copies live in `std::env::temp_dir()` (`pages.rs:58`). That is often tmpfs, which is
  RAM. Move them to `glib::user_cache_dir()/lumen/work-<pid>`.
- Delete files that drop out of the history when a new commit cuts off the redo branch.
- Cap the history at about 100 steps.
- **Done when:** 30 rotations of a 50 MB PDF leave at most (history size × file) on disk, not in
  RAM, and the folder is gone after quitting.

### - [x] B11 🟢 Debounce search typing
- `AppMsg::SearchChanged` (`src/app.rs:3053`) restarts the search on every keystroke. Wait
  150 ms after the last keystroke. Enter searches at once.

---

## Phase C: Missing everyday features (small, high value)

Each one should be a small addition that fits the design rules. Nothing here adds a permanent
toolbar.

### - [x] C1 (preference, on by default) 🟡 Browse the folder of a single opened picture *(maintainer decision)*
- 7.11 chose deliberately to show only the files that were picked. That breaks the most common
  viewer flow: double-click a photo in the file manager, then press →.
- **Do:** Add a preference, **"Browse the folder of a single picture"**, defaulting to on. When
  exactly one *picture* is opened (not a PDF and not a folder), show it first. Then list its
  folder off the main thread and splice the siblings in around it without changing `index`'s
  file. Preloading starts after the first paint.
- **Done when:** `lumen ~/Pictures/a.jpg` shows `a.jpg` with no slower first paint, then → goes
  to the next file in the folder.

### - [x] C2 (Alt+Left/Right and a Back toast; mouse buttons 8/9 not wired) 🟢 Back and forward after following a link
- Clicking an internal link or an outline entry pushes the current position (page and y offset)
  onto a stack. `Alt+Left` / `Alt+Right` (and mouse buttons 8 and 9) go back and forward. A
  small "← Back to page 12" OSD pill appears for 4 s after a jump.

### - [x] C3 🟡 Reload when the file changes on disk
- Watch the open file with `gio::FileMonitor`. When it changes and nothing is unsaved, reload
  and keep page and zoom (this already works for identical layouts). When there are unsaved
  edits, show a toast: "Changed on disk. Reload". This makes Lumen useful for previewing
  LaTeX, Typst and exported images.

### - [x] C4 (bound to Ctrl+Delete and the main menu; plain Delete would clash with annotations and page deletion) 🟢 Move to Trash, with Undo
- `Delete` on a picture (not in markup mode, nothing selected) calls `gio::File::trash`, removes
  it from the list, moves to the next file and shows a toast with **Undo**. Undo restores from
  the trash via `trash:///`, or keeps the path and moves the file back.
- Guard it so it never applies to a dirty document.

### - [x] C5 (copy only; paste not done) 🟢 Copy and paste pictures
- `Ctrl+C` with a picture shown and no text selection puts the *shown* image (with edits and
  markup flattened) on the clipboard as a texture, plus the file as `text/uri-list`.
- `Ctrl+V` with a picture on the clipboard opens it as an unsaved image ("Pasted image"; Save
  asks for a name).

### - [x] C6 (Show in Folder and Open With; drag-out not done) 🟢 Drag out, Show in Folder, Open With
- Add a `gtk::DragSource` on the canvas that drags the current file. Use middle-drag or
  Alt+drag, so it does not clash with panning.
- Add **Show in Folder** and **Open With…** to the main menu (`gtk::FileLauncher`:
  `open_containing_folder`, and `launch` with `always_ask`).

### - [x] C7 (fit width/page and night mode; dual-page layout not done; remembered globally, not per document) 🟡 PDF reading modes
- Fit Page and Fit Width. Today it is always fit-width, capped at 300 % (`canvas.rs:134`),
  which is too big on wide screens. Make it a menu in the zoom pill and remember it per
  document.
- **Dual page** (cover alone, then spreads) as a layout option in `PageLayout`.
- **Night mode** (invert lightness, keep hue) as a GSK `ColorMatrix` node on the page nodes. It
  is cheap and needs no re-render.

### - [x] C8 (toggles; per-page hit badges not done) 🟢 Search options
- Case-sensitive and whole-word toggles in the search bar (Poppler
  `FindFlags::CASE_SENSITIVE | WHOLE_WORDS_ONLY`). Show the hit count per page in the page
  sidebar as a small badge.

### - [x] C9 🟢 Markup: tool shortcuts
- While the markup toolbar is open: `V` select, `R` rectangle, `O` ellipse, `A` arrow,
  `L` line, `P` pen, `T` text, `H` highlight, `S` signature. Shown in tooltips
  ("Rectangle (R)"). These go through the existing plain-key controller, so they never steal
  keys from text entries.

### - [x] C10 🟢 Empty state that does something
- Replace the bare "Open a Picture or PDF" page with the same status page plus a row of up to 6
  **recent files** as thumbnails (from the B5 cache). Clicking one opens it. The page stays
  minimal: no grid browser.

### - [x] C11 (wrap around and browse folder; JPEG quality and sort order not done) 🟢 Preferences worth having (and no more)
- **JPEG quality** (from A4).
- **Wrap around** at the end of the list.
- **Sort by** name or date taken.
- **Browse folder** (from C1).

Nothing else. Each new preference needs a reason in the commit message.

---

## Phase D: Feel (modern, minimal, distinctive)

### - [x] D1 (save, export, page deletion; trash toast comes with C4) 🟡 Toasts instead of dialogs for reversible actions
- Wrap the content in `adw::ToastOverlay`.
- Deleting pages (sidebar `Delete`) shows "3 pages deleted · Undo".
- Trashing a file (C4) does the same.
- Saving shows "Saved · Show in Folder".
- Export finished shows "Exported · Open".
- Errors that do not need a decision ("Could not export") also become toasts with a details
  button.
- The "Replace the Original?" dialog stays, because overwriting is not undoable.

### - [x] D2 (thumbnail stand-in and fade; the first-paint image crossfade between two decoded files is not done) 🟢 Crossfade between files, and a placeholder from the thumbnail
- When moving to a file that is not decoded yet, show its sidebar thumbnail scaled to fit
  (with a light blur). Crossfade to the full texture in 150 ms when it arrives. The spinner
  appears only after 300 ms without a placeholder.
- Honour `gtk-enable-animations`.

### - [x] D3 🟢 Low-resolution page placeholders while scrolling fast
- Pages without a texture draw their page-strip thumbnail scaled up (when it exists) instead of
  flat white. That is free with B1, because thumbnails share the worker. Together with B2, a
  page is never blank once it has been seen.

### - [x] D4 🟡 Header bar diet
- Today the header can hold: sidebar, open, main menu, Pages menu, Edit menu, Markup, and
  Properties.
- Merge the **Edit** and **Pages** menus into one context menu button whose content depends on
  the document (picture: rotate, flip, crop, resize, adjust, export; PDF: page operations,
  export).
- Move **Properties** into the main menu (`Alt+Enter` keeps working).
- The end result is sidebar, open | title | markup, document menu, main menu.

### - [x] D5 (amber accent app-wide, checked in light and dark; hairline pages and the high-contrast/2× recheck not done) 🟢 A visual rule pass
- One accent: the active tool, selection rings, the current search hit, the crop frame and the
  page ring all use the same amber.
- Use the OSD pill style for every floating control: zoom pill, back pill (C2) and toasts.
- Pages get a 1 px hairline instead of a heavy shadow in dark mode.
- Check every screen in light, dark and high-contrast, and at 1× and 2×, and retake the
  screenshots with `build-aux/screenshot.sh`.

### - [x] D6 🟡 Optional: command palette (`Ctrl+Shift+P`)
- Only if D1–D5 are done and it still feels useful. A searchable list of every window action
  (built from the same action table as E3), with its shortcut. It makes everything
  discoverable without adding chrome. Leave it out if it grows beyond about 200 lines.

---

## Phase E: Code health and consistency

### - [x] E1 (app.rs went from 3,322 to 1,683 lines by moving code into `app/` modules; the `view!` tree and `init` were not split into components) 🟠 Split `app.rs` (3,300 lines)
- Move the `view!` macro's sub-trees into small Relm4 components or plain widget builders:
  - `HeaderBar`
  - `MarkupToolbar` (with the swatches and `color_circle`)
  - `EditBar` (crop and adjust)
  - `ViewControls` (zoom and page pill)
- Move `update_inner`'s arms into the existing submodules (`navigation.rs` for
  open/list/preload, `search.rs`, `presentation.rs` for fullscreen, slideshow and cursor).
- Target: `app.rs` under 1,200 lines. This is a pure refactor: no behaviour change, and all
  tests stay green.

### - [x] E2 (PDF page transforms; the picture-side `origin` logic stays in the canvas) 🟢 Put the transforms in one place, as the rules promised
- PLAN.md rule 3 and ARCHITECTURE.md refer to `src/geometry.rs`, which does not exist. Page,
  widget and document transforms are spread through `canvas.rs` (`page_origin`, `doc_point`,
  `page_point_at`, `origin`) and the snapshot code repeats the same rect maths 5 times.
- Create `geometry.rs` with `ViewTransform { zoom, origin }` and `page_rect_to_widget(...)`,
  give it unit tests, and use it in `canvas.rs`.

### - [x] E3 🟢 One table for actions, shortcuts and the shortcuts dialog
- `shortcuts.rs` repeats the accelerators by hand, and they have already drifted (A9). Build the
  dialog from the same `nav_actions` / `view_actions` tables (add a section and a title to each
  entry). Add a unit test that every registered accelerator appears in the dialog.

### - [x] E4 🟢 Docs drift
- **ARCHITECTURE.md:**
  - Remove "Target layout (Phase N)" and list the real modules.
  - Replace "a single render-worker thread owns the document" with what is actually true
    (after B1 it will be true).
  - Mention the global Poppler lock, the working-copy history and the control interface.
  - Fix "Relm4 avoids Rc<RefCell>", since `Store` is shared through one.
- **README:**
  - "use rustup if your Rust is older than 1.92" should say 1.93 (the requirement stated
    above it).
  - Update the security paragraph after A5.
- **PLAN.md:** add "Phase 9 → see IMPROVEMENT_PLAN.md".

### - [x] E5 (pot check and required control tests; cargo-deny not added) 🟢 CI hardening
- Check that `po/lumen.pot` is up to date: run `update-pot.sh` in CI and fail on `git diff`.
- Run `tests/control.rs` in CI explicitly. Install `gtk4-broadway` (in `gtk4` on Arch) and
  `dbus`, and make sure the tests fail rather than skip when Broadway is missing.
- Add `cargo deny check licenses advisories`. The GPL-3.0 app must not pull in incompatible
  crates.

### - [x] E6 (done: A1, C1, C3 end-to-end tests, A3/A4/B8/E2/E3 unit tests, damaged-file test; not done: Space scrolling, link back, and trash end-to-end, which need pointer/keyboard or trash support in the test environment) 🟡 Test gaps to close alongside the fixes
- End-to-end tests (control interface):
  - A1: the list survives a save.
  - A9: Space scrolls a PDF.
  - C2: back after a link.
  - C3: reload on change.
  - C4: trash and undo.
- Unit tests:
  - A3: permissions, symlink and hard link.
  - A4: ICC profile round trip.
  - B8: JPEG passthrough size.
  - E2: geometry.
- A fuzz-style test: open every file in `tests/fixtures` plus a few truncated copies, and
  expect errors, never panics.

---

## Phase F: Release

### - [x] F1 (version bumped and release notes written; the Flatpak build, the manual release-workflow run, the tag and the AUR checksum are still to do) 🟢 Cut 0.4.0
- Follow `docs/RELEASING.md`:
  - Update `metainfo.xml` releases.
  - Update CHANGELOG.
  - Retake the screenshots (D5).
  - Update the AUR `updpkgsums`.
- Then Flathub submission (open item: the README says "not on Flathub yet"). Phase A and A5 in
  particular make the sandbox claim safe to publish.

---

## Deliberately not planned (keep Lumen minimal)

- A library, albums, tags or ratings (that is a photo manager, not a viewer).
- Layers, filters beyond exposure, contrast and saturation, or healing tools.
- Certificate-based (cryptographic) PDF signing. Possible later as its own feature; it needs
  NSS and careful UX.
- OCR and side-by-side compare (stretch goals in PLAN.md). Revisit after 0.4.
- MCP server (PLAN 8.3). It stays optional and separate.

## Suggested order and sizing

| Order | Items | Rough size |
|---|---|---|
| 1 | A1, A2, A6, A7, A9, A10 | 1 day |
| 2 | A3, A4 | 1–2 days |
| 3 | A5 | 1–2 days |
| 4 | A8 (verify), B2, B7, B8, B10, B11 | 1–2 days |
| 5 | B1 (then B3 prototype) | 2–3 days |
| 6 | B5, B6, B4 | 3 days |
| 7 | C2, C3, C4, C5, C6, C9, C1 | 3 days |
| 8 | D1–D5, C7, C8, C10, C11 | 3–4 days |
| 9 | E1–E6, F1 | 2–3 days |
