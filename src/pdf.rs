//! PDF documents, rendered by `hayro` (pure Rust, `#![forbid(unsafe_code)]`).
//!
//! A document owns a small pool of render threads. Each thread parses its own
//! copy of the document (sharing the file bytes), because hayro's render cache
//! is single-threaded. Requests are served newest-first so that, while the user
//! scrolls quickly, the pages they are looking at now win over stale ones.

use std::collections::{BTreeMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use tokio::sync::oneshot;

use crate::annotate::pdf::{Fill, Imported, ObjRef, PageEdit};
use crate::annotate::{self, AnnotationLayer};
use crate::forms::{self, Form};
use crate::raster::Picture;
use crate::structure::{self, Link, OutlineItem, PageText};

/// Largest bitmap we render for a single page, in pixels per side.
const MAX_SIDE: f32 = 8192.0;

/// A render thread starts its caches afresh after this many pixels.
const CACHE_PIXELS: usize = 200_000_000;

enum Work {
    Render { scale: f32, night: bool, reply: oneshot::Sender<Result<Picture>> },
    Text { reply: oneshot::Sender<Arc<PageText>> },
}

struct Job {
    page: usize,
    work: Work,
}

impl Job {
    fn is_cancelled(&self) -> bool {
        match &self.work {
            Work::Render { reply, .. } => reply.is_closed(),
            Work::Text { reply } => reply.is_closed(),
        }
    }
}

#[derive(Default)]
struct Queue {
    jobs: Mutex<(VecDeque<Job>, bool)>,
    ready: Condvar,
}

impl Queue {
    fn push(&self, job: Job) {
        let mut guard = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        guard.0.push_back(job);
        self.ready.notify_one();
    }

    /// Newest job first; `None` once the document is closed.
    fn pop(&self) -> Option<Job> {
        let mut guard = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if guard.1 {
                return None;
            }
            while let Some(job) = guard.0.pop_back() {
                if !job.is_cancelled() {
                    return Some(job);
                }
            }
            guard = self.ready.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn close(&self) {
        let mut guard = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        guard.1 = true;
        guard.0.clear();
        self.ready.notify_all();
    }
}

/// A PDF opened with what Lumen shows on top of it.
#[derive(Debug, Clone)]
pub struct Opened {
    /// Why part of its form or markup was not read, if so.
    pub warning: Option<&'static str>,
    /// Which objects new annotations became, when it was just saved.
    pub placed: annotate::pdf::Placed,
    pub doc: Document,
    /// Editable annotations, when opened for markup.
    pub imported: Option<Imported>,
    pub form: Arc<Form>,
}

/// What to write into a PDF: annotation layers for `pages`, and form values.
pub struct Changes<'a> {
    pub layers: &'a BTreeMap<usize, AnnotationLayer>,
    pub refs: &'a [Vec<ObjRef>],
    pub pages: &'a [usize],
    pub form: &'a Form,
    /// One per field of `form`; the ones that differ from the file are written.
    pub values: &'a [forms::Value],
    /// The password the PDF was opened with.
    pub password: Option<&'a str>,
}

/// An open PDF. Cheap to clone; the render threads stop when the last clone drops.
#[derive(Clone)]
pub struct Document {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    original: Arc<Vec<u8>>,
    /// Page sizes in PDF points, after applying /Rotate.
    pages: Vec<(f32, f32)>,
    outline: Vec<OutlineItem>,
    links: Vec<Vec<Link>>,
    /// The pages' own names, when they have any but their numbers.
    labels: Option<Vec<String>>,
    /// Extracted text per page, filled in as search needs it.
    text: Mutex<Vec<Option<Arc<PageText>>>>,
    queue: Arc<Queue>,
    /// What opened it, if it is password-protected; kept in memory only.
    password: Option<String>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.queue.close();
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document").field("path", &self.inner.path).field("pages", &self.inner.pages.len()).finish()
    }
}

/// The PDF is password-protected and the password given (if any) does not open it.
#[derive(Debug, Clone, Copy)]
pub struct PasswordNeeded {
    /// A password was given, and it was wrong.
    pub wrong: bool,
}

impl std::fmt::Display for PasswordNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.wrong { "the password is not right" } else { "this PDF is password-protected" })
    }
}

impl std::error::Error for PasswordNeeded {}

/// Why a PDF did not open: it wants a password, or something else went wrong.
#[derive(Debug, Clone)]
pub enum OpenError {
    Password { wrong: bool },
    Failed(String),
}

impl From<anyhow::Error> for OpenError {
    fn from(e: anyhow::Error) -> Self {
        match e.downcast_ref::<PasswordNeeded>() {
            Some(p) => Self::Password { wrong: p.wrong },
            None => Self::Failed(format!("{e:#}")),
        }
    }
}

