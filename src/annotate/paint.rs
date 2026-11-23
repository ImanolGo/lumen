//! What an annotation looks like, as paths: one description shared by the
//! screen overlay, PDF appearance streams and pictures saved with markup, so
//! the three always agree.

use hayro::kurbo::{self, BezPath, Shape as _};

use super::model::{Annotation, Color, Point, Rect, Shape, arrow_head};

/// How opaque a highlight is on screen and in saved pictures (PDFs multiply instead).
pub const MARKER_ALPHA: f32 = 0.4;

/// One filled and/or stroked path, in document coordinates.
pub struct Prim {
    pub path: BezPath,
    /// Colour and line width. Lines have round caps and joins.
    pub stroke: Option<(Color, f64)>,
    pub fill: Option<Color>,
    /// A highlighter mark: drawn translucent (or multiplied) so the text shows through.
    pub marker: bool,
}

/// The text of a text annotation, ready to lay out.
pub struct TextBlock<'a> {
    pub bounds: Rect,
    pub lines: Vec<&'a str>,
    pub size: f64,
    pub color: Color,
}

/// Distance between baselines, as a multiple of the font size.
pub const LINE_HEIGHT: f64 = 1.25;
/// Space between a text box's edge and its text, as a multiple of the font size.
pub const TEXT_PAD: f64 = 0.2;

fn pt(p: Point) -> kurbo::Point {
    kurbo::Point::new(p.x, p.y)
}

fn rect(r: Rect) -> kurbo::Rect {
    kurbo::Rect::new(r.x, r.y, r.right(), r.bottom())
}

fn polyline(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    if let Some((first, rest)) = points.split_first() {
        path.move_to(pt(*first));
        if rest.is_empty() {
            // A single click with the pen leaves a dot.
            path.line_to(pt(*first) + kurbo::Vec2::new(0.01, 0.0));
        }
        for p in rest {
            path.line_to(pt(*p));
        }
    }
    path
}

/// The paths of `a`. Text annotations have none (see [`text`]) unless they have a background.
pub fn prims(a: &Annotation) -> Vec<Prim> {
    let style = &a.style;
    let stroke = (style.width > 0.0).then_some((style.stroke, style.width));
    let line =
        |path: BezPath| Prim { path, stroke: Some((style.stroke, style.width.max(0.5))), fill: None, marker: false };
    match &a.shape {
        Shape::Rect(r) => vec![Prim { path: rect(*r).to_path(0.1), stroke, fill: style.fill, marker: false }],
        Shape::Ellipse(r) => {
            let path = kurbo::Ellipse::from_rect(rect(*r)).to_path(0.1);
            vec![Prim { path, stroke, fill: style.fill, marker: false }]
        }
        Shape::Line(p, q) => vec![line(polyline(&[*p, *q]))],
        Shape::Arrow(p, q) => {
            let [h1, h2] = arrow_head(*p, *q, style.width);
            vec![line(polyline(&[*p, *q])), line(polyline(&[h1, *q, h2]))]
        }
        Shape::Freehand(points) => vec![line(polyline(points))],
        Shape::Ink(strokes) => strokes.iter().map(|s| line(polyline(s))).collect(),
        Shape::Highlight(rects) => {
            let mut path = BezPath::new();
            for r in rects {
                path.extend(rect(*r).path_elements(0.1));
            }
            vec![Prim { path, stroke: None, fill: Some(style.fill.unwrap_or(style.stroke)), marker: true }]
        }
        Shape::Text { bounds, .. } => match style.fill {
            Some(fill) => {
                vec![Prim { path: rect(*bounds).to_path(0.1), stroke: None, fill: Some(fill), marker: false }]
            }
            None => Vec::new(),
        },
    }
}

/// The text of `a`, if it is a text annotation.
pub fn text(a: &Annotation) -> Option<TextBlock<'_>> {
    match &a.shape {
        Shape::Text { bounds, text } => Some(TextBlock {
            bounds: *bounds,
            lines: text.split('\n').collect(),
            size: a.style.font_size,
            color: a.style.stroke,
        }),
        _ => None,
    }
}

/// Bounds including the stroke, for a PDF annotation's /Rect.
pub fn outer_bounds(a: &Annotation) -> Rect {
    let reach = match a.shape {
        // Text boxes and highlights have no outline: their geometry is their extent.
        Shape::Text { .. } | Shape::Highlight(_) => 0.0,
        Shape::Arrow(..) | Shape::Line(..) | Shape::Freehand(_) | Shape::Ink(_) => a.style.width.max(1.0),
        _ => a.style.width / 2.0 + 1.0,
    };
    a.bounds().inflate(reach)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::model::Style;

    #[test]
    fn every_shape_has_something_to_draw() {
        let style = Style::default();
        let shapes = [
            Shape::Rect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            Shape::Ellipse(Rect::new(0.0, 0.0, 10.0, 10.0)),
            Shape::Line(Point::new(0.0, 0.0), Point::new(5.0, 5.0)),
            Shape::Arrow(Point::new(0.0, 0.0), Point::new(50.0, 0.0)),
            Shape::Freehand(vec![Point::new(1.0, 1.0)]),
            Shape::Highlight(vec![Rect::new(0.0, 0.0, 10.0, 4.0), Rect::new(0.0, 5.0, 10.0, 4.0)]),
        ];
        for shape in shapes {
            let a = Annotation { id: 1, shape: shape.clone(), style: style.clone() };
            let prims = prims(&a);
            assert!(!prims.is_empty() && prims.iter().all(|p| !p.path.elements().is_empty()), "{shape:?}");
            assert!(outer_bounds(&a).w >= a.bounds().w);
        }
        let note = Annotation {
            id: 2,
            shape: Shape::Text { bounds: Rect::new(0.0, 0.0, 50.0, 20.0), text: "a\nb".into() },
            style,
        };
        assert!(prims(&note).is_empty());
        assert_eq!(text(&note).unwrap().lines, ["a", "b"]);
    }
}
