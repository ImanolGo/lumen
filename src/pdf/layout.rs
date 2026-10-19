//! Where every page sits in the continuous-scroll view, in points.
//!
//! Page sizes are known without rendering, so the whole document can be laid
//! out (and scrolled) before a single page has been drawn.

/// Space between consecutive pages.
pub const PAGE_GAP: f64 = 10.0;
/// Space around the whole stack of pages.
pub const MARGIN: f64 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone)]
pub struct PageLayout {
    /// Total size including the margin.
    pub size: (f64, f64),
    pub rects: Vec<PageRect>,
}

impl PageRect {
    pub fn size(&self) -> (f64, f64) {
        (self.w, self.h)
    }
}

impl PageLayout {
    /// Stack pages vertically, centred on the widest one.
    pub fn new(page_sizes: &[(f64, f64)]) -> Self {
        let widest = page_sizes.iter().map(|s| s.0).fold(0.0, f64::max);
        let mut y = MARGIN;
        let rects = page_sizes
            .iter()
            .map(|&(w, h)| {
                let rect = PageRect {
                    x: MARGIN + (widest - w) / 2.0,
                    y,
                    w,
                    h,
                };
                y += h + PAGE_GAP;
                rect
            })
            .collect();
        let height = (y - PAGE_GAP + MARGIN).max(2.0 * MARGIN);
        Self {
            size: (widest + 2.0 * MARGIN, height),
            rects,
        }
    }

    /// The page nearest to the point (`x`, `y`) of the whole layout, and that
    /// point relative to the page's top-left corner. The point may lie outside
    /// the page (in a gap or margin), so selections can be dragged past the edges.
    pub fn locate(&self, x: f64, y: f64) -> Option<(usize, f64, f64)> {
        let last = self.rects.len().checked_sub(1)?;
        // First page that ends below `y`.
        let below = self.rects.partition_point(|r| r.y + r.h <= y).min(last);
        let page = match below.checked_sub(1) {
            // In the gap between two pages: the closer one wins.
            Some(above) if y < self.rects[below].y => {
                let gap_above = y - (self.rects[above].y + self.rects[above].h);
                let gap_below = self.rects[below].y - y;
                if gap_above <= gap_below { above } else { below }
            }
            _ => below,
        };
        let r = self.rects[page];
        Some((page, x - r.x, y - r.y))
    }

    /// First and last page touching the vertical span `top..bottom`. When the
    /// span only covers gaps or margin, the nearest page is returned.
    pub fn visible(&self, top: f64, bottom: f64) -> Option<(usize, usize)> {
        let last_index = self.rects.len().checked_sub(1)?;
        let first = self.rects.partition_point(|r| r.y + r.h <= top).min(last_index);
        let end = self.rects.partition_point(|r| r.y < bottom);
        Some((first, end.saturating_sub(1).clamp(first, last_index)))
    }

    /// The page that fills most of the span `top..bottom`.
    pub fn current(&self, top: f64, bottom: f64) -> Option<usize> {
        let (first, last) = self.visible(top, bottom)?;
        let overlap = |r: &PageRect| (r.y + r.h).min(bottom) - r.y.max(top);
        (first..=last).max_by(|&a, &b| {
            overlap(&self.rects[a])
                .total_cmp(&overlap(&self.rects[b]))
                // On a tie the earlier page wins.
                .then(b.cmp(&a))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> PageLayout {
        // Three pages: 100×200, 200×100, 100×200.
        PageLayout::new(&[(100.0, 200.0), (200.0, 100.0), (100.0, 200.0)])
    }

    #[test]
    fn pages_are_stacked_and_centred() {
        let l = layout();
        assert_eq!(l.size.0, 200.0 + 2.0 * MARGIN);
        assert_eq!(l.rects[0].y, MARGIN);
        assert_eq!(l.rects[1].y, MARGIN + 200.0 + PAGE_GAP);
        // The narrow page is centred on the wide one.
        assert_eq!(l.rects[0].x, MARGIN + 50.0);
        assert_eq!(l.rects[1].x, MARGIN);
        assert_eq!(l.size.1, MARGIN + 200.0 + 100.0 + 200.0 + 2.0 * PAGE_GAP + MARGIN);
    }

    #[test]
    fn visible_range() {
        let l = layout();
        assert_eq!(l.visible(0.0, 50.0), Some((0, 0)));
        let second_top = l.rects[1].y;
        assert_eq!(l.visible(second_top - 15.0, second_top + 5.0), Some((0, 1)));
        assert_eq!(l.visible(0.0, 10_000.0), Some((0, 2)));
        // Only a gap in view: nearest page, never out of bounds.
        let gap = l.rects[0].y + l.rects[0].h + 1.0;
        assert_eq!(l.visible(gap, gap + 2.0), Some((1, 1)));
        assert_eq!(l.visible(10_000.0, 10_100.0), Some((2, 2)));
        assert_eq!(PageLayout::new(&[]).visible(0.0, 10.0), None);
    }

    #[test]
    fn locating_points() {
        let l = layout();
        let r1 = l.rects[1];
        // Inside page 1.
        assert_eq!(l.locate(r1.x + 10.0, r1.y + 20.0), Some((1, 10.0, 20.0)));
        // Left of the page and above everything: clamps to the first page, unclamped coordinates.
        assert_eq!(l.locate(0.0, 0.0), Some((0, -l.rects[0].x, -l.rects[0].y)));
        // Below everything: the last page.
        assert_eq!(l.locate(0.0, 10_000.0).map(|p| p.0), Some(2));
        // In the gap: the nearer page.
        let gap_top = l.rects[0].y + l.rects[0].h;
        assert_eq!(l.locate(0.0, gap_top + 2.0).map(|p| p.0), Some(0));
        assert_eq!(l.locate(0.0, gap_top + PAGE_GAP - 2.0).map(|p| p.0), Some(1));
        assert_eq!(PageLayout::new(&[]).locate(0.0, 0.0), None);
    }

    #[test]
    fn current_page_is_the_one_filling_the_view() {
        let l = layout();
        assert_eq!(l.current(0.0, 100.0), Some(0));
        let second_top = l.rects[1].y;
        assert_eq!(l.current(second_top - 20.0, second_top + 80.0), Some(1));
        assert_eq!(l.current(second_top - 80.0, second_top + 20.0), Some(0));
    }
}
