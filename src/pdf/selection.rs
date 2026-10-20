//! Text selection: turning a drag into the text and screen rectangles it covers.
//!
//! The geometry (`spans`) is plain functions on page coordinates. Poppler does
//! the text layout, on a worker thread that owns its own document and keeps the
//! pages it has already laid out, so repeated drag updates stay cheap.

use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    thread,
};

use poppler::{Rectangle, SelectionStyle};

use super::{HitRect, LazyDoc, PdfSource, poppler_lock};

/// A point on a page, in points from the page's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PagePoint {
    pub page: usize,
    pub x: f64,
    pub y: f64,
}

/// How much text a selection snaps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    Glyph,
    Word,
    Line,
}

impl Granularity {
    fn style(self) -> SelectionStyle {
        match self {
            Self::Glyph => SelectionStyle::Glyph,
            Self::Word => SelectionStyle::Word,
            Self::Line => SelectionStyle::Line,
        }
    }
}

/// The part of one page a selection covers, as a start and an end point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub page: usize,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

/// Two points closer than this vertically are on the same line.
const SAME_LINE: f64 = 5.0;

/// Whether `a` comes before `b` in reading order.
fn before(a: PagePoint, b: PagePoint) -> bool {
    if a.page != b.page {
        return a.page < b.page;
    }
    if (a.y - b.y).abs() <= SAME_LINE {
        return a.x < b.x;
    }
    a.y < b.y
}

/// The spans a drag from `anchor` to `head` covers, one per page, in reading
/// order. Selecting backwards gives the same result as selecting forwards.
pub fn spans(
    anchor: PagePoint,
    head: PagePoint,
    mut page_size: impl FnMut(usize) -> (f64, f64),
) -> Vec<Span> {
    let (start, end) = if before(head, anchor) {
        (head, anchor)
    } else {
        (anchor, head)
    };
    (start.page..=end.page)
        .map(|page| {
            let (w, h) = page_size(page);
            let (x1, y1) = if page == start.page {
                (start.x, start.y)
            } else {
                (0.0, 0.0)
            };
            let (x2, y2) = if page == end.page {
                (end.x, end.y)
            } else {
                (w, h)
            };
            Span {
                page,
                x1,
                y1,
                x2,
                y2,
            }
        })
        .collect()
}

pub struct SelectRequest {
    pub id: u64,
    pub anchor: PagePoint,
    pub head: PagePoint,
    pub granularity: Granularity,
    /// Pages in view; screen rectangles are only computed near them.
    pub visible: (usize, usize),
    /// Also extract the selected text (for copying).
    pub want_text: bool,
}

#[derive(Debug)]
pub struct SelectionResult {
    pub id: u64,
    /// Screen rectangles per page, in points from the page's top-left.
    pub rects: Vec<(usize, Vec<HitRect>)>,
    pub text: Option<String>,
}

/// Pull `x` onto the text of the line at (`x`, `y`). Poppler ignores an end
/// point that is not over a glyph, so dragging past the end of a line would
/// drop that line; clamped to its text, the whole line is selected instead.
fn snap_to_line(page: &poppler::Page, x: f64, y: f64) -> f64 {
    // Probe at the left page edge: Poppler finds the line at this height there,
    // but not at an `x` that lies past the end of the text.
    let mut area = Rectangle::new();
    area.set_x1(0.0);
    area.set_y1(y);
    area.set_x2(0.0);
    area.set_y2(y);
    let Some(region) = page.selected_region(REGION_SCALE, SelectionStyle::Line, &mut area) else {
        return x;
    };
    let edges = (0..region.num_rectangles())
        .map(|i| region.rectangle(i))
        .fold(None, |acc: Option<(f64, f64)>, r| {
            let (left, right) = (
                r.x() as f64 / REGION_SCALE,
                (r.x() + r.width()) as f64 / REGION_SCALE,
            );
            Some(acc.map_or((left, right), |(l, rt)| (l.min(left), rt.max(right))))
        });
    match edges {
        Some((left, right)) if right - left > 1.0 => x.clamp(left + 0.5, right - 0.5),
        _ => x,
    }
}

/// Rectangles are computed this many times finer than a point.
const REGION_SCALE: f64 = 4.0;
/// Pages around the view that get rectangles while selecting.
const REGION_MARGIN: usize = 2;
/// Laid-out pages kept per document.
const PAGE_CACHE: usize = 64;

