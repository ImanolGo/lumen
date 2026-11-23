//! Printing through the XDG desktop portal, or straight to CUPS without one.
//!
//! The portal shows the desktop's own print dialog (GNOME, KDE, GTK…) and hands
//! the job to CUPS, so this works the same everywhere, including in a Flatpak.
//! Without a portal, Lumen shows its own small dialog and sends the job to CUPS
//! itself (see `cups.rs`). PDFs are sent as they are; pictures are first wrapped
//! into a one-page PDF sized to the paper chosen.

use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ashpd::desktop::ResponseError;
use ashpd::desktop::print::{Orientation, PageSetup, PreparePrintOptions, PrintOptions, PrintProxy, Settings};
use image::codecs::jpeg::JpegEncoder;
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};

use crate::library::Kind;
use crate::raster;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    Cancelled,
    /// No print portal answered: print with Lumen's own dialog instead.
    NoPortal,
}

const MM_TO_PT: f32 = 72.0 / 25.4;
/// Pictures are resampled to at most this resolution on paper.
const PRINT_DPI: f32 = 300.0;

fn cancelled(e: &ashpd::Error) -> bool {
    matches!(e, ashpd::Error::Response(ResponseError::Cancelled))
}

fn title_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Document".into())
}

/// Shows the desktop's print dialog for `path` and, if confirmed, prints it.
pub async fn print(path: PathBuf) -> Result<Outcome> {
    if crate::portal::disabled() {
        return Ok(Outcome::NoPortal);
    }
    let title = title_of(&path);
    let Ok(proxy) = PrintProxy::new().await else { return Ok(Outcome::NoPortal) };
    let has_pages = Kind::of(&path) == Some(Kind::Pdf);
    let request = proxy
        .prepare_print(
            None,
            &title,
            Settings::default(),
            PageSetup::default(),
            PreparePrintOptions::default().set_modal(true).set_accept_label("Print").set_has_current_page(has_pages),
        )
        .await;
    let prepared = match request.and_then(|r| r.response()) {
        Ok(p) => p,
        Err(e) if cancelled(&e) => return Ok(Outcome::Cancelled),
        // The portal is there but cannot print (no backend for it): print without it.
        Err(e) => {
            tracing::info!("the print portal failed ({e}); printing directly");
            return Ok(Outcome::NoPortal);
        }
    };

    let token = prepared.token;
    let file = if has_pages {
        File::open(&path).with_context(|| format!("cannot read {}", path.display()))?
    } else {
        let paper = Paper::from_setup(&prepared.page_setup);
        let src = path.clone();
        tokio::task::spawn_blocking(move || picture_to_pdf_file(&src, &paper))
            .await
            .context("print preparation stopped")??
    };

    let sent = proxy
        .print(None, &title, &file.as_fd(), PrintOptions::default().set_token(token).set_modal(true))
        .await
        .and_then(|r| r.response());
    match sent {
        Ok(()) => Ok(Outcome::Sent),
        Err(e) if cancelled(&e) => Ok(Outcome::Cancelled),
        Err(e) => Err(e).context("the print job failed"),
    }
}

/// A sheet of paper: its size and margins (top, right, bottom, left) in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    pub width: f32,
    pub height: f32,
    pub margins: [f32; 4],
}

/// The margin printers can usually reach, a quarter of an inch.
const MARGIN: f32 = 6.35 * MM_TO_PT;

impl Paper {
    /// A sheet of `width`×`height` points with the usual margins.
    pub fn sized(width: f32, height: f32) -> Self {
        Self { width, height, margins: [MARGIN; 4] }
    }

    /// The paper chosen in the portal's dialog, with orientation applied. Defaults to A4.
    fn from_setup(setup: &PageSetup) -> Self {
        let mm = |v: Option<f64>, d: f32| v.map(|v| v as f32).filter(|v| *v > 0.0).unwrap_or(d) * MM_TO_PT;
        let (mut w, mut h) = (mm(setup.width, 210.0), mm(setup.height, 297.0));
        let landscape = matches!(setup.orientation, Some(Orientation::Landscape) | Some(Orientation::ReverseLandscape));
        if landscape && w < h || !landscape && w > h && setup.orientation.is_some() {
            std::mem::swap(&mut w, &mut h);
        }
        let margin = |v: Option<f64>| v.map(|v| v as f32 * MM_TO_PT).filter(|v| *v >= 0.0).unwrap_or(MARGIN);
        let margins = [
            margin(setup.margin_top),
            margin(setup.margin_right),
            margin(setup.margin_bottom),
            margin(setup.margin_left),
        ];
        Self { width: w, height: h, margins }
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
    let (box_w, box_h) = ((page_w - left - right).max(36.0), (page_h - top - bottom).max(36.0));

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

/// Writes the picture's PDF to a private temporary file and returns it open,
/// already unlinked, so nothing is left behind whatever happens next.
fn picture_to_pdf_file(path: &Path, paper: &Paper) -> Result<File> {
    let bytes = picture_to_pdf(path, paper)?;
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!("lumen-rs-print-{}-{nanos}.pdf", std::process::id()));
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    let file = File::open(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Ok(file)
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
    fn wraps_a_picture_in_a_valid_pdf() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-print-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        image::RgbaImage::from_pixel(1200, 800, image::Rgba([200, 0, 0, 128])).save(&path).unwrap();

        let bytes = picture_to_pdf(&path, &Paper::from_setup(&PageSetup::default())).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        // The result must be readable by our own PDF engine, and be A4.
        let doc = hayro::hayro_syntax::Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        let (w, h) = doc.pages()[0].render_dimensions();
        assert!((w - 595.3).abs() < 1.0 && (h - 841.9).abs() < 1.0, "{w}x{h}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
