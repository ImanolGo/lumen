//! Decoding pictures (raster and SVG) into RGBA, plus fast thumbnails.
//!
//! Everything here is blocking and CPU-bound; callers run it on a worker thread.
//! All decoders are pure, memory-safe Rust (`image`, `resvg`), and decoding
//! runs under allocation limits so a hostile file cannot exhaust memory.

use std::io::Cursor;
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
/// No decoded picture is bigger than this many bytes of RGBA (a resize step can follow the decode limit).
pub const MAX_BYTES: u64 = MAX_ALLOC + (1 << 28);
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

/// Decodes `path` for display, honouring EXIF orientation. Raster pictures are decoded in a
/// sandboxed child process when that is on (see `sandbox`).
pub fn load(path: &Path) -> Result<Picture> {
    match Kind::of(path) {
        Some(Kind::Svg) => load_svg(path, 2048),
        Some(Kind::Image) => crate::sandbox::run(crate::sandbox::Mode::Load, path),
        _ => bail!("not a picture"),
    }
}

/// The extension of `path`, lowercase, which tells the formats apart that `image` cannot sniff.
pub fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase()
}

/// `data` (a file with extension `ext`) decoded for display, honouring EXIF orientation.
/// This is what the sandboxed worker runs.
pub fn load_bytes(data: &[u8], ext: &str) -> Result<Picture> {
    let img = decode(data, ext)?;
    let original = (img.width(), img.height());
    let img = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(Picture::from_rgba(to_rgba8(img), original))
}

/// Full-resolution pixels of `data`, oriented, for printing (the worker's third job).
pub fn print_bytes(data: &[u8], ext: &str) -> Result<Picture> {
    let img = to_rgba8(decode(data, ext)?);
    let size = img.dimensions();
    Ok(Picture::from_rgba(img, size))
}

/// A small preview of `data`: the worker's second job.
pub fn thumbnail_bytes(data: &[u8], ext: &str) -> Result<Picture> {
    if let Some(t) = exif_thumbnail(data).or_else(|| jpeg_thumbnail(data)).or_else(|| png_thumbnail(data)) {
        return Ok(t);
    }
    // Full decodes for thumbnails run one at a time, so a folder of big pictures
    // does not hold several decoded copies in memory at once.
    static FULL_DECODE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = FULL_DECODE.lock().unwrap_or_else(|e| e.into_inner());
    let img = decode(data, ext)?;
    let original = (img.width(), img.height());
    Ok(Picture::from_rgba(to_rgba8(img.thumbnail(THUMB_SIDE, THUMB_SIDE)), original))
}

/// AVIF and HEIC go through the system's libheif, which needs a path and is not sandboxed.
fn needs_path(ext: &str) -> bool {
    cfg!(feature = "heif") && matches!(ext, "heic" | "heif" | "hif" | "avif")
}

/// Whether the sandbox can take this file.
pub fn sandboxable(path: &Path) -> bool {
    !needs_path(&extension(path))
}