#[derive(Default)]
struct State {
    pending: Option<SelectRequest>,
    shutdown: bool,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

pub struct SelectionWorker {
    shared: Shared,
}

impl SelectionWorker {
    /// `on_result` is called on the worker thread.
    pub fn new(source: &PdfSource, on_result: impl Fn(SelectionResult) + Send + 'static) -> Self {
        let shared = Shared::default();
        let worker = Arc::clone(&shared);
        let source = source.clone();
        thread::spawn(move || {
            let mut doc = LazyDoc::new(&source);
            let mut pages: HashMap<usize, poppler::Page> = HashMap::new();
            loop {
                let request = {
                    let (lock, condvar) = &*worker;
                    let mut state = lock.lock().unwrap();
                    loop {
                        if state.shutdown {
                            return;
                        }
                        if let Some(request) = state.pending.take() {
                            break request;
                        }
                        state = condvar.wait(state).unwrap();
                    }
                };
                let Ok(doc) = doc.get() else { continue };
                let _poppler = poppler_lock();
                if pages.len() > PAGE_CACHE {
                    pages.clear();
                }

                let mut page_of = |index: usize| -> Option<poppler::Page> {
                    if let Some(page) = pages.get(&index) {
                        return Some(page.clone());
                    }
                    let page = doc.page(index as i32)?;
                    pages.insert(index, page.clone());
                    Some(page)
                };

                let style = request.granularity.style();
                let mut all = spans(request.anchor, request.head, |p| {
                    page_of(p).map_or((612.0, 792.0), |page| page.size())
                });
                if request.granularity == Granularity::Glyph {
                    let first_page = all.first().and_then(|f| page_of(f.page));
                    if let (Some(first), Some(page)) = (all.first_mut(), first_page) {
                        first.x1 = snap_to_line(&page, first.x1, first.y1);
                    }
                    let last_page = all.last().and_then(|l| page_of(l.page));
                    if let (Some(last), Some(page)) = (all.last_mut(), last_page) {
                        last.x2 = snap_to_line(&page, last.x2, last.y2);
                    }
                }
                let (first, last) = request.visible;
                let near = first.saturating_sub(REGION_MARGIN)..=last + REGION_MARGIN;

                let mut rects = Vec::new();
                let mut text = request.want_text.then(String::new);
                for span in all {
                    let Some(page) = page_of(span.page) else {
                        continue;
                    };
                    let mut area = Rectangle::new();
                    area.set_x1(span.x1);
                    area.set_y1(span.y1);
                    area.set_x2(span.x2);
                    area.set_y2(span.y2);

                    let region = near
                        .contains(&span.page)
                        .then(|| page.selected_region(REGION_SCALE, style, &mut area))
                        .flatten();
                    if let Some(region) = region {
                        let hits = (0..region.num_rectangles())
                            .map(|i| region.rectangle(i))
                            .map(|r| HitRect {
                                x1: r.x() as f64 / REGION_SCALE,
                                y1: r.y() as f64 / REGION_SCALE,
                                x2: (r.x() + r.width()) as f64 / REGION_SCALE,
                                y2: (r.y() + r.height()) as f64 / REGION_SCALE,
                            })
                            .collect();
                        rects.push((span.page, hits));
                    }
                    if let (Some(all_text), Some(chunk)) =
                        (&mut text, page.selected_text(style, &mut area))
                    {
                        if !all_text.is_empty() && !chunk.is_empty() {
                            all_text.push('\n');
                        }
                        all_text.push_str(&chunk);
                    }
                }
                on_result(SelectionResult {
                    id: request.id,
                    rects,
                    text,
                });
            }
        });
        Self { shared }
    }

    /// Replace any request that has not started yet.
    pub fn request(&self, request: SelectRequest) {
        let (lock, condvar) = &*self.shared;
        lock.lock().unwrap().pending = Some(request);
        condvar.notify_all();
    }
}

impl Drop for SelectionWorker {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.shared;
        lock.lock().unwrap().shutdown = true;
        condvar.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(page: usize, x: f64, y: f64) -> PagePoint {
        PagePoint { page, x, y }
    }

    fn size(_: usize) -> (f64, f64) {
        (600.0, 800.0)
    }

    #[test]
    fn same_page_forward_and_backward_agree() {
        let forward = spans(at(0, 100.0, 100.0), at(0, 300.0, 200.0), size);
        let backward = spans(at(0, 300.0, 200.0), at(0, 100.0, 100.0), size);
        assert_eq!(forward, backward);
        assert_eq!(
            forward,
            vec![Span {
                page: 0,
                x1: 100.0,
                y1: 100.0,
                x2: 300.0,
                y2: 200.0
            }]
        );
    }