pub(crate) fn parse(bytes: impl Into<hayro::hayro_syntax::PdfData>, password: Option<&str>) -> Result<Pdf> {
    Pdf::new_with_password(bytes, password.unwrap_or("")).map_err(|e| match e {
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
            anyhow::Error::new(PasswordNeeded { wrong: password.is_some_and(|p| !p.is_empty()) })
        }
        LoadPdfError::Decryption(e) => anyhow!("this PDF's encryption is not supported ({e:?})"),
        LoadPdfError::Invalid => anyhow!("this PDF is damaged or not a PDF"),
    })
}

impl Document {
    /// Opens `path` and starts its render threads. Blocking.
    #[cfg(test)]
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let bytes = Arc::new(bytes);
        Self::from_bytes(path, bytes.clone(), bytes, None)
    }

    /// Opens `path` with its form, and (with `markup`) its editable annotations. What Lumen
    /// shows itself (form inputs, annotations being edited) is hidden from the renderer. Blocking.
    /// A `password` opens a password-protected PDF (a wrong or missing one gives [`PasswordNeeded`]).
    pub fn open_full(path: &Path, markup: bool, password: Option<&str>) -> Result<Opened> {
        let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        Self::full_from_bytes(path, Arc::new(bytes), markup, password)
    }

    /// The same document again, from the bytes it was opened from (whatever became of the file). Blocking.
    pub fn reopen(&self, markup: bool) -> Result<Opened> {
        Self::full_from_bytes(&self.inner.path, self.inner.original.clone(), markup, self.password())
    }

    fn full_from_bytes(path: &Path, bytes: Arc<Vec<u8>>, markup: bool, password: Option<&str>) -> Result<Opened> {
        let (mut form, imported) = {
            let pdf = parse(bytes.clone(), password)?;
            let form = catch_unwind(AssertUnwindSafe(|| forms::read(&pdf))).unwrap_or_else(|_| {
                tracing::warn!("could not read the form of {}", path.display());
                Form::default()
            });
            let imported =
                markup.then(|| catch_unwind(AssertUnwindSafe(|| annotate::pdf::import(&pdf))).unwrap_or_default());
            (form, imported)
        };
        let mut hidden = form.hidden_widgets();
        hidden.extend(imported.iter().flat_map(|i| i.refs.iter().flatten().copied()));
        let shown = if hidden.is_empty() {
            bytes.clone()
        } else {
            match annotate::pdf::hide(&bytes, &hidden, password) {
                Ok(shown) => Arc::new(shown),
                // Markup cannot work without it; a form can still be shown, just not filled in.
                Err(e) if imported.is_none() => {
                    tracing::warn!("the form of {} cannot be filled in: {e:#}", path.display());
                    form = Form::default();
                    bytes.clone()
                }
                Err(e) => return Err(e),
            }
        };
        let warning = if form.truncated {
            Some("This form is too large: the rest of its fields cannot be filled in")
        } else if imported.as_ref().is_some_and(|i| i.truncated) {
            Some("This PDF has too much markup: the rest of it cannot be edited")
        } else {
            None
        };
        Ok(Opened {
            doc: Self::from_bytes(path, shown, bytes, password)?,
            imported,
            form: Arc::new(form),
            warning,
            placed: Vec::new(),
        })
    }

    /// Writes `changes` into this PDF and saves the result to `target` (which may be its own
    /// file), then opens it again. Blocking.
    pub fn save_changes(&self, target: &Path, changes: &Changes<'_>, markup: bool) -> Result<Opened> {
        let (bytes, placed) = self.changed(changes)?;
        let opened = Self::save_bytes(target, bytes, markup, changes.password, self.page_count())?;
        Ok(Opened { placed, ..opened })
    }

    /// Saves a PDF Lumen made to `target`, but only if it opens again with all its `pages`. Blocking.
    pub fn save_bytes(
        target: &Path,
        bytes: Vec<u8>,
        markup: bool,
        password: Option<&str>,
        pages: usize,
    ) -> Result<Opened> {
        let bytes = Arc::new(bytes);
        let opened = Self::full_from_bytes(target, bytes.clone(), markup, password)
            .context("the new PDF could not be read again, so nothing was saved")?;
        if opened.doc.page_count() != pages {
            bail!("the new PDF did not have all its pages, so nothing was saved");
        }
        crate::fsio::write_atomic(target, &bytes)?;
        Ok(opened)
    }

    /// The bytes this PDF was opened from: what changes are written into, even if another
    /// program has replaced the file since.
    pub fn original(&self) -> &[u8] {
        &self.inner.original
    }

    /// The bytes of this PDF with `changes` written in. Blocking.
    pub fn with_changes(&self, changes: &Changes<'_>) -> Result<Vec<u8>> {
        Ok(self.changed(changes)?.0)
    }

    fn changed(&self, changes: &Changes<'_>) -> Result<(Vec<u8>, annotate::pdf::Placed)> {
        let bytes = self.original();
        let edits: Vec<PageEdit<'_>> = changes
            .pages
            .iter()
            .map(|&page| PageEdit {
                page,
                layer: changes.layers.get(&page),
                replaces: changes.refs.get(page).map_or(&[], Vec::as_slice),
            })
            .collect();
        let fills: Vec<Fill<'_>> = changes
            .form
            .fields
            .iter()
            .zip(changes.values)
            .filter(|(field, value)| field.value != **value)
            .map(|(field, value)| Fill { field, value })
            .collect();
        catch_unwind(AssertUnwindSafe(|| annotate::pdf::save(bytes, &edits, &fills, changes.password)))
            .map_err(|_| anyhow!("the PDF could not be written"))?
    }

    /// `bytes` is what is rendered, `original` what the file holds.
    fn from_bytes(path: &Path, bytes: Arc<Vec<u8>>, original: Arc<Vec<u8>>, password: Option<&str>) -> Result<Self> {
        let pdf = parse(bytes.clone(), password)?;
        let pages: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
        if pages.is_empty() {
            bail!("this PDF has no pages");
        }
        // Outline and links are cheap to read; a malformed outline must not stop the document opening.
        let (outline, links) = catch_unwind(AssertUnwindSafe(|| structure::read(&pdf))).unwrap_or_else(|_| {
            tracing::warn!("could not read the outline or links of {}", path.display());
            (Vec::new(), vec![Vec::new(); pages.len()])
        });
        let labels = catch_unwind(AssertUnwindSafe(|| structure::page_labels(&pdf))).ok().flatten();
        drop(pdf);

        let queue = Arc::new(Queue::default());
        let threads =
            std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(2).clamp(1, 4).min(pages.len());
        for i in 0..threads {
            let queue = queue.clone();
            let bytes = bytes.clone();
            let password = password.map(str::to_owned);
            std::thread::Builder::new()
                .name(format!("pdf-render-{i}"))
                .spawn(move || worker(bytes, password.as_deref(), &queue))
                .context("cannot start a render thread")?;
        }

        let text = Mutex::new(vec![None; pages.len()]);
        let password = password.filter(|p| !p.is_empty()).map(str::to_owned);
        Ok(Self {
            inner: Arc::new(Inner {
                path: path.to_owned(),
                original,
                pages,
                outline,
                links,
                labels,
                text,
                queue,
                password,
            }),
        })
    }

    /// The name page `page` gives itself ("iv", "A-3"), if it is not just its number.
    pub fn label(&self, page: usize) -> Option<&str> {
        self.inner.labels.as_ref()?.get(page).map(String::as_str).filter(|l| *l != (page + 1).to_string())
    }

    /// The page called `name`, ignoring case.
    pub fn page_named(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        self.inner.labels.as_ref()?.iter().position(|l| l.eq_ignore_ascii_case(name))
    }

    pub fn has_labels(&self) -> bool {
        self.inner.labels.is_some()
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The password it was opened with, if it needed one.
    pub fn password(&self) -> Option<&str> {
        self.inner.password.as_deref()
    }

    pub fn page_count(&self) -> usize {
        self.inner.pages.len()
    }

    /// Size of `page` in PDF points (1/72 inch).
    pub fn page_size(&self, page: usize) -> (f32, f32) {
        self.inner.pages[page]
    }

    pub fn pages(&self) -> &[(f32, f32)] {
        &self.inner.pages
    }

    /// The table of contents, flattened in reading order.
    pub fn outline(&self) -> &[OutlineItem] {
        &self.inner.outline
    }

    pub fn links(&self, page: usize) -> &[Link] {
        self.inner.links.get(page).map_or(&[], Vec::as_slice)
    }

    /// The scale `page` is really rendered at when asked for `scale`; `None` if it is too large.
    pub fn real_scale(&self, page: usize, scale: f32) -> Option<f32> {
        let (w, h) = *self.inner.pages.get(page)?;
        render_scale(scale, w, h)
    }

    /// Renders `page` at `scale` pixels per point, off the UI thread.
    pub async fn render(&self, page: usize, scale: f32) -> Result<Picture> {
        self.render_toned(page, scale, false).await
    }

    /// Like [`Document::render`], for night mode when `night` is set (see [`night_tone`]).
    pub async fn render_toned(&self, page: usize, scale: f32, night: bool) -> Result<Picture> {
        let (reply, rx) = oneshot::channel();
        self.inner.queue.push(Job { page, work: Work::Render { scale, night, reply } });
        rx.await.map_err(|_| anyhow!("the document was closed"))?
    }

    /// The text of `page`, extracted on a render thread the first time and cached.
    pub async fn text(&self, page: usize) -> Arc<PageText> {
        if let Some(Some(t)) = self.inner.text.lock().unwrap_or_else(|e| e.into_inner()).get(page) {
            return t.clone();
        }
        let (reply, rx) = oneshot::channel();
        self.inner.queue.push(Job { page, work: Work::Text { reply } });
        let text = rx.await.unwrap_or_default();
        if let Some(slot) = self.inner.text.lock().unwrap_or_else(|e| e.into_inner()).get_mut(page) {
            *slot = Some(text.clone());
        }
        text
    }
}

