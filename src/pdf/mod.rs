//! PDF documents, via Poppler.
//!
//! Poppler objects are not thread-safe, so nothing here is shared between
//! threads: every function opens the file itself and returns plain data.

pub mod annots;
mod cache;
pub mod compose;
pub mod forms;
pub mod layout;
mod links;
mod outline;
pub mod pageops;
mod renderer;
mod search;
mod selection;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use relm4::gtk::{cairo, gdk, gio, glib, prelude::*};

pub use cache::PageCache;
pub use forms::{FieldKind, FieldValue, FormField};
pub use layout::PageLayout;
pub use links::{Link, LinkTarget, LinkWorker, LinksLoaded, NamedAction, is_safe_uri};
pub use outline::{OutlineItem, OutlineTarget};
pub use renderer::{PageRenderer, RenderJob};
pub use search::{HitRect, SearchEvent, SearchOptions, SearchWorker};
pub use selection::{Granularity, PagePoint, SelectRequest, SelectionResult, SelectionWorker};

/// Rendering is capped so a high zoom on a huge page cannot exhaust memory.
const MAX_DIMENSION: f64 = 12_000.0;
const MAX_PIXELS: f64 = 32_000_000.0;

/// What is known about a document without rendering it.
#[derive(Debug, Clone)]
pub struct PdfInfo {
    pub n_pages: usize,
    /// Size of every page in points (1/72 inch), already rotated.
    pub page_sizes: Vec<(f64, f64)>,
    /// Table of contents; empty if the document has none.
    pub outline: Vec<std::sync::Arc<OutlineItem>>,
    /// The password that opened it, so workers can open it too.
    pub password: Option<String>,
    /// Annotations the file already holds that Lumen can edit, per page.
    pub annotations: std::collections::BTreeMap<usize, crate::annotate::AnnotationLayer>,
    pub metadata: PdfMetadata,
    /// Form fields, per page (only pages that have some).
    pub forms: std::collections::BTreeMap<usize, Vec<FormField>>,
}

/// What the document says about itself, for the properties panel. Fields the file
/// leaves empty are `None`.
#[derive(Debug, Clone, Default)]
pub struct PdfMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    /// Already formatted for the user's locale.
    pub created: Option<String>,
    pub version: Option<String>,
}

impl PdfMetadata {
    fn read(doc: &poppler::Document) -> Self {
        let text = |value: Option<glib::GString>| {
            value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
        };
        Self {
            title: text(doc.title()),
            author: text(doc.author()),
            subject: text(doc.subject()),
            creator: text(doc.creator()),
            producer: text(doc.producer()),
            created: doc
                .creation_date_time()
                .and_then(|date| date.to_local().ok()?.format("%x %X").ok())
                .map(String::from),
            version: text(doc.pdf_version_string()),
        }
    }
}

/// A rendered page: premultiplied BGRA pixels, safe to move between threads.
#[derive(Debug)]
pub struct RenderedPage {
    width: i32,
    height: i32,
    stride: usize,
    data: Vec<u8>,
}

impl RenderedPage {
    pub fn into_texture(self) -> gdk::Texture {
        gdk::MemoryTexture::new(
            self.width,
            self.height,
            // Cairo's ARGB32 is native-endian; this is its byte order on little-endian.
            gdk::MemoryFormat::B8g8r8a8Premultiplied,
            &glib::Bytes::from_owned(self.data),
            self.stride,
        )
        .upcast()
    }
}

/// Poppler is not safe to call from several threads at once, even for different
/// documents (font and glyph caches are shared), so every use holds this lock.
/// Never take it while already holding it.
fn poppler_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A panic in another Poppler call does not make the library state unusable.
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A document a worker thread opens only when its first job arrives, so a worker that is
/// never needed (no search, no thumbnails) never parses the file.
struct LazyDoc {
    source: PdfSource,
    doc: Option<Result<poppler::Document, String>>,
}

impl LazyDoc {
    fn new(source: &PdfSource) -> Self {
        Self {
            source: source.clone(),
            doc: None,
        }
    }

