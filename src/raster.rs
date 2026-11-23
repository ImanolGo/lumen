//! Decoding pictures (raster and SVG) into RGBA, plus fast thumbnails.
//!
//! Everything here is blocking and CPU-bound; callers run it on a worker thread.
//! All decoders are pure, memory-safe Rust (`image`, `resvg`), and decoding
//! runs under allocation limits so a hostile file cannot exhaust memory.

use std::io::{BufReader, Cursor};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use bytes::Bytes;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, RgbaImage, metadata::Orientation};
use resvg::{tiny_skia, usvg};

use crate::library::Kind;

/// Longest side we keep in memory for display. Bigger pictures are downscaled.
const MAX_SIDE: u32 = 16_384;
/// Hard cap for a single decode allocation (1 GiB).
const MAX_ALLOC: u64 = 1 << 30;
/// Thumbnails are this many pixels on their longest side.
pub const THUMB_SIDE: u32 = 256;

/// A decoded picture, ready to upload: straight (not premultiplied) RGBA8.
#[derive(Debug, Clone)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Size of the original file's pixels (before any downscaling).
    pub original: (u32, u32),
    /// Shared and immutable: handing it to the renderer or a cache never copies the pixels.
    pub rgba: Bytes,
}

impl Picture {
    fn from_rgba(img: RgbaImage, original: (u32, u32)) -> Self {
        let (width, height) = img.dimensions();
        Self { width, height, original, rgba: img.into_raw().into() }
    }
}

/// Decodes `path` for display, honouring EXIF orientation.
pub fn load(path: &Path) -> Result<Picture> {
    match Kind::of(path) {
        Some(Kind::Svg) => load_svg(path, 2048),
        Some(Kind::Image) => load_raster(path),
        _ => bail!("not a picture"),
    }
}

/// Full-resolution pixels for printing, oriented, alpha flattened onto white.
pub fn load_for_print(path: &Path) -> Result<image::RgbImage> {
    let rgba: RgbaImage = match Kind::of(path) {
        Some(Kind::Svg) => {
            let pic = load_svg(path, 3000)?;
            let rgba = Vec::from(pic.rgba);
            RgbaImage::from_raw(pic.width, pic.height, rgba).context("bad SVG raster")?
        }
        Some(Kind::Image) => decode(path)?.into_rgba8(),
        _ => bail!("not a picture"),
    };
    let (w, h) = rgba.dimensions();
    let mut rgb = image::RgbImage::new(w, h);
    for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
        let a = src[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        *dst = image::Rgb([blend(src[0]), blend(src[1]), blend(src[2])]);
    }
    Ok(rgb)
}

/// Pixel count of a picture from its header alone (SVGs count as small).
pub fn pixel_count(path: &Path) -> Option<u64> {
    if Kind::of(path) != Some(Kind::Image) {
        return Some(0);
    }
    let (w, h) = ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_dimensions().ok()?;
    Some(w as u64 * h as u64)
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

fn decode(path: &Path) -> Result<DynamicImage> {
    let reader =
        ImageReader::open(path).with_context(|| format!("cannot open {}", path.display()))?.with_guessed_format()?;
    let mut reader = reader;
    reader.limits(limits());
    let mut decoder = reader.into_decoder().context("unsupported picture format")?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).context("the picture is damaged")?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn load_raster(path: &Path) -> Result<Picture> {
    let img = decode(path)?;
    let original = (img.width(), img.height());
    let img = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(Picture::from_rgba(img.into_rgba8(), original))
}

pub(crate) fn svg_options() -> usvg::Options<'static> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fontdb = FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone();
    // No resources_dir: an SVG cannot pull in other files from disk.
    usvg::Options { fontdb, ..Default::default() }
}

fn load_svg(path: &Path, target_side: u32) -> Result<Picture> {
    let data = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let tree = usvg::Tree::from_data(&data, &svg_options()).context("the SVG is damaged")?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    let scale = (target_side as f32 / w.max(h)).clamp(0.01, 16.0);
    let pw = ((w * scale).ceil() as u32).clamp(1, MAX_SIDE);
    let ph = ((h * scale).ceil() as u32).clamp(1, MAX_SIDE);
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).context("SVG too large")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let mut rgba = Vec::with_capacity((pw * ph * 4) as usize);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok(Picture { width: pw, height: ph, original: (w.round() as u32, h.round() as u32), rgba: rgba.into() })
}

