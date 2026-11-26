//! Selecting a part of a picture: a rectangle or a freehand outline (or everything else), then copy
//! it, crop to it, invert it or clear it. Or the pixels of about one colour: click, and drag away to
//! take in more.

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Frame, Geometry, LineDash, Path, Stroke};
use iced::widget::{button, container, opaque, row, text};
use iced::{Alignment, Color, Element, Point, Rectangle, Renderer, Size, Task, Theme, Vector};

use super::{App, Doc, Message, icon_button, muted};
use crate::edit::crop::{self, Grab, Rect};
use crate::edit::select::{self, Selection, Shape};
use crate::i18n::tr;
use crate::{icons, style};

/// A selection smaller than this on screen was a click.
const LEAST: f32 = 4.0;
/// How near an edge is grabbed, and the size of the handles.
const REACH: f32 = 8.0;
const HANDLE: f32 = 7.0;

pub struct Region {
    /// In picture pixels.
    pub selection: Selection,
    drag: Option<Drag>,
    /// How a selection by colour is shown: the pixels tinted.
    tint: Option<iced::widget::image::Handle>,
}

/// How alike colours are taken to be, to begin with and at most; dragging away adds to it.
const TOLERANCE: (f64, f64) = (24.0, 255.0);

/// How a drag selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Rect,
    Lasso,
    /// The pixels of about the colour clicked.
    Colour,
}

impl Region {
    /// Whether it is shown by tinting its pixels.
    #[cfg(test)]
    pub fn has_tint(&self) -> bool {
        self.tint.is_some()
    }

    fn rect(&self) -> Option<Rect> {
        match &self.selection.shape {
            Shape::Rect(r) => Some(*r),
            _ => None,
        }
    }
}

