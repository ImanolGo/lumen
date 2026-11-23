//! The markup style menu, as in the GTK Lumen: a palette, a colour picker for any other
//! colour, opacity, and a size slider (line width, or text size, which can also be typed).

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, gradient};
use iced::widget::{button, canvas as canvas_widget, column, container, opaque, row, slider, space, text, text_input};
use iced::{Alignment, Color as IcedColor, Element, Length, Point, Rectangle, Renderer, Size, Theme};

use super::markup::{MarkupMsg, SWATCHES};
use super::{App, Message, muted, tip};
use crate::annotate::{Color, Shape, Tool};
use crate::i18n::tr;
use crate::style;

const PICKER_W: f32 = 252.0;
const SQUARE_H: f32 = 120.0;
const HUE_H: f32 = 14.0;
const GAP: f32 = 10.0;

/// Hue in degrees, saturation and value in 0..=1.
pub fn to_hsv(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (c.r, c.g, c.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

pub fn from_hsv(h: f32, s: f32, v: f32, a: f32) -> Color {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color::rgba(r + m, g + m, b + m, a)
}

pub fn to_hex(c: Color) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
}

pub fn from_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    let ch = |shift: u32| ((v >> shift) & 0xff) as f32 / 255.0;
    Some(Color::rgba(ch(16), ch(8), ch(0), 1.0))
}

fn iced(c: Color) -> IcedColor {
    IcedColor::from_rgba(c.r, c.g, c.b, c.a)
}

impl App {
    /// The colour the style menu shows: the selected annotation's, or the tool's.
    pub(super) fn current_color(&self) -> Color {
        let selected = self.marks.as_ref().and_then(|m| m.store.selected());
        match selected {
            Some(a) if matches!(a.shape, Shape::Highlight(_)) => a.style.fill.unwrap_or(a.style.stroke),
            Some(a) => a.style.stroke,
            None if self.editor.tool == Tool::Highlight => self.editor.highlight_color,
            None => self.editor.style.stroke,
        }
    }

    /// Whether the style menu is about text (its size is a text size, not a line width).
    pub(super) fn styling_text(&self) -> bool {
        match self.marks.as_ref().and_then(|m| m.store.selected()).map(|a| &a.shape) {
            Some(shape) => matches!(shape, Shape::Text { .. }),
            None => self.editor.tool == Tool::Text,
        }
    }

    fn styling_highlight(&self) -> bool {
        match self.marks.as_ref().and_then(|m| m.store.selected()).map(|a| &a.shape) {
            Some(shape) => matches!(shape, Shape::Highlight(_)),
            None => self.editor.tool == Tool::Highlight,
        }
    }

    /// The size the slider shows, in points (PDFs) or the picture's own units.
    pub(super) fn current_size(&self) -> f64 {
        let unit = self.marks.as_ref().map_or(1.0, |m| m.unit);
        let selected = self.marks.as_ref().and_then(|m| m.store.selected());
        let text = self.styling_text();
        match selected {
            Some(a) if text => a.style.font_size / unit,
            Some(a) => a.style.width / unit,
            None if text => self.text_size,
            None => self.line_width,
        }
    }

