//! The geometry of the crop rectangle: which edges a drag moves and how the
//! result is kept inside the image, above a minimum size, and at a fixed aspect
//! ratio if one is chosen. Pure functions, so every case can be tested.

/// A rectangle in image pixels (floating point while dragging).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    fn contains(&self, px: f64, py: f64) -> bool {
        (self.x..=self.right()).contains(&px) && (self.y..=self.bottom()).contains(&py)
    }
}

/// Smallest crop edge in pixels.
pub const MIN_SIZE: f64 = 8.0;

/// What a drag does to the rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    /// Move the whole rectangle.
    Move,
    /// Move the edges flagged here: a corner has two, a side one.
    Edges {
        left: bool,
        right: bool,
        top: bool,
        bottom: bool,
    },
}

impl Grab {
    /// The cursor name that describes the grab.
    pub fn cursor(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Edges {
                left,
                right,
                top,
                bottom,
            } => match (left, right, top, bottom) {
                (true, _, true, _) => "nw-resize",
                (_, true, true, _) => "ne-resize",
                (true, _, _, true) => "sw-resize",
                (_, true, _, true) => "se-resize",
                (true, ..) | (_, true, ..) => "ew-resize",
                _ => "ns-resize",
            },
        }
    }
}

/// What lies under the point (`px`, `py`), given `reach` image pixels of tolerance
/// around the handles: a corner, a side, the inside, or nothing.
pub fn grab_at(rect: Rect, px: f64, py: f64, reach: f64) -> Option<Grab> {
    let near = |a: f64, b: f64| (a - b).abs() <= reach;
    let within_x = px >= rect.x - reach && px <= rect.right() + reach;
    let within_y = py >= rect.y - reach && py <= rect.bottom() + reach;
    if !within_x || !within_y {
        return None;
    }
    let left = near(px, rect.x);
    let right = near(px, rect.right());
    let top = near(py, rect.y);
    let bottom = near(py, rect.bottom());
    // When the rectangle is tiny on screen both sides can be near; prefer the closer one.
    let (left, right) = if left && right {
        (px < rect.x + rect.w / 2.0, px >= rect.x + rect.w / 2.0)
    } else {
        (left, right)
    };
    let (top, bottom) = if top && bottom {
        (py < rect.y + rect.h / 2.0, py >= rect.y + rect.h / 2.0)
    } else {
        (top, bottom)
    };
    if left || right || top || bottom {
        Some(Grab::Edges {
            left,
            right,
            top,
            bottom,
        })
    } else if rect.contains(px, py) {
        Some(Grab::Move)
    } else {
        None
    }
}

/// The rectangle after dragging `grab` by (`dx`, `dy`) from `start`, inside an
/// image of size `bounds`, optionally keeping `aspect` (width / height).
pub fn drag(
    start: Rect,
    grab: Grab,
    dx: f64,
    dy: f64,
    bounds: (f64, f64),
    aspect: Option<f64>,
) -> Rect {
    let (bw, bh) = bounds;
    match grab {
        Grab::Move => Rect {
            x: (start.x + dx).clamp(0.0, (bw - start.w).max(0.0)),
            y: (start.y + dy).clamp(0.0, (bh - start.h).max(0.0)),
            ..start
        },
        Grab::Edges {
            left,
            right,
            top,
            bottom,
        } => {
            let min = MIN_SIZE.min(bw).min(bh);
            let mut l = start.x;
            let mut r = start.right();
            let mut t = start.y;
            let mut b = start.bottom();
            if left {
                l = (start.x + dx).clamp(0.0, r - min);
            }
            if right {
                r = (start.right() + dx).clamp(l + min, bw);
            }
            if top {
                t = (start.y + dy).clamp(0.0, b - min);
            }
            if bottom {
                b = (start.bottom() + dy).clamp(t + min, bh);
            }
            let free = Rect {
                x: l,
                y: t,
                w: r - l,
                h: b - t,
            };
            match aspect {
                Some(aspect) => keep_aspect(free, start, grab, aspect, bounds),
                None => free,
            }
        }
    }
}

