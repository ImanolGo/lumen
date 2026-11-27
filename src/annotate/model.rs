//! The annotation model: shapes drawn over an image or a PDF page.
//!
//! Pure Rust with no GTK, so everything here is unit-tested. Coordinates are
//! document coordinates with the origin at the top-left and y pointing down:
//! pixels of the displayed image, or points on a PDF page. Converting to a
//! PDF's bottom-up space happens only where annotations are written to a file.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn distance_to(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// The rectangle with these two opposite corners, whichever way round they are.
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        (self.x..=self.right()).contains(&p.x) && (self.y..=self.bottom()).contains(&p.y)
    }

    pub fn inflate(&self, by: f64) -> Self {
        Self::new(self.x - by, self.y - by, self.w + 2.0 * by, self.h + 2.0 * by)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let (x, y) = (self.x.min(other.x), self.y.min(other.y));
        Rect::new(x, y, self.right().max(other.right()) - x, self.bottom().max(other.bottom()) - y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const RED: Self = Self::rgba(0.9, 0.1, 0.1, 1.0);
    /// The highlighter: the yellow of Lumen's icon.
    pub const YELLOW: Self = Self::rgba(0.965, 0.827, 0.176, 1.0);
    /// The default for shapes, lines and text: the amber of Lumen's icon.
    pub const AMBER: Self = Self::rgba(0.898, 0.647, 0.039, 1.0);
}

#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub stroke: Color,
    pub fill: Option<Color>,
    /// Line width in document units.
    pub width: f64,
    /// Text size in document units.
    pub font_size: f64,
    /// Outlines and lines drawn as dashes.
    pub dashed: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self { stroke: Color::AMBER, fill: None, width: 3.0, font_size: 18.0, dashed: false }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect(Rect),
    Ellipse(Rect),
    Line(Point, Point),
    Arrow(Point, Point),
    /// An arrow with a head at both ends.
    DoubleArrow(Point, Point),
    /// A closed outline that fills `bounds`: a star, a polygon, a speech bubble…
    Figure {
        bounds: Rect,
        kind: Figure,
        /// The points of a star or the sides of a polygon.
        corners: u8,
    },
    Freehand(Vec<Point>),
    /// Several strokes that belong together, such as a signature.
    Ink(Vec<Vec<Point>>),
    Text {
        bounds: Rect,
        text: String,
    },
    /// Translucent marker over one or more rectangles (a block of text may span lines).
    Highlight(Vec<Rect>),
}

/// The closed outlines besides rectangles and ellipses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Figure {
    Rounded,
    Star,
    Polygon,
    /// A speech bubble, its tail at the bottom left.
    Bubble,
}

impl Figure {
    /// The corners a new figure of this kind has.
    pub fn default_corners(self) -> u8 {
        match self {
            Self::Star => 5,
            Self::Polygon => 6,
            Self::Rounded | Self::Bubble => 0,
        }
    }

    /// Its name in files (and the control interface).
    pub fn name(self) -> &'static str {
        match self {
            Self::Rounded => "rounded",
            Self::Star => "star",
            Self::Polygon => "polygon",
            Self::Bubble => "bubble",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Rounded, Self::Star, Self::Polygon, Self::Bubble].into_iter().find(|f| f.name() == name)
    }

    /// Whether the number of corners can be chosen.
    pub fn counts(self) -> bool {
        matches!(self, Self::Star | Self::Polygon)
    }
}

/// The fewest and most corners of a star or polygon.
pub const CORNERS: std::ops::RangeInclusive<u8> = 3..=12;

/// Points on each rounded corner.
const ARC_STEPS: usize = 10;

/// A quarter circle around `c` of radius `r`, from angle `from` (radians), as `steps` points.
fn arc(out: &mut Vec<Point>, c: Point, rx: f64, ry: f64, from: f64, steps: usize) {
    for i in 0..=steps {
        let a = from + std::f64::consts::FRAC_PI_2 * i as f64 / steps as f64;
        out.push(Point::new(c.x + rx * a.cos(), c.y + ry * a.sin()));
    }
}

/// A rounded rectangle's outline, clockwise from the top-right corner's start; `bottom`
/// is where its lower edge is (a speech bubble's body is shorter than its box).
fn rounded(r: Rect, bottom: f64, out: &mut Vec<Point>) {
    use std::f64::consts::PI;
    let h = bottom - r.y;
    let radius = (r.w.min(h) * 0.2).min(r.w / 2.0).min(h / 2.0).max(0.0);
    const STEPS: usize = ARC_STEPS;
    arc(out, Point::new(r.right() - radius, r.y + radius), radius, radius, -PI / 2.0, STEPS);
    arc(out, Point::new(r.right() - radius, bottom - radius), radius, radius, 0.0, STEPS);
    arc(out, Point::new(r.x + radius, bottom - radius), radius, radius, PI / 2.0, STEPS);
    arc(out, Point::new(r.x + radius, r.y + radius), radius, radius, PI, STEPS);
}

