//! What is selected of a picture: a rectangle, a shape drawn freehand, or the pixels of a colour,
//! and possibly everything but that. Positions are picture pixels.

use std::sync::Arc;

use super::crop::Rect;

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect(Rect),
    /// The corners of a closed outline.
    Lasso(Vec<(f64, f64)>),
    /// One flag per pixel of `rect` (row by row).
    Pixels {
        rect: Rect,
        on: Arc<Vec<bool>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub shape: Shape,
    /// Everything but the shape.
    pub inverted: bool,
}

/// Whether `(x, y)` is inside the polygon `points` (even-odd).
fn in_polygon(points: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = points.len().wrapping_sub(1);
    for (i, &(xi, yi)) in points.iter().enumerate() {
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

impl Shape {
    /// The smallest rectangle around it, on whole pixels and inside a picture of `size`.
    pub fn bounds(&self, size: (u32, u32)) -> Rect {
        let (w, h) = (f64::from(size.0), f64::from(size.1));
        let (x0, y0, x1, y1) = match self {
            Shape::Rect(r) | Shape::Pixels { rect: r, .. } => (r.x, r.y, r.right(), r.bottom()),
            Shape::Lasso(points) => {
                points.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |(a, b, c, d), &(x, y)| {
                    (a.min(x), b.min(y), c.max(x), d.max(y))
                })
            }
        };
        let (x0, y0) = (x0.clamp(0.0, w).floor(), y0.clamp(0.0, h).floor());
        let (x1, y1) = (x1.clamp(0.0, w).ceil(), y1.clamp(0.0, h).ceil());
        Rect { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
    }

    /// Whether the pixel at `(x, y)` is in it (its middle, for an outline).
    pub fn contains(&self, x: u32, y: u32) -> bool {
        let (px, py) = (f64::from(x), f64::from(y));
        match self {
            Shape::Rect(r) => px >= r.x && py >= r.y && px < r.right() && py < r.bottom(),
            Shape::Lasso(points) => points.len() > 2 && in_polygon(points, px + 0.5, py + 0.5),
            Shape::Pixels { rect, on } => {
                let (dx, dy) = (px - rect.x, py - rect.y);
                dx >= 0.0
                    && dy >= 0.0
                    && dx < rect.w
                    && dy < rect.h
                    && on.get(dy as usize * rect.w as usize + dx as usize).copied().unwrap_or(false)
            }
        }
    }
}

/// The pixels, around `seed`, of about its colour: those that differ from it by at most `tolerance`
/// in every channel, and touch it through others that do.
pub fn by_colour(rgba: &[u8], size: (u32, u32), seed: (u32, u32), tolerance: u8) -> Option<Shape> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    if seed.0 as usize >= w || seed.1 as usize >= h || rgba.len() < w * h * 4 {
        return None;
    }
    let at = |x: usize, y: usize| &rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
    let target: [u8; 4] = at(seed.0 as usize, seed.1 as usize).try_into().ok()?;
    let near = |p: &[u8]| p.iter().zip(target).all(|(a, b)| a.abs_diff(b) <= tolerance);
    let mut on = vec![false; w * h];
    let mut stack = vec![(seed.0 as usize, seed.1 as usize)];
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    while let Some((x, y)) = stack.pop() {
        if on[y * w + x] || !near(at(x, y)) {
            continue;
        }
        // The whole run of this row, then the rows above and below it.
        let (mut l, mut r) = (x, x);
        while l > 0 && !on[y * w + l - 1] && near(at(l - 1, y)) {
            l -= 1;
        }
        while r + 1 < w && !on[y * w + r + 1] && near(at(r + 1, y)) {
            r += 1;
        }
        on[y * w + l..=y * w + r].fill(true);
        (x0, x1, y0, y1) = (x0.min(l), x1.max(r), y0.min(y), y1.max(y));
        for row in [y.wrapping_sub(1), y + 1].into_iter().filter(|&row| row < h) {
            stack.extend((l..=r).filter(|&c| !on[row * w + c]).map(|c| (c, row)));
        }
    }
    let rect = Rect { x: x0 as f64, y: y0 as f64, w: (x1 - x0 + 1) as f64, h: (y1 - y0 + 1) as f64 };
    let bits = (y0..=y1).flat_map(|y| on[y * w + x0..=y * w + x1].to_vec()).collect();
    Some(Shape::Pixels { rect, on: Arc::new(bits) })
}

impl Selection {
    pub fn new(shape: Shape) -> Self {
        Self { shape, inverted: false }
    }

    /// The part of a picture of `size` that has anything selected in it.
    pub fn bounds(&self, size: (u32, u32)) -> Rect {
        if self.inverted {
            Rect { x: 0.0, y: 0.0, w: f64::from(size.0), h: f64::from(size.1) }
        } else {
            self.shape.bounds(size)
        }
    }

    /// Whether it is a plain rectangle, which a crop takes as it is.
    pub fn is_rect(&self) -> bool {
        matches!(self.shape, Shape::Rect(_)) && !self.inverted
    }

    /// Whether each pixel of `bounds` is selected, row by row: outlines a row at a time, not a
    /// pixel at a time, as a picture has millions.
    fn mask(&self, size: (u32, u32)) -> (Rect, Vec<bool>) {
        let b = self.bounds(size);
        let (x0, y0, w, h) = (b.x as u32, b.y as u32, b.w as usize, b.h as usize);
        let mut on = vec![false; w * h];
        match &self.shape {
            Shape::Lasso(points) if points.len() > 2 => {
                let edges: Vec<_> = points.iter().zip(points.iter().cycle().skip(1)).collect();
                let mut cuts = Vec::new();
                for row in 0..h {
                    let y = f64::from(y0) + row as f64 + 0.5;
                    cuts.clear();
                    for &(&(xa, ya), &(xb, yb)) in &edges {
                        if (ya > y) != (yb > y) {
                            cuts.push(xa + (y - ya) / (yb - ya) * (xb - xa));
                        }
                    }
                    cuts.sort_by(f64::total_cmp);
                    for pair in cuts.chunks(2) {
                        if let [from, to] = pair {
                            let first = (from - f64::from(x0) - 0.5).ceil().max(0.0) as usize;
                            let last = ((to - f64::from(x0) - 0.5).ceil().max(0.0) as usize).min(w);
                            if first < last {
                                on[row * w + first..row * w + last].fill(true);
                            }
                        }
                    }
                }
            }
            shape => {
                for row in 0..h {
                    for col in 0..w {
                        on[row * w + col] = shape.contains(x0 + col as u32, y0 + row as u32);
                    }
                }
            }
        }
        if self.inverted {
            on.iter_mut().for_each(|o| *o = !*o);
        }
        (b, on)
    }

    #[cfg(test)]
    fn contains(&self, x: u32, y: u32) -> bool {
        self.shape.contains(x, y) != self.inverted
    }

    /// What is selected of `rgba` (`size` pixels): its bounds, with what is not selected clear.
    pub fn cut(&self, rgba: &[u8], size: (u32, u32)) -> Option<(u32, u32, Vec<u8>)> {
        let (b, on) = self.mask(size);
        let (x0, y0, w, h) = (b.x as usize, b.y as usize, b.w as usize, b.h as usize);
        if w == 0 || h == 0 {
            return None;
        }
        let mut out = Vec::with_capacity(w * h * 4);
        for row in 0..h {
            for col in 0..w {
                let at = ((y0 + row) * size.0 as usize + x0 + col) * 4;
                let pixel = rgba.get(at..at + 4)?;
                out.extend_from_slice(if on[row * w + col] { pixel } else { &[0, 0, 0, 0] });
            }
        }
        Some((w as u32, h as u32, out))
    }

    /// `rgba` with what is selected made clear.
    pub fn erase(&self, rgba: &mut [u8], size: (u32, u32)) {
        let (b, on) = self.mask(size);
        let (x0, y0, w) = (b.x as usize, b.y as usize, b.w as usize);
        for (i, _) in on.iter().enumerate().filter(|(_, o)| **o) {
            let at = ((y0 + i / w) * size.0 as usize + x0 + i % w) * 4;
            if let Some(pixel) = rgba.get_mut(at..at + 4) {
                pixel.copy_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    /// A picture of its bounds in `colour` where it is selected, to show it by.
    pub fn tint(&self, size: (u32, u32), colour: [u8; 4]) -> (u32, u32, Vec<u8>) {
        let (b, on) = self.mask(size);
        (b.w as u32, b.h as u32, on.iter().flat_map(|&o| if o { colour } else { [0; 4] }).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (4, 4);

    fn picture() -> Vec<u8> {
        (0..16u8).flat_map(|i| [i, i, i, 255]).collect()
    }

    #[test]
    fn a_rectangle_and_its_inverse() {
        let s = Selection::new(Shape::Rect(Rect { x: 1.0, y: 1.0, w: 2.0, h: 2.0 }));
        let (w, h, part) = s.cut(&picture(), SIZE).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(part.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), [5, 6, 9, 10]);
        let inverse = Selection { inverted: true, ..s };
        assert_eq!(inverse.bounds(SIZE), Rect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 });
        let (_, _, all) = inverse.cut(&picture(), SIZE).unwrap();
        assert_eq!(all[5 * 4 + 3], 0, "the hole is clear");
        assert_eq!(all[3], 255);
    }

    #[test]
    fn a_lasso_takes_the_pixels_whose_middles_are_inside() {
        // A triangle over the top-left half.
        let s = Selection::new(Shape::Lasso(vec![(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)]));
        assert_eq!(s.bounds(SIZE), Rect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 });
        assert!(s.contains(0, 0) && s.contains(2, 0) && s.contains(0, 2));
        assert!(!s.contains(3, 3) && !s.contains(2, 2));
        // Row by row, the same pixels.
        let (_, on) = s.mask(SIZE);
        let one_by_one: Vec<bool> =
            (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).map(|(x, y)| s.contains(x, y)).collect();
        assert_eq!(on, one_by_one);
        let mut rgba = picture();
        s.erase(&mut rgba, SIZE);
        assert_eq!(rgba[3], 0);
        assert_eq!(rgba[15 * 4 + 3], 255);
    }

    #[test]
    fn by_colour_takes_the_touching_pixels_of_about_that_colour() {
        // A white frame around black, and one black pixel in the corner that touches nothing black.
        #[rustfmt::skip]
        let grey = [
            0, 255, 255, 255,
            255, 0, 0, 255,
            255, 0, 10, 255,
            255, 255, 255, 255,
        ];
        let rgba: Vec<u8> = grey.iter().flat_map(|&g| [g, g, g, 255]).collect();
        let s = Selection::new(by_colour(&rgba, SIZE, (1, 1), 4).unwrap());
        assert_eq!(s.bounds(SIZE), Rect { x: 1.0, y: 1.0, w: 2.0, h: 2.0 });
        assert!(s.contains(1, 2) && !s.contains(2, 2) && !s.contains(0, 0), "not too far, and not apart");
        let wider = Selection::new(by_colour(&rgba, SIZE, (1, 1), 12).unwrap());
        assert!(wider.contains(2, 2));
        assert!(by_colour(&rgba, SIZE, (9, 9), 4).is_none());
    }

    #[test]
    fn pixels_are_taken_as_flagged() {
        let on = Arc::new(vec![true, false, false, true]);
        let s = Selection::new(Shape::Pixels { rect: Rect { x: 2.0, y: 2.0, w: 2.0, h: 2.0 }, on });
        assert!(s.contains(2, 2) && !s.contains(3, 2) && s.contains(3, 3) && !s.contains(0, 0));
    }
}