    /// The button in the markup island that opens the menu: the colour, and the line width or "Aa".
    pub(super) fn style_button(&self) -> Element<'_, Message> {
        let c = iced(self.current_color());
        let text_mode = self.styling_text();
        let width = (self.current_size() as f32).clamp(1.0, 8.0);
        let face: Element<'_, Message> = if text_mode {
            text(tr("Aa")).size(14).font(style::font(iced::font::Weight::Semibold)).color(c).into()
        } else {
            container(space().width(16).height(16))
                .style(move |t: &Theme| container::Style {
                    background: Some(c.into()),
                    border: iced::Border {
                        radius: 8.0.into(),
                        width: 1.0,
                        color: t.extended_palette().background.base.text.scale_alpha(0.25),
                    },
                    ..Default::default()
                })
                .into()
        };
        let line = container(space().width(14).height(width.min(4.0))).style(move |t: &Theme| container::Style {
            background: Some(t.extended_palette().background.base.text.scale_alpha(0.7).into()),
            border: iced::Border { radius: 2.0.into(), ..Default::default() },
            ..Default::default()
        });
        let inner: Element<'_, Message> =
            if text_mode { face } else { column![face, line].spacing(3).align_x(Alignment::Center).into() };
        let b = button(container(inner).center_x(32).center_y(32))
            .padding(0)
            .style(if self.style_menu { style::tool_active } else { style::tool })
            .on_press(Message::Markup(MarkupMsg::StyleMenu));
        tip(b.into(), tr("Colour and size"))
    }

    /// The menu itself, floating under the markup island.
    pub(super) fn style_menu_view(&self) -> Element<'_, Message> {
        let m = |x: MarkupMsg| Message::Markup(x);
        let current = self.current_color();
        let label = |t: String| text(t).size(11).font(style::font(iced::font::Weight::Medium)).style(muted);

        let mut swatches = row![].spacing(6).align_y(Alignment::Center);
        for (i, (name, c)) in SWATCHES.iter().enumerate() {
            let chosen =
                (c.r - current.r).abs() < 0.01 && (c.g - current.g).abs() < 0.01 && (c.b - current.b).abs() < 0.01;
            let dot = button(space().width(18).height(18))
                .padding(0)
                .width(22)
                .height(22)
                .style(style::swatch(IcedColor::from_rgb(c.r, c.g, c.b), chosen))
                .on_press(m(MarkupMsg::Swatch(i)));
            swatches = swatches.push(tip(dot.into(), *name));
        }
        let more = button(text("+").size(15).align_x(Alignment::Center).width(22))
            .padding(0)
            .height(22)
            .style(if self.picker_open { style::tool_active } else { style::tool })
            .on_press(m(MarkupMsg::CustomColor));
        swatches = swatches.push(tip(more.into(), tr("Any colour…")));

        let mut col = column![label(tr("COLOUR")), swatches].spacing(8).width(PICKER_W);
        if self.picker_open {
            let (h, s, v) = to_hsv(current);
            let hue = if s == 0.0 || v == 0.0 { self.picker_hue } else { h };
            col =
                col.push(canvas_widget(Picker { hue, sat: s, val: v }).width(PICKER_W).height(SQUARE_H + GAP + HUE_H));
            let swatch = container(space().width(22).height(22)).style(move |t: &Theme| container::Style {
                background: Some(iced(current).into()),
                border: iced::Border {
                    radius: 6.0.into(),
                    width: 1.0,
                    color: t.extended_palette().background.base.text.scale_alpha(0.2),
                },
                ..Default::default()
            });
            col = col.push(
                row![
                    swatch,
                    text_input("#rrggbb", &self.hex_input)
                        .on_input(move |t| m(MarkupMsg::Hex(t)))
                        .on_submit(m(MarkupMsg::StyleCommit))
                        .size(13)
                        .padding([5, 8])
                        .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        let opacity = current.a;
        col = col.push(
            row![
                label(tr("OPACITY")).width(70),
                slider(0.1..=1.0, opacity, move |a| m(MarkupMsg::Opacity(a)))
                    .step(0.01_f32)
                    .on_release(m(MarkupMsg::StyleCommit)),
                text(format!("{:.0}%", opacity * 100.0)).size(12).style(muted).width(38).align_x(Alignment::End),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        if self.styling_text() {
            let size = self.current_size() as f32;
            col = col.push(
                row![
                    label(tr("TEXT SIZE")).width(70),
                    slider(6.0..=96.0, size.clamp(6.0, 96.0), move |s| m(MarkupMsg::TextSize(s)))
                        .step(1.0_f32)
                        .on_release(m(MarkupMsg::StyleCommit)),
                    text_input("18", &self.size_input)
                        .on_input(move |t| m(MarkupMsg::TextSizeTyped(t)))
                        .on_submit(m(MarkupMsg::StyleCommit))
                        .size(12)
                        .padding([4, 6])
                        .width(46),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        } else if !self.styling_highlight() {
            let width = self.current_size() as f32;
            col = col.push(
                row![
                    label(tr("LINE WIDTH")).width(70),
                    slider(0.5..=24.0, width.clamp(0.5, 24.0), move |w| m(MarkupMsg::LineWidth(w)))
                        .step(0.5_f32)
                        .on_release(m(MarkupMsg::StyleCommit)),
                    text(format!("{width:.1}")).size(12).style(muted).width(38).align_x(Alignment::End),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        opaque(container(col).padding(14).style(style::panel))
    }
}

/// The saturation/brightness square and the hue bar.
struct Picker {
    hue: f32,
    sat: f32,
    val: f32,
}

#[derive(Default)]
enum Grab {
    #[default]
    None,
    Square,
    Hue,
}

impl Picker {
    fn pick(&self, grab: &Grab, p: Point) -> Option<Message> {
        let x = (p.x / PICKER_W).clamp(0.0, 1.0);
        let msg = match grab {
            Grab::Square => {
                let y = (p.y / SQUARE_H).clamp(0.0, 1.0);
                MarkupMsg::Pick { h: self.hue, s: x, v: 1.0 - y }
            }
            Grab::Hue => MarkupMsg::Pick { h: x * 359.9, s: self.sat.max(0.05), v: self.val.max(0.05) },
            Grab::None => return None,
        };
        Some(Message::Markup(msg))
    }
}

impl canvas::Program<Message> for Picker {
    type State = Grab;

    fn update(
        &self,
        grab: &mut Grab,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        let canvas::Event::Mouse(event) = event else { return None };
        match event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let p = cursor.position_in(bounds)?;
                *grab = if p.y <= SQUARE_H {
                    Grab::Square
                } else if p.y >= SQUARE_H + GAP - 4.0 {
                    Grab::Hue
                } else {
                    return None;
                };
                self.pick(grab, p).map(|m| canvas::Action::publish(m).and_capture())
            }
            mouse::Event::CursorMoved { position } if !matches!(grab, Grab::None) => {
                let p = Point::new(position.x - bounds.x, position.y - bounds.y);
                self.pick(grab, p).map(canvas::Action::publish)
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) if !matches!(grab, Grab::None) => {
                *grab = Grab::None;
                Some(canvas::Action::publish(Message::Markup(MarkupMsg::StyleCommit)))
            }
            _ => None,
        }
    }

    fn draw(&self, _: &Grab, renderer: &Renderer, theme: &Theme, bounds: Rectangle, _: Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let pure = iced(from_hsv(self.hue, 1.0, 1.0, 1.0));
        // Saturation left to right over white, then brightness top to bottom over that.
        frame.fill_rectangle(
            Point::ORIGIN,
            Size::new(w, SQUARE_H),
            gradient::Linear::new(Point::ORIGIN, Point::new(w, 0.0))
                .add_stop(0.0, IcedColor::WHITE)
                .add_stop(1.0, pure),
        );
        frame.fill_rectangle(
            Point::ORIGIN,
            Size::new(w, SQUARE_H),
            gradient::Linear::new(Point::ORIGIN, Point::new(0.0, SQUARE_H))
                .add_stop(0.0, IcedColor::TRANSPARENT)
                .add_stop(1.0, IcedColor::BLACK),
        );
        let edge = Stroke::default()
            .with_color(theme.extended_palette().background.base.text.scale_alpha(0.2))
            .with_width(1.0);
        frame.stroke(&Path::rectangle(Point::ORIGIN, Size::new(w, SQUARE_H)), edge);
        let mut hue = gradient::Linear::new(Point::new(0.0, 0.0), Point::new(w, 0.0));
        for k in 0..=6 {
            hue = hue.add_stop(k as f32 / 6.0, iced(from_hsv(k as f32 * 60.0 % 360.0, 1.0, 1.0, 1.0)));
        }
        let top = SQUARE_H + GAP;
        frame.fill(&Path::rounded_rectangle(Point::new(0.0, top), Size::new(w, HUE_H), 7.0.into()), hue);
        // Markers.
        let ring = |frame: &mut Frame, at: Point| {
            frame.stroke(&Path::circle(at, 6.0), Stroke::default().with_color(IcedColor::WHITE).with_width(2.0));
            frame.stroke(
                &Path::circle(at, 7.5),
                Stroke::default().with_color(IcedColor::from_rgba(0.0, 0.0, 0.0, 0.4)).with_width(1.0),
            );
        };
        ring(&mut frame, Point::new(self.sat * w, (1.0 - self.val) * SQUARE_H));
        ring(&mut frame, Point::new(self.hue / 360.0 * w, top + HUE_H / 2.0));
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, _: &Grab, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if cursor.is_over(bounds) { mouse::Interaction::Crosshair } else { mouse::Interaction::None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_survive_the_round_trip() {
        for c in SWATCHES.iter().map(|(_, c)| *c) {
            let (h, s, v) = to_hsv(c);
            let back = from_hsv(h, s, v, 1.0);
            assert!((back.r - c.r).abs() < 1e-4 && (back.g - c.g).abs() < 1e-4 && (back.b - c.b).abs() < 1e-4, "{c:?}");
            let hex = from_hex(&to_hex(c)).unwrap();
            assert!((hex.r - c.r).abs() < 0.003 && (hex.b - c.b).abs() < 0.003);
        }
        assert_eq!(to_hex(from_hsv(0.0, 1.0, 1.0, 1.0)), "#ff0000");
        assert_eq!(from_hex("zz0000"), None);
        assert_eq!(from_hex("#00ff00").map(|c| (c.g * 255.0).round()), Some(255.0));
    }
}