/// The same jobs without the sandbox: read the file and do them here.
pub fn run_in_process(mode: crate::sandbox::Mode, path: &Path) -> Result<Picture> {
    use crate::sandbox::Mode;
    let ext = extension(path);
    #[cfg(feature = "heif")]
    if needs_path(&ext) {
        let img = crate::heif::decode(path, MAX_ALLOC)?;
        let original = (img.width(), img.height());
        return Ok(match mode {
            Mode::Thumbnail => Picture::from_rgba(to_rgba8(img.thumbnail(THUMB_SIDE, THUMB_SIDE)), original),
            _ => Picture::from_rgba(to_rgba8(img), original),
        });
    }
    let data = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    match mode {
        Mode::Load => load_bytes(&data, &ext),
        Mode::Thumbnail => thumbnail_bytes(&data, &ext),
        Mode::Print => print_bytes(&data, &ext),
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
        Some(Kind::Image) => {
            let pic = crate::sandbox::run(crate::sandbox::Mode::Print, path)?;
            RgbaImage::from_raw(pic.width, pic.height, Vec::from(pic.rgba)).context("bad picture raster")?
        }
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

pub(crate) fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

/// The formats `image` cannot read, told apart by their extension.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Special {
    JpegXl,
    Jpeg2000,
    #[cfg(feature = "heif")]
    Heif,
}

fn special(ext: &str) -> Option<Special> {
    match ext {
        "jxl" => Some(Special::JpegXl),
        "jp2" | "j2k" | "j2c" | "jpx" => Some(Special::Jpeg2000),
        #[cfg(feature = "heif")]
        "heic" | "heif" | "hif" | "avif" => Some(Special::Heif),
        _ => None,
    }
}

/// JPEG XL, rendered in sRGB (jxl-oxide, pure Rust).
fn decode_jxl(data: &[u8]) -> Result<DynamicImage> {
    use jxl_oxide::{EnumColourEncoding, JxlImage, PixelFormat, RenderingIntent};
    let mut image = JxlImage::builder()
        .read(Cursor::new(data))
        .map_err(|e| anyhow::anyhow!("unsupported or damaged JPEG XL picture ({e})"))?;
    let (w, h) = (image.width(), image.height());
    if u64::from(w) * u64::from(h) * 4 > MAX_ALLOC {
        bail!("the picture is too large");
    }
    image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    let channels = match image.pixel_format() {
        PixelFormat::Gray => 1,
        PixelFormat::Graya => 2,
        PixelFormat::Rgb => 3,
        PixelFormat::Rgba => 4,
        _ => bail!("CMYK JPEG XL pictures are not supported"),
    };
    let render = image.render_frame(0).map_err(|e| anyhow::anyhow!("the JPEG XL picture is damaged ({e})"))?;
    let mut stream = render.stream();
    let mut samples = vec![0u8; w as usize * h as usize * channels];
    stream.write_to_buffer(&mut samples);
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for px in samples.chunks_exact(channels) {
        rgba.extend_from_slice(&match channels {
            1 => [px[0], px[0], px[0], 255],
            2 => [px[0], px[0], px[0], px[1]],
            3 => [px[0], px[1], px[2], 255],
            _ => [px[0], px[1], px[2], px[3]],
        });
    }
    let (sw, sh) = (stream.width(), stream.height());
    Ok(DynamicImage::ImageRgba8(RgbaImage::from_raw(sw, sh, rgba).context("bad JPEG XL raster")?))
}

/// JPEG 2000 (hayro-jpeg2000, pure Rust).
fn decode_jpeg2000(data: &[u8]) -> Result<DynamicImage> {
    use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext, Image};
    let image = Image::new(data, &DecodeSettings::default())
        .map_err(|e| anyhow::anyhow!("unsupported or damaged JPEG 2000 picture ({e})"))?;
    let (w, h) = (image.width(), image.height());
    if u64::from(w) * u64::from(h) * 4 > MAX_ALLOC {
        bail!("the picture is too large");
    }
    let alpha = image.has_alpha();
    let channels = match image.color_space() {
        ColorSpace::Gray => 1 + usize::from(alpha),
        ColorSpace::RGB => 3 + usize::from(alpha),
        ColorSpace::Icc { num_channels, .. } if *num_channels == 1 || *num_channels == 3 => {
            *num_channels as usize + usize::from(alpha)
        }
        _ => bail!("this colour space of JPEG 2000 pictures is not supported"),
    };
    let mut context = DecoderContext::default();
    let decoded = image.decode(&mut context).map_err(|e| anyhow::anyhow!("the JPEG 2000 picture is damaged ({e})"))?;
    let samples = decoded.data_u8();
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for px in samples.chunks_exact(channels) {
        rgba.extend_from_slice(&match channels {
            1 => [px[0], px[0], px[0], 255],
            2 => [px[0], px[0], px[0], px[1]],
            3 => [px[0], px[1], px[2], 255],
            _ => [px[0], px[1], px[2], px[3]],
        });
    }
    Ok(DynamicImage::ImageRgba8(RgbaImage::from_raw(w, h, rgba).context("bad JPEG 2000 raster")?))
}