/// The outline of a figure as a closed polygon (the last point joins the first). It fills
/// `bounds` exactly, so the box it is read back from is the one it was drawn in.
pub fn figure_outline(bounds: Rect, kind: Figure, corners: u8) -> Vec<Point> {
    use std::f64::consts::{PI, TAU};
    let n = corners.clamp(*CORNERS.start(), *CORNERS.end()) as usize;
    let mut out = Vec::new();
    match kind {
        Figure::Rounded => rounded(bounds, bounds.bottom(), &mut out),
        Figure::Bubble => {
            // The body is the top four fifths; the tail hangs from its lower edge.
            let body = bounds.y + bounds.h * 0.8;
            rounded(bounds, body, &mut out);
            // The lower edge runs right to left, between the second and third corners.
            let at = 2 * (ARC_STEPS + 1);
            let tail = [
                Point::new(bounds.x + bounds.w * 0.38, body),
                Point::new(bounds.x + bounds.w * 0.18, bounds.bottom()),
                Point::new(bounds.x + bounds.w * 0.22, body),
            ];
            out.splice(at..at, tail);
            return out;
        }
        Figure::Polygon | Figure::Star => {
            let star = kind == Figure::Star;
            let count = if star { 2 * n } else { n };
            // A classic star's inner points are this far out.
            let inner = (PI / n as f64).cos() * if n > 4 { 0.5 } else { 0.42 };
            for i in 0..count {
                let a = -PI / 2.0 + TAU * i as f64 / count as f64;
                let r = if star && i % 2 == 1 { inner } else { 1.0 };
                out.push(Point::new(r * a.cos(), r * a.sin()));
            }
            // Stretched to fill the box.
            let (x0, x1) = out.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
            let (y0, y1) = out.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
            for p in &mut out {
                p.x = bounds.x + (p.x - x0) / (x1 - x0) * bounds.w;
                p.y = bounds.y + (p.y - y0) / (y1 - y0) * bounds.h;
            }
        }
    }
    out
}

/// Whether `p` is inside the closed polygon `points` (even-odd).
fn inside(points: &[Point], p: Point) -> bool {
    let mut inside = false;
    let mut j = points.len().wrapping_sub(1);
    for (i, a) in points.iter().enumerate() {
        let b = points[j];
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Which grab point of a selected annotation is being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// A line or arrow end: 0 is the start, 1 the end.
    End(usize),
    /// An edge or corner of the bounding box.
    Edges { left: bool, right: bool, top: bool, bottom: bool },
}

/// A change to the coordinate system that annotations have to follow, for
/// example when the image under them is rotated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GeometryChange {
    /// A quarter turn clockwise of an image that was `height` tall.
    RotateCw {
        height: f64,
    },
    /// A quarter turn counter-clockwise of an image that was `width` wide.
    RotateCcw {
        width: f64,
    },
    FlipH {
        width: f64,
    },
    FlipV {
        height: f64,
    },
    /// The top-left corner of the crop becomes the new origin.
    Crop {
        x: f64,
        y: f64,
    },
    Scale {
        sx: f64,
        sy: f64,
    },
}

impl GeometryChange {
    fn map(self, p: Point) -> Point {
        match self {
            Self::RotateCw { height } => Point::new(height - p.y, p.x),
            Self::RotateCcw { width } => Point::new(p.y, width - p.x),
            Self::FlipH { width } => Point::new(width - p.x, p.y),
            Self::FlipV { height } => Point::new(p.x, height - p.y),
            Self::Crop { x, y } => Point::new(p.x - x, p.y - y),
            Self::Scale { sx, sy } => Point::new(p.x * sx, p.y * sy),
        }
    }

    fn map_rect(self, r: Rect) -> Rect {
        Rect::from_corners(self.map(Point::new(r.x, r.y)), self.map(Point::new(r.right(), r.bottom())))
    }