enum Drag {
    /// An outline being drawn.
    Lasso,
    /// A colour picked here, its tolerance growing with the distance dragged.
    Colour(f64, f64),
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
    Invert,
    /// Makes what is selected clear.
    Delete,
    Mode(Mode),
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
                let grabbed = self
                    .region
                    .as_ref()
                    .filter(|r| !r.selection.inverted)
                    .and_then(|r| r.rect())
                    .and_then(|rect| Some((crop::grab_at(rect, x, y, reach)?, rect)));
                let new = |shape, drag| Some(Region { selection: Selection::new(shape), drag: Some(drag), tint: None });
                self.region = match (grabbed, self.select_mode) {
                    (Some((grab, rect)), _) => new(Shape::Rect(rect), Drag::Grab(grab, rect, (x, y))),
                    (None, Mode::Rect) => new(Shape::Rect(between((x, y), (x, y), bounds)), Drag::New(x, y)),
                    (None, Mode::Lasso) => new(Shape::Lasso(vec![(x, y)]), Drag::Lasso),
                    (None, Mode::Colour) => {
                        self.region = None;
                        return self.select_colour((x, y), TOLERANCE.0);
                    }
                };
            }
            RegionMsg::Drag(x, y) => {
                if let Some(Region { drag: Some(Drag::Colour(sx, sy)), .. }) = self.region {
                    let far = ((x - sx).powi(2) + (y - sy).powi(2)).sqrt() * f64::from(zoom);
                    return self.select_colour((sx, sy), TOLERANCE.0 + far * 0.6);
                }
                if let Some(region) = &mut self.region {
                    let (x, y) = (x.clamp(0.0, bounds.0), y.clamp(0.0, bounds.1));
                    match (&region.drag, &mut region.selection.shape) {
                        (Some(Drag::New(sx, sy)), shape) => *shape = Shape::Rect(between((*sx, *sy), (x, y), bounds)),
                        (Some(Drag::Grab(grab, start, (sx, sy))), shape) => {
                            let r = crop::drag(*start, *grab, x - sx, y - sy, bounds, None);
                            *shape = Shape::Rect(between((r.x, r.y), (r.right(), r.bottom()), bounds));
                        }
                        (Some(Drag::Lasso), Shape::Lasso(points)) => {
                            // A point every screen pixel or two is plenty.
                            let near = points.last().is_some_and(|&(px, py)| {
                                ((px - x).powi(2) + (py - y).powi(2)).sqrt() * f64::from(zoom) < 2.0
                            });
                            if !near {
                                points.push((x, y));
                            }
                        }
                        _ => {}
                    }
                }
            }
            RegionMsg::Release => {
                if let Some(Region { drag: Some(Drag::Colour(..)), .. }) = self.region {
                    // The last tolerance, also on a very large picture.
                    if let Some(region) = &mut self.region {
                        region.drag = None;
                    }
                    return Task::none();
                }
                if let Some(region) = &mut self.region {
                    region.drag = None;
                    let b = region.selection.shape.bounds((bounds.0 as u32, bounds.1 as u32));
                    let (w, h) = (b.w as f32 * zoom, b.h as f32 * zoom);
                    if w < LEAST || h < LEAST {
                        self.region = None;
                    }
                }
            }
            RegionMsg::All => {
                let all = Rect { x: 0.0, y: 0.0, w: bounds.0, h: bounds.1 };
                self.region = Some(Region { selection: Selection::new(Shape::Rect(all)), drag: None, tint: None });
            }
            RegionMsg::Clear => self.region = None,
            RegionMsg::Invert => {
                if let Some(region) = &mut self.region {
                    region.selection.inverted = !region.selection.inverted;
                }
                return self.tint_selection();
            }
            RegionMsg::Delete => {
                let Some(region) = self.region.take() else { return Task::none() };
                let Doc::Image(view) = &self.doc else { return Task::none() };
                let mut picture = view.shown.picture.clone();
                let mut rgba = picture.rgba.to_vec();
                region.selection.erase(&mut rgba, (picture.width, picture.height));
                picture.rgba = rgba.into();
                // Kept as it was where clear is not, in the end.
                let flat = matches!(
                    view.path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
                    Some("jpg" | "jpeg" | "jpe" | "jfif")
                );
                let baked = self.bake(picture);
                if flat {
                    return Task::batch([
                        baked,
                        self.notice(tr("A JPEG has no transparency: saved, the cleared part is white")),
                    ]);
                }
                return baked;
            }
            RegionMsg::Mode(mode) => {
                self.select_mode = mode;
                self.region = None;
            }
            RegionMsg::Crop => {
                let Some(region) = self.region.take() else { return Task::none() };
                let size = (bounds.0 as u32, bounds.1 as u32);
                return self.crop_to(region.selection.bounds(size));
            }
        }
        Task::none()
    }

    /// Selects the pixels of about the colour at `seed`, keeping the drag that is going on.
    fn select_colour(&mut self, seed: (f64, f64), tolerance: f64) -> Task<Message> {
        let Doc::Image(view) = &self.doc else { return Task::none() };
        let picture = &view.shown.picture;
        let size = (picture.width, picture.height);
        let at = (seed.0.max(0.0) as u32, seed.1.max(0.0) as u32);
        let tolerance = tolerance.min(TOLERANCE.1) as u8;
        // A very large picture is only looked at again once the drag has moved on a bit.
        if let Some(Region { drag: Some(Drag::Colour(..)), .. }) = &self.region
            && size.0 as u64 * size.1 as u64 > 8_000_000
            && self.colour_tolerance.abs_diff(tolerance) < 12
        {
            return Task::none();
        }
        self.colour_tolerance = tolerance;
        let Some(shape) = select::by_colour(&picture.rgba, size, at, tolerance) else { return Task::none() };
        self.region =
            Some(Region { selection: Selection::new(shape), drag: Some(Drag::Colour(seed.0, seed.1)), tint: None });
        self.tint_selection()
    }

    /// How a selection of pixels is shown.
    fn tint_selection(&mut self) -> Task<Message> {
        let Doc::Image(view) = &self.doc else { return Task::none() };
        let size = (view.shown.width, view.shown.height);
        let Some(region) = &mut self.region else { return Task::none() };
        region.tint = matches!(region.selection.shape, Shape::Pixels { .. }).then(|| {
            let (w, h, rgba) = region.selection.tint(size, [255, 140, 0, 110]);
            iced::widget::image::Handle::from_rgba(w, h, rgba)
        });
        Task::none()
    }

    /// What is selected, if anything.
    pub(super) fn selection(&self) -> Option<&Selection> {
        self.region.as_ref().map(|r| &r.selection)
    }

    /// The choice of how to select, for the editing tools.
    pub(super) fn select_modes(&self) -> Element<'_, Message> {
        let mode = |m: Mode, icon, label: String| {
            icon_button(icon, label, Some(Message::Region(RegionMsg::Mode(m))), self.select_mode == m)
        };
        row![
            mode(Mode::Rect, icons::select_rect(), tr("Select a rectangle")),
            mode(Mode::Lasso, icons::lasso(), tr("Select freehand")),
            mode(Mode::Colour, icons::wand(), tr("Select by colour: click, and drag away to take in more")),
        ]
        .spacing(2)
        .into()
    }

    pub(super) fn region_layer(&self, zoom: f32, w: f32, h: f32) -> Option<Element<'_, Message>> {
        if !self.selects_region() {
            return None;
        }
        let selection = self.region.as_ref().map(|r| r.selection.clone());
        let lasso = self.select_mode != Mode::Rect;
        let outline = iced::widget::canvas(Overlay { selection, zoom, lasso }).width(w).height(h);
        // Always the same layers, so that the overlay keeps the press it is in the middle of.
        let tinted = self.region.as_ref().and_then(|r| Some((r, r.tint.clone()?)));
        let (Some((region, tint)), Doc::Image(view)) = (tinted, &self.doc) else {
            return Some(iced::widget::stack![iced::widget::space(), outline].width(w).height(h).into());
        };
        let b = region.selection.bounds((view.shown.width, view.shown.height));
        let tint = iced::widget::image(tint)
            .filter_method(iced::widget::image::FilterMethod::Nearest)
            .content_fit(iced::ContentFit::Fill)
            .width(b.w as f32 * zoom)
            .height(b.h as f32 * zoom);
        let tint = container(tint).padding(iced::Padding {
            top: b.y as f32 * zoom,
            left: b.x as f32 * zoom,
            ..iced::Padding::ZERO
        });
        Some(iced::widget::stack![tint, outline].width(w).height(h).into())
    }

    pub(super) fn region_bar(&self) -> Option<Element<'_, Message>> {
        let region = self.region.as_ref().filter(|_| self.selects_region())?;
        let Doc::Image(view) = &self.doc else { return None };
        let b = region.selection.bounds((view.shown.width, view.shown.height));
        let size = text(format!("{} × {}", b.w, b.h)).size(style::BODY).style(muted);
        let action = |label: String, message: Message| {
            button(text(label).size(style::BODY)).padding([6, 12]).style(style::secondary).on_press(message)
        };
        let bar = row![
            size,
            action(tr("Copy"), Message::CopyPicture),
            action(tr("Crop"), Message::Region(RegionMsg::Crop)),
            action(tr("Invert"), Message::Region(RegionMsg::Invert)),
            action(tr("Delete"), Message::Region(RegionMsg::Delete)),
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
    selection: Option<Selection>,
    zoom: f32,
    lasso: bool,
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
        let Some(selection) = &self.selection else { return vec![frame.into_geometry()] };
        let z = self.zoom;
        let at = |(x, y): (f64, f64)| Point::new(x as f32 * z, y as f32 * z);
        let outline = Path::new(|p| match &selection.shape {
            Shape::Rect(r) => p.rectangle(at((r.x, r.y)), Size::new(r.w as f32 * z, r.h as f32 * z)),
            Shape::Lasso(points) => {
                if let Some((first, rest)) = points.split_first() {
                    p.move_to(at(*first));
                    rest.iter().for_each(|q| p.line_to(at(*q)));
                    p.close();
                }
            }
            // Its pixels are tinted instead.
            Shape::Pixels { .. } => {}
        });
        // Dimmed is what is not selected: outside the outline, or inside it once inverted. The
        // outline goes the other way round from the window's edge, so that it is a hole in it.
        let shade = Color::from_rgba(0.0, 0.0, 0.0, 0.35);
        let corners: Vec<Point> = match &selection.shape {
            Shape::Rect(r) => {
                vec![at((r.x, r.y)), at((r.right(), r.y)), at((r.right(), r.bottom())), at((r.x, r.bottom()))]
            }
            Shape::Lasso(points) => points.iter().map(|q| at(*q)).collect(),
            Shape::Pixels { .. } => Vec::new(),
        };
        if corners.len() > 2 {
            let turning: f32 =
                corners.iter().zip(corners.iter().cycle().skip(1)).map(|(a, b)| a.x * b.y - b.x * a.y).sum();
            let (w, h) = (bounds.width, bounds.height);
            let shape = |p: &mut canvas::path::Builder, reverse: bool| {
                let mut ring: Vec<Point> = corners.clone();
                if reverse {
                    ring.reverse();
                }
                p.move_to(ring[0]);
                ring[1..].iter().for_each(|q| p.line_to(*q));
                p.close();
            };
            let shaded = if selection.inverted {
                Path::new(|p| shape(p, false))
            } else {
                Path::new(|p| {
                    // The edge clockwise on screen; the outline counter-clockwise.
                    p.move_to(Point::ORIGIN);
                    p.line_to(Point::new(w, 0.0));
                    p.line_to(Point::new(w, h));
                    p.line_to(Point::new(0.0, h));
                    p.close();
                    shape(p, turning > 0.0);
                })
            };
            frame.fill(&shaded, shade);
        }
        // Black and white dashes show on any picture.
        frame.stroke(&outline, Stroke::default().with_color(Color::BLACK).with_width(1.0));
        let dashes = LineDash { segments: &[4.0, 4.0], offset: 0 };
        let dashed = Stroke { line_dash: dashes, ..Stroke::default().with_color(Color::WHITE).with_width(1.0) };
        frame.stroke(&outline, dashed);
        if let (Shape::Rect(r), false) = (&selection.shape, selection.inverted) {
            let (x, y, w, h) = (r.x as f32 * z, r.y as f32 * z, r.w as f32 * z, r.h as f32 * z);
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
        let rect = self.selection.as_ref().filter(|s| s.is_rect()).and_then(|s| match s.shape {
            Shape::Rect(r) => Some(r),
            _ => None,
        });
        match rect.and_then(|r| crop::grab_at(r, x, y, f64::from(REACH / self.zoom))) {
            Some(Grab::Move) if *down => mouse::Interaction::Grabbing,
            Some(Grab::Move) => mouse::Interaction::Grab,
            Some(Grab::Edges { left, right, top, bottom }) => match (left || right, top || bottom) {
                (true, false) => mouse::Interaction::ResizingHorizontally,
                (false, true) => mouse::Interaction::ResizingVertically,
                _ if (left && top) || (right && bottom) => mouse::Interaction::ResizingDiagonallyDown,
                _ => mouse::Interaction::ResizingDiagonallyUp,
            },
            None if self.lasso => mouse::Interaction::Crosshair,
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