/// 8-bit RGBA from any decoded picture. Floating-point pictures (OpenEXR) hold linear
/// light, so they are clamped to the displayable range and given the sRGB curve.
pub fn to_rgba8(img: DynamicImage) -> RgbaImage {
    if !matches!(img, DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)) {
        return img.into_rgba8();
    }
    let linear = img.into_rgba32f();
    let (w, h) = linear.dimensions();
    let encode = |v: f32| {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0 + 0.5) as u8
    };
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for px in linear.pixels() {
        out.extend_from_slice(&[
            encode(px[0]),
            encode(px[1]),
            encode(px[2]),
            (px[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        ]);
    }
    RgbaImage::from_raw(w, h, out).expect("the buffer has the right size")
}

fn decode(data: &[u8], ext: &str) -> Result<DynamicImage> {
    match special(ext) {
        Some(Special::JpegXl) => return decode_jxl(data),
        Some(Special::Jpeg2000) => return decode_jpeg2000(data),
        #[cfg(feature = "heif")]
        Some(Special::Heif) => bail!("this format is only read from a file"),
        None => {}
    }
    // The extension is the hint for formats with no signature (TGA); what the bytes say wins.
    let mut reader = ImageReader::new(Cursor::new(data));
    if let Some(format) = image::ImageFormat::from_extension(ext) {
        reader.set_format(format);
    }
    let mut reader = reader.with_guessed_format()?;
    reader.limits(limits());
    let mut decoder = reader.into_decoder().context("unsupported picture format")?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    // `from_decoder` does not count its own buffer.
    let mut limits = limits();
    limits.reserve(decoder.total_bytes()).map_err(|_| anyhow::anyhow!("the picture is too large"))?;
    decoder.set_limits(limits).context("the picture is too large")?;
    let mut img = DynamicImage::from_decoder(decoder).context("the picture is damaged")?;
    img.apply_orientation(orientation);
    Ok(img)
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
    // Pictures inside an SVG come from the SVG itself (`data:` URLs), never from other files.
    let image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
        resolve_string: Box::new(|_, _| None),
    };
    usvg::Options { fontdb, image_href_resolver, ..Default::default() }
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
        Some(Kind::Image) => crate::sandbox::run(crate::sandbox::Mode::Thumbnail, path),
        _ => bail!("not a picture"),
    }
}