    /// How much line widths change.
    fn width_factor(self) -> f64 {
        match self {
            Self::Scale { sx, sy } => (sx + sy) / 2.0,
            _ => 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub id: u64,
    pub shape: Shape,
    pub style: Style,
}

/// How far an arrow's head extends, as a multiple of the line width (at least `MIN_HEAD`).
pub const HEAD_WIDTHS: f64 = 5.0;
pub const MIN_HEAD: f64 = 12.0;

/// The two barbs of an arrow pointing from `from` to `to`.
pub fn arrow_head(from: Point, to: Point, width: f64) -> [Point; 2] {
    let length = (HEAD_WIDTHS * width).max(MIN_HEAD);
    let angle = (from.y - to.y).atan2(from.x - to.x);
    let spread = std::f64::consts::PI / 7.0;
    [-spread, spread].map(|a| Point::new(to.x + length * (angle + a).cos(), to.y + length * (angle + a).sin()))
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return p.distance_to(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length_squared).clamp(0.0, 1.0);
    p.distance_to(Point::new(a.x + t * dx, a.y + t * dy))
}

impl Annotation {
    /// The extent of the geometry, not counting the stroke width.
    pub fn bounds(&self) -> Rect {
        match &self.shape {
            Shape::Rect(r) | Shape::Ellipse(r) => *r,
            Shape::Text { bounds, .. } | Shape::Figure { bounds, .. } => *bounds,
            Shape::Line(a, b) => Rect::from_corners(*a, *b),
            Shape::Arrow(a, b) => {
                let [h1, h2] = arrow_head(*a, *b, self.style.width);
                [h1, h2].iter().fold(Rect::from_corners(*a, *b), |r, p| r.union(&Rect::new(p.x, p.y, 0.0, 0.0)))
            }
            Shape::DoubleArrow(a, b) => {
                let heads =
                    arrow_head(*a, *b, self.style.width).into_iter().chain(arrow_head(*b, *a, self.style.width));
                heads.fold(Rect::from_corners(*a, *b), |r, p| r.union(&Rect::new(p.x, p.y, 0.0, 0.0)))
            }
            Shape::Freehand(points) => {
                points.iter().map(|p| Rect::new(p.x, p.y, 0.0, 0.0)).reduce(|a, b| a.union(&b)).unwrap_or_default()
            }
            Shape::Ink(strokes) => strokes
                .iter()
                .flatten()
                .map(|p| Rect::new(p.x, p.y, 0.0, 0.0))
                .reduce(|a, b| a.union(&b))
                .unwrap_or_default(),
            Shape::Highlight(rects) => rects.iter().copied().reduce(|a, b| a.union(&b)).unwrap_or_default(),
        }
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        let shift = |p: &mut Point| {
            p.x += dx;
            p.y += dy;
        };
        let shift_rect = |r: &mut Rect| {
            r.x += dx;
            r.y += dy;
        };
        match &mut self.shape {
            Shape::Rect(r) | Shape::Ellipse(r) => shift_rect(r),
            Shape::Text { bounds, .. } | Shape::Figure { bounds, .. } => shift_rect(bounds),
            Shape::Line(a, b) | Shape::Arrow(a, b) | Shape::DoubleArrow(a, b) => {
                shift(a);
                shift(b);
            }
            Shape::Freehand(points) => points.iter_mut().for_each(shift),
            Shape::Ink(strokes) => strokes.iter_mut().flatten().for_each(shift),
            Shape::Highlight(rects) => rects.iter_mut().for_each(shift_rect),
        }
    }

    /// Stretch the geometry so that what filled the current bounds fills `target`.
    pub fn fit_to(&mut self, target: Rect) {
        let from = self.bounds();
        // A zero-sized extent (a horizontal line) cannot be stretched along that axis.
        let scale = |size_from: f64, size_to: f64| {
            if size_from.abs() < 1e-9 { 1.0 } else { size_to / size_from }
        };
        let (sx, sy) = (scale(from.w, target.w), scale(from.h, target.h));
        let map = |p: Point| {
            Point::new(
                target.x + if from.w.abs() < 1e-9 { 0.0 } else { (p.x - from.x) * sx },
                target.y + if from.h.abs() < 1e-9 { 0.0 } else { (p.y - from.y) * sy },
            )
        };
        let map_rect = |r: Rect| Rect::from_corners(map(Point::new(r.x, r.y)), map(Point::new(r.right(), r.bottom())));
        match &mut self.shape {
            Shape::Rect(r) | Shape::Ellipse(r) => *r = target,
            Shape::Text { bounds, .. } | Shape::Figure { bounds, .. } => *bounds = target,
            // An arrow's bounds include its head; scale the shaft's ends instead.
            Shape::Line(a, b) | Shape::Arrow(a, b) | Shape::DoubleArrow(a, b) => {
                *a = map(*a);
                *b = map(*b);
            }
            Shape::Freehand(points) => points.iter_mut().for_each(|p| *p = map(*p)),
            Shape::Ink(strokes) => strokes.iter_mut().flatten().for_each(|p| *p = map(*p)),
            Shape::Highlight(rects) => rects.iter_mut().for_each(|r| *r = map_rect(*r)),
        }
    }

    /// Follow a change of the coordinate system underneath the annotation.
    pub fn apply(&mut self, change: GeometryChange) {
        match &mut self.shape {
            Shape::Rect(r) | Shape::Ellipse(r) => *r = change.map_rect(*r),
            Shape::Text { bounds, .. } | Shape::Figure { bounds, .. } => *bounds = change.map_rect(*bounds),
            Shape::Line(a, b) | Shape::Arrow(a, b) | Shape::DoubleArrow(a, b) => {
                *a = change.map(*a);
                *b = change.map(*b);
            }
            Shape::Freehand(points) => points.iter_mut().for_each(|p| *p = change.map(*p)),
            Shape::Ink(strokes) => strokes.iter_mut().flatten().for_each(|p| *p = change.map(*p)),
            Shape::Highlight(rects) => rects.iter_mut().for_each(|r| *r = change.map_rect(*r)),
        }
        self.style.width *= change.width_factor();
    }

    /// Whether `p` is on the annotation, within `tolerance` document units.
    pub fn hit(&self, p: Point, tolerance: f64) -> bool {
        let reach = tolerance + self.style.width / 2.0;
        match &self.shape {
            Shape::Rect(r) => {
                if self.style.fill.is_some() {
                    r.inflate(reach).contains(p)
                } else {
                    r.inflate(reach).contains(p) && !r.inflate(-reach).contains(p)
                }
            }
            Shape::Ellipse(r) => {
                let (cx, cy) = (r.center().x, r.center().y);
                let (rx, ry) = ((r.w / 2.0).max(1e-9), (r.h / 2.0).max(1e-9));
                // Scaled distance from the centre: 1 on the outline.
                let d = (((p.x - cx) / rx).powi(2) + ((p.y - cy) / ry).powi(2)).sqrt();
                if self.style.fill.is_some() {
                    d <= 1.0 + reach / rx.min(ry)
                } else {
                    (d - 1.0).abs() <= reach / rx.min(ry)
                }
            }
            Shape::Line(a, b) => distance_to_segment(p, *a, *b) <= reach,
            Shape::Arrow(a, b) => {
                distance_to_segment(p, *a, *b) <= reach
                    || arrow_head(*a, *b, self.style.width).iter().any(|h| distance_to_segment(p, *b, *h) <= reach)
            }
            Shape::DoubleArrow(a, b) => {
                let w = self.style.width;
                distance_to_segment(p, *a, *b) <= reach
                    || arrow_head(*a, *b, w).iter().any(|h| distance_to_segment(p, *b, *h) <= reach)
                    || arrow_head(*b, *a, w).iter().any(|h| distance_to_segment(p, *a, *h) <= reach)
            }
            Shape::Figure { bounds, kind, corners } => {
                let outline = figure_outline(*bounds, *kind, *corners);
                let on_edge = outline
                    .iter()
                    .zip(outline.iter().cycle().skip(1))
                    .any(|(a, b)| distance_to_segment(p, *a, *b) <= reach);
                on_edge || (self.style.fill.is_some() && inside(&outline, p))
            }
            Shape::Freehand(points) => match points.as_slice() {
                [] => false,
                [only] => p.distance_to(*only) <= reach,
                _ => points.windows(2).any(|w| distance_to_segment(p, w[0], w[1]) <= reach),
            },
            // A signature is picked up anywhere inside it, not just on its thin strokes.
            Shape::Ink(_) => self.bounds().inflate(tolerance).contains(p),
            Shape::Text { bounds, .. } => bounds.inflate(tolerance).contains(p),
            Shape::Highlight(rects) => rects.iter().any(|r| r.inflate(tolerance).contains(p)),
        }
    }

    /// Grab points to show when the annotation is selected.
    pub fn handles(&self) -> Vec<(Handle, Point)> {
        match &self.shape {
            Shape::Line(a, b) | Shape::Arrow(a, b) | Shape::DoubleArrow(a, b) => {
                vec![(Handle::End(0), *a), (Handle::End(1), *b)]
            }
            _ => {
                let r = self.bounds();
                let (cx, cy) = (r.center().x, r.center().y);
                let edges = |left, right, top, bottom| Handle::Edges { left, right, top, bottom };
                vec![
                    (edges(true, false, true, false), Point::new(r.x, r.y)),
                    (edges(false, false, true, false), Point::new(cx, r.y)),
                    (edges(false, true, true, false), Point::new(r.right(), r.y)),
                    (edges(true, false, false, false), Point::new(r.x, cy)),
                    (edges(false, true, false, false), Point::new(r.right(), cy)),
                    (edges(true, false, false, true), Point::new(r.x, r.bottom())),
                    (edges(false, false, false, true), Point::new(cx, r.bottom())),
                    (edges(false, true, false, true), Point::new(r.right(), r.bottom())),
                ]
            }
        }
    }

    pub fn handle_at(&self, p: Point, tolerance: f64) -> Option<Handle> {
        self.handles()
            .into_iter()
            .map(|(handle, at)| (handle, at.distance_to(p)))
            .filter(|(_, d)| *d <= tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(handle, _)| handle)
    }

    /// Drag `handle` to `to`, starting from the annotation as it was when the drag began.
    pub fn drag_handle(&mut self, original: &Annotation, handle: Handle, to: Point) {
        *self = original.clone();
        match (handle, &mut self.shape) {
            (Handle::End(i), Shape::Line(a, b) | Shape::Arrow(a, b) | Shape::DoubleArrow(a, b)) => {
                if i == 0 {
                    *a = to;
                } else {
                    *b = to;
                }
            }
            (Handle::Edges { left, right, top, bottom }, _) => {
                let r = original.bounds();
                let (mut l, mut t, mut rr, mut b) = (r.x, r.y, r.right(), r.bottom());
                if left {
                    l = to.x;
                }
                if right {
                    rr = to.x;
                }
                if top {
                    t = to.y;
                }
                if bottom {
                    b = to.y;
                }
                // Dragging past the opposite edge flips the shape instead of inverting it.
                self.fit_to(Rect::from_corners(Point::new(l, t), Point::new(rr, b)));
            }
            _ => {}
        }
    }
}

/// All annotations of one image or page, in drawing order (the last is on top).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnnotationLayer {
    pub items: Vec<Annotation>,
    /// The comment and author a PDF annotation came with, kept so saving does not drop them.
    pub notes: BTreeMap<u64, Note>,
    /// The PDF object (number, generation) an annotation was read from or saved as.
    pub origins: BTreeMap<u64, (u32, u16)>,
    next_id: u64,
}

/// What a PDF annotation says besides its shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Note {
    pub contents: Option<String>,
    pub author: Option<String>,
}

