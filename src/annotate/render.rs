//! Drawing annotations with cairo.
//!
//! One set of functions serves the screen overlay, flattening onto an image for
//! saving, and printing: they draw in document coordinates onto a context that
//! the caller has already moved and scaled.

use std::f64::consts::PI;

use image::{DynamicImage, RgbaImage};
use relm4::gtk::cairo;

use super::model::{Annotation, AnnotationLayer, Color, Shape, arrow_head};

/// Highlights are translucent whatever colour is chosen.
pub const HIGHLIGHT_ALPHA: f64 = 0.4;

fn set_color(cr: &cairo::Context, color: Color, alpha: Option<f64>) {
    cr.set_source_rgba(
        color.r as f64,
        color.g as f64,
        color.b as f64,
        alpha.unwrap_or(color.a as f64),
    );
}

/// Draw every annotation of `layer`, bottom to top.
pub fn draw_layer(cr: &cairo::Context, layer: &AnnotationLayer) {
    for annotation in &layer.items {
        draw_annotation(cr, annotation);
    }
}

pub fn draw_annotation(cr: &cairo::Context, a: &Annotation) {
    // Drawing errors only mean the target surface is broken; there is nothing useful to do.
    let _ = draw(cr, a);
}

fn draw(cr: &cairo::Context, a: &Annotation) -> Result<(), cairo::Error> {
    cr.save()?;
    cr.set_line_width(a.style.width.max(0.0));
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);

    match &a.shape {
        Shape::Rect(r) => {
            cr.rectangle(r.x, r.y, r.w, r.h);
            fill_and_stroke(cr, a)?;
        }
        Shape::Ellipse(r) => {
            // Build the path under a stretch, but stroke after undoing it so the line keeps its width.
            let matrix = cr.matrix();
            cr.translate(r.center().x, r.center().y);
            cr.scale((r.w / 2.0).max(1e-6), (r.h / 2.0).max(1e-6));
            cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
            cr.set_matrix(matrix);
            fill_and_stroke(cr, a)?;
        }
        Shape::Line(from, to) => {
            cr.move_to(from.x, from.y);
            cr.line_to(to.x, to.y);
            set_color(cr, a.style.stroke, None);
            cr.stroke()?;
        }
        Shape::Arrow(from, to) => {
            cr.move_to(from.x, from.y);
            cr.line_to(to.x, to.y);
            let [left, right] = arrow_head(*from, *to, a.style.width);
            cr.move_to(left.x, left.y);
            cr.line_to(to.x, to.y);
            cr.line_to(right.x, right.y);
            set_color(cr, a.style.stroke, None);
            cr.stroke()?;
        }
        Shape::Freehand(points) => {
            if let Some((first, rest)) = points.split_first() {
                cr.move_to(first.x, first.y);
                if rest.is_empty() {
                    // A single point is a dot.
                    cr.line_to(first.x, first.y);
                }
                for p in rest {
                    cr.line_to(p.x, p.y);
                }
                set_color(cr, a.style.stroke, None);
                cr.stroke()?;
            }
        }
        Shape::Highlight(rects) => {
            for r in rects {
                cr.rectangle(r.x, r.y, r.w, r.h);
            }
            set_color(
                cr,
                a.style.fill.unwrap_or(a.style.stroke),
                Some(HIGHLIGHT_ALPHA),
            );
            cr.fill()?;
        }
        Shape::Text { bounds, text } => {
            set_color(cr, a.style.stroke, None);
            cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            cr.set_font_size(a.style.font_size);
            cr.rectangle(bounds.x, bounds.y, bounds.w, bounds.h);
            cr.clip();
            let line_height = a.style.font_size * 1.4;
            for (i, line) in text.lines().enumerate() {
                cr.move_to(
                    bounds.x + 2.0,
                    bounds.y + a.style.font_size + i as f64 * line_height,
                );
                cr.show_text(line)?;
            }
        }
    }
    cr.restore()
}

