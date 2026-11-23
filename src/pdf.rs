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
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use tokio::sync::oneshot;

use crate::annotate::pdf::{Fill, Imported, ObjRef, PageEdit};
use crate::annotate::{self, AnnotationLayer};
use crate::forms::{self, Form};
use crate::raster::Picture;
use crate::structure::{self, Link, OutlineItem, PageText};

/// Largest bitmap we render for a single page, in pixels per side.
const MAX_SIDE: f32 = 8192.0;

enum Work {
    Render { scale: f32, reply: oneshot::Sender<Result<Picture>> },
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
}

/// An open PDF. Cheap to clone; the render threads stop when the last clone drops.
#[derive(Clone)]
pub struct Document {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    /// Page sizes in PDF points, after applying /Rotate.
    pages: Vec<(f32, f32)>,
    outline: Vec<OutlineItem>,
    links: Vec<Vec<Link>>,
    /// Extracted text per page, filled in as search needs it.
    text: Mutex<Vec<Option<Arc<PageText>>>>,
    queue: Arc<Queue>,
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

fn describe(e: LoadPdfError) -> anyhow::Error {
    match e {
        LoadPdfError::Decryption(_) => anyhow!("this PDF is password-protected"),
        LoadPdfError::Invalid => anyhow!("this PDF is damaged or not a PDF"),
    }
}

impl Document {
    /// Opens `path` and starts its render threads. Blocking.
    #[cfg(test)]
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        Self::from_bytes(path, bytes)
    }

    /// Opens `path` with its form, and (with `markup`) its editable annotations. What Lumen
    /// shows itself (form inputs, annotations being edited) is hidden from the renderer. Blocking.
    pub fn open_full(path: &Path, markup: bool) -> Result<Opened> {
        let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        Self::full_from_bytes(path, bytes, markup)
    }

    fn full_from_bytes(path: &Path, bytes: Vec<u8>, markup: bool) -> Result<Opened> {
        let (mut form, imported) = {
            let pdf = Pdf::new(Arc::new(bytes.clone())).map_err(describe)?;
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
            bytes
        } else {
            match annotate::pdf::hide(&bytes, &hidden) {
                Ok(shown) => shown,
                // Markup cannot work without it; a form can still be shown, just not filled in.
                Err(e) if imported.is_none() => {
                    tracing::warn!("the form of {} cannot be filled in: {e:#}", path.display());
                    form = Form::default();
                    bytes
                }
                Err(e) => return Err(e),
            }
        };
        Ok(Opened { doc: Self::from_bytes(path, shown)?, imported, form: Arc::new(form) })
    }

    /// Writes `changes` into the PDF at `source` and saves the result to `target` (which may
    /// be `source`), then opens it again. Blocking.
    pub fn save_changes(source: &Path, target: &Path, changes: &Changes<'_>, markup: bool) -> Result<Opened> {
        let bytes = Self::with_changes(source, changes)?;
        write_atomic(target, &bytes)?;
        Self::full_from_bytes(target, bytes, markup)
    }

    /// The bytes of the PDF at `source` with `changes` written in. Blocking.
    pub fn with_changes(source: &Path, changes: &Changes<'_>) -> Result<Vec<u8>> {
        let bytes = std::fs::read(source).with_context(|| format!("cannot read {}", source.display()))?;
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
        catch_unwind(AssertUnwindSafe(|| annotate::pdf::save(&bytes, &edits, &fills)))
            .map_err(|_| anyhow!("the PDF could not be written"))?
    }

    fn from_bytes(path: &Path, bytes: Vec<u8>) -> Result<Self> {
        let bytes = Arc::new(bytes);
        let pdf = Pdf::new(bytes.clone()).map_err(describe)?;
        let pages: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
        if pages.is_empty() {
            bail!("this PDF has no pages");
        }
        // Outline and links are cheap to read; a malformed outline must not stop the document opening.
        let (outline, links) = catch_unwind(AssertUnwindSafe(|| structure::read(&pdf))).unwrap_or_else(|_| {
            tracing::warn!("could not read the outline or links of {}", path.display());
            (Vec::new(), vec![Vec::new(); pages.len()])
        });
        drop(pdf);

        let queue = Arc::new(Queue::default());
        let threads =
            std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(2).clamp(1, 4).min(pages.len());
        for i in 0..threads {
            let queue = queue.clone();
            let bytes = bytes.clone();
            std::thread::Builder::new()
                .name(format!("pdf-render-{i}"))
                .spawn(move || worker(bytes, &queue))
                .context("cannot start a render thread")?;
        }

        let text = Mutex::new(vec![None; pages.len()]);
        Ok(Self { inner: Arc::new(Inner { path: path.to_owned(), pages, outline, links, text, queue }) })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
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

    /// Renders `page` at `scale` pixels per point, off the UI thread.
    pub async fn render(&self, page: usize, scale: f32) -> Result<Picture> {
        let (reply, rx) = oneshot::channel();
        self.inner.queue.push(Job { page, work: Work::Render { scale, reply } });
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

fn worker(bytes: Arc<Vec<u8>>, queue: &Queue) {
    let Ok(pdf) = Pdf::new(bytes) else { return };
    let pages = pdf.pages();
    let cache = RenderCache::new();
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
            Work::Render { scale, reply } => {
                let (w, h) = page.render_dimensions();
                // Keep the bitmap within limits whatever the zoom.
                let scale = scale.min(MAX_SIDE / w.max(h).max(1.0)).max(0.01);
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
                    let rgba = pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha);
                    Picture { width, height, original: (w.round() as u32, h.round() as u32), rgba: rgba.into() }
                });
                let _ = reply.send(result);
            }
        }
    }
}

/// Replaces `path` with `bytes` all at once: a crash half-way leaves the old file, not half a new one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.lumen-tmp"));
    let result = (|| -> Result<()> {
        let mut file = std::fs::File::create(&temp).context("cannot write there")?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&temp, meta.permissions());
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.with_context(|| format!("cannot save {}", path.display()))
}

/// A first-page thumbnail for the file strip. Blocking.
pub fn thumbnail(path: &Path, side: u32) -> Result<Picture> {
    let bytes = Arc::new(std::fs::read(path)?);
    let pdf = Pdf::new(bytes).map_err(describe)?;
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
mod tests {
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

    #[test]
    fn rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.pdf");
        std::fs::write(&path, b"%PDF-1.7 nonsense").unwrap();
        assert!(Document::open(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two pages: text, a link to page 2, a web link, and a two-entry outline.
    fn structured_pdf() -> PathBuf {
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
        let path = std::env::temp_dir().join(format!("lumen-rs-structured-{}.pdf", std::process::id()));
        std::fs::write(&path, w.finish()).unwrap();
        path
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

    #[test]
    fn thumbnails() {
        let t = thumbnail(&fixture(), 128).unwrap();
        assert_eq!(t.width.max(t.height), 128);
    }
}
