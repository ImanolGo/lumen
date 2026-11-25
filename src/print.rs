//! Printing through the XDG desktop portal, or straight to CUPS without one.
//!
//! The portal shows the desktop's own print dialog (GNOME, KDE, GTK…) and hands
//! the job to CUPS, so this works the same everywhere, including in a Flatpak.
//! Without a portal, Lumen shows its own small dialog and sends the job to CUPS
//! itself (see `cups.rs`). PDFs are sent as they are; pictures are first wrapped
//! into a one-page PDF sized to the paper chosen.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};

use crate::library::Kind;
use crate::raster;

/// A connection to a printing service.
pub trait Connection: std::io::Read + std::io::Write {}
impl<T: std::io::Read + std::io::Write> Connection for T {}

/// What is printed: a file, or a copy made for printing that is deleted when nothing uses it any more.
#[derive(Debug, Clone)]
pub struct Job {
    pub path: PathBuf,
    _copy: Option<Arc<PrivateCopy>>,
}

impl Job {
    pub fn file(path: PathBuf) -> Self {
        Self { path, _copy: None }
    }

    /// A copy called `name`, in a folder only this user can look into, with what `write` puts there.
    pub fn copy(name: &str, write: impl FnOnce(&Path) -> Result<()>) -> Result<Self> {
        let copy = PrivateCopy(crate::fsio::private_dir("lumen-rs-print").context("no place for the copy to print")?);
        COPIES.lock().unwrap_or_else(|e| e.into_inner()).push(copy.0.clone());
        let path = copy.0.join(name);
        write(&path)?;
        Ok(Self { path, _copy: Some(Arc::new(copy)) })
    }
}

#[derive(Debug)]
struct PrivateCopy(PathBuf);

impl Drop for PrivateCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
        COPIES.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| *p != self.0);
    }
}

static COPIES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Deletes the copies still there, when Lumen ends.
pub fn delete_copies() {
    for dir in COPIES.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code, reason = "until docs/improvement-plan.md package 7"))]
pub enum Outcome {
    Sent,
    Cancelled,
    /// No print portal answered: print with Lumen's own dialog instead.
    NoPortal,
}

pub const MM_TO_PT: f32 = 72.0 / 25.4;
/// Pictures are resampled to at most this resolution on paper.
const PRINT_DPI: f32 = 300.0;

pub fn title_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Document".into())
}

/// A sheet of paper: its size and margins (top, right, bottom, left) in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    pub width: f32,
    pub height: f32,
    pub margins: [f32; 4],
}

/// The margin printers can usually reach, a quarter of an inch.
pub const MARGIN: f32 = 6.35 * MM_TO_PT;

impl Paper {
    /// A sheet of `width`×`height` points with the usual margins.
    pub fn sized(width: f32, height: f32) -> Self {
        Self { width, height, margins: [MARGIN; 4] }
    }
}

/// Builds a one-page PDF with the picture centred and fitted inside the margins.
pub fn picture_to_pdf(path: &Path, paper: &Paper) -> Result<Vec<u8>> {
    image_to_pdf(raster::load_for_print(path)?, paper)
}

/// A one-page PDF exactly as big as the picture is at `dpi`, with no margins (for exporting
/// rather than printing: nothing is cropped, shrunk or surrounded by white).
pub fn picture_to_pdf_native(path: &Path, dpi: f32) -> Result<Vec<u8>> {
    let img = raster::load_for_print(path)?;
    let (w, h) = (img.width() as f32 * 72.0 / dpi, img.height() as f32 * 72.0 / dpi);
    image_to_pdf(img, &Paper { width: w, height: h, margins: [0.0; 4] })
}