fn fill_and_stroke(cr: &cairo::Context, a: &Annotation) -> Result<(), cairo::Error> {
    if let Some(fill) = a.style.fill {
        set_color(cr, fill, None);
        cr.fill_preserve()?;
    }
    if a.style.width > 0.0 {
        set_color(cr, a.style.stroke, None);
        cr.stroke()?;
    } else {
        cr.new_path();
    }
    Ok(())
}

/// The frame and grab points of a selected annotation. `scale` is the zoom, so
/// the handles keep the same size on screen at any zoom.
pub fn draw_selection(cr: &cairo::Context, a: &Annotation, scale: f64) {
    let _ = (|| -> Result<(), cairo::Error> {
        cr.save()?;
        let b = a.bounds();
        let pad = 3.0 / scale;
        cr.set_source_rgba(0.2, 0.5, 1.0, 0.9);
        cr.set_line_width(1.5 / scale);
        cr.set_dash(&[4.0 / scale, 3.0 / scale], 0.0);
        cr.rectangle(b.x - pad, b.y - pad, b.w + 2.0 * pad, b.h + 2.0 * pad);
        cr.stroke()?;
        cr.set_dash(&[], 0.0);

        let half = 4.5 / scale;
        for (_, at) in a.handles() {
            cr.rectangle(at.x - half, at.y - half, 2.0 * half, 2.0 * half);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.fill_preserve()?;
            cr.set_source_rgba(0.2, 0.5, 1.0, 1.0);
            cr.stroke()?;
        }
        cr.restore()
    })();
}