impl AnnotationLayer {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn add(&mut self, shape: Shape, style: Style) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(Annotation { id, shape, style });
        id
    }

    /// Put back an annotation taken from this layer (keeps its id).
    pub fn insert(&mut self, annotation: Annotation) {
        self.next_id = self.next_id.max(annotation.id);
        self.items.push(annotation);
    }

    pub fn remove(&mut self, id: u64) -> Option<Annotation> {
        let index = self.items.iter().position(|a| a.id == id)?;
        Some(self.items.remove(index))
    }

    pub fn get(&self, id: u64) -> Option<&Annotation> {
        self.items.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Annotation> {
        self.items.iter_mut().find(|a| a.id == id)
    }

    /// The topmost annotation under `p`.
    pub fn hit(&self, p: Point, tolerance: f64) -> Option<u64> {
        self.items.iter().rev().find(|a| a.hit(p, tolerance)).map(|a| a.id)
    }

    /// Move everything along with a change of the coordinate system.
    pub fn apply(&mut self, change: GeometryChange) {
        self.items.iter_mut().for_each(|a| a.apply(change));
    }
}

impl AnnotationLayer {
    /// Same annotations (and notes), whatever ids were handed out on the way.
    pub fn same_as(&self, other: &AnnotationLayer) -> bool {
        self.items == other.items && self.notes == other.notes
    }
}

