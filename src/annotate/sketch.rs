//! The sketch tool: a shape drawn by hand becomes the clean shape it looks like (a line, an arrow,
//! an ellipse or a rectangle), or stays as it was drawn.

use super::model::{Point, Rect, Shape};

/// How far a point may stray from a clean shape, as a share of the shape's size, and still be it.
const LOOSE: f64 = 0.09;

fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// How far `p` is from the straight line through `a` and `b`.
fn off_line(p: Point, a: Point, b: Point) -> f64 {
    let length = distance(a, b);
    if length < 1e-9 {
        return distance(p, a);
    }
    ((b.x - a.x) * (a.y - p.y) - (a.x - p.x) * (b.y - a.y)).abs() / length
}

fn length(points: &[Point]) -> f64 {
    points.windows(2).map(|w| distance(w[0], w[1])).sum()
}

fn bounds(points: &[Point]) -> Rect {
    let (x0, y0, x1, y1) = points.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |(a, b, c, d), p| {
        (a.min(p.x), b.min(p.y), c.max(p.x), d.max(p.y))
    });
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// What a stroke drawn by hand looks like, if it looks like anything.
pub fn recognise(points: &[Point]) -> Option<Shape> {
    let (&first, &last) = (points.first()?, points.last()?);
    let total = length(points);
    if points.len() < 3 || total < 8.0 {
        return None;
    }
    let straight = |a: Point, b: Point, part: &[Point]| {
        let reach = distance(a, b);
        reach > 0.0 && part.iter().all(|&p| off_line(p, a, b) <= LOOSE * reach.max(20.0))
    };
    // Open, and straight from end to end: a line.
    if straight(first, last, points) && distance(first, last) > 0.8 * total {
        return Some(Shape::Line(first, last));
    }
    // Straight to the point farthest from the start, then a short hook back: an arrow.
    let (tip_at, &tip) =
        points.iter().enumerate().max_by(|a, b| distance(first, *a.1).total_cmp(&distance(first, *b.1)))?;
    let shaft = &points[..=tip_at];
    let head = length(&points[tip_at..]);
    if tip_at > 1 && straight(first, tip, shaft) && head > 0.08 * total && head < 0.45 * total {
        return Some(Shape::Arrow(first, tip));
    }
    // Closed: an ellipse or a rectangle in the box around it, whichever it is nearer.
    let b = bounds(points);
    let size = b.w.max(b.h);
    if distance(first, last) > 0.25 * size || b.w < 4.0 || b.h < 4.0 {
        return None;
    }
    let (cx, cy, rx, ry) = (b.x + b.w / 2.0, b.y + b.h / 2.0, b.w / 2.0, b.h / 2.0);
    let n = points.len() as f64;
    let ellipse = points.iter().map(|p| (((p.x - cx) / rx).hypot((p.y - cy) / ry) - 1.0).abs()).sum::<f64>() / n;
    let rectangle = points
        .iter()
        .map(|p| {
            ((p.x - b.x).abs().min((p.x - b.right()).abs()) / rx)
                .min((p.y - b.y).abs().min((p.y - b.bottom()).abs()) / ry)
        })
        .sum::<f64>()
        / n;
    match (ellipse.min(rectangle) < LOOSE * 1.5, rectangle < ellipse) {
        (false, _) => None,
        (true, true) => Some(Shape::Rect(b)),
        (true, false) => Some(Shape::Ellipse(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wobbly(points: impl IntoIterator<Item = (f64, f64)>) -> Vec<Point> {
        points
            .into_iter()
            .enumerate()
            .map(|(i, (x, y))| Point::new(x + (i % 3) as f64 * 0.8, y - (i % 2) as f64))
            .collect()
    }

    #[test]
    fn a_rough_line_is_a_line() {
        let line = wobbly((0..40).map(|i| (i as f64 * 5.0, i as f64 * 2.0)));
        assert!(matches!(recognise(&line), Some(Shape::Line(..))));
    }

    #[test]
    fn a_line_with_a_hook_at_its_end_is_an_arrow() {
        let mut stroke: Vec<(f64, f64)> = (0..40).map(|i| (i as f64 * 5.0, 100.0)).collect();
        stroke.extend((1..8).map(|i| (195.0 - i as f64 * 3.0, 100.0 - i as f64 * 3.0)));
        match recognise(&wobbly(stroke)) {
            Some(Shape::Arrow(a, b)) => assert!(a.x < 2.0 && b.x > 190.0, "{a:?} {b:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_rough_circle_is_an_ellipse_and_a_rough_box_a_rectangle() {
        let circle = wobbly((0..=60).map(|i| {
            let a = i as f64 / 60.0 * std::f64::consts::TAU;
            (100.0 + 80.0 * a.cos(), 100.0 + 50.0 * a.sin())
        }));
        assert!(matches!(recognise(&circle), Some(Shape::Ellipse(_))));
        let mut corners = Vec::new();
        for (from, to) in [
            ((0.0, 0.0), (150.0, 0.0)),
            ((150.0, 0.0), (150.0, 90.0)),
            ((150.0, 90.0), (0.0, 90.0)),
            ((0.0, 90.0), (0.0, 2.0)),
        ] {
            corners.extend((0..20).map(|i| {
                let t = f64::from(i) / 20.0;
                (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
            }));
        }
        assert!(matches!(recognise(&wobbly(corners)), Some(Shape::Rect(_))));
    }

    #[test]
    fn a_scribble_stays_a_scribble() {
        let zigzag: Vec<(f64, f64)> = (0..30).map(|i| (i as f64 * 6.0, if i % 2 == 0 { 0.0 } else { 40.0 })).collect();
        assert!(recognise(&wobbly(zigzag)).is_none());
        assert!(recognise(&[Point::new(0.0, 0.0), Point::new(1.0, 1.0)]).is_none());
    }
}