/// Burn `layer` into `image` (whose pixels are the document coordinates).
pub fn flatten(
    image: &DynamicImage,
    layer: &AnnotationLayer,
) -> Result<DynamicImage, cairo::Error> {
    if layer.is_empty() {
        return Ok(image.clone());
    }
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width() as i32, rgba.height() as i32);
    let stride = width as usize * 4;

    // Cairo wants premultiplied BGRA (on little-endian machines).
    let mut data = vec![0u8; stride * height as usize];
    for (src, dst) in rgba.pixels().zip(data.as_chunks_mut::<4>().0.iter_mut()) {
        let [r, g, b, a] = src.0;
        let premultiply = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
        *dst = [premultiply(b), premultiply(g), premultiply(r), a];
    }

    let mut surface = cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        width,
        height,
        stride as i32,
    )?;
    {
        let cr = cairo::Context::new(&surface)?;
        draw_layer(&cr, layer);
    }
    surface.flush();
    let data = surface.data().map_err(|_| cairo::Error::SurfaceFinished)?;

    let mut out = RgbaImage::new(width as u32, height as u32);
    for (src, dst) in data.as_chunks::<4>().0.iter().zip(out.pixels_mut()) {
        let [b, g, r, a] = [src[0], src[1], src[2], src[3]];
        let unpremultiply = |c: u8| {
            if a == 0 {
                0
            } else {
                ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
            }
        };
        dst.0 = [unpremultiply(r), unpremultiply(g), unpremultiply(b), a];
    }
    Ok(DynamicImage::ImageRgba8(out))
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;
    use crate::annotate::model::{Point, Rect, Style};

    fn white(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255])))
    }

    fn pixel(image: &DynamicImage, x: u32, y: u32) -> [u8; 4] {
        image.to_rgba8().get_pixel(x, y).0
    }

    fn red() -> Color {
        Color::rgba(1.0, 0.0, 0.0, 1.0)
    }

    #[test]
    fn an_empty_layer_leaves_the_image_untouched() {
        let image = white(8, 8);
        let out = flatten(&image, &AnnotationLayer::default()).unwrap();
        assert_eq!(image.to_rgba8(), out.to_rgba8());
    }

    #[test]
    fn a_filled_rectangle_lands_where_it_says() {
        let mut layer = AnnotationLayer::default();
        layer.add(
            Shape::Rect(Rect::new(5.0, 5.0, 10.0, 10.0)),
            Style {
                fill: Some(red()),
                stroke: red(),
                width: 0.0,
                font_size: 12.0,
            },
        );
        let out = flatten(&white(20, 20), &layer).unwrap();
        assert_eq!(pixel(&out, 10, 10), [255, 0, 0, 255]);
        assert_eq!(pixel(&out, 1, 1), [255, 255, 255, 255]);
        assert_eq!(pixel(&out, 17, 10), [255, 255, 255, 255]);
    }

    #[test]
    fn lines_and_arrows_draw_pixels_along_their_path() {
        let mut layer = AnnotationLayer::default();
        let style = Style {
            stroke: red(),
            width: 4.0,
            ..Style::default()
        };
        layer.add(
            Shape::Line(Point::new(2.0, 10.0), Point::new(38.0, 10.0)),
            style.clone(),
        );
        layer.add(
            Shape::Arrow(Point::new(2.0, 30.0), Point::new(38.0, 30.0)),
            style,
        );
        let out = flatten(&white(40, 40), &layer).unwrap();
        assert_eq!(pixel(&out, 20, 10), [255, 0, 0, 255]);
        assert_eq!(pixel(&out, 20, 30), [255, 0, 0, 255]);
        // The arrow head spreads away from the shaft near the tip; the plain line does not.
        let [_, g, _, _] = pixel(&out, 30, 24);
        assert!(g < 100, "a barb above the shaft: green channel {g}");
        assert_eq!(pixel(&out, 30, 4), [255, 255, 255, 255]);
    }

    #[test]
    fn highlights_tint_what_is_beneath_without_hiding_it() {
        let mut layer = AnnotationLayer::default();
        let style = Style {
            stroke: Color::YELLOW,
            fill: Some(Color::YELLOW),
            width: 0.0,
            font_size: 12.0,
        };
        layer.add(
            Shape::Highlight(vec![Rect::new(0.0, 0.0, 10.0, 10.0)]),
            style,
        );
        let mut base = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        base.put_pixel(5, 5, Rgba([0, 0, 255, 255]));
        let out = flatten(&DynamicImage::ImageRgba8(base), &layer).unwrap();
        let [r, g, b, a] = pixel(&out, 1, 1);
        assert!(
            r > 240 && g > 220 && b < 200 && a == 255,
            "pale yellow over white: {r} {g} {b}"
        );
        let [_, _, b, _] = pixel(&out, 5, 5);
        assert!(b > 100, "the blue pixel is still there under the marker");
    }

    #[test]
    fn transparent_pixels_stay_transparent_where_nothing_is_drawn() {
        let mut layer = AnnotationLayer::default();
        layer.add(
            Shape::Rect(Rect::new(0.0, 0.0, 4.0, 4.0)),
            Style {
                fill: Some(red()),
                width: 0.0,
                ..Style::default()
            },
        );
        let clear = DynamicImage::ImageRgba8(RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 0])));
        let out = flatten(&clear, &layer).unwrap();
        assert_eq!(pixel(&out, 8, 8), [0, 0, 0, 0]);
        assert_eq!(pixel(&out, 2, 2), [255, 0, 0, 255]);
    }

    #[test]
    fn text_marks_its_box() {
        let mut layer = AnnotationLayer::default();
        layer.add(
            Shape::Text {
                bounds: Rect::new(2.0, 2.0, 80.0, 30.0),
                text: "MMMM".into(),
            },
            Style {
                stroke: Color::rgba(0.0, 0.0, 0.0, 1.0),
                font_size: 24.0,
                ..Style::default()
            },
        );
        let out = flatten(&white(90, 40), &layer).unwrap().to_rgba8();
        let dark = out.pixels().filter(|p| p.0[0] < 100).count();
        assert!(dark > 50, "glyphs were drawn: {dark} dark pixels");
        // Nothing outside the box.
        assert!((0..40).all(|y| out.get_pixel(88, y).0 == [255, 255, 255, 255]));
    }
}
