//! Selecting a part of a picture: drag over it, then copy it or crop to it.

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Frame, Geometry, LineDash, Path, Stroke};
use iced::widget::{button, container, opaque, row, text};
use iced::{Alignment, Color, Element, Point, Rectangle, Renderer, Size, Task, Theme, Vector};

use super::{App, Doc, Message, icon_button, muted};
use crate::edit::crop::{self, Grab, Rect};
use crate::i18n::tr;
use crate::{icons, style};

/// A selection smaller than this on screen was a click.
const LEAST: f32 = 4.0;
/// How near an edge is grabbed, and the size of the handles.
const REACH: f32 = 8.0;
const HANDLE: f32 = 7.0;

pub struct Region {
    /// In picture pixels.
    pub rect: Rect,
    drag: Option<Drag>,
}

enum Drag {
    /// Drawn out from this corner.
    New(f64, f64),
    /// A side, a corner or the whole, from where it was.
    Grab(Grab, Rect, (f64, f64)),
}

#[derive(Debug, Clone)]
pub enum RegionMsg {
    Press(f64, f64),
    Drag(f64, f64),
    Release,
    All,
    Clear,
    Crop,
}

/// The rectangle between two corners, on whole pixels and inside a picture of `bounds`.
fn between(a: (f64, f64), b: (f64, f64), bounds: (f64, f64)) -> Rect {
    let side = |p: f64, q: f64, max: f64| {
        let (lo, hi) = (p.min(q).clamp(0.0, max).floor(), p.max(q).clamp(0.0, max).ceil());
        (lo, hi - lo)
    };
    let ((x, w), (y, h)) = (side(a.0, b.0, bounds.0), side(a.1, b.1, bounds.1));
    Rect { x, y, w, h }
}

impl App {
    /// Whether dragging over the picture selects a part of it.
    fn selects_region(&self) -> bool {
        matches!(self.doc, Doc::Image(_))
            && self.prefs.select_on_drag
            && !self.markup_open
            && self.crop.is_none()
            && self.slideshow.is_none()
    }

    fn region_zoom(&self) -> f32 {
        self.shown_zoom().unwrap_or(1.0)
    }

    pub(super) fn region_update(&mut self, msg: RegionMsg) -> Task<Message> {
        let Doc::Image(view) = &self.doc else { return Task::none() };
        let bounds = (f64::from(view.shown.width), f64::from(view.shown.height));
        let zoom = self.region_zoom();
        match msg {
            RegionMsg::Press(x, y) => {
                let reach = f64::from(REACH / zoom);
                let grabbed = self.region.as_ref().and_then(|r| Some((crop::grab_at(r.rect, x, y, reach)?, r.rect)));
                self.region = Some(match grabbed {
                    Some((grab, rect)) => Region { rect, drag: Some(Drag::Grab(grab, rect, (x, y))) },
                    None => Region { rect: between((x, y), (x, y), bounds), drag: Some(Drag::New(x, y)) },
                });
            }
            RegionMsg::Drag(x, y) => {
                if let Some(region) = &mut self.region {
                    match region.drag {
                        Some(Drag::New(sx, sy)) => region.rect = between((sx, sy), (x, y), bounds),
                        Some(Drag::Grab(grab, start, (sx, sy))) => {
                            let r = crop::drag(start, grab, x - sx, y - sy, bounds, None);
                            region.rect = between((r.x, r.y), (r.right(), r.bottom()), bounds);
                        }
                        None => {}
                    }
                }
            }
            RegionMsg::Release => {
                if let Some(region) = &mut self.region {
                    region.drag = None;
                    let (w, h) = (region.rect.w as f32 * zoom, region.rect.h as f32 * zoom);
                    if w < LEAST || h < LEAST {
                        self.region = None;
                    }
                }
            }
            RegionMsg::All => {
                self.region = Some(Region { rect: Rect { x: 0.0, y: 0.0, w: bounds.0, h: bounds.1 }, drag: None });
            }
            RegionMsg::Clear => self.region = None,
            RegionMsg::Crop => {
                let Some(region) = self.region.take() else { return Task::none() };
                return self.crop_to(region.rect);
            }
        }
        Task::none()
    }

    /// The selected part of the picture, in whole pixels: x, y, width, height.
    pub(super) fn region_pixels(&self) -> Option<(u32, u32, u32, u32)> {
        let r = self.region.as_ref()?.rect;
        (r.w >= 1.0 && r.h >= 1.0).then_some((r.x as u32, r.y as u32, r.w as u32, r.h as u32))
    }