/// Adjust a freely dragged rectangle to `aspect`, holding the part that did not move.
fn keep_aspect(free: Rect, start: Rect, grab: Grab, aspect: f64, bounds: (f64, f64)) -> Rect {
    let Grab::Edges {
        left,
        right,
        top,
        bottom,
    } = grab
    else {
        return free;
    };
    let horizontal = left || right;
    let vertical = top || bottom;
    let (bw, bh) = bounds;

    // The point that stays put: the opposite corner, or the middle of the opposite side.
    let anchor_x = if left {
        start.right()
    } else if right {
        start.x
    } else {
        start.x + start.w / 2.0
    };
    let anchor_y = if top {
        start.bottom()
    } else if bottom {
        start.y
    } else {
        start.y + start.h / 2.0
    };

    // The requested size, with the other dimension following the ratio.
    let (mut w, mut h) = (free.w, free.h);
    if horizontal && !vertical {
        h = w / aspect;
    } else if vertical && !horizontal {
        w = h * aspect;
    } else if w / aspect >= h {
        // Corner: the larger request wins.
        h = w / aspect;
    } else {
        w = h * aspect;
    }

    // The most room the anchored rectangle has in each direction.
    let room_w = match (left, right) {
        (true, _) => anchor_x,
        (_, true) => bw - anchor_x,
        _ => 2.0 * anchor_x.min(bw - anchor_x),
    };
    let room_h = match (top, bottom) {
        (true, _) => anchor_y,
        (_, true) => bh - anchor_y,
        _ => 2.0 * anchor_y.min(bh - anchor_y),
    };
    let scale = (room_w / w).min(room_h / h).min(1.0);
    let min_scale = (MIN_SIZE / w).max(MIN_SIZE / h);
    let scale = scale.max(min_scale.min(1.0));
    let (w, h) = (w * scale, h * scale);

    let x = if left {
        anchor_x - w
    } else if right {
        anchor_x
    } else {
        anchor_x - w / 2.0
    };
    let y = if top {
        anchor_y - h
    } else if bottom {
        anchor_y
    } else {
        anchor_y - h / 2.0
    };
    Rect { x, y, w, h }
}