/// Annotation layers keyed by page (images use page 0), with snapshot undo and redo.
#[derive(Debug, Clone, Default)]
pub struct Pages {
    pub layers: BTreeMap<usize, AnnotationLayer>,
}

impl Pages {
    pub fn layer_mut(&mut self, page: usize) -> &mut AnnotationLayer {
        self.layers.entry(page).or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.values().all(AnnotationLayer::is_empty)
    }
}

/// Undo and redo by snapshot: annotation sets are small, so keeping whole copies
/// is simpler and safer than inverting each change.
#[derive(Debug, Clone)]
pub struct History<T: Clone> {
    present: T,
    past: Vec<T>,
    future: Vec<T>,
}

impl<T: Clone> History<T> {
    pub fn new(initial: T) -> Self {
        Self { present: initial, past: Vec::new(), future: Vec::new() }
    }

    pub fn present(&self) -> &T {
        &self.present
    }

    /// Record `state` as the new present, dropping anything that could have been redone.
    /// The dropped states are returned.
    pub fn commit(&mut self, state: T) -> Vec<T> {
        self.past.push(std::mem::replace(&mut self.present, state));
        std::mem::take(&mut self.future)
    }

    /// Whether any state in the history, past, present or future, satisfies `matches`.
    pub fn any(&self, matches: impl Fn(&T) -> bool) -> bool {
        matches(&self.present) || self.past.iter().chain(&self.future).any(matches)
    }

