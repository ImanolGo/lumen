//! PDF documents, rendered by `hayro` (pure Rust, `#![forbid(unsafe_code)]`).
//!
//! A document owns a small pool of render threads. Each thread parses its own
//! copy of the document (sharing the file bytes), because hayro's render cache
//! is single-threaded. Requests are served newest-first so that, while the user
//! scrolls quickly, the pages they are looking at now win over stale ones.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use tokio::sync::oneshot;

use crate::raster::Picture;

/// Largest bitmap we render for a single page, in pixels per side.
const MAX_SIDE: f32 = 8192.0;

struct Job {
    page: usize,
    scale: f32,
    reply: oneshot::Sender<Result<Picture>>,
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
                if !job.reply.is_closed() {
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

/// An open PDF. Cheap to clone; the render threads stop when the last clone drops.
#[derive(Clone)]
pub struct Document {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    /// Page sizes in PDF points, after applying /Rotate.
    pages: Vec<(f32, f32)>,
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
    pub fn open(path: &Path) -> Result<Self> {
        let bytes: Arc<Vec<u8>> =
            Arc::new(std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?);
        let pdf = Pdf::new(bytes.clone()).map_err(describe)?;
        let pages: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
        if pages.is_empty() {
            bail!("this PDF has no pages");
        }
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

        Ok(Self { inner: Arc::new(Inner { path: path.to_owned(), pages, queue }) })
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

    /// Renders `page` at `scale` pixels per point, off the UI thread.
    pub async fn render(&self, page: usize, scale: f32) -> Result<Picture> {
        let (reply, rx) = oneshot::channel();
        self.inner.queue.push(Job { page, scale, reply });
        rx.await.map_err(|_| anyhow!("the document was closed"))?
    }
}

fn worker(bytes: Arc<Vec<u8>>, queue: &Queue) {
    let Ok(pdf) = Pdf::new(bytes) else { return };
    let pages = pdf.pages();
    let cache = RenderCache::new();
    let interpreter = InterpreterSettings::default();
    let settings = RenderSettings::default();

    while let Some(job) = queue.pop() {
        let result = match pages.get(job.page) {
            None => Err(anyhow!("no such page")),
            Some(page) => {
                let (w, h) = page.render_dimensions();
                // Keep the bitmap within limits whatever the zoom.
                let scale = job.scale.min(MAX_SIDE / w.max(h).max(1.0)).max(0.01);
                let pixmap_settings = PixmapSettings {
                    x_scale: scale,
                    y_scale: scale,
                    bg_color: hayro::vello_cpu::color::palette::css::WHITE,
                };
                // A malformed page must never take the viewer down with it.
                catch_unwind(AssertUnwindSafe(|| {
                    hayro::render(page, &cache, &interpreter, &settings, &pixmap_settings)
                }))
                .map_err(|_| anyhow!("page {} could not be rendered", job.page + 1))
                .map(|pixmap| {
                    let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
                    let rgba = pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha);
                    Picture { width, height, original: (w.round() as u32, h.round() as u32), rgba: rgba.into() }
                })
            }
        };
        let _ = job.reply.send(result);
    }
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

    #[test]
    fn thumbnails() {
        let t = thumbnail(&fixture(), 128).unwrap();
        assert_eq!(t.width.max(t.height), 128);
    }
}
