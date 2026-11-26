//! Pictures with markup: the annotations are drawn into the pixels (as an SVG
//! overlay rendered by resvg, so text gets real fonts) and saved as a new file.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use image::{ImageEncoder, ImageFormat, RgbaImage};
use resvg::{tiny_skia, usvg};

use super::model::{AnnotationLayer, Color};
use super::paint::{self, LINE_HEIGHT, MARKER_ALPHA, TEXT_PAD};

fn css(c: Color) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The layer as an SVG document the size of the picture.
pub fn svg(layer: &AnnotationLayer, width: u32, height: u32) -> String {
    let mut s = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">"#);
    for a in &layer.items {
        let alpha = a.style.stroke.a;
        for prim in paint::prims(a) {
            let fill = match prim.fill {
                Some(f) if prim.marker => format!(r#"fill="{}" fill-opacity="{MARKER_ALPHA}""#, css(f)),
                Some(f) => format!(r#"fill="{}" fill-opacity="{alpha}""#, css(f)),
                None => r#"fill="none""#.into(),
            };
            let stroke = match prim.stroke {
                Some((c, w)) => format!(
                    r#"stroke="{}" stroke-opacity="{alpha}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round""#,
                    css(c)
                ),
                None => String::new(),
            };
            let _ = write!(s, r#"<path d="{}" {fill} {stroke}/>"#, prim.path.to_svg());
        }
        if let Some(block) = paint::text(a) {
            let pad = block.size * TEXT_PAD;
            let _ = write!(
                s,
                r#"<text font-family="sans-serif" font-size="{}" fill="{}" fill-opacity="{alpha}" xml:space="preserve">"#,
                block.size,
                css(block.color)
            );
            for (i, line) in block.lines.iter().enumerate() {
                let y = block.bounds.y + pad + block.size * (i as f64 * LINE_HEIGHT + 0.95);
                let _ = write!(s, r#"<tspan x="{}" y="{y}">{}</tspan>"#, block.bounds.x + pad, escape(line));
            }
            s.push_str("</text>");
        }
    }
    s.push_str("</svg>");
    s
}

/// `picture` (straight RGBA) with `layer` drawn on top.
pub fn flatten(mut picture: RgbaImage, layer: &AnnotationLayer) -> Result<RgbaImage> {
    let (w, h) = picture.dimensions();
    let tree = usvg::Tree::from_str(&svg(layer, w, h), &crate::raster::svg_options()).context("bad markup")?;
    let mut overlay = tiny_skia::Pixmap::new(w, h).context("picture too large")?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut overlay.as_mut());
    for (dst, src) in picture.pixels_mut().zip(overlay.pixels()) {
        let sa = src.alpha() as f32 / 255.0;
        if sa == 0.0 {
            continue;
        }
        // Source over, with the overlay premultiplied and the picture not.
        let da = dst[3] as f32 / 255.0;
        let out_a = sa + da * (1.0 - sa);
        let blend = |s: u8, d: u8| {
            let v = (s as f32 / 255.0 + d as f32 / 255.0 * da * (1.0 - sa)) / out_a.max(1e-6);
            (v.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        *dst = image::Rgba([
            blend(src.red(), dst[0]),
            blend(src.green(), dst[1]),
            blend(src.blue(), dst[2]),
            (out_a * 255.0).round() as u8,
        ]);
    }
    Ok(picture)
}

/// Saves `picture` with its markup to `path`, in the format its extension names, keeping
/// `meta` (EXIF and colour profile) where the format can hold it.
pub fn save(picture: RgbaImage, layer: &AnnotationLayer, path: &Path, meta: &crate::metadata::Metadata) -> Result<()> {
    let format = ImageFormat::from_path(path)
        .ok()
        .filter(|f| f.can_write())
        .context("choose a file name ending in .png, .jpg, .webp, .tif or another format Lumen can write")?;
    let flat = flatten(picture, layer)?;
    crate::fsio::write_atomic_with(path, |file| {
        // Formats that cannot hold metadata just get the pixels.
        let keep = |encoder: &mut dyn image::ImageEncoder| {
            if let Some(icc) = &meta.icc {
                let _ = encoder.set_icc_profile(icc.clone());
            }
            if let Some(exif) = &meta.exif {
                let _ = encoder.set_exif_metadata(exif.clone());
            }
        };
        let (w, h) = flat.dimensions();
        match format {
            ImageFormat::Jpeg => {
                // No alpha in JPEG: put the picture on white first.
                let mut white = RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
                image::imageops::overlay(&mut white, &flat, 0, 0);
                let rgb = image::DynamicImage::ImageRgba8(white).to_rgb8();
                let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut *file, 92);
                keep(&mut encoder);
                encoder.write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)?;
            }
            ImageFormat::Png => {
                let mut encoder = image::codecs::png::PngEncoder::new(&mut *file);
                keep(&mut encoder);
                encoder.write_image(&flat, w, h, image::ExtendedColorType::Rgba8)?;
            }
            ImageFormat::WebP => {
                let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut *file);
                keep(&mut encoder);
                encoder.write_image(&flat, w, h, image::ExtendedColorType::Rgba8)?;
            }
            ImageFormat::Bmp | ImageFormat::Pnm => {
                let mut white = RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
                image::imageops::overlay(&mut white, &flat, 0, 0);
                image::DynamicImage::ImageRgba8(white).to_rgb8().write_to(&mut *file, format)?;
            }
            _ => flat.write_to(&mut *file, format)?,
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::model::{Rect, Shape, Style};

    #[test]
    fn draws_markup_into_the_pixels() {
        let mut layer = AnnotationLayer::default();
        let style = Style { fill: Some(Color::RED), width: 2.0, ..Style::default() };
        layer.add(Shape::Rect(Rect::new(10.0, 10.0, 20.0, 20.0)), style.clone());
        layer.add(Shape::Text { bounds: Rect::new(40.0, 40.0, 100.0, 30.0), text: "a < b & c".into() }, style);
        let picture = RgbaImage::from_pixel(100, 100, image::Rgba([0, 0, 255, 255]));
        let flat = flatten(picture, &layer).unwrap();
        assert_eq!(flat.get_pixel(20, 20).0, [230, 26, 26, 255], "inside the red square");
        assert_eq!(flat.get_pixel(90, 5).0, [0, 0, 255, 255], "untouched elsewhere");

        let dir = std::env::temp_dir().join(format!("lumen-flatten-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["out.png", "out.jpg", "out.webp"] {
            save(
                RgbaImage::from_pixel(64, 48, image::Rgba([9, 9, 9, 255])),
                &layer,
                &dir.join(name),
                &Default::default(),
            )
            .unwrap();
            assert_eq!(image::open(dir.join(name)).unwrap().width(), 64, "{name}");
        }
        assert!(save(RgbaImage::new(4, 4), &layer, &dir.join("out.xyz"), &Default::default()).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