    /// The document, opened now if it was not yet. Takes the Poppler lock while opening, so
    /// call it without holding the lock.
    fn get(&mut self) -> Result<&poppler::Document, String> {
        let source = &self.source;
        self.doc
            .get_or_insert_with(|| {
                let _poppler = poppler_lock();
                open_document(source).map_err(|err| format!("{err:#}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

/// A PDF file and the password that unlocks it, if it has one.
#[derive(Debug, Clone)]
pub struct PdfSource {
    pub path: PathBuf,
    pub password: Option<String>,
}

impl PdfSource {
    pub fn new(path: &Path, password: Option<&str>) -> Self {
        Self {
            path: path.to_owned(),
            password: password.map(str::to_owned),
        }
    }
}

/// Why a PDF could not be loaded.
#[derive(Debug)]
pub enum PdfError {
    /// Encrypted, and the password is missing or wrong.
    PasswordRequired,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for PdfError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

fn open_pdf(source: &PdfSource) -> Result<poppler::Document, PdfError> {
    let uri = gio::File::for_path(&source.path).uri();
    poppler::Document::from_file(&uri, source.password.as_deref()).map_err(|err| {
        if err.matches(poppler::Error::Encrypted) {
            PdfError::PasswordRequired
        } else {
            PdfError::Other(anyhow::Error::new(err).context("failed to open PDF"))
        }
    })
}

/// For worker threads, which only care that opening failed.
fn open_document(source: &PdfSource) -> Result<poppler::Document> {
    open_pdf(source).map_err(|err| match err {
        PdfError::PasswordRequired => anyhow::anyhow!("the PDF is password protected"),
        PdfError::Other(err) => err,
    })
}

/// Blocking: call it off the main thread.
pub fn load_info(source: &PdfSource) -> Result<PdfInfo, PdfError> {
    let _poppler = poppler_lock();
    let doc = open_pdf(source)?;
    let n_pages = doc.n_pages().max(0) as usize;
    if n_pages == 0 {
        return Err(anyhow::anyhow!("the PDF has no pages").into());
    }
    let page_sizes: Vec<(f64, f64)> = (0..n_pages)
        .map(|i| {
            doc.page(i as i32)
                .map_or((612.0, 792.0), |page| page.size())
        })
        .collect();
    let outline = outline::read_outline(&doc, &page_sizes);
    let annotations = annots::read_all(&doc, &page_sizes);
    Ok(PdfInfo {
        n_pages,
        page_sizes,
        outline,
        password: source.password.clone(),
        annotations,
        metadata: PdfMetadata::read(&doc),
        forms: forms::read_all(&doc),
    })
}

/// The scale a page of `size` points is actually rendered at when `scale` is
/// asked for: lower if that would exceed the size caps.
pub fn effective_scale(size: (f64, f64), scale: f64) -> f64 {
    let (w, h) = size;
    scale
        .min(MAX_DIMENSION / w.max(h))
        .min((MAX_PIXELS / (w * h)).sqrt())
}

/// Render the first page of the PDF at `path` to fit in `size` × `size` pixels, as
/// other readers show it (with its annotations). For file thumbnails; blocking.
pub fn render_thumbnail(path: &Path, size: i32) -> Result<RenderedPage> {
    let _poppler = poppler_lock();
    let doc = open_document(&PdfSource::new(path, None))?;
    let page = doc.page(0).context("the PDF has no pages")?;
    let (w, h) = page.size();
    let scale = effective_scale((w, h), size as f64 / w.max(h));
    let (pixel_w, pixel_h) = ((w * scale).ceil() as i32, (h * scale).ceil() as i32);
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, pixel_w, pixel_h)
        .context("cannot allocate the thumbnail")?;
    {
        let cr = cairo::Context::new(&surface)?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint()?;
        cr.scale(pixel_w as f64 / w, pixel_h as f64 / h);
        page.render(&cr);
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data()?.to_vec();
    Ok(RenderedPage {
        width: pixel_w,
        height: pixel_h,
        stride,
        data,
    })
}

/// Render page `index` at `scale` device pixels per point. Blocking.
fn render_page(doc: &poppler::Document, index: usize, scale: f64) -> Result<RenderedPage> {
    let page = doc
        .page(index as i32)
        .with_context(|| format!("page {} does not exist", index + 1))?;
    // Lumen draws the annotations it edits itself, on top of the page.
    annots::hide_supported(&page);
    let (w, h) = page.size();

    let scale = effective_scale((w, h), scale);
    let (pixel_w, pixel_h) = ((w * scale).ceil() as i32, (h * scale).ceil() as i32);

    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, pixel_w, pixel_h)
        .context("cannot allocate page surface")?;
    {
        let cr = cairo::Context::new(&surface)?;
        // PDF pages are transparent; paper is white.
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint()?;
        cr.scale(pixel_w as f64 / w, pixel_h as f64 / h);
        page.render(&cr);
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data()?.to_vec();

    Ok(RenderedPage {
        width: pixel_w,
        height: pixel_h,
        stride,
        data,
    })
}

/// A PDF opened for printing. Lives on the main thread, because printing draws
/// from GTK's draw-page signal; Poppler calls still take the global lock.
pub struct PrintDocument(poppler::Document);

impl PrintDocument {
    pub fn open(source: &PdfSource) -> Result<Self> {
        let _poppler = poppler_lock();
        open_document(source).map(Self)
    }

    /// Draw page `index` at its natural size (1 point = 1 unit) onto `cr`.
    pub fn render_for_printing(&self, index: usize, cr: &cairo::Context) -> Result<()> {
        let _poppler = poppler_lock();
        let page = self
            .0
            .page(index as i32)
            .with_context(|| format!("page {} does not exist", index + 1))?;
        // Lumen prints its own annotations over the page, so Poppler must not draw them too.
        annots::hide_supported(&page);
        page.render_for_printing(cr);
        Ok(())
    }
}

/// Render page `index` at `scale` device pixels per point, with `layer`'s annotations
/// drawn over it, and write it to `dest` as a PNG. Blocking.
pub fn export_png(
    source: &PdfSource,
    index: usize,
    scale: f64,
    layer: Option<&crate::annotate::AnnotationLayer>,
    dest: &Path,
) -> Result<()> {
    let _poppler = poppler_lock();
    let doc = open_document(source)?;
    let page = doc
        .page(index as i32)
        .with_context(|| format!("page {} does not exist", index + 1))?;
    annots::hide_supported(&page);
    let (w, h) = page.size();
    let scale = effective_scale((w, h), scale);
    let (pixel_w, pixel_h) = ((w * scale).ceil() as i32, (h * scale).ceil() as i32);

    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, pixel_w, pixel_h)
        .context("cannot allocate the page")?;
    {
        let cr = cairo::Context::new(&surface)?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint()?;
        cr.scale(pixel_w as f64 / w, pixel_h as f64 / h);
        page.render(&cr);
        if let Some(layer) = layer {
            crate::annotate::render::draw_layer(&cr, layer);
        }
    }
    let mut file =
        std::fs::File::create(dest).with_context(|| format!("cannot create {}", dest.display()))?;
    surface
        .write_to_png(&mut file)
        .context("cannot write the PNG")?;
    Ok(())
}
