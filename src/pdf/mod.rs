//! PDF documents, via Poppler.
//!
//! Poppler objects are not thread-safe, so nothing here is shared between
//! threads: every function opens the file itself and returns plain data.

mod cache;
pub mod layout;
mod outline;
mod renderer;

use std::path::Path;

use anyhow::{Context, Result};
use relm4::gtk::{cairo, gdk, gio, glib, prelude::*};

pub use cache::PageCache;
pub use layout::PageLayout;
pub use outline::{OutlineItem, OutlineTarget};
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

fn open_document(path: &Path) -> Result<poppler::Document> {
    let uri = gio::File::for_path(path).uri();
    poppler::Document::from_file(&uri, None).context("failed to open PDF")
}

/// Blocking: call it off the main thread.
pub fn load_info(path: &Path) -> Result<PdfInfo> {
    let doc = open_document(path)?;
    let n_pages = doc.n_pages().max(0) as usize;
    anyhow::ensure!(n_pages > 0, "the PDF has no pages");
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
