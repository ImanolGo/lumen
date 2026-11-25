//! The "Pages" sidebar tab: a thumbnail of every page of the open PDF, drawn only
//! for the rows on screen, with the page in view ringed.

use std::collections::{HashMap, HashSet};

use iced::widget::{Id, button, column, container, image, operation, row, scrollable, space, text};
use iced::{Alignment, ContentFit, Element, Length, Task, Theme};

use super::{App, Doc, Message};
use crate::i18n::tr;
use crate::raster::Picture;
use crate::style;
use crate::ui;

/// Height of one row: the thumbnail box and the page number underneath.
pub const PAGE_ROW_H: f32 = 176.0;
/// Height of the thumbnail box inside a row.
const THUMB_BOX: f32 = 146.0;
/// Longest side of a rendered thumbnail, in pixels (the box is 146 logical px, so this stays sharp).
const THUMB_PX: f32 = 260.0;
/// Rows kept ready beyond the ones on screen.
const MARGIN_ROWS: usize = 5;
/// Thumbnails kept in memory; the ones far from the view are dropped past this.
const KEEP: usize = 80;

pub fn list_id() -> Id {
    Id::new("page-thumbs")
}

fn goto_input_id() -> Id {
    Id::new("goto-page")
}

/// The page number typed into the go-to-page box: digits only, as many as any page count needs.
pub fn clean_page_input(typed: &str) -> String {
    typed.chars().filter(char::is_ascii_digit).take(6).collect()
}

/// The page (counted from 0) a typed number stands for, clamped to the document; none if it is not a number.
pub fn typed_page(typed: &str, pages: usize) -> Option<usize> {
    let n: usize = typed.trim().parse().ok()?;
    Some(n.clamp(1, pages.max(1)) - 1)
}

/// Thumbnails of the PDF on screen.
#[derive(Default)]
pub struct PageThumbs {
    handles: HashMap<usize, image::Handle>,
    asked: HashSet<usize>,
    /// Scroll offset and height of the list (logical px).
    scroll: (f32, f32),
}

impl PageThumbs {
    /// Rows to have ready, as a page range.
    fn wanted(&self, pages: usize, row_h: f32) -> std::ops::Range<usize> {
        let (offset, height) = self.scroll;
        let height = if height > 0.0 { height } else { 600.0 };
        let first = (offset / row_h).floor().max(0.0) as usize;
        let last = ((offset + height) / row_h).ceil() as usize;
        first.saturating_sub(MARGIN_ROWS).min(pages)..(last + MARGIN_ROWS).min(pages)
    }

    /// Forgets what is far from `range` once there are many thumbnails.
    fn trim(&mut self, range: &std::ops::Range<usize>) {
        if self.handles.len() <= KEEP {
            return;
        }
        let middle = (range.start + range.end) / 2;
        let mut pages: Vec<usize> = self.handles.keys().copied().collect();
        pages.sort_by_key(|p| p.abs_diff(middle));
        for page in pages.into_iter().skip(KEEP / 2) {
            self.handles.remove(&page);
            self.asked.remove(&page);
        }
    }
}

impl App {
    /// True when the Pages tab is what the sidebar shows.
    fn page_thumbs_shown(&self) -> bool {
        self.show_sidebar && !self.fullscreen && self.sidebar_shows() == Some(super::SidebarTab::Pages)
    }

    fn page_row_h(&self) -> f32 {
        PAGE_ROW_H - THUMB_BOX + THUMB_BOX * self.sidebar_scale()
    }

    /// The sidebar was made wider or narrower: they are drawn again at that size, replacing the old.
    pub(super) fn page_thumbs_resized(&mut self) -> Task<Message> {
        self.page_thumbs.asked.clear();
        self.request_page_thumbs()
    }

    /// Starts rendering the thumbnails that are on screen and not there yet.
    pub(super) fn request_page_thumbs(&mut self) -> Task<Message> {
        if !self.page_thumbs_shown() {
            return Task::none();
        }
        let Doc::Pdf(view) = &self.doc else { return Task::none() };
        let doc = view.doc.clone();
        let range = self.page_thumbs.wanted(doc.page_count(), self.page_row_h());
        self.page_thumbs.trim(&range);
        let generation = self.generation;
        let night = self.night;
        let mut tasks = Vec::new();
        for page in range {
            if !self.page_thumbs.asked.insert(page) {
                continue;
            }
            let (w, h) = doc.page_size(page);
            let scale = THUMB_PX * self.sidebar_scale() / w.max(h).max(1.0);
            let doc = doc.clone();
            tasks.push(Task::perform(
                async move { doc.render_toned(page, scale, night).await.map_err(|e| e.to_string()) },
                move |r| Message::PageThumb(generation, page, night, r),
            ));
        }
        Task::batch(tasks)
    }

