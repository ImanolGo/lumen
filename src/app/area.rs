//! A rectangle of a PDF page, to copy as a picture: with "Select an Area" on, a drag over a page
//! draws it instead of moving the view.

use iced::widget::{button, container, opaque, row, space, text};
use iced::{Alignment, Element, Task};

use super::{App, Doc, Message, blocking, icon_button, muted, placed};
use crate::i18n::tr;
use crate::{icons, style};
use hayro::kurbo::Rect;

/// The most pixels on the longer side of a copy, and the resolution aimed at below that.
const MOST: f32 = 4096.0;
const DPI: f32 = 300.0;

/// What is selected: the page, and two corners in its points (from its top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub page: usize,
    pub from: (f64, f64),
    pub to: (f64, f64),
    /// Still being drawn.
    pub drawing: bool,
}

impl Area {
    pub fn rect(&self) -> Rect {
        let (x0, x1) = (self.from.0.min(self.to.0), self.from.0.max(self.to.0));
        let (y0, y1) = (self.from.1.min(self.to.1), self.from.1.max(self.to.1));
        Rect::new(x0, y0, x1, y1)
    }
}

#[derive(Debug, Clone)]
pub enum AreaMsg {
    /// Turns the mode on or off.
    Toggle,
    Copy,
    Clear,
}

/// The scale to render a page at for a copy of `rect` (points): 300 dpi, unless that is too large.
fn copy_scale(rect: &Rect) -> f32 {
    let side = rect.width().max(rect.height()).max(1.0) as f32;
    (DPI / 72.0).min(MOST / side)
}

/// The part `rect` (points) of a page drawn at `scale`.
fn cut(page: &crate::raster::Picture, rect: &Rect, scale: f32) -> (u32, u32, Vec<u8>) {
    let px = |v: f64| (v * f64::from(scale)).round().max(0.0) as u32;
    let (x, y) = (px(rect.x0).min(page.width - 1), px(rect.y0).min(page.height - 1));
    let w = px(rect.width()).clamp(1, page.width - x);
    let h = px(rect.height()).clamp(1, page.height - y);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let start = ((row * page.width + x) * 4) as usize;
        rgba.extend_from_slice(&page.rgba[start..start + (w * 4) as usize]);
    }
    (w, h, rgba)
}

impl App {
    pub(super) fn area_update(&mut self, msg: AreaMsg) -> Task<Message> {
        match msg {
            AreaMsg::Toggle => {
                self.area_mode = !self.area_mode && matches!(self.doc, Doc::Pdf(_));
                self.page_area = None;
                Task::none()
            }
            AreaMsg::Clear => {
                self.page_area = None;
                Task::none()
            }
            AreaMsg::Copy => {
                let (Some(area), Doc::Pdf(view)) = (self.page_area, &self.doc) else { return Task::none() };
                let rect = area.rect();
                let scale = copy_scale(&rect);
                let doc = view.doc.clone();
                Task::perform(
                    async move {
                        let page = doc.render(area.page, scale).await.map_err(|e| e.to_string())?;
                        blocking(move || {
                            let (w, h, rgba) = cut(&page, &rect, scale);
                            let png = crate::clipboard::encode_png(w, h, &rgba)?;
                            crate::platform::clipboard::copy_png(png)
                        })
                        .await
                        .and_then(|r| r)
                    },
                    Message::PictureCopied,
                )
            }
        }
    }

    /// The mouse went down on a page: an area begins there, if that is what dragging does now.
    pub(super) fn area_press(&mut self, page: usize, at: (f64, f64)) -> bool {
        if !self.area_mode {
            return false;
        }
        self.page_area = Some(Area { page, from: at, to: at, drawing: true });
        true
    }

    /// The pointer moved over `page` while the button is down.
    pub(super) fn area_drag(&mut self, page: usize, at: (f64, f64)) {
        if let Some(area) = &mut self.page_area
            && area.drawing
            && area.page == page
        {
            area.to = at;
        }
    }

    /// The button went up: what was drawn is kept, unless it was a click.
    pub(super) fn area_release(&mut self) -> bool {
        let Some(area) = &mut self.page_area else { return false };
        if !area.drawing {
            return false;
        }
        area.drawing = false;
        let r = area.rect();
        if r.width() < 2.0 || r.height() < 2.0 {
            self.page_area = None;
        }
        true
    }

    /// The rectangle on page `page`, drawn at `zoom`.
    pub(super) fn area_layer(&self, page: usize, zoom: f32) -> Option<Element<'_, Message>> {
        let area = self.page_area.filter(|a| a.page == page)?;
        let r = area.rect();
        let mark =
            container(space().width(r.width() as f32 * zoom).height(r.height() as f32 * zoom)).style(style::area);
        Some(placed(&r, zoom, mark.into()))
    }

    pub(super) fn area_bar(&self) -> Option<Element<'_, Message>> {
        let area = self.page_area.filter(|a| !a.drawing)?;
        let r = area.rect();
        let size = text(trf!("{} × {} pt", r.width().round(), r.height().round())).size(style::BODY).style(muted);
        let bar = row![
            size,
            button(text(tr("Copy")).size(style::BODY))
                .padding([6, 12])
                .style(style::secondary)
                .on_press(Message::Area(AreaMsg::Copy)),
            icon_button(icons::close(), tr("Clear the selection (Esc)"), Some(Message::Area(AreaMsg::Clear)), false),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        Some(opaque(container(bar).padding([4, 10]).style(style::panel)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_sharp_but_not_huge() {
        assert!((copy_scale(&Rect::new(0.0, 0.0, 100.0, 50.0)) - 300.0 / 72.0).abs() < 1e-3);
        let page = copy_scale(&Rect::new(0.0, 0.0, 2000.0, 1000.0));
        assert!((page * 2000.0 - MOST).abs() < 0.5);
        let a = Area { page: 0, from: (30.0, 40.0), to: (10.0, 5.0), drawing: false };
        assert_eq!((a.rect().x0, a.rect().y0, a.rect().width(), a.rect().height()), (10.0, 5.0, 20.0, 35.0));
    }

    #[test]
    fn the_copy_is_that_part_of_the_page() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages.pdf");
        let doc = crate::pdf::Document::open(&path).unwrap();
        let rect = Rect::new(50.0, 40.0, 250.0, 90.0);
        let scale = copy_scale(&rect);
        let page = futures::executor::block_on(doc.render(0, scale)).unwrap();
        let (w, h, rgba) = cut(&page, &rect, scale);
        assert_eq!((w, h), ((200.0 * scale).round() as u32, (50.0 * scale).round() as u32));
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        // "Page 1" is written there: some of it is dark.
        assert!(rgba.chunks(4).any(|p| p[0] < 100), "the text of the page is in it");
        // Past the edge, it stops at the edge.
        let (w, _, _) = cut(&page, &Rect::new(500.0, 0.0, 9000.0, 10.0), scale);
        assert!(w <= page.width);
    }
}
