//! One line of text that ends in "…" when there is not room for all of it (iced only cuts it off).

use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Widget, renderer};
use iced::alignment;
use iced::{Color, Element, Font, Length, Pixels, Rectangle, Renderer, Size, Theme, mouse};

type Paragraph = <Renderer as text::Renderer>::Paragraph;

pub struct Ellipsis<'a> {
    content: std::borrow::Cow<'a, str>,
    size: f32,
    font: Font,
    centered: bool,
    color: Option<fn(&Theme) -> Color>,
}

/// `content` on one line as wide as there is room.
pub fn ellipsis<'a>(content: impl Into<std::borrow::Cow<'a, str>>, size: impl Into<Pixels>) -> Ellipsis<'a> {
    Ellipsis { content: content.into(), size: size.into().0, font: crate::style::FONT, centered: false, color: None }
}

impl Ellipsis<'_> {
    pub fn font(mut self, font: Font) -> Self {
        self.font = font;
        self
    }

    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    pub fn color(mut self, color: fn(&Theme) -> Color) -> Self {
        self.color = Some(color);
        self
    }

    fn paragraph(&self, content: &str) -> Paragraph {
        Paragraph::with_text(text::Text {
            content,
            bounds: Size::INFINITE,
            size: self.size.into(),
            line_height: text::LineHeight::default(),
            font: self.font,
            align_x: text::Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: text::Shaping::Auto,
            wrapping: text::Wrapping::None,
        })
    }
}

/// `content`, or as much of its start as `fits` with "…" after it.
fn shortened(content: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    if fits(content) {
        return content.to_owned();
    }
    let ends: Vec<usize> = content.char_indices().map(|(i, _)| i).collect();
    let cut = |chars: usize| format!("{}…", content[..ends[chars]].trim_end());
    // The most characters that fit is in `low..high`.
    let (mut low, mut high) = (0, ends.len());
    while high - low > 1 {
        let middle = (low + high) / 2;
        if fits(&cut(middle)) { low = middle } else { high = middle }
    }
    if ends.is_empty() { String::new() } else { cut(low) }
}

/// What was laid out, and for what.
#[derive(Default)]
struct State {
    content: String,
    room: f32,
    font: Option<(f32, Font)>,
    paragraph: Paragraph,
}

impl<Message> Widget<Message, Theme, Renderer> for Ellipsis<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let room = limits.max().width;
        let font = Some((self.size, self.font));
        if state.content != self.content || state.room != room || state.font != font {
            let shown = shortened(&self.content, |s| self.paragraph(s).min_width() <= room);
            *state = State { content: self.content.to_string(), room, font, paragraph: self.paragraph(&shown) };
        }
        let height = state.paragraph.min_height();
        layout::Node::new(limits.resolve(Length::Fill, Length::Shrink, Size::new(state.paragraph.min_width(), height)))
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let mut at = bounds.position();
        if self.centered {
            at.x += ((bounds.width - state.paragraph.min_width()) / 2.0).max(0.0);
        }
        let color = self.color.map_or(style.text_color, |color| color(theme));
        if let Some(clip) = bounds.intersection(viewport) {
            text::Renderer::fill_paragraph(renderer, &state.paragraph, at, color, clip);
        }
    }
}

impl<'a, Message: 'a> From<Ellipsis<'a>> for Element<'a, Message> {
    fn from(e: Ellipsis<'a>) -> Self {
        Element::new(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_line_keeps_as_much_of_its_start_as_fits() {
        let room = |n: usize| move |s: &str| s.chars().count() <= n;
        assert_eq!(shortened("Chapter", room(7)), "Chapter");
        assert_eq!(shortened("Chapter", room(6)), "Chapt…");
        assert_eq!(shortened("One two", room(5)), "One…", "no space before the dots");
        assert_eq!(shortened("Índice útil", room(4)), "Índ…");
        assert_eq!(shortened("ab", room(0)), "…");
        assert_eq!(shortened("", room(0)), "");
    }

    #[test]
    fn it_is_never_wider_than_its_room() {
        let e = ellipsis("Interfacing with other programming languages", 14.0);
        let shown = shortened(&e.content, |s| e.paragraph(s).min_width() <= 120.0);
        assert!(shown.ends_with('…') && shown.len() > 8, "{shown}");
        assert!(e.paragraph(&shown).min_width() <= 120.0);
    }
}