/// The largest centred rectangle of ratio `aspect` inside `bounds`, shrunk to `fraction` of it.
pub fn initial(bounds: (f64, f64), aspect: Option<f64>, fraction: f64) -> Rect {
    let (bw, bh) = bounds;
    let (mut w, mut h) = (bw * fraction, bh * fraction);
    if let Some(aspect) = aspect {
        if w / aspect <= h {
            h = w / aspect;
        } else {
            w = h * aspect;
        }
    }
    Rect {
        x: (bw - w) / 2.0,
        y: (bh - h) / 2.0,
        w,
        h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: (f64, f64) = (400.0, 300.0);
    const START: Rect = Rect {
        x: 100.0,
        y: 100.0,
        w: 200.0,
        h: 100.0,
    };

    fn corner(left: bool, top: bool) -> Grab {
        Grab::Edges {
            left,
            right: !left,
            top,
            bottom: !top,
        }
    }

    fn inside(r: Rect) -> bool {
        r.x >= -1e-9
            && r.y >= -1e-9
            && r.right() <= BOUNDS.0 + 1e-9
            && r.bottom() <= BOUNDS.1 + 1e-9
    }

    #[test]
    fn moving_stays_inside_the_image() {
        let moved = drag(START, Grab::Move, 1000.0, -1000.0, BOUNDS, None);
        assert_eq!(
            moved,
            Rect {
                x: 200.0,
                y: 0.0,
                w: 200.0,
                h: 100.0
            }
        );
        let moved = drag(START, Grab::Move, 10.0, 20.0, BOUNDS, None);
        assert_eq!(
            (moved.x, moved.y, moved.w, moved.h),
            (110.0, 120.0, 200.0, 100.0)
        );
    }

    #[test]
    fn free_resize_moves_only_the_grabbed_edges() {
        let right = Grab::Edges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let r = drag(START, right, 50.0, 999.0, BOUNDS, None);
        assert_eq!((r.x, r.y, r.w, r.h), (100.0, 100.0, 250.0, 100.0));
        let r = drag(START, corner(true, true), -30.0, -40.0, BOUNDS, None);
        assert_eq!(
            (r.x, r.y, r.right(), r.bottom()),
            (70.0, 60.0, 300.0, 200.0)
        );
    }

    #[test]
    fn edges_cannot_cross_or_leave_the_image() {
        let right = Grab::Edges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        // Dragging past the opposite edge leaves the minimum size.
        let r = drag(START, right, -1000.0, 0.0, BOUNDS, None);
        assert_eq!((r.x, r.w), (100.0, MIN_SIZE));
        // Dragging past the image stops at its border.
        let r = drag(START, right, 1000.0, 0.0, BOUNDS, None);
        assert_eq!(r.right(), BOUNDS.0);
    }

    #[test]
    fn corner_drag_keeps_the_aspect_ratio() {
        let aspect = Some(2.0);
        for (dx, dy) in [(40.0, 5.0), (-40.0, -5.0), (5.0, 60.0), (-70.0, 30.0)] {
            for grab in [
                corner(true, true),
                corner(false, true),
                corner(true, false),
                corner(false, false),
            ] {
                let r = drag(START, grab, dx, dy, BOUNDS, aspect);
                assert!((r.w / r.h - 2.0).abs() < 1e-9, "{grab:?} {dx},{dy}: {r:?}");
                assert!(inside(r), "{grab:?} {dx},{dy}: {r:?}");
            }
        }
    }

    #[test]
    fn corner_drag_holds_the_opposite_corner() {
        let r = drag(START, corner(false, false), 40.0, 0.0, BOUNDS, Some(2.0));
        assert_eq!((r.x, r.y), (START.x, START.y));
        let r = drag(START, corner(true, true), -40.0, 0.0, BOUNDS, Some(2.0));
        assert!(
            (r.right() - START.right()).abs() < 1e-9 && (r.bottom() - START.bottom()).abs() < 1e-9
        );
    }

    #[test]
    fn side_drag_with_aspect_changes_the_other_side_around_the_centre() {
        let right = Grab::Edges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let r = drag(START, right, 40.0, 0.0, BOUNDS, Some(2.0));
        assert_eq!((r.w, r.h), (240.0, 120.0));
        assert!(
            ((r.y + r.h / 2.0) - 150.0).abs() < 1e-9,
            "stays centred vertically: {r:?}"
        );
    }

    #[test]
    fn aspect_resize_shrinks_instead_of_leaving_the_image() {
        // Growing far beyond the border still ends inside, at the ratio.
        let r = drag(
            START,
            corner(false, false),
            5000.0,
            5000.0,
            BOUNDS,
            Some(2.0),
        );
        assert!(inside(r) && (r.w / r.h - 2.0).abs() < 1e-9, "{r:?}");
        let r = drag(
            START,
            Grab::Edges {
                left: false,
                right: true,
                top: false,
                bottom: false,
            },
            5000.0,
            0.0,
            BOUNDS,
            Some(2.0),
        );
        assert!(inside(r) && (r.w / r.h - 2.0).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn hit_testing_finds_corners_sides_and_inside() {
        let reach = 6.0;
        assert_eq!(
            grab_at(START, 101.0, 99.0, reach),
            Some(Grab::Edges {
                left: true,
                right: false,
                top: true,
                bottom: false
            })
        );
        assert_eq!(
            grab_at(START, 300.0, 200.0, reach),
            Some(Grab::Edges {
                left: false,
                right: true,
                top: false,
                bottom: true
            })
        );
        assert_eq!(
            grab_at(START, 200.0, 101.0, reach),
            Some(Grab::Edges {
                left: false,
                right: false,
                top: true,
                bottom: false
            })
        );
        assert_eq!(grab_at(START, 200.0, 150.0, reach), Some(Grab::Move));
        assert_eq!(grab_at(START, 20.0, 20.0, reach), None);
    }

    #[test]
    fn initial_rectangle_is_centred_and_has_the_ratio() {
        let r = initial(BOUNDS, None, 0.8);
        assert_eq!(
            r,
            Rect {
                x: 40.0,
                y: 30.0,
                w: 320.0,
                h: 240.0
            }
        );
        let square = initial(BOUNDS, Some(1.0), 0.8);
        assert_eq!((square.w, square.h), (240.0, 240.0));
        assert_eq!(square.x, (400.0 - 240.0) / 2.0);
        let wide = initial(BOUNDS, Some(16.0 / 9.0), 1.0);
        assert!((wide.w / wide.h - 16.0 / 9.0).abs() < 1e-9 && inside(wide));
    }

    #[test]
    fn cursors_match_the_grab() {
        assert_eq!(corner(true, true).cursor(), "nw-resize");
        assert_eq!(corner(false, false).cursor(), "se-resize");
        assert_eq!(Grab::Move.cursor(), "move");
        assert_eq!(
            Grab::Edges {
                left: false,
                right: true,
                top: false,
                bottom: false
            }
            .cursor(),
            "ew-resize"
        );
        assert_eq!(
            Grab::Edges {
                left: false,
                right: false,
                top: false,
                bottom: true
            }
            .cursor(),
            "ns-resize"
        );
    }
}