/// A small preview of a picture. Uses the JPEG's embedded EXIF thumbnail when it
/// has one (camera photos almost always do), which avoids a full decode.
pub fn thumbnail(path: &Path) -> Result<Picture> {
    match Kind::of(path) {
        Some(Kind::Svg) => load_svg(path, THUMB_SIDE),
        Some(Kind::Image) => {
            if let Some(t) = exif_thumbnail(path).or_else(|| jpeg_thumbnail(path)).or_else(|| png_thumbnail(path)) {
                return Ok(t);
            }
            // Full decodes for thumbnails run one at a time, so a folder of big pictures
            // does not hold several decoded copies in memory at once.
            static FULL_DECODE: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _turn = FULL_DECODE.lock().unwrap_or_else(|e| e.into_inner());
            let img = decode(path)?;
            let original = (img.width(), img.height());
            Ok(Picture::from_rgba(img.thumbnail(THUMB_SIDE, THUMB_SIDE).into_rgba8(), original))
        }
        _ => bail!("not a picture"),
    }
}

/// Thumbnail of a JPEG without an embedded preview, decoded at 1/2, 1/4 or 1/8 size
/// straight from the DCT coefficients, so the full picture is never built.
fn jpeg_thumbnail(path: &Path) -> Option<Picture> {
    use jpeg_decoder::PixelFormat;
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = jpeg_decoder::Decoder::new(BufReader::new(file));
    decoder.read_info().ok()?;
    let full = decoder.info()?;
    let original = (full.width as u32, full.height as u32);
    let side = THUMB_SIDE as u16;
    let (w, h) = decoder.scale(side, side).ok()?;
    let pixels = decoder.decode().ok()?;
    let rgba: Vec<u8> = match decoder.info()?.pixel_format {
        PixelFormat::L8 => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        PixelFormat::RGB24 => pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => return None,
    };
    let mut img = DynamicImage::ImageRgba8(RgbaImage::from_raw(w as u32, h as u32, rgba)?);
    let orientation = decoder
        .exif_data()
        .and_then(|raw| exif::Reader::new().read_raw(raw.to_vec()).ok())
        .and_then(|e| e.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)))
        .and_then(|v| Orientation::from_exif(v as u8))
        .unwrap_or(Orientation::NoTransforms);
    img.apply_orientation(orientation);
    let original = if orientation.to_exif() > 4 { (original.1, original.0) } else { original };
    Some(Picture::from_rgba(img.thumbnail(THUMB_SIDE, THUMB_SIDE).into_rgba8(), original))
}

/// Thumbnail of a plain (non-interlaced) PNG made while its rows are read, by
/// averaging blocks of pixels, so a big picture is never held whole in memory.
fn png_thumbnail(path: &Path) -> Option<Picture> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let info = reader.info();
    if info.interlaced {
        return None;
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight || w == 0 || h == 0 {
        return None;
    }
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    let side = THUMB_SIDE as usize;
    let (tw, th) =
        if w >= h { (side.min(w), (h * side / w).clamp(1, h)) } else { ((w * side / h).clamp(1, w), side.min(h)) };
    let mut sums = vec![0u32; tw * th * 4];
    let mut counts = vec![0u32; tw * th];
    let mut y = 0;
    while let Some(row) = reader.next_row().ok()? {
        let oy = y * th / h;
        for (x, px) in row.data().chunks_exact(channels).enumerate().take(w) {
            let cell = oy * tw + x * tw / w;
            let rgba = match channels {
                1 => [px[0], px[0], px[0], 255],
                2 => [px[0], px[0], px[0], px[1]],
                3 => [px[0], px[1], px[2], 255],
                _ => [px[0], px[1], px[2], px[3]],
            };
            for (sum, v) in sums[cell * 4..cell * 4 + 4].iter_mut().zip(rgba) {
                *sum += v as u32;
            }
            counts[cell] += 1;
        }
        y += 1;
    }
    if y != h {
        return None;
    }
    let mut out = Vec::with_capacity(tw * th * 4);
    for (cell, &n) in counts.iter().enumerate() {
        let n = n.max(1);
        out.extend(sums[cell * 4..cell * 4 + 4].iter().map(|&v| ((v + n / 2) / n) as u8));
    }
    let img = RgbaImage::from_raw(tw as u32, th as u32, out)?;
    Some(Picture::from_rgba(img, (w as u32, h as u32)))
}