    pub(super) fn page_thumb_ready(&mut self, generation: u64, page: usize, result: Result<Picture, String>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(p) => {
                self.page_thumbs.handles.insert(page, image::Handle::from_rgba(p.width, p.height, p.rgba));
            }
            // Leave it asked: a page that cannot be drawn keeps its blank frame.
            Err(e) => tracing::warn!("page {} thumbnail: {e}", page + 1),
        }
    }

    pub(super) fn page_thumbs_scrolled(&mut self, offset: f32, height: f32) -> Task<Message> {
        self.page_thumbs.scroll = (offset, height);
        self.request_page_thumbs()
    }

    /// A new PDF was opened: start over.
    pub(super) fn reset_page_thumbs(&mut self) {
        self.page_thumbs = PageThumbs::default();
    }

    /// Scrolls the list just enough to show the page in view.
    pub(super) fn reveal_page_thumb(&self) -> Task<Message> {
        let Doc::Pdf(view) = &self.doc else { return Task::none() };
        let (offset, height) = self.page_thumbs.scroll;
        let height = if height > 0.0 { height } else { self.area.height };
        let row = self.page_row_h();
        let top = view.current_page as f32 * row;
        let y = if top < offset {
            top
        } else if top + row > offset + height {
            top + row - height
        } else {
            return Task::none();
        };
        operation::scroll_to(list_id(), scrollable::AbsoluteOffset { x: None, y: Some(y.max(0.0)) })
    }

    pub(super) fn goto_open(&mut self) -> Task<Message> {
        if !matches!(self.doc, Doc::Pdf(_)) {
            return Task::none();
        }
        self.goto = Some(String::new());
        Task::batch([operation::focus(goto_input_id()), operation::select_all(goto_input_id())])
    }

    pub(super) fn goto_submit(&mut self) -> Task<Message> {
        let Doc::Pdf(view) = &self.doc else { return Task::none() };
        let page = self.goto.as_deref().and_then(|t| typed_page(t, view.doc.page_count()));
        self.goto = None;
        match page {
            Some(page) => self.go_to_page(page),
            None => Task::none(),
        }
    }

    /// The small floating box asking for a page number.
    pub(super) fn goto_bar<'a>(&'a self, typed: &'a str, pages: usize) -> Element<'a, Message> {
        container(
            row![
                ui::input(&trf!("Page (1–{})", pages), typed)
                    .id(goto_input_id())
                    .on_input(|t| Message::GoToInput(clean_page_input(&t)))
                    .on_submit(Message::GoToSubmit)
                    .width(130),
                text(trf!("of {}", pages)).size(style::SMALL).style(super::muted),
                super::icon_button(crate::icons::close(), tr("Close (Esc)"), Some(Message::GoToClose), false),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([6, 10])
        .style(style::panel)
        .into()
    }

    pub(super) fn page_list(&self) -> Element<'_, Message> {
        let row_h = self.page_row_h();
        let Doc::Pdf(view) = self.shown_doc() else { return space().into() };
        let pages = view.doc.page_count();
        let range = self.page_thumbs.wanted(pages, row_h);
        let (first, last) = (range.start, range.end);
        let mut list = column![space().height(first as f32 * row_h)];
        for page in first..last {
            let thumb: Element<'_, Message> = match self.page_thumbs.handles.get(&page) {
                Some(handle) => image(handle.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .border_radius(2)
                    .into(),
                None => {
                    container(space()).width(Length::Fill).height(Length::Fill).style(style::thumb_placeholder).into()
                }
            };
            let current = page == view.current_page;
            let number = text(format!("{}", page + 1))
                .size(style::CAPTION)
                .font(style::font(if current { iced::font::Weight::Medium } else { iced::font::Weight::Normal }))
                .align_x(Alignment::Center)
                .width(Length::Fill)
                .style(move |t: &Theme| text::Style {
                    color: Some(t.extended_palette().background.base.text.scale_alpha(if current {
                        1.0
                    } else {
                        0.55
                    })),
                });
            let framed = container(container(thumb).width(Length::Fill).height(Length::Fill))
                .padding(3)
                .width(Length::Fill)
                .height(THUMB_BOX * self.sidebar_scale())
                .style(style::thumb_frame(current));
            let content = column![framed, number].spacing(4);
            list = list.push(
                container(
                    button(content)
                        .padding(4)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(style::thumb(false))
                        .on_press(Message::GoToPage(page)),
                )
                .padding([2, 8])
                .width(Length::Fill)
                .height(row_h),
            );
        }
        list = list.push(space().height((pages - last) as f32 * row_h));
        ui::thin_scroll(list.width(Length::Fill))
            .id(list_id())
            .on_scroll(|vp| Message::PageThumbsScrolled(vp.absolute_offset().y, vp.bounds().height))
            .height(Length::Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_the_rows_on_screen_plus_a_margin() {
        let mut t = PageThumbs { scroll: (0.0, 600.0), ..Default::default() };
        assert_eq!(t.wanted(100, PAGE_ROW_H), 0..(4 + MARGIN_ROWS));
        t.scroll = (PAGE_ROW_H * 50.0, 600.0);
        assert_eq!(t.wanted(100, PAGE_ROW_H), (50 - MARGIN_ROWS)..(54 + MARGIN_ROWS));
        // Never past the last page, and nothing for an empty document.
        t.scroll = (PAGE_ROW_H * 98.0, 600.0);
        assert_eq!(t.wanted(100, PAGE_ROW_H).end, 100);
        assert!(t.wanted(0, PAGE_ROW_H).is_empty());
    }

    #[test]
    fn typed_pages_are_clamped_and_counted_from_zero() {
        assert_eq!(typed_page("1", 42), Some(0));
        assert_eq!(typed_page(" 31 ", 42), Some(30));
        assert_eq!(typed_page("0", 42), Some(0));
        assert_eq!(typed_page("999", 42), Some(41));
        assert_eq!(typed_page("", 42), None);
        assert_eq!(typed_page("x", 42), None);
        assert_eq!(clean_page_input("1a2 3-"), "123");
        assert_eq!(clean_page_input("12345678"), "123456");
    }

    #[test]
    fn trimming_keeps_the_thumbnails_nearest_the_view() {
        let mut t = PageThumbs::default();
        for page in 0..(KEEP + 20) {
            t.handles.insert(page, image::Handle::from_rgba(1, 1, vec![0, 0, 0, 255]));
            t.asked.insert(page);
        }
        t.trim(&(0..10));
        assert_eq!(t.handles.len(), KEEP / 2);
        assert!(t.handles.contains_key(&5) && !t.handles.contains_key(&(KEEP + 10)));
        // What was dropped can be asked for again.
        assert!(!t.asked.contains(&(KEEP + 10)));
    }
}