/// `scale` brought down so that a page of `w` × `h` points stays within [`MAX_SIDE`] whatever the
/// zoom; `None` when even the smallest scale does not.
fn render_scale(scale: f32, w: f32, h: f32) -> Option<f32> {
    let longest = w.max(h).max(1.0);
    let scale = scale.min(MAX_SIDE / longest).max(0.01);
    (longest.is_finite() && longest * scale <= MAX_SIDE + 1.0).then_some(scale)
}

fn worker(bytes: Arc<Vec<u8>>, password: Option<&str>, queue: &Queue) {
    let Ok(pdf) = parse(bytes, password) else { return };
    let pages = pdf.pages();
    let mut cache = RenderCache::new();
    // Pixels rendered with this cache: it keeps the glyphs of every zoom level and never lets one go.
    let mut rendered = 0usize;
    let interpreter = InterpreterSettings::default();
    let settings = RenderSettings::default();
    let text_cache = hayro::hayro_interpret::InterpreterCache::new();

    while let Some(job) = queue.pop() {
        let Some(page) = pages.get(job.page) else {
            if let Work::Render { reply, .. } = job.work {
                let _ = reply.send(Err(anyhow!("no such page")));
            }
            continue;
        };
        match job.work {
            Work::Text { reply } => {
                let text =
                    catch_unwind(AssertUnwindSafe(|| structure::page_text(page, &text_cache))).unwrap_or_else(|_| {
                        tracing::warn!("could not read the text of page {}", job.page + 1);
                        PageText::default()
                    });
                let _ = reply.send(Arc::new(text));
            }
            Work::Render { scale, night, reply } => {
                let (w, h) = page.render_dimensions();
                // Keep the bitmap within limits whatever the zoom.
                let Some(scale) = render_scale(scale, w, h) else {
                    let _ = reply.send(Err(anyhow!("page {} is too large to show", job.page + 1)));
                    continue;
                };
                rendered += (w * scale) as usize * (h * scale) as usize;
                if rendered > CACHE_PIXELS {
                    cache = RenderCache::new();
                    rendered = 0;
                }
                let pixmap_settings = PixmapSettings {
                    x_scale: scale,
                    y_scale: scale,
                    bg_color: hayro::vello_cpu::color::palette::css::WHITE,
                };
                // A malformed page must never take the viewer down with it.
                let result = catch_unwind(AssertUnwindSafe(|| {
                    hayro::render(page, &cache, &interpreter, &settings, &pixmap_settings)
                }))
                .map_err(|_| anyhow!("page {} could not be rendered", job.page + 1))
                .map(|pixmap| {
                    let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
                    let mut rgba = pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha);
                    if night {
                        night_tone(&mut rgba);
                    }
                    Picture { width, height, original: (w.round() as u32, h.round() as u32), rgba: rgba.into() }
                });
                let _ = reply.send(result);
            }
        }
    }
}

