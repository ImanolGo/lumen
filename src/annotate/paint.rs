//! What an annotation looks like, as paths: one description shared by the
//! screen overlay, PDF appearance streams and pictures saved with markup, so
//! the three always agree.

use hayro::kurbo::{self, BezPath, Shape as _};

use super::model::{Annotation, Color, Point, Rect, Shape, arrow_head, figure_outline};

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
    /// Drawn as dashes (see [`dashes`]).
    pub dashed: bool,
}

/// The lengths of a dash and of the gap after it, for a line `width` wide. With round caps
/// the dash looks a width longer and the gap a width shorter.
pub fn dashes(width: f64) -> [f64; 2] {
    let w = width.max(1.0);
    [2.0 * w, 3.0 * w]
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

/// A smooth line through drawn points: from the middle of one step to the middle of the next, bent
/// towards the point between them. It stays inside the points' hull.
fn smooth(points: &[Point]) -> BezPath {
    if points.len() < 3 {
        return polyline(points);
    }
    let mut path = BezPath::new();
    path.move_to(pt(points[0]));
    path.line_to(pt(points[0]).midpoint(pt(points[1])));
    for pair in points[1..].windows(2) {
        path.quad_to(pt(pair[0]), pt(pair[0]).midpoint(pt(pair[1])));
    }
    path.line_to(pt(points[points.len() - 1]));
    path
}

/// The paths of `a`. Text annotations have none (see [`text`]) unless they have a background.
pub fn prims(a: &Annotation) -> Vec<Prim> {
    let style = &a.style;
    let stroke = (style.width > 0.0).then_some((style.stroke, style.width));
    let dashed = style.dashed;
    let solid = |path: BezPath, dashed: bool| Prim {
        path,
        stroke: Some((style.stroke, style.width.max(0.5))),
        fill: None,
        marker: false,
        dashed,
    };
    let line = |path: BezPath| solid(path, dashed);
    // Arrow heads stay solid on a dashed shaft.
    let head = |p: Point, q: Point| {
        let [h1, h2] = arrow_head(p, q, style.width);
        solid(polyline(&[h1, q, h2]), false)
    };
    let outline = |path: BezPath| Prim { path, stroke, fill: style.fill, marker: false, dashed };
    match &a.shape {
        Shape::Rect(r) => vec![outline(rect(*r).to_path(0.1))],
        Shape::Ellipse(r) => vec![outline(kurbo::Ellipse::from_rect(rect(*r)).to_path(0.1))],
        Shape::Figure { bounds, kind, corners } => {
            let mut path = polyline(&figure_outline(*bounds, *kind, *corners));
            path.close_path();
            vec![outline(path)]
        }
        Shape::Line(p, q) => vec![line(polyline(&[*p, *q]))],
        Shape::Arrow(p, q) => vec![line(polyline(&[*p, *q])), head(*p, *q)],
        Shape::DoubleArrow(p, q) => vec![line(polyline(&[*p, *q])), head(*p, *q), head(*q, *p)],
        Shape::Freehand(points) => vec![line(smooth(points))],
        Shape::Ink(strokes) => strokes.iter().map(|s| line(smooth(s))).collect(),
        Shape::Highlight(rects) => {
            let mut path = BezPath::new();
            for r in rects {
                path.extend(rect(*r).path_elements(0.1));
            }
            vec![Prim {
                path,
                stroke: None,
                fill: Some(style.fill.unwrap_or(style.stroke)),
                marker: true,
                dashed: false,
            }]
        }
        Shape::Text { bounds, .. } => match style.fill {
            Some(fill) => {
                vec![Prim {
                    path: rect(*bounds).to_path(0.1),
                    stroke: None,
                    fill: Some(fill),
                    marker: false,
                    dashed: false,
                }]
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
        Shape::Arrow(..) | Shape::DoubleArrow(..) | Shape::Line(..) | Shape::Freehand(_) | Shape::Ink(_) => {
            a.style.width.max(1.0)
        }
        // A star's sharp points reach past the box by more than half the line.
        Shape::Figure { .. } => a.style.width * 2.0 + 1.0,
        _ => a.style.width / 2.0 + 1.0,
    };
    a.bounds().inflate(reach)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::model::Style;

    #[test]
    fn a_drawn_line_is_smooth_and_keeps_its_ends() {
        let points = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (20.0, 10.0)].map(|(x, y)| Point::new(x, y));
        let path = smooth(&points);
        let els = path.elements();
        assert!(matches!(els[0], kurbo::PathEl::MoveTo(p) if p == kurbo::Point::ZERO));
        assert_eq!(els.iter().filter(|e| matches!(e, kurbo::PathEl::QuadTo(..))).count(), 2);
        assert!(matches!(els.last(), Some(kurbo::PathEl::LineTo(p)) if *p == kurbo::Point::new(20.0, 10.0)));
        let b = path.bounding_box();
        assert!(b.x0 >= 0.0 && b.y0 >= 0.0 && b.x1 <= 20.0 && b.y1 <= 10.0, "{b:?}");
    }

    #[test]
    fn every_shape_has_something_to_draw() {
        let style = Style::default();
        let shapes = [
            Shape::Rect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            Shape::Ellipse(Rect::new(0.0, 0.0, 10.0, 10.0)),
            Shape::Line(Point::new(0.0, 0.0), Point::new(5.0, 5.0)),
            Shape::Arrow(Point::new(0.0, 0.0), Point::new(50.0, 0.0)),
            Shape::DoubleArrow(Point::new(0.0, 0.0), Point::new(50.0, 0.0)),
            Shape::Figure {
                bounds: Rect::new(0.0, 0.0, 30.0, 20.0),
                kind: crate::annotate::model::Figure::Star,
                corners: 5,
            },
            Shape::Figure {
                bounds: Rect::new(0.0, 0.0, 30.0, 20.0),
                kind: crate::annotate::model::Figure::Bubble,
                corners: 0,
            },
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