    #[test]
    fn same_line_orders_by_x_even_if_y_wobbles() {
        // Dragging leftwards along a line, ending 3 pt lower, must still start at the left.
        let s = spans(at(0, 300.0, 100.0), at(0, 100.0, 103.0), size);
        assert_eq!((s[0].x1, s[0].x2), (100.0, 300.0));
    }

    #[test]
    fn multi_page_covers_the_ends_and_everything_between() {
        let s = spans(at(1, 50.0, 700.0), at(3, 400.0, 20.0), size);
        assert_eq!(s.len(), 3);
        assert_eq!(
            s[0],
            Span {
                page: 1,
                x1: 50.0,
                y1: 700.0,
                x2: 600.0,
                y2: 800.0
            }
        );
        assert_eq!(
            s[1],
            Span {
                page: 2,
                x1: 0.0,
                y1: 0.0,
                x2: 600.0,
                y2: 800.0
            }
        );
        assert_eq!(
            s[2],
            Span {
                page: 3,
                x1: 0.0,
                y1: 0.0,
                x2: 400.0,
                y2: 20.0
            }
        );
    }

    #[test]
    fn selecting_upwards_across_pages_is_normalised() {
        let up = spans(at(3, 400.0, 20.0), at(1, 50.0, 700.0), size);
        let down = spans(at(1, 50.0, 700.0), at(3, 400.0, 20.0), size);
        assert_eq!(up, down);
    }
}

#[cfg(test)]
mod poppler_tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    /// Page 1 has three lines with baselines at 100, 130 and 160 points from the
    /// top, starting at x = 72; page 2 has one line.
    fn fixture() -> PdfSource {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/selection.pdf");
        PdfSource::new(&path, None)
    }

    fn select(anchor: PagePoint, head: PagePoint, granularity: Granularity) -> SelectionResult {
        let (tx, rx) = mpsc::channel();
        let worker = SelectionWorker::new(&fixture(), move |result| {
            let _ = tx.send(result);
        });
        worker.request(SelectRequest {
            id: 1,
            anchor,
            head,
            granularity,
            visible: (0, 1),
            want_text: true,
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("worker answered")
    }

    fn at(page: usize, x: f64, y: f64) -> PagePoint {
        PagePoint { page, x, y }
    }

    #[test]
    fn selects_a_line_from_top_left_coordinates() {
        // A wide drag across the first line only (y = 100 is its baseline).
        let result = select(at(0, 60.0, 92.0), at(0, 380.0, 102.0), Granularity::Glyph);
        let text = result.text.expect("text was requested");
        assert!(text.contains("Hello brave new world"), "got {text:?}");
        assert!(!text.contains("Second"), "got {text:?}");

        // Highlight rectangles sit on that line, measured from the top of the page.
        let (page, rects) = &result.rects[0];
        assert_eq!(*page, 0);
        assert!(!rects.is_empty());
        for r in rects {
            assert!(
                r.y1 >= 75.0 && r.y2 <= 115.0,
                "rect {r:?} is not around y = 100"
            );
            assert!(r.x1 >= 60.0);
        }
    }

    #[test]
    fn selection_works_backwards_and_over_several_lines() {
        let result = select(at(0, 380.0, 135.0), at(0, 60.0, 95.0), Granularity::Glyph);
        let text = result.text.unwrap();
        assert!(
            text.contains("Hello") && text.contains("Second line here"),
            "got {text:?}"
        );
        assert!(!text.contains("Third"), "got {text:?}");
    }

    #[test]
    fn word_granularity_snaps_to_whole_words() {
        // From the middle of "brave" (x 115-160) to the middle of "new" (x 165-196).
        let result = select(at(0, 130.0, 95.0), at(0, 180.0, 97.0), Granularity::Word);
        let text = result.text.unwrap();
        assert_eq!(text.trim(), "brave new");
    }

    #[test]
    fn selection_spans_pages() {
        let result = select(at(0, 60.0, 155.0), at(1, 380.0, 105.0), Granularity::Glyph);
        let text = result.text.unwrap();
        assert!(
            text.contains("Third line") && text.contains("Page two text"),
            "got {text:?}"
        );
        assert_eq!(result.rects.len(), 2);
    }

    #[test]
    fn double_and_triple_click_select_a_word_and_a_line() {
        let word = select(at(0, 130.0, 95.0), at(0, 130.0, 95.0), Granularity::Word);
        assert_eq!(word.text.unwrap().trim(), "brave");
        let line = select(at(0, 130.0, 95.0), at(0, 130.0, 95.0), Granularity::Line);
        assert_eq!(line.text.unwrap().trim(), "Hello brave new world");
    }
}