fn exif_thumbnail(path: &Path) -> Option<Picture> {
    use exif::{In, Tag};
    let file = std::fs::File::open(path).ok()?;
    let exif = exif::Reader::new().read_from_container(&mut BufReader::new(file)).ok()?;
    let offset = exif.get_field(Tag::JPEGInterchangeFormat, In::THUMBNAIL)?.value.get_uint(0)? as usize;
    let len = exif.get_field(Tag::JPEGInterchangeFormatLength, In::THUMBNAIL)?.value.get_uint(0)? as usize;
    let bytes = exif.buf().get(offset..offset.checked_add(len)?)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Jpeg);
    reader.limits(limits());
    let mut img = reader.decode().ok()?;
    // Tiny embedded previews look worse than a real decode; only use decent ones.
    if img.width().max(img.height()) < 120 {
        return None;
    }
    let orientation = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .and_then(|v| Orientation::from_exif(v as u8))
        .unwrap_or(Orientation::NoTransforms);
    img.apply_orientation(orientation);
    let original = (img.width(), img.height());
    Some(Picture::from_rgba(img.thumbnail(THUMB_SIDE, THUMB_SIDE).into_rgba8(), original))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_png_and_thumbnails() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-raster-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        RgbaImage::from_pixel(640, 320, image::Rgba([10, 20, 30, 255])).save(&path).unwrap();

        let pic = load(&path).unwrap();
        assert_eq!((pic.width, pic.height), (640, 320));
        assert_eq!(pic.rgba.len(), 640 * 320 * 4);
        assert_eq!(&pic.rgba[..4], &[10, 20, 30, 255]);

        let thumb = thumbnail(&path).unwrap();
        assert_eq!((thumb.width, thumb.height), (THUMB_SIDE, THUMB_SIDE / 2));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn counts_pixels_from_the_header() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-pixels-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        RgbaImage::from_pixel(300, 200, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
        assert_eq!(pixel_count(&path), Some(60_000));
        assert_eq!(pixel_count(&dir.join("missing.png")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn streamed_png_thumbnail_matches_shape_and_colour() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-pngthumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.png");
        image::RgbImage::from_pixel(1024, 512, image::Rgb([200, 100, 50])).save(&path).unwrap();
        let t = png_thumbnail(&path).unwrap();
        assert_eq!((t.width, t.height), (THUMB_SIDE, THUMB_SIDE / 2));
        assert_eq!(t.original, (1024, 512));
        assert_eq!(&t.rgba[..4], &[200, 100, 50, 255]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scaled_jpeg_thumbnail_matches_shape_and_colour() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-jpgthumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.jpg");
        image::RgbImage::from_pixel(2048, 1024, image::Rgb([200, 100, 50])).save(&path).unwrap();
        let t = jpeg_thumbnail(&path).unwrap();
        assert_eq!((t.width, t.height), (THUMB_SIDE, THUMB_SIDE / 2));
        assert_eq!(t.original, (2048, 1024));
        for (got, want) in t.rgba[..3].iter().zip([200u8, 100, 50]) {
            assert!(got.abs_diff(want) <= 3, "{got} vs {want}");
        }
        // The dispatcher reaches the same result for a picture without an embedded preview.
        assert_eq!(thumbnail(&path).unwrap().width, THUMB_SIDE);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-garbage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.jpg");
        std::fs::write(&path, b"definitely not a jpeg").unwrap();
        assert!(load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
