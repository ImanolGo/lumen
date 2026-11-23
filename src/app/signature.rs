//! Signing: a pad to draw a signature on, the menu of saved ones, and placing one
//! on the page as ink, ready to be moved and resized like any other markup.
//!
//! This is a handwritten signature, as on paper; it is not a cryptographic one.

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Frame, Geometry, LineCap, LineJoin, Path, Stroke};
use iced::widget::{button, canvas as canvas_widget, center, column, container, mouse_area, row, space, text};
use iced::{Alignment, Color, Element, Font, Length, Padding, Point, Rectangle, Renderer, Size, Task, Theme, Vector};

use super::markup::MarkupMsg;
use super::{App, Doc, Message, TOOLBAR_H, icon_button, muted};
use crate::annotate::model::Style;
use crate::annotate::{Color as InkColor, Rect, Tool};
use crate::i18n::tr;
use crate::signatures::Signature;
use crate::{icons, style};

/// Blue-black, like a fountain pen.
const INK: InkColor = InkColor::rgba(0.10, 0.14, 0.42, 1.0);
const PAD: Size = Size::new(520.0, 200.0);
const PREVIEW: Size = Size::new(190.0, 56.0);

#[derive(Default)]
pub struct Pad {
    strokes: Vec<Vec<(f32, f32)>>,
    down: bool,
}

#[derive(Debug, Clone)]
pub enum SignMsg {
    Menu,
    Close,
    Use(usize),
    Remove(usize),
    Draw,
    /// A signature field was clicked: x, y, width, height in page points.
    Field {
        page: usize,
        rect: [f64; 4],
    },
    PadPress(f32, f32),
    PadMove(f32, f32),
    PadRelease,
    PadClear,
    PadCancel,
    PadDone,
}

impl App {
    pub(super) fn sign_update(&mut self, msg: SignMsg) -> Task<Message> {
        match msg {
            SignMsg::Menu => {
                self.sign_menu = !self.sign_menu;
                if !self.sign_menu {
                    self.sign_target = None;
                }
                Task::none()
            }
            SignMsg::Close => {
                self.sign_menu = false;
                self.sign_target = None;
                Task::none()
            }
            SignMsg::Field { page, rect } => {
                self.sign_target = Some((page, Rect::new(rect[0], rect[1], rect[2], rect[3])));
                if self.signatures.items.is_empty() {
                    self.pad = Some(Pad::default());
                } else {
                    self.sign_menu = true;
                }
                Task::none()
            }
            SignMsg::Use(i) => {
                self.sign_menu = false;
                let Some(signature) = self.signatures.items.get(i).cloned() else { return Task::none() };
                self.sign(signature)
            }
            SignMsg::Remove(i) => {
                self.signatures.remove(i);
                Task::none()
            }
            SignMsg::Draw => {
                self.sign_menu = false;
                self.pad = Some(Pad::default());
                Task::none()
            }
            SignMsg::PadPress(x, y) => {
                if let Some(pad) = &mut self.pad {
                    pad.down = true;
                    pad.strokes.push(vec![(x, y)]);
                }
                Task::none()
            }
            SignMsg::PadMove(x, y) => {
                if let Some(pad) = &mut self.pad
                    && pad.down
                    && let Some(stroke) = pad.strokes.last_mut()
                    && stroke.last().is_none_or(|&(px, py)| (px - x).hypot(py - y) >= 1.0)
                {
                    stroke.push((x.clamp(0.0, PAD.width), y.clamp(0.0, PAD.height)));
                }
                Task::none()
            }
            SignMsg::PadRelease => {
                if let Some(pad) = &mut self.pad {
                    pad.down = false;
                }
                Task::none()
            }
            SignMsg::PadClear => {
                if let Some(pad) = &mut self.pad {
                    pad.strokes.clear();
                }
                Task::none()
            }
            SignMsg::PadCancel => {
                self.pad = None;
                self.sign_target = None;
                Task::none()
            }
            SignMsg::PadDone => {
                let Some(pad) = self.pad.take() else { return Task::none() };
                let strokes: Vec<Vec<(f64, f64)>> =
                    pad.strokes.iter().map(|s| s.iter().map(|&(x, y)| (x as f64, y as f64)).collect()).collect();
                let Some(signature) = Signature::from_strokes(&strokes) else { return Task::none() };
                self.signatures.add(signature.clone());
                self.sign(signature)
            }
        }
    }