    pub fn undo(&mut self) -> Option<&T> {
        let previous = self.past.pop()?;
        self.future.push(std::mem::replace(&mut self.present, previous));
        Some(&self.present)
    }

    pub fn redo(&mut self) -> Option<&T> {
        let next = self.future.pop()?;
        self.past.push(std::mem::replace(&mut self.present, next));
        Some(&self.present)
    }

    /// Change every state, past, present and future (when what they are measured against changes).
    pub fn map_all(&mut self, mut f: impl FnMut(&mut T)) {
        f(&mut self.present);
        self.past.iter_mut().chain(self.future.iter_mut()).for_each(f);
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Shape {
        Shape::Rect(Rect::new(x, y, w, h))
    }

    fn style() -> Style {
        Style { width: 2.0, ..Style::default() }
    }

    #[test]
    fn adding_removing_and_ids() {
        let mut layer = AnnotationLayer::default();
        let a = layer.add(rect(0.0, 0.0, 10.0, 10.0), style());
        let b = layer.add(Shape::Line(Point::new(0.0, 0.0), Point::new(5.0, 5.0)), style());
        assert_ne!(a, b);
        assert_eq!(layer.items.len(), 2);
        let removed = layer.remove(a).unwrap();
        assert!(layer.get(a).is_none() && layer.get(b).is_some());
        // Putting it back keeps the id and never reuses it for a new item.
        layer.insert(removed);
        let c = layer.add(rect(1.0, 1.0, 2.0, 2.0), style());
        assert!(c != a && c != b);
    }

    #[test]
    fn move_and_resize_a_rectangle() {
        let mut layer = AnnotationLayer::default();
        let id = layer.add(rect(10.0, 10.0, 40.0, 20.0), style());
        let item = layer.get_mut(id).unwrap();
        item.translate(5.0, -5.0);
        assert_eq!(item.bounds(), Rect::new(15.0, 5.0, 40.0, 20.0));

        // Drag the bottom-right corner.
        let original = item.clone();
        let corner = Handle::Edges { left: false, right: true, top: false, bottom: true };
        item.drag_handle(&original, corner, Point::new(75.0, 45.0));
        assert_eq!(item.bounds(), Rect::new(15.0, 5.0, 60.0, 40.0));

        // Dragging a corner past the opposite one flips instead of producing a negative size.
        item.drag_handle(&original, corner, Point::new(0.0, 0.0));
        assert_eq!(item.bounds(), Rect::new(0.0, 0.0, 15.0, 5.0));
    }

    #[test]
    fn line_ends_move_independently() {
        let mut line =
            Annotation { id: 1, shape: Shape::Line(Point::new(0.0, 0.0), Point::new(10.0, 0.0)), style: style() };
        let original = line.clone();
        line.drag_handle(&original, Handle::End(1), Point::new(10.0, 10.0));
        assert_eq!(line.shape, Shape::Line(Point::new(0.0, 0.0), Point::new(10.0, 10.0)));
        assert_eq!(line.handle_at(Point::new(10.5, 9.5), 2.0), Some(Handle::End(1)));
        assert_eq!(line.handle_at(Point::new(5.0, 5.0), 2.0), None);
    }

    #[test]
    fn resizing_scales_freehand_and_highlights_with_the_box() {
        let mut pen = Annotation {
            id: 1,
            shape: Shape::Freehand(vec![Point::new(0.0, 0.0), Point::new(10.0, 10.0), Point::new(20.0, 0.0)]),
            style: style(),
        };
        pen.fit_to(Rect::new(0.0, 0.0, 40.0, 20.0));
        assert_eq!(
            pen.shape,
            Shape::Freehand(vec![Point::new(0.0, 0.0), Point::new(20.0, 20.0), Point::new(40.0, 0.0)])
        );

        let mut marker = Annotation {
            id: 2,
            shape: Shape::Highlight(vec![Rect::new(0.0, 0.0, 10.0, 5.0), Rect::new(0.0, 5.0, 4.0, 5.0)]),
            style: style(),
        };
        marker.fit_to(Rect::new(10.0, 10.0, 20.0, 20.0));
        assert_eq!(marker.bounds(), Rect::new(10.0, 10.0, 20.0, 20.0));
    }

    #[test]
    fn hit_testing_outlines_fills_and_lines() {
        let mut layer = AnnotationLayer::default();
        let hollow = layer.add(rect(0.0, 0.0, 100.0, 100.0), style());
        // Only the outline of an unfilled rectangle is hit.
        assert!(layer.get(hollow).unwrap().hit(Point::new(50.0, 0.5), 3.0));
        assert!(!layer.get(hollow).unwrap().hit(Point::new(50.0, 50.0), 3.0));
        layer.get_mut(hollow).unwrap().style.fill = Some(Color::YELLOW);
        assert!(layer.get(hollow).unwrap().hit(Point::new(50.0, 50.0), 3.0));

        let line = layer.add(Shape::Line(Point::new(0.0, 200.0), Point::new(100.0, 200.0)), style());
        assert!(layer.get(line).unwrap().hit(Point::new(50.0, 202.0), 3.0));
        assert!(!layer.get(line).unwrap().hit(Point::new(50.0, 210.0), 3.0));
        // Beyond the end of a segment does not count.
        assert!(!layer.get(line).unwrap().hit(Point::new(120.0, 200.0), 3.0));

        let ring = layer.add(Shape::Ellipse(Rect::new(300.0, 0.0, 100.0, 60.0)), style());
        assert!(layer.get(ring).unwrap().hit(Point::new(300.5, 30.0), 3.0), "left tip");
        assert!(!layer.get(ring).unwrap().hit(Point::new(350.0, 30.0), 3.0), "centre of a hollow ellipse");

        let pen = layer.add(
            Shape::Freehand(vec![Point::new(0.0, 300.0), Point::new(50.0, 320.0), Point::new(100.0, 300.0)]),
            style(),
        );
        assert!(layer.get(pen).unwrap().hit(Point::new(25.0, 310.0), 3.0));
        assert!(!layer.get(pen).unwrap().hit(Point::new(25.0, 330.0), 3.0));
    }

    #[test]
    fn the_topmost_item_wins_a_hit() {
        let mut layer = AnnotationLayer::default();
        let below = layer.add(rect(0.0, 0.0, 50.0, 50.0), Style { fill: Some(Color::YELLOW), ..style() });
        let above = layer.add(rect(25.0, 25.0, 50.0, 50.0), Style { fill: Some(Color::RED), ..style() });
        assert_eq!(layer.hit(Point::new(40.0, 40.0), 1.0), Some(above));
        assert_eq!(layer.hit(Point::new(10.0, 10.0), 1.0), Some(below));
        assert_eq!(layer.hit(Point::new(500.0, 500.0), 1.0), None);
    }

    #[test]
    fn rotating_the_image_carries_annotations_along() {
        // A 100×50 image with a dot near the top-left corner.
        let mut dot = Annotation { id: 1, shape: Shape::Freehand(vec![Point::new(10.0, 5.0)]), style: style() };
        dot.apply(GeometryChange::RotateCw { height: 50.0 });
        // After a clockwise quarter turn the image is 50×100 and the dot is near the top-right.
        assert_eq!(dot.shape, Shape::Freehand(vec![Point::new(45.0, 10.0)]));
        dot.apply(GeometryChange::RotateCcw { width: 50.0 });
        assert_eq!(dot.shape, Shape::Freehand(vec![Point::new(10.0, 5.0)]), "turning back restores it");

        let mut item = Annotation { id: 2, shape: rect(10.0, 10.0, 20.0, 10.0), style: style() };
        item.apply(GeometryChange::FlipH { width: 100.0 });
        assert_eq!(item.bounds(), Rect::new(70.0, 10.0, 20.0, 10.0));
        item.apply(GeometryChange::Crop { x: 50.0, y: 5.0 });
        assert_eq!(item.bounds(), Rect::new(20.0, 5.0, 20.0, 10.0));
        item.apply(GeometryChange::Scale { sx: 2.0, sy: 2.0 });
        assert_eq!(item.bounds(), Rect::new(40.0, 10.0, 40.0, 20.0));
        assert_eq!(item.style.width, 4.0, "strokes scale with the image");
    }

    #[test]
    fn arrow_head_barbs_trail_behind_the_tip() {
        let [a, b] = arrow_head(Point::new(0.0, 0.0), Point::new(100.0, 0.0), 2.0);
        assert!(a.x < 100.0 && b.x < 100.0, "barbs point back along the shaft");
        assert!((a.y + b.y).abs() < 1e-9, "symmetric about the shaft");
        let arrow =
            Annotation { id: 1, shape: Shape::Arrow(Point::new(0.0, 0.0), Point::new(100.0, 0.0)), style: style() };
        assert!(arrow.bounds().h > 0.0, "the head is part of the bounds");
        assert!(arrow.hit(Point::new(95.0, 3.0), 2.0));
    }

    #[test]
    fn figures_fill_their_box_and_are_hit_on_their_outline() {
        let bounds = Rect::new(10.0, 20.0, 100.0, 80.0);
        for kind in [Figure::Rounded, Figure::Star, Figure::Polygon, Figure::Bubble] {
            for corners in [3, 5, 6, 12] {
                let outline = figure_outline(bounds, kind, corners);
                let r = outline.iter().map(|p| Rect::new(p.x, p.y, 0.0, 0.0)).reduce(|a, b| a.union(&b)).unwrap();
                for (got, want) in [(r.x, 10.0), (r.y, 20.0), (r.w, 100.0), (r.h, 80.0)] {
                    assert!((got - want).abs() < 1e-6, "{kind:?} {corners}: {r:?}");
                }
            }
        }
        assert_eq!(figure_outline(bounds, Figure::Star, 5).len(), 10);
        assert_eq!(figure_outline(bounds, Figure::Polygon, 6).len(), 6);
        // A five-pointed star: its tip is at the top middle, its middle is empty unless filled.
        let mut star =
            Annotation { id: 1, shape: Shape::Figure { bounds, kind: Figure::Star, corners: 5 }, style: style() };
        assert!(star.hit(Point::new(60.0, 20.5), 2.0), "tip");
        assert!(!star.hit(Point::new(60.0, 60.0), 2.0));
        star.style.fill = Some(Color::RED);
        assert!(star.hit(Point::new(60.0, 60.0), 2.0));
        assert!(!star.hit(Point::new(12.0, 98.0), 2.0), "outside, in the corner of the box");
        // A bubble's tail reaches the bottom of its box.
        let bubble =
            Annotation { id: 2, shape: Shape::Figure { bounds, kind: Figure::Bubble, corners: 0 }, style: style() };
        assert!(bubble.hit(Point::new(28.0, 100.0), 2.0), "tail tip");
        assert!(!bubble.hit(Point::new(100.0, 100.0), 2.0));
    }

    #[test]
    fn a_double_arrow_has_a_head_at_each_end() {
        let a = Annotation {
            id: 1,
            shape: Shape::DoubleArrow(Point::new(0.0, 50.0), Point::new(100.0, 50.0)),
            style: style(),
        };
        assert!(a.hit(Point::new(5.0, 53.0), 2.0) && a.hit(Point::new(95.0, 53.0), 2.0));
        assert!(a.bounds().h > 0.0);
        assert_eq!(a.handles().len(), 2);
    }

    #[test]
    fn history_undoes_and_redoes_snapshots() {
        let mut history = History::new(Pages::default());
        let mut pages = history.present().clone();
        pages.layer_mut(0).add(rect(0.0, 0.0, 1.0, 1.0), style());
        history.commit(pages.clone());
        pages.layer_mut(0).add(rect(5.0, 5.0, 1.0, 1.0), style());
        history.commit(pages);

        assert_eq!(history.present().layers[&0].items.len(), 2);
        assert_eq!(history.undo().unwrap().layers[&0].items.len(), 1);
        assert!(history.undo().unwrap().is_empty());
        assert!(history.undo().is_none() && !history.can_undo());
        assert_eq!(history.redo().unwrap().layers[&0].items.len(), 1);
        // A new edit discards the redo branch.
        let mut other = history.present().clone();
        other.layer_mut(0).add(rect(9.0, 9.0, 1.0, 1.0), style());
        history.commit(other);
        assert!(!history.can_redo());
    }

    #[test]
    fn committing_hands_back_the_states_that_can_no_longer_be_reached() {
        let mut history = History::new(1);
        history.commit(2);
        history.commit(3);
        history.undo();
        history.undo();
        assert!(history.any(|&s| s == 3), "still reachable by redo");
        let mut dropped = history.commit(9);
        dropped.sort();
        assert_eq!(dropped, vec![2, 3]);
        assert!(!history.any(|&s| s == 3));
        assert!(history.any(|&s| s == 1) && history.any(|&s| s == 9));
    }
}
