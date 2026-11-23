//! The markup layer over a picture or a PDF page: draws the annotations and
//! turns pointer events into editor messages, in document coordinates.

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Frame, Geometry, LineCap, LineDash, LineJoin, Path, Stroke};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, Vector};

use super::{MarkupMsg, Message};
use crate::annotate::model::{Annotation, Handle};
use crate::annotate::paint::{self, LINE_HEIGHT, MARKER_ALPHA, TEXT_PAD};
use crate::annotate::{AnnotationLayer, Shape};

/// Pointer reach for picking an annotation or a handle, in screen pixels.
const REACH: f32 = 6.0;
const HANDLE: f32 = 8.0;

/// Which presses the layer takes for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// Draws only; every event goes to the page underneath.
    None,
    /// Every press starts a shape.
    All,
    /// Presses on an annotation or a handle (the select tool); the rest select text or pan.
    Annotations,
    /// Presses that are not on text (the highlighter, whose presses on text select it).
    OffText,
}

pub struct Overlay<'a> {
    pub page: usize,
    /// Screen pixels per document unit.
    pub zoom: f32,
    pub layer: Option<&'a AnnotationLayer>,
    pub draft: Option<&'a Annotation>,
    pub selected: Option<&'a Annotation>,
    /// The text box being edited is drawn by the editor widget instead.
    pub hidden: Option<u64>,
    pub capture: Capture,
    pub over_text: bool,
    pub text_tool: bool,
}

#[derive(Default)]
pub struct State {
    dragging: bool,
    /// Where and when the last press was, and how many came in quick succession.
    last_click: Option<(Point, std::time::Instant, u32)>,
}

fn color(c: crate::annotate::Color) -> Color {
    Color::from_rgba(c.r, c.g, c.b, c.a)
}

impl Overlay<'_> {
    fn to_doc(&self, p: Point) -> (f64, f64) {
        ((p.x / self.zoom) as f64, (p.y / self.zoom) as f64)
    }

    fn handle_at(&self, x: f64, y: f64) -> Option<Handle> {
        self.selected?.handle_at(crate::annotate::Point::new(x, y), (HANDLE / self.zoom) as f64)
    }

    fn hits(&self, x: f64, y: f64) -> bool {
        let p = crate::annotate::Point::new(x, y);
        self.handle_at(x, y).is_some() || self.layer.is_some_and(|l| l.hit(p, (REACH / self.zoom) as f64).is_some())
    }

    fn takes_press(&self, x: f64, y: f64) -> bool {
        match self.capture {
            Capture::None => false,
            Capture::All => true,
            Capture::Annotations => self.hits(x, y),
            Capture::OffText => !self.over_text,
        }
    }

    fn draw_annotation(&self, frame: &mut Frame, a: &Annotation) {
        let z = self.zoom as f64;
        for prim in paint::prims(a) {
            let path = Path::new(|b| {
                for el in prim.path.elements() {
                    let p = |q: hayro::kurbo::Point| Point::new((q.x * z) as f32, (q.y * z) as f32);
                    match *el {
                        hayro::kurbo::PathEl::MoveTo(q) => b.move_to(p(q)),
                        hayro::kurbo::PathEl::LineTo(q) => b.line_to(p(q)),
                        hayro::kurbo::PathEl::QuadTo(c, q) => b.quadratic_curve_to(p(c), p(q)),
                        hayro::kurbo::PathEl::CurveTo(c1, c2, q) => b.bezier_curve_to(p(c1), p(c2), p(q)),
                        hayro::kurbo::PathEl::ClosePath => b.close(),
                    }
                }
            });
            if let Some(fill) = prim.fill {
                let mut c = color(fill);
                if prim.marker {
                    c.a = MARKER_ALPHA;
                }
                frame.fill(&path, c);
            }
            if let Some((c, w)) = prim.stroke {
                frame.stroke(
                    &path,
                    Stroke::default()
                        .with_color(color(c))
                        .with_width(((w * z) as f32).max(1.0))
                        .with_line_cap(LineCap::Round)
                        .with_line_join(LineJoin::Round),
                );
            }
        }
        if let Some(block) = paint::text(a) {
            let size = (block.size * z) as f32;
            let pad = (block.size * TEXT_PAD * z) as f32;
            for (i, line) in block.lines.iter().enumerate() {
                frame.fill_text(canvas::Text {
                    content: (*line).to_owned(),
                    position: Point::new(
                        (block.bounds.x * z) as f32 + pad,
                        (block.bounds.y * z) as f32 + pad + i as f32 * size * LINE_HEIGHT as f32,
                    ),
                    color: color(block.color),
                    size: size.into(),
                    line_height: iced::widget::text::LineHeight::Relative(LINE_HEIGHT as f32),
                    ..canvas::Text::default()
                });
            }
        }
    }

    fn draw_selection(&self, frame: &mut Frame, a: &Annotation, theme: &Theme) {
        let accent = theme.extended_palette().primary.base.color;
        let z = self.zoom;
        let b = a.bounds();
        let r = Rectangle::new(
            Point::new(b.x as f32 * z - 3.0, b.y as f32 * z - 3.0),
            Size::new(b.w as f32 * z + 6.0, b.h as f32 * z + 6.0),
        );
        if !matches!(a.shape, Shape::Line(..) | Shape::Arrow(..)) {
            frame.stroke(
                &Path::rectangle(r.position(), r.size()),
                Stroke {
                    line_dash: LineDash { segments: &[4.0, 3.0], offset: 0 },
                    ..Stroke::default().with_color(accent).with_width(1.0)
                },
            );
        }
        for (_, at) in a.handles() {
            let c = Point::new(at.x as f32 * z, at.y as f32 * z);
            let square = Path::rectangle(c - Vector::new(HANDLE / 2.0, HANDLE / 2.0), Size::new(HANDLE, HANDLE));
            frame.fill(&square, Color::WHITE);
            frame.stroke(&square, Stroke::default().with_color(accent).with_width(1.5));
        }
    }
}

