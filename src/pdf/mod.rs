//! PDF documents, via Poppler.
//!
//! Poppler objects are not thread-safe, so nothing here is shared between
//! threads: every function opens the file itself and returns plain data.

mod cache;
pub mod layout;
mod links;
mod outline;
mod search;
mod selection;
mod renderer;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use relm4::gtk::{cairo, gdk, gio, glib, prelude::*};

pub use cache::PageCache;
pub use layout::PageLayout;
pub use links::{Link, LinkTarget, LinkWorker, LinksLoaded, NamedAction, is_safe_uri};
pub use outline::{OutlineItem, OutlineTarget};
pub use search::{HitRect, SearchEvent, SearchWorker};
pub use selection::{Granularity, PagePoint, SelectRequest, SelectionResult, SelectionWorker};
pub use renderer::{PageRenderer, RenderJob};

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
    Ok(PdfInfo {
        n_pages,
        page_sizes,
        outline,
        password: source.password.clone(),
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

/// Render page `index` at `scale` device pixels per point. Blocking.
fn render_page(doc: &poppler::Document, index: usize, scale: f64) -> Result<RenderedPage> {
    let page = doc
        .page(index as i32)
        .with_context(|| format!("page {} does not exist", index + 1))?;
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