    pub(super) fn region_layer(&self, zoom: f32, w: f32, h: f32) -> Option<Element<'_, Message>> {
        if !self.selects_region() {
            return None;
        }
        let rect = self.region.as_ref().map(|r| r.rect);
        Some(iced::widget::canvas(Overlay { rect, zoom }).width(w).height(h).into())
    }

    pub(super) fn region_bar(&self) -> Option<Element<'_, Message>> {
        let region = self.region.as_ref().filter(|_| self.selects_region())?;
        let size = text(format!("{} × {}", region.rect.w, region.rect.h)).size(style::BODY).style(muted);
        let action = |label: String, message: Message| {
            button(text(label).size(style::BODY)).padding([6, 12]).style(style::secondary).on_press(message)
        };
        let bar = row![
            size,
            action(tr("Copy"), Message::CopyPicture),
            action(tr("Crop"), Message::Region(RegionMsg::Crop)),
            icon_button(
                icons::close(),
                tr("Clear the selection (Esc)"),
                Some(Message::Region(RegionMsg::Clear)),
                false
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        Some(opaque(container(bar).padding([4, 10]).style(style::panel)))
    }
}

struct Overlay {
    rect: Option<Rect>,
    zoom: f32,
}

impl Overlay {
    fn to_picture(&self, p: Point) -> (f64, f64) {
        (f64::from(p.x / self.zoom), f64::from(p.y / self.zoom))
    }
}

impl canvas::Program<Message> for Overlay {
    /// Whether the button is down.
    type State = bool;

    fn update(
        &self,
        down: &mut bool,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        let canvas::Event::Mouse(event) = event else { return None };
        let say = |m| canvas::Action::publish(Message::Region(m));
        match event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let (x, y) = self.to_picture(cursor.position_in(bounds)?);
                *down = true;
                Some(say(RegionMsg::Press(x, y)).and_capture())
            }
            mouse::Event::CursorMoved { .. } if *down => {
                let p = cursor.land().position()?;
                let (x, y) = self.to_picture(p - Vector::new(bounds.x, bounds.y));
                Some(say(RegionMsg::Drag(x, y)))
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) if *down => {
                *down = false;
                Some(say(RegionMsg::Release))
            }
            _ => None,
        }
    }

    fn draw(&self, _: &bool, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if let Some(r) = self.rect {
            let z = self.zoom;
            let (x, y, w, h) = (r.x as f32 * z, r.y as f32 * z, r.w as f32 * z, r.h as f32 * z);
            let shade = Color::from_rgba(0.0, 0.0, 0.0, 0.35);
            let (bw, bh) = (bounds.width, bounds.height);
            frame.fill_rectangle(Point::ORIGIN, Size::new(bw, y), shade);
            frame.fill_rectangle(Point::new(0.0, y + h), Size::new(bw, (bh - y - h).max(0.0)), shade);
            frame.fill_rectangle(Point::new(0.0, y), Size::new(x, h), shade);
            frame.fill_rectangle(Point::new(x + w, y), Size::new((bw - x - w).max(0.0), h), shade);
            // Black and white dashes show on any picture.
            let outline = Path::rectangle(Point::new(x, y), Size::new(w, h));
            frame.stroke(&outline, Stroke::default().with_color(Color::BLACK).with_width(1.0));
            let dashes = LineDash { segments: &[4.0, 4.0], offset: 0 };
            let dashed = Stroke { line_dash: dashes, ..Stroke::default().with_color(Color::WHITE).with_width(1.0) };
            frame.stroke(&outline, dashed);
            for (cx, cy) in [
                (x, y),
                (x + w / 2.0, y),
                (x + w, y),
                (x, y + h / 2.0),
                (x + w, y + h / 2.0),
                (x, y + h),
                (x + w / 2.0, y + h),
                (x + w, y + h),
            ] {
                let knob = Path::circle(Point::new(cx, cy), HANDLE / 2.0);
                frame.fill(&knob, Color::WHITE);
                frame.stroke(&knob, Stroke::default().with_color(Color::from_rgba(0.0, 0.0, 0.0, 0.6)).with_width(1.0));
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, down: &bool, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        let Some(at) = cursor.position_in(bounds) else { return mouse::Interaction::None };
        let (x, y) = self.to_picture(at);
        match self.rect.and_then(|r| crop::grab_at(r, x, y, f64::from(REACH / self.zoom))) {
            Some(Grab::Move) if *down => mouse::Interaction::Grabbing,
            Some(Grab::Move) => mouse::Interaction::Grab,
            Some(Grab::Edges { left, right, top, bottom }) => match (left || right, top || bottom) {
                (true, false) => mouse::Interaction::ResizingHorizontally,
                (false, true) => mouse::Interaction::ResizingVertically,
                _ if (left && top) || (right && bottom) => mouse::Interaction::ResizingDiagonallyDown,
                _ => mouse::Interaction::ResizingDiagonallyUp,
            },
            None => mouse::Interaction::Crosshair,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_is_whole_pixels_inside_the_picture() {
        let r = between((10.4, 20.6), (3.2, 5.1), (100.0, 50.0));
        assert_eq!((r.x, r.y, r.w, r.h), (3.0, 5.0, 8.0, 16.0));
        let r = between((90.0, 40.0), (500.0, -3.0), (100.0, 50.0));
        assert_eq!((r.x, r.y, r.w, r.h), (90.0, 0.0, 10.0, 40.0));
    }
}