    /// Escape: close the pad, then the menu.
    pub(super) fn sign_escape(&mut self) -> bool {
        if self.pad.take().is_some() || std::mem::take(&mut self.sign_menu) {
            self.sign_target = None;
            return true;
        }
        false
    }

    /// Where a signature goes when no field was picked: the middle of what is in view.
    fn default_sign_target(&self) -> Option<(usize, Rect)> {
        match &self.doc {
            Doc::Pdf(view) => {
                let page = view.current_page;
                let (pw, ph) = view.doc.page_size(page);
                let mid = (self.viewport.y + self.area.height / 2.0 - view.tops[page]) / view.zoom;
                let (w, h) = (160.0_f64, 56.0_f64);
                let y = (mid as f64 - h / 2.0).clamp(0.0, (ph as f64 - h).max(0.0));
                Some((page, Rect::new((pw as f64 - w) / 2.0, y, w, h)))
            }
            Doc::Image(view) => {
                let (iw, ih) = (view.shown.width as f64, view.shown.height as f64);
                let w = iw * 0.3;
                let h = (w / 3.0).min(ih * 0.3);
                Some((0, Rect::new((iw - w) / 2.0, (ih - h) / 2.0, w, h)))
            }
            _ => None,
        }
    }

    /// Puts `signature` on the page as ink, selected so it can be moved into place.
    pub(super) fn sign(&mut self, signature: Signature) -> Task<Message> {
        let Some((page, target)) = self.sign_target.take().or_else(|| self.default_sign_target()) else {
            return Task::none();
        };
        let opened = if self.markup_open { Task::none() } else { self.markup_update(MarkupMsg::Toggle) };
        if self.marks.is_none() {
            // A PDF is still being prepared for markup; sign once it is ready.
            self.pending_sign = Some((signature, page, target));
            return opened;
        }
        Task::batch([opened, self.place_signature(signature, page, target)])
    }

    pub(super) fn place_signature(&mut self, signature: Signature, page: usize, target: Rect) -> Task<Message> {
        let Some(marks) = &mut self.marks else { return Task::none() };
        let shape = signature.shape_in(target);
        let height = target.h.min(target.w / signature.aspect());
        let style = Style { stroke: INK, fill: None, width: (height / 28.0).clamp(0.6, 40.0), ..Style::default() };
        let outcome = self.editor.place(&mut marks.store, page, shape, style);
        self.editor.tool = Tool::Select;
        if outcome.committed {
            marks.history.commit(marks.store.pages.layers.clone());
        }
        Task::none()
    }

    // ------------------------------------------------------------ view

    pub(super) fn sign_menu_view(&self) -> Element<'_, Message> {
        let mut list = column![
            text(if self.sign_target.is_some() { tr("Sign this field") } else { tr("Add your signature") })
                .size(14)
                .font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT }),
        ]
        .spacing(8);
        if self.signatures.items.is_empty() {
            list = list.push(text(tr("No saved signatures yet.")).size(13).style(muted));
        }
        for (i, signature) in self.signatures.items.iter().enumerate() {
            let preview = canvas_widget(Preview { signature }).width(PREVIEW.width).height(PREVIEW.height);
            let pick = button(preview).padding(4).style(style::thumb(false)).on_press(Message::Sign(SignMsg::Use(i)));
            let remove = icon_button(
                icons::delete(),
                tr("Forget this signature"),
                Some(Message::Sign(SignMsg::Remove(i))),
                false,
            );
            list = list.push(row![pick, remove].spacing(4).align_y(Alignment::Center));
        }
        list = list.push(
            button(text(tr("Draw a New Signature…")).size(13))
                .padding([7, 12])
                .width(Length::Fill)
                .style(style::secondary)
                .on_press(Message::Sign(SignMsg::Draw)),
        );
        let panel = container(list.width(250)).padding(14).style(style::panel);
        // A click anywhere else closes the menu.
        let backdrop =
            mouse_area(space().width(Length::Fill).height(Length::Fill)).on_press(Message::Sign(SignMsg::Close));
        iced::widget::stack![
            backdrop,
            container(panel).width(Length::Fill).align_x(Alignment::End).padding(Padding {
                top: TOOLBAR_H + 4.0,
                right: 60.0,
                ..Padding::ZERO
            }),
        ]
        .into()
    }

    pub(super) fn pad_view(&self) -> Element<'_, Message> {
        let Some(pad) = &self.pad else { return space().into() };
        let drawing = canvas_widget(PadCanvas { pad }).width(PAD.width).height(PAD.height);
        let has_ink = pad.strokes.iter().any(|s| s.len() > 1);
        let card = container(
            column![
                text(tr("Draw your signature"))
                    .size(18)
                    .font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT }),
                text(tr("It is kept for next time, on this computer only.")).size(13).style(muted),
                drawing,
                row![
                    button(text(tr("Clear")).size(14))
                        .padding([8, 16])
                        .style(style::secondary)
                        .on_press(Message::Sign(SignMsg::PadClear)),
                    space().width(Length::Fill),
                    button(text(tr("Cancel")).size(14))
                        .padding([8, 16])
                        .style(style::secondary)
                        .on_press(Message::Sign(SignMsg::PadCancel)),
                    button(text(tr("Use Signature")).size(14))
                        .padding([8, 16])
                        .style(style::primary)
                        .on_press_maybe(has_ink.then_some(Message::Sign(SignMsg::PadDone))),
                ]
                .spacing(10),
            ]
            .spacing(10)
            .width(PAD.width),
        )
        .padding(22)
        .style(style::panel);
        center(card).style(style::scrim).into()
    }
}