fn image_to_pdf(mut img: image::RgbImage, paper: &Paper) -> Result<Vec<u8>> {
    let (page_w, page_h, [top, right, bottom, left]) = (paper.width, paper.height, paper.margins);
    let (box_w, box_h) = (page_w - left - right, page_h - top - bottom);
    // Margins that leave no room on the paper are not margins anyone meant.
    let (box_w, left) = if box_w > 0.0 { (box_w, left) } else { (page_w, 0.0) };
    let (box_h, bottom) = if box_h > 0.0 { (box_h, bottom) } else { (page_h, 0.0) };

    // Fit the printable area, keeping the aspect ratio.
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let scale = (box_w / iw).min(box_h / ih);
    let (draw_w, draw_h) = (iw * scale, ih * scale);

    // Resample down to PRINT_DPI at the printed size: smaller file, same output.
    let max_px_w = (draw_w / 72.0 * PRINT_DPI).ceil() as u32;
    if img.width() > max_px_w.max(1) {
        let max_px_h = (draw_h / 72.0 * PRINT_DPI).ceil() as u32;
        img = image::imageops::resize(&img, max_px_w, max_px_h.max(1), image::imageops::FilterType::Lanczos3);
    }

    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 92).encode_image(&img)?;

    let (catalog, tree, page_id, image_id, content_id) =
        (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4), Ref::new(5));
    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids([page_id]).count(1);
    let mut page = pdf.page(page_id);
    page.media_box(Rect::new(0.0, 0.0, page_w, page_h));
    page.parent(tree);
    page.contents(content_id);
    page.resources().x_objects().pair(Name(b"Im0"), image_id);
    page.finish();

    let mut xobject = pdf.image_xobject(image_id, &jpeg);
    xobject.filter(Filter::DctDecode);
    xobject.width(img.width() as i32);
    xobject.height(img.height() as i32);
    xobject.color_space().device_rgb();
    xobject.bits_per_component(8);
    xobject.finish();

    let x = left + (box_w - draw_w) / 2.0;
    let y = bottom + (box_h - draw_h) / 2.0;
    let mut content = Content::new();
    content.save_state();
    content.transform([draw_w, 0.0, 0.0, draw_h, x, y]);
    content.x_object(Name(b"Im0"));
    content.restore_state();
    pdf.stream(content_id, &content.finish());
    Ok(pdf.finish())
}

/// Prints `path` on `printer` through CUPS, without a portal. Pictures are put on the
/// job's paper (A4 when it is not known). Blocking.
pub fn print_direct(path: &Path, printer: &str, job: &crate::cups::Job) -> Result<()> {
    let bytes = if Kind::of(path) == Some(Kind::Pdf) {
        std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?
    } else {
        let (w, h) =
            job.media.as_deref().and_then(crate::cups::media_size).unwrap_or((210.0 * MM_TO_PT, 297.0 * MM_TO_PT));
        picture_to_pdf(path, &Paper::sized(w, h))?
    };
    crate::cups::print(printer, &title_of(path), job, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn a_copy_for_printing_is_private_and_goes_away() {
        use std::os::unix::fs::PermissionsExt;
        let job = Job::copy("secret.pdf", |p| Ok(crate::fsio::write_new(p, b"decrypted")?)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = job.path.parent().unwrap().to_owned();
        assert_eq!((mode(&job.path), mode(&dir)), (0o600, 0o700));
        let (path, other) = (job.path.clone(), job.clone());
        drop(job);
        assert!(path.exists(), "still in use");
        drop(other);
        assert!(!dir.exists());
        // Also when writing it fails, and at the end.
        let mut left = PathBuf::new();
        assert!(
            Job::copy("x", |p| {
                left = p.to_owned();
                anyhow::bail!("no")
            })
            .is_err()
        );
        assert!(!left.parent().unwrap().exists());
    }

    #[test]
    fn a_small_picture_is_not_cropped() {
        let banner =
            image::RgbImage::from_fn(1000, 50, |_, y| image::Rgb(if y < 12 { [255, 0, 0] } else { [0, 0, 255] }));
        let dir = std::env::temp_dir().join(format!("lumen-rs-banner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.png");
        banner.save(&path).unwrap();
        let bytes = picture_to_pdf_native(&path, 150.0).unwrap();
        let pdf = hayro::hayro_syntax::Pdf::new(bytes.clone()).unwrap();
        let (w, h) = pdf.pages()[0].render_dimensions();
        assert!((w - 480.0).abs() < 0.1 && (h - 24.0).abs() < 0.1, "{w}×{h}");
        // The picture is drawn at exactly the page's size.
        let content = String::from_utf8_lossy(&bytes).into_owned();
        assert!(content.contains("480 0 0 24 0 0 cm"), "{content}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wraps_a_picture_in_a_valid_pdf() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-print-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        image::RgbaImage::from_pixel(1200, 800, image::Rgba([200, 0, 0, 128])).save(&path).unwrap();

        let bytes = picture_to_pdf(&path, &Paper::sized(210.0 * MM_TO_PT, 297.0 * MM_TO_PT)).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        // The result must be readable by our own PDF engine, and be A4.
        let doc = hayro::hayro_syntax::Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        let (w, h) = doc.pages()[0].render_dimensions();
        assert!((w - 595.3).abs() < 1.0 && (h - 841.9).abs() < 1.0, "{w}x{h}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