/// Night mode for an opaque RGBA bitmap: the colours inverted, then turned half way round
/// the colour wheel, so paper goes dark, text goes light and pictures keep their hues
/// instead of turning into negatives (the same matrix as the GTK Lumen).
pub fn night_tone(rgba: &mut [u8]) {
    // Fixed point, 10 fractional bits; the last term is the offset of 1.0.
    const M: [[i32; 3]; 3] = [[588, -1464, -147], [-436, -440, -147], [-436, -1464, 877]];
    const OFFSET: i32 = 255 * 1024 + 512;
    for px in rgba.as_chunks_mut::<4>().0 {
        let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
        for (out, row) in px.iter_mut().zip(M) {
            *out = ((row[0] * r + row[1] * g + row[2] * b + OFFSET) >> 10).clamp(0, 255) as u8;
        }
    }
}

/// A copy of a password-protected PDF without the protection, for printing (the print
/// system cannot open it). Blocking.
pub fn decrypted(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    let mut doc = lopdf::Document::load_mem_with_options(bytes, lopdf::LoadOptions::with_password(password))
        .context("the PDF could not be unlocked")?;
    let mut out = Vec::with_capacity(bytes.len());
    doc.save_to(&mut out).context("the PDF could not be written")?;
    Ok(out)
}