fn ink_stroke(width: f32) -> Stroke<'static> {
    Stroke::default()
        .with_color(Color::from_rgba(INK.r, INK.g, INK.b, 1.0))
        .with_width(width)
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round)
}

/// A saved signature, small.
struct Preview<'a> {
    signature: &'a Signature,
}

impl canvas::Program<Message> for Preview<'_> {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), Color::WHITE);
        let pad = 6.0;
        let scale =
            ((bounds.height - 2.0 * pad) as f64).min((bounds.width - 2.0 * pad) as f64 / self.signature.aspect());
        let left = (bounds.width as f64 - self.signature.aspect() * scale) / 2.0;
        let top = (bounds.height as f64 - scale) / 2.0;
        for s in &self.signature.strokes {
            let path = Path::new(|b| {
                for (i, p) in s.iter().enumerate() {
                    let q = Point::new((left + p[0] * scale) as f32, (top + p[1] * scale) as f32);
                    if i == 0 {
                        b.move_to(q);
                        b.line_to(q + Vector::new(0.01, 0.0));
                    } else {
                        b.line_to(q);
                    }
                }
            });
            frame.stroke(&path, ink_stroke(1.6));
        }
        vec![frame.into_geometry()]
    }
}

/// The drawing pad.
struct PadCanvas<'a> {
    pad: &'a Pad,
}

impl canvas::Program<Message> for PadCanvas<'_> {
    type State = bool;

    fn update(
        &self,
        down: &mut bool,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        let canvas::Event::Mouse(event) = event else { return None };
        let at = |p: Point| (p.x - bounds.x, p.y - bounds.y);
        match event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let p = cursor.position_in(bounds)?;
                *down = true;
                Some(canvas::Action::publish(Message::Sign(SignMsg::PadPress(p.x, p.y))).and_capture())
            }
            mouse::Event::CursorMoved { position } if *down => {
                let (x, y) = at(*position);
                Some(canvas::Action::publish(Message::Sign(SignMsg::PadMove(x, y))))
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) if *down => {
                *down = false;
                Some(canvas::Action::publish(Message::Sign(SignMsg::PadRelease)))
            }
            _ => None,
        }
    }

    fn draw(&self, _: &bool, renderer: &Renderer, theme: &Theme, bounds: Rectangle, _: Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let edge = theme.extended_palette().background.strong.color;
        let paper = Path::rounded_rectangle(Point::ORIGIN, bounds.size(), 8.0.into());
        frame.fill(&paper, Color::WHITE);
        frame.stroke(&paper, Stroke::default().with_color(edge).with_width(1.0));
        let base = bounds.height * 0.72;
        frame.stroke(
            &Path::line(Point::new(30.0, base), Point::new(bounds.width - 30.0, base)),
            Stroke::default().with_color(Color::from_rgba(0.0, 0.0, 0.0, 0.18)).with_width(1.0),
        );
        for s in &self.pad.strokes {
            let path = Path::new(|b| {
                for (i, &(x, y)) in s.iter().enumerate() {
                    if i == 0 {
                        b.move_to(Point::new(x, y));
                        b.line_to(Point::new(x + 0.01, y));
                    } else {
                        b.line_to(Point::new(x, y));
                    }
                }
            });
            frame.stroke(&path, ink_stroke(2.6));
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, _: &bool, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if cursor.is_over(bounds) { mouse::Interaction::Crosshair } else { mouse::Interaction::None }
    }
}