impl canvas::Program<Message> for Overlay<'_> {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        let canvas::Event::Mouse(event) = event else { return None };
        match event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let at = cursor.position_in(bounds)?;
                let (x, y) = self.to_doc(at);
                if !self.takes_press(x, y) {
                    return None;
                }
                let now = std::time::Instant::now();
                let clicks = match state.last_click {
                    Some((p, t, n)) if now.duration_since(t).as_millis() < 400 && p.distance(at) < 5.0 => n % 3 + 1,
                    _ => 1,
                };
                state.last_click = Some((at, now, clicks));
                state.dragging = true;
                Some(
                    canvas::Action::publish(Message::Markup(MarkupMsg::Press { page: self.page, x, y, clicks }))
                        .and_capture(),
                )
            }
            mouse::Event::CursorMoved { position } if state.dragging => {
                let (x, y) = self.to_doc(*position - Vector::new(bounds.x, bounds.y));
                Some(canvas::Action::publish(Message::Markup(MarkupMsg::Drag { x, y })))
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) if state.dragging => {
                state.dragging = false;
                let position = cursor.position()?;
                let (x, y) = self.to_doc(position - Vector::new(bounds.x, bounds.y));
                Some(canvas::Action::publish(Message::Markup(MarkupMsg::Release { x, y })))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if let Some(layer) = self.layer {
            for a in layer.items.iter().filter(|a| Some(a.id) != self.hidden) {
                self.draw_annotation(&mut frame, a);
            }
        }
        if let Some(draft) = self.draft {
            self.draw_annotation(&mut frame, draft);
        }
        if let Some(selected) = self.selected.filter(|a| Some(a.id) != self.hidden) {
            self.draw_selection(&mut frame, selected, theme);
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        let Some(at) = cursor.position_in(bounds) else { return mouse::Interaction::None };
        let (x, y) = self.to_doc(at);
        match self.capture {
            Capture::None => mouse::Interaction::None,
            Capture::All | Capture::OffText if self.text_tool => mouse::Interaction::Text,
            Capture::All => mouse::Interaction::Crosshair,
            Capture::OffText if self.over_text => mouse::Interaction::None,
            Capture::OffText => mouse::Interaction::Crosshair,
            Capture::Annotations => match self.handle_at(x, y) {
                Some(Handle::End(_)) => mouse::Interaction::Crosshair,
                Some(Handle::Edges { left, right, top, bottom }) => match (left || right, top || bottom) {
                    (true, false) => mouse::Interaction::ResizingHorizontally,
                    (false, true) => mouse::Interaction::ResizingVertically,
                    _ if (left && top) || (right && bottom) => mouse::Interaction::ResizingDiagonallyDown,
                    _ => mouse::Interaction::ResizingDiagonallyUp,
                },
                None if state.dragging => mouse::Interaction::Grabbing,
                None if self.hits(x, y) => mouse::Interaction::Grab,
                None => mouse::Interaction::None,
            },
        }
    }
}