/// A first-page thumbnail for the file strip. Blocking.
/// A password-protected PDF needs its `password`; without it this fails with [`PasswordNeeded`].
pub fn thumbnail(path: &Path, side: u32, password: Option<&str>) -> Result<Picture> {
    let bytes = Arc::new(std::fs::read(path)?);
    let pdf = parse(bytes, password)?;
    let page = pdf.pages().first().context("no pages")?;
    let (w, h) = page.render_dimensions();
    let scale = side as f32 / w.max(h).max(1.0);
    let settings =
        PixmapSettings { x_scale: scale, y_scale: scale, bg_color: hayro::vello_cpu::color::palette::css::WHITE };
    let cache = RenderCache::new();
    let pixmap = catch_unwind(AssertUnwindSafe(|| {
        hayro::render(page, &cache, &InterpreterSettings::default(), &RenderSettings::default(), &settings)
    }))
    .map_err(|_| anyhow!("render failed"))?;
    let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
    Ok(Picture {
        width,
        height,
        original: (w as u32, h as u32),
        rgba: pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha).into(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/form.pdf")
    }

    #[tokio::test]
    async fn opens_and_renders_pages() {
        let doc = Document::open(&fixture()).unwrap();
        assert!(doc.page_count() >= 1);
        let (w, h) = doc.page_size(0);
        let start = std::time::Instant::now();
        let pic = doc.render(0, 2.0).await.unwrap();
        eprintln!("page 1 at 2x: {:?}", start.elapsed());
        assert_eq!(pic.width, (w * 2.0) as u32);
        assert_eq!(pic.height, (h * 2.0) as u32);
        assert_eq!(pic.rgba.len(), (pic.width * pic.height * 4) as usize);
        // Paper is white and opaque.
        assert_eq!(&pic.rgba[..4], &[255, 255, 255, 255]);
        assert!(doc.render(9999, 1.0).await.is_err());
    }

    /// Renders pages for tools/fidelity.py, which compares them with poppler's. Not a test of
    /// anything by itself: it needs `LUMEN_CORPUS_LIST` (lines of `path|1,5,9`, pages from 1)
    /// and `LUMEN_CORPUS_OUT` (a folder), and runs with
    /// `cargo test --release render_corpus -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn render_corpus() {
        let (Some(list), Some(out)) = (std::env::var_os("LUMEN_CORPUS_LIST"), std::env::var_os("LUMEN_CORPUS_OUT"))
        else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        // Pixels per point (1.0 is 72 dpi); `LUMEN_CORPUS_SCALE` changes it.
        let scale: f32 = std::env::var("LUMEN_CORPUS_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        for (n, line) in std::fs::read_to_string(list).unwrap().lines().enumerate() {
            let Some((path, pages)) = line.split_once('|') else { continue };
            let doc = match Document::open(Path::new(path)) {
                Ok(d) => d,
                Err(e) => {
                    println!("OPEN-ERR\t{n}\t{path}\t{e}");
                    continue;
                }
            };
            for page in pages.split(',').filter_map(|p| p.trim().parse::<usize>().ok()) {
                let start = std::time::Instant::now();
                // The same size as `pdftoppm -r (72 * scale)`.
                match doc.render(page - 1, scale).await {
                    Ok(pic) => {
                        let img = image::RgbaImage::from_raw(pic.width, pic.height, pic.rgba.to_vec()).unwrap();
                        img.save(out.join(format!("{n}-{page}.png"))).unwrap();
                        println!("OK\t{n}\t{page}\t{}\t{}", start.elapsed().as_millis(), path);
                    }
                    Err(e) => println!("RENDER-ERR\t{n}\t{page}\t{path}\t{e}"),
                }
            }
        }
    }

    #[test]
    fn night_tone_turns_paper_dark_and_keeps_hues() {
        let mut px = [255, 255, 255, 255, 0, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255];
        night_tone(&mut px);
        // White paper goes to black, black text to white.
        assert!(px[..3].iter().all(|&c| c <= 1), "{:?}", &px[..3]);
        assert!(px[4..7].iter().all(|&c| c >= 254), "{:?}", &px[4..7]);
        // Red stays red (a lighter one), blue stays blue.
        assert!(px[8] > px[9] && px[8] > px[10], "{:?}", &px[8..11]);
        assert!(px[14] > px[12] && px[14] > px[13], "{:?}", &px[12..15]);
        // Alpha is untouched.
        assert!([px[3], px[7], px[11], px[15]].iter().all(|&a| a == 255));
    }

    #[test]
    fn rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("lumen-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.pdf");
        std::fs::write(&path, b"%PDF-1.7 nonsense").unwrap();
        assert!(Document::open(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn structured_pdf() -> PathBuf {
        let path = std::env::temp_dir().join(format!("lumen-structured-{}.pdf", std::process::id()));
        std::fs::write(&path, structured_bytes()).unwrap();
        path
    }

    /// Two pages: text, a link to page 2, a web link, and a two-entry outline.
    pub fn structured_bytes() -> Vec<u8> {
        use pdf_writer::types::{ActionType, AnnotationType};
        use pdf_writer::{Content, Finish, Name, Rect, Ref, Str, TextStr};
        let (catalog, tree, p1, p2, c1, c2, font, outline, o1, o2, a1, a2) = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12);
        let r = Ref::new;
        let mut w = pdf_writer::Pdf::new();
        w.catalog(r(catalog)).pages(r(tree)).outlines(r(outline));
        w.pages(r(tree)).kids([r(p1), r(p2)]).count(2);
        for (page, content, text, annots) in
            [(p1, c1, &b"Hello Lumen World"[..], vec![r(a1), r(a2)]), (p2, c2, &b"Second page"[..], vec![])]
        {
            let mut pg = w.page(r(page));
            pg.parent(r(tree)).media_box(Rect::new(0.0, 0.0, 600.0, 800.0)).contents(r(content));
            pg.resources().fonts().pair(Name(b"F1"), r(font));
            if !annots.is_empty() {
                pg.annotations(annots);
            }
            pg.finish();
            let mut c = Content::new();
            c.begin_text().set_font(Name(b"F1"), 20.0).next_line(72.0, 700.0).show(Str(text)).end_text();
            w.stream(r(content), &c.finish());
        }
        w.type1_font(r(font)).base_font(Name(b"Helvetica"));
        let mut link = w.annotation(r(a1));
        link.subtype(AnnotationType::Link).rect(Rect::new(72.0, 690.0, 200.0, 720.0));
        link.action().action_type(ActionType::GoTo).destination().page(r(p2)).xyz(0.0, 600.0, None);
        link.finish();
        let mut web = w.annotation(r(a2));
        web.subtype(AnnotationType::Link).rect(Rect::new(72.0, 600.0, 200.0, 620.0));
        web.action().action_type(ActionType::Uri).uri(Str(b"https://example.com/"));
        web.finish();
        w.outline(r(outline)).first(r(o1)).last(r(o2)).count(2);
        w.outline_item(r(o1)).parent(r(outline)).next(r(o2)).title(TextStr("Introduction")).dest().page(r(p1)).fit();
        w.outline_item(r(o2))
            .parent(r(outline))
            .prev(r(o1))
            .title(TextStr("Ünïcode chapter"))
            .dest()
            .page(r(p2))
            .xyz(0.0, 500.0, None);
        w.finish()
    }

    #[tokio::test]
    async fn outline_links_and_search() {
        use crate::structure::Target;
        let path = structured_pdf();
        let doc = Document::open(&path).unwrap();

        let titles: Vec<_> = doc.outline().iter().map(|o| (o.title.as_str(), o.target.clone())).collect();
        assert_eq!(titles[0], ("Introduction", Some(Target::Page { page: 0, top: None })));
        // y = 500 from the bottom of an 800 pt page is 300 pt from the top.
        assert_eq!(titles[1], ("Ünïcode chapter", Some(Target::Page { page: 1, top: Some(300.0) })));

        let links = doc.links(0);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, Target::Page { page: 1, top: Some(200.0) });
        assert!((links[0].rect.y0 - 80.0).abs() < 0.01 && (links[0].rect.x0 - 72.0).abs() < 0.01);
        assert_eq!(links[1].target, Target::Uri("https://example.com/".into()));

        let text = doc.text(0).await;
        let hits = text.find("lumen");
        assert_eq!(hits.len(), 1, "{text:?}");
        let r = hits[0][0];
        // Baseline at 100 pt from the top, 20 pt Helvetica.
        assert!(r.y0 > 75.0 && r.y1 < 110.0 && r.x0 > 72.0 && r.x1 < 300.0, "{r:?}");
        assert_eq!(text.find("HELLO LUMEN world").len(), 1);
        assert_eq!(doc.text(1).await.find("second").len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pages_are_called_by_their_labels() {
        use lopdf::{Object, StringFormat, dictionary};
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages = pdf.new_object_id();
        let media: Vec<Object> = [0, 0, 600, 800].map(Object::Integer).to_vec();
        let kids: Vec<Object> = (0..5)
            .map(|_| {
                pdf.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => media.clone() }).into()
            })
            .collect();
        pdf.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 5 }.into());
        // i, ii, then A-1, A-2, A-3.
        let prefix = Object::String(b"A-".to_vec(), StringFormat::Literal);
        let nums: Vec<Object> = vec![
            0.into(),
            dictionary! { "S" => "r" }.into(),
            2.into(),
            dictionary! { "S" => "D", "P" => prefix }.into(),
        ];
        let labels = pdf.add_object(dictionary! { "Nums" => nums });
        let root = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "PageLabels" => labels });
        pdf.trailer.set("Root", root);
        let path = std::env::temp_dir().join(format!("lumen-labels-{}.pdf", std::process::id()));
        pdf.save(&path).unwrap();
        let doc = Document::open(&path).unwrap();
        assert_eq!((0..5).map(|p| doc.label(p).unwrap_or("-")).collect::<Vec<_>>(), ["i", "ii", "A-1", "A-2", "A-3"]);
        assert_eq!((doc.page_named("II"), doc.page_named("a-3"), doc.page_named("9")), (Some(1), Some(4), None));
        std::fs::remove_file(&path).unwrap();
        // Plain numbers are no labels.
        assert!(Document::open(&structured_pdf()).unwrap().label(0).is_none());
    }

    #[test]
    fn destinations_on_turned_pages_are_where_they_are_seen() {
        use crate::structure::Target;
        use lopdf::{Object, dictionary};
        let dest = |kind: &str, n: &[i64], turn: i64| {
            let mut pdf = lopdf::Document::with_version("1.7");
            let (pages, page) = (pdf.new_object_id(), pdf.new_object_id());
            let mut to: Vec<Object> = vec![page.into(), Object::Name(kind.into())];
            to.extend(n.iter().map(|v| Object::Integer(*v)));
            let rect: Vec<Object> = [10, 10, 50, 50].map(Object::Integer).to_vec();
            let link = pdf.add_object(dictionary! { "Subtype" => "Link", "Rect" => rect, "Dest" => to });
            let media: Vec<Object> = [0, 0, 600, 800].map(Object::Integer).to_vec();
            pdf.objects.insert(page, dictionary! {
                "Type" => "Page", "Parent" => pages, "MediaBox" => media, "Rotate" => turn, "Annots" => vec![link.into()],
            }.into());
            let tree = dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 };
            pdf.objects.insert(pages, tree.into());
            let root = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
            pdf.trailer.set("Root", root);
            let mut out = Vec::new();
            pdf.save_to(&mut out).unwrap();
            let links = structure::read(&parse(out, None).unwrap()).1;
            match links[0][0].target {
                Target::Page { top, .. } => top,
                _ => panic!(),
            }
        };
        // The point 100 from the left and 700 up, as each turn shows it.
        assert_eq!(dest("XYZ", &[100, 700, 0], 0), Some(100.0));
        assert_eq!(dest("XYZ", &[100, 700, 0], 90), Some(100.0));
        assert_eq!(dest("XYZ", &[100, 700, 0], 180), Some(700.0));
        assert_eq!(dest("XYZ", &[100, 700, 0], 270), Some(500.0));
        // A height says nothing about a page on its side; a distance from the left does.
        assert_eq!(dest("FitH", &[700], 0), Some(100.0));
        assert_eq!(dest("FitH", &[700], 90), None);
        assert_eq!(dest("FitV", &[100], 90), Some(100.0));
        for (turn, top) in [(0, 100.0), (90, 100.0), (180, 600.0), (270, 400.0)] {
            assert_eq!(dest("FitR", &[100, 600, 200, 700], turn), Some(top), "{turn}");
        }
    }

    /// Memory while zooming: `LUMEN_PDF=file.pdf cargo test zoom_memory -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn zoom_memory() {
        let Ok(path) = std::env::var("LUMEN_PDF") else { return };
        let doc = Document::open(Path::new(&path)).unwrap();
        let rss = || {
            let status = std::fs::read_to_string(Path::new("/").join("proc/self/status")).unwrap_or_default();
            status.lines().find(|l| l.starts_with("VmRSS")).unwrap_or_default().to_owned()
        };
        for step in 0..120 {
            let scale = 0.5 + step as f32 * 0.05;
            for page in 20..24 {
                let _ = doc.render(page.min(doc.page_count() - 1), scale).await;
            }
            if step % 15 == 14 {
                println!("{} zoom levels: {}", step + 1, rss());
            }
        }
    }

    /// Prints what Lumen reads from a PDF: `LUMEN_PDF=file.pdf cargo test inspect -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn inspect() {
        let Ok(path) = std::env::var("LUMEN_PDF") else { return };
        let doc = Document::open(Path::new(&path)).unwrap();
        println!("{} pages, {} outline entries", doc.page_count(), doc.outline().len());
        for item in doc.outline().iter().take(15) {
            println!("{}{} -> {:?}", "  ".repeat(item.depth as usize), item.title, item.target);
        }
        let links: usize = (0..doc.page_count()).map(|p| doc.links(p).len()).sum();
        println!("{links} links; first page with links:");
        if let Some(p) = (0..doc.page_count()).find(|&p| !doc.links(p).is_empty()) {
            for l in doc.links(p).iter().take(5) {
                println!("  page {}: {:?}", p + 1, l);
            }
        }
        let start = std::time::Instant::now();
        let mut chars = 0;
        for p in 0..doc.page_count() {
            chars += doc.text(p).await.find("the").len();
        }
        println!("'the' found {chars} times; text of all pages in {:?}", start.elapsed());
    }

    /// `tests/fixtures/encrypted.pdf` has the user password "secret".
    #[tokio::test]
    async fn password_protected_pdfs() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encrypted.pdf");
        let wants = |r: Result<Opened>| match r.map_err(OpenError::from) {
            Err(OpenError::Password { wrong }) => Some(wrong),
            _ => None,
        };
        assert_eq!(wants(Document::open_full(&path, false, None)), Some(false));
        assert_eq!(wants(Document::open_full(&path, false, Some("wrong"))), Some(true));
        let opened = Document::open_full(&path, true, Some("secret")).unwrap();
        assert_eq!(opened.doc.password(), Some("secret"));
        let pic = opened.doc.render(0, 0.5).await.unwrap();
        assert!(pic.rgba.chunks(4).any(|p| p[0] < 128), "the page has visible content");
        assert!(!opened.doc.text(0).await.find("e").is_empty(), "the text is readable");

        // Markup saved into it stays protected, and comes back with the password.
        let mut layer = AnnotationLayer::default();
        layer.add(
            annotate::Shape::Rect(annotate::Rect::new(50.0, 50.0, 100.0, 60.0)),
            annotate::model::Style::default(),
        );
        let layers = BTreeMap::from([(0, layer)]);
        let form = Form::default();
        let refs = opened.imported.as_ref().unwrap().refs.clone();
        let changes =
            Changes { layers: &layers, refs: &refs, pages: &[0], form: &form, values: &[], password: Some("secret") };
        let dir = std::env::temp_dir().join(format!("lumen-locked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("saved.pdf");
        let again = opened.doc.save_changes(&target, &changes, true).unwrap();
        assert_eq!(again.imported.unwrap().layers[&0].items.len(), 1);
        assert_eq!(wants(Document::open_full(&target, false, None)), Some(false), "still protected");

        // Another program rewriting the file does not matter: the changes go into what was opened.
        let replaced = dir.join("replaced.pdf");
        std::fs::copy(&path, &replaced).unwrap();
        let opened = Document::open_full(&replaced, true, Some("secret")).unwrap();
        std::fs::write(&replaced, structured_bytes()).unwrap();
        let refs = opened.imported.as_ref().unwrap().refs.clone();
        let changes = Changes { refs: &refs, ..changes };
        let again = opened.doc.save_changes(&replaced, &changes, true).unwrap();
        assert_eq!(again.imported.unwrap().layers[&0].items.len(), 1);
        assert!(std::fs::read(&replaced).unwrap().starts_with(&std::fs::read(&path).unwrap()));
        // And bytes that do not open again never replace a file.
        let before = std::fs::read(&target).unwrap();
        assert!(Document::save_bytes(&target, b"%PDF-1.7 broken".to_vec(), false, None, 1).is_err());
        assert!(Document::save_bytes(&target, structured_bytes(), false, None, 9).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), before);

        // The printable copy opens without a password.
        let plain = decrypted(&std::fs::read(&target).unwrap(), "secret").unwrap();
        let copy = dir.join("plain.pdf");
        std::fs::write(&copy, plain).unwrap();
        let unlocked = Document::open_full(&copy, true, None).unwrap();
        assert_eq!(unlocked.imported.unwrap().layers[&0].items.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_page_of_absurd_size_is_refused_instead_of_rendered() {
        use lopdf::dictionary;
        assert_eq!(render_scale(2.0, 612.0, 792.0), Some(2.0));
        assert_eq!(render_scale(100.0, 612.0, 819.2), Some(10.0));
        assert!(render_scale(1.0, 100_000.0, 800_000.0).is_some());
        assert_eq!(render_scale(1.0, 6_553_500.0, 6_553_500.0), None);
        assert_eq!(render_scale(1.0, f32::INFINITY, 1.0), None);
        // Its thumbnail too.
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages = pdf.new_object_id();
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(), 0.into(), 6_553_500.into(), 6_553_500.into()],
        });
        pdf.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }.into());
        let root = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        pdf.trailer.set("Root", root);
        let path = std::env::temp_dir().join(format!("lumen-huge-page-{}.pdf", std::process::id()));
        pdf.save(&path).unwrap();
        if let Ok(t) = thumbnail(&path, 64, None) {
            assert!(t.width <= 64 && t.height <= 64);
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn thumbnails() {
        let locked = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encrypted.pdf");
        let needs = thumbnail(&locked, 64, None).unwrap_err();
        assert!(needs.downcast_ref::<PasswordNeeded>().is_some());
        assert_eq!(thumbnail(&locked, 64, Some("secret")).unwrap().width.max(64), 64);
        let t = thumbnail(&fixture(), 128, None).unwrap();
        assert_eq!(t.width.max(t.height), 128);
    }
}
