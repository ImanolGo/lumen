//! How a document sits in the widget: the one place that converts between widget pixels,
//! the layout of a document (page positions, in points) and areas inside a page.
//!
//! The canvas owns the zoom and the scroll position; this is the arithmetic on top of them,
//! kept free of GTK so it can be tested.

use crate::pdf::{HitRect, layout::PageRect};

/// The mapping from layout points to widget pixels: scale by `zoom`, then shift by `origin`
/// (where the layout's top-left corner is in the widget).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub zoom: f64,
    pub origin: (f64, f64),
}

/// A rectangle in widget pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl View {
    pub fn new(zoom: f64, origin: (f64, f64)) -> Self {
        Self { zoom, origin }
    }

    /// The widget position of the layout point (`x`, `y`).
    pub fn to_widget(self, (x, y): (f64, f64)) -> (f64, f64) {
        (self.origin.0 + x * self.zoom, self.origin.1 + y * self.zoom)
    }

    /// The layout point under the widget position (`x`, `y`).
    pub fn to_layout(self, (x, y): (f64, f64)) -> (f64, f64) {
        (
            (x - self.origin.0) / self.zoom,
            (y - self.origin.1) / self.zoom,
        )
    }

    /// Where the top-left corner of `page` is in the widget.
    pub fn page_origin(&self, page: &PageRect) -> (f64, f64) {
        self.to_widget((page.x, page.y))
    }

    /// The widget area of `area`, given in points from the top-left corner of `page`.
    pub fn area_in_page(&self, page: &PageRect, area: &HitRect) -> Area {
        let (x, y) = self.to_widget((page.x + area.x1, page.y + area.y1));
        Area {
            x,
            y,
            w: (area.x2 - area.x1) * self.zoom,
            h: (area.y2 - area.y1) * self.zoom,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: PageRect = PageRect {
        x: 12.0,
        y: 100.0,
        w: 600.0,
        h: 800.0,
    };

    #[test]
    fn widget_and_layout_points_convert_both_ways() {
        let view = View::new(2.0, (-30.0, 50.0));
        assert_eq!(view.to_widget((10.0, 20.0)), (-10.0, 90.0));
        assert_eq!(view.to_layout((-10.0, 90.0)), (10.0, 20.0));
        let back = view.to_layout(view.to_widget((3.3, 4.4)));
        assert!((back.0 - 3.3).abs() < 1e-9 && (back.1 - 4.4).abs() < 1e-9);
    }

    #[test]
    fn an_area_of_a_page_lands_where_the_page_is() {
        let view = View::new(0.5, (10.0, 20.0));
        assert_eq!(view.page_origin(&PAGE), (16.0, 70.0));
        let hit = HitRect {
            x1: 100.0,
            y1: 40.0,
            x2: 300.0,
            y2: 60.0,
        };
        assert_eq!(
            view.area_in_page(&PAGE, &hit),
            Area {
                x: 66.0,
                y: 90.0,
                w: 100.0,
                h: 10.0
            }
        );
    }
}
