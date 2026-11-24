# Screenshots

The screenshots in `data/screenshots/` are of sample files drawn by `make_samples.py` (so they can be
published), taken from the real app with scripted input, on a 1200×800 window.

| File | What it shows | Scene | Theme |
|---|---|---|---|
| `image-markup.png` | A picture marked up | `markup` | dark |
| `pdf-forms.png` | A membership form filled in and signed | `forms` | light |
| `pdf-annotations.png` | A PDF with highlights, a box and a note, and the Pages sidebar | `annot` | light |
| `image-crop.png` | Cropping | `crop` | dark |
| `pages-organiser.png` | The page organiser, one page turned | `organiser` | light |
| `folder-dark.png` | Browsing a folder | `folder` | dark |
| `night-mode.png` | A PDF in night mode, with its contents | `night` | dark |

## How they are made

There is no way to send real mouse and keyboard events where this was developed, so the app is
driven by the messages those events would produce.

1. `python3 make_samples.py DIR` makes `DIR/*.jpg` (six landscapes) and three PDFs. Put the
   pictures and the PDFs in two folders: `pictures/` and `docs/`.
2. Copy `demo.rs.txt` to `src/app/demo.rs` and add the hook it needs to `App::new` and `Message`
   (three lines marked `// DEMO` and a block between `// DEMO-HOOK-START` and `// DEMO-HOOK-END`,
   as described in `docs/DEVELOPMENT.md`). `cargo build --release`.
3. `SCRATCH=DIR ./shoot.sh SCENE NAME THEME SECONDS FILE_OR_FOLDER`, for example
   `./shoot.sh markup image-markup dark 6.5 pictures/golden-hour.jpg`. It starts the app in a
   throw-away config, resizes its window on niri, waits, takes the screenshot and stops the app.
4. **Remove the demo code again** before committing: `grep -rc DEMO src` must find nothing.

`demo.rs.txt` is a reference copy that is not compiled.