/// Thumbnail of a JPEG without an embedded preview, decoded at 1/2, 1/4 or 1/8 size
/// straight from the DCT coefficients, so the full picture is never built.
fn jpeg_thumbnail(data: &[u8]) -> Option<Picture> {
    use jpeg_decoder::PixelFormat;
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(data));
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
fn png_thumbnail(data: &[u8]) -> Option<Picture> {
    let mut decoder = png::Decoder::new(Cursor::new(data));
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

fn exif_thumbnail(data: &[u8]) -> Option<Picture> {
    use exif::{In, Tag};
    let exif = exif::Reader::new().read_from_container(&mut Cursor::new(data)).ok()?;
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

    /// A PNG header and nothing else: 50 000 × 50 000 pixels.
    fn huge_png() -> Vec<u8> {
        let mut data = Vec::new();
        let mut encoder = png::Encoder::new(&mut data, 50_000, 50_000);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_chunk(png::chunk::IDAT, &[0x78, 0x9c]).unwrap();
        drop(writer);
        data
    }

    #[test]
    fn the_limit_covers_the_pixels_too() {
        let e = load_bytes(&huge_png(), "png").unwrap_err();
        assert!(e.to_string().contains("too large"), "{e:#}");
    }

    #[test]
    fn an_svg_reads_no_other_files() {
        use base64_png::PNG;
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pictures/g.png");
        let dir = std::env::temp_dir().join(format!("lumen-rs-svg-href-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.svg");
        let opaque = |href: &str| {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><image href="{href}" width="40" height="40"/></svg>"#
            );
            std::fs::write(&path, svg).unwrap();
            load_svg(&path, 40).unwrap().rgba.chunks(4).any(|p| p[3] > 0)
        };
        assert!(!opaque(file.to_str().unwrap()), "the other file was read");
        assert!(opaque(&format!("data:image/png;base64,{PNG}")), "what the SVG holds is shown");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    mod base64_png {
        /// One opaque pixel.
        pub const PNG: &str =
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    }

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
        let t = png_thumbnail(&std::fs::read(&path).unwrap()).unwrap();
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
        let t = jpeg_thumbnail(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((t.width, t.height), (THUMB_SIDE, THUMB_SIDE / 2));
        assert_eq!(t.original, (2048, 1024));
        for (got, want) in t.rgba[..3].iter().zip([200u8, 100, 50]) {
            assert!(got.abs_diff(want) <= 3, "{got} vs {want}");
        }
        // The dispatcher reaches the same result for a picture without an embedded preview.
        assert_eq!(thumbnail(&path).unwrap().width, THUMB_SIDE);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    #[test]
    fn decodes_jpeg_xl_and_jpeg_2000() {
        // Both fixtures are the same 64x32 gradient: red grows to the right, green downwards, blue is 128.
        for name in ["gradient.jxl", "gradient.jp2"] {
            let pic = load(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!((pic.width, pic.height), (64, 32), "{name}");
            let px = |x: usize, y: usize| &pic.rgba[(y * 64 + x) * 4..][..4];
            let (a, b) = (px(0, 0), px(63, 31));
            assert!(a[0] < 10 && a[1] < 10, "{name}: {a:?}");
            assert!(b[0] > 200 && b[1] > 200, "{name}: {b:?}");
            assert!(a[2].abs_diff(128) < 8 && a[3] == 255, "{name}: {a:?}");
            // Thumbnails and the file list know them too.
            assert_eq!(Kind::of(&fixture(name)), Some(Kind::Image));
            assert!(thumbnail(&fixture(name)).is_ok());
        }
    }

    /// Only with `--features heif`: the same gradient as AVIF and as HEIC (lossy, so a wider tolerance).
    #[cfg(feature = "heif")]
    #[test]
    fn decodes_avif_and_heic_with_the_heif_feature() {
        for name in ["gradient.avif", "gradient.heic"] {
            let pic = load(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!((pic.width, pic.height), (64, 32), "{name}");
            let px = |x: usize, y: usize| &pic.rgba[(y * 64 + x) * 4..][..4];
            let (a, b) = (px(0, 0), px(63, 31));
            assert!(a[0] < 40 && a[1] < 40, "{name}: {a:?}");
            assert!(b[0] > 180 && b[1] > 180, "{name}: {b:?}");
            assert_eq!(Kind::of(&fixture(name)), Some(Kind::Image));
        }
    }

    #[test]
    fn decodes_openexr_with_the_srgb_curve() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-exr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.exr");
        // Linear light: 0.5 should come out as about 188 after the sRGB curve, 2.0 clamps to 255.
        let mut img = image::Rgba32FImage::from_pixel(8, 4, image::Rgba([0.5, 0.0, 2.0, 1.0]));
        img.put_pixel(0, 0, image::Rgba([0.2140, 1.0, 0.0, 0.5]));
        DynamicImage::ImageRgba32F(img).save(&path).unwrap();
        let pic = load(&path).unwrap();
        assert_eq!((pic.width, pic.height), (8, 4));
        let px = &pic.rgba[4..8];
        assert!(px[0].abs_diff(188) <= 1, "{px:?}");
        assert_eq!((px[1], px[2], px[3]), (0, 255, 255));
        let first = &pic.rgba[..4];
        assert!(first[0].abs_diff(128) <= 2 && first[1] == 255 && first[3].abs_diff(128) <= 1, "{first:?}");
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
