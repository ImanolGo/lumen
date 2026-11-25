//! The words that show while the pointer rests on something. Unlike iced's tooltip they go away
//! when it is clicked or a key is pressed, until the pointer comes back: what the click opened is
//! not to be covered by them. Nor do they show while a menu is open.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, keyboard, mouse};

/// Between the words and what they are about, and the window's edge.
const GAP: f32 = 6.0;
const EDGE: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Below; above where there is no room.
    Below,
    Right,
    /// Over the pointer.
    Pointer,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum State {
    #[default]
    Idle,
    /// With the pointer there.
    Shown(Point),
    /// Clicked: nothing until the pointer has left.
    Dismissed,
}

impl State {
    /// After `event`, with the pointer at `over` if it is over the content.
    fn after(self, event: &Event, over: Option<Point>) -> State {
        let dismisses = matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(_)) | Event::Keyboard(keyboard::Event::KeyPressed { .. })
        );
        match (self, over) {
            (_, None) => State::Idle,
            (State::Dismissed, _) => State::Dismissed,
            (State::Shown(_), _) if dismisses => State::Dismissed,
            (State::Idle, _) if dismisses => State::Dismissed,
            (_, Some(p)) => State::Shown(p),
        }
    }
}

/// `content`, with `words` shown beside it.
pub struct Hint<'a, Message> {
    parts: [Element<'a, Message>; 2],
    side: Side,
}

pub fn hint<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    words: impl Into<Element<'a, Message>>,
    side: Side,
) -> Element<'a, Message> {
    Element::new(Hint { parts: [content.into(), words.into()], side })
}

fn place(side: Side, about: Rectangle, pointer: Point, words: Size, window: Size) -> Point {
    let mut at = match side {
        Side::Below => Point::new(about.x + (about.width - words.width) / 2.0, about.y + about.height + GAP),
        Side::Right => Point::new(about.x + about.width + GAP, about.y + (about.height - words.height) / 2.0),
        Side::Pointer => Point::new(pointer.x, pointer.y - words.height - GAP),
    };
    if side == Side::Below && at.y + words.height + EDGE > window.height {
        at.y = about.y - GAP - words.height;
    }
    let within = |v: f32, size: f32, all: f32| v.min(all - size - EDGE).max(EDGE);
    Point::new(within(at.x, words.width, window.width), within(at.y, words.height, window.height))
}

impl<Message> Widget<Message, Theme, Renderer> for Hint<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.parts.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.parts);
    }

    fn size(&self) -> Size<Length> {
        self.parts[0].as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.parts[0].as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.parts[0].as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.parts[0].as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if matches!(
            event,
            Event::Mouse(_) | Event::Keyboard(_) | Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            let state = tree.state.downcast_mut::<State>();
            let before = *state;
            *state = before.after(event, cursor.position_over(layout.bounds()));
            let shown = |s: State| matches!(s, State::Shown(_));
            if shown(before) != shown(*state) || (self.side == Side::Pointer && before != *state) {
                shell.invalidate_layout();
                shell.request_redraw();
            }
        }
        self.parts[0].as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.parts[0].as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.parts[0].as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let state = *tree.state.downcast_ref::<State>();
        let [content, words] = &mut self.parts;
        let (below, above) = tree.children.split_at_mut(1);
        let inner = content.as_widget_mut().overlay(&mut below[0], layout, renderer, viewport, translation);
        let own = match state {
            State::Shown(p) => Some(overlay::Element::new(Box::new(Words {
                words,
                tree: &mut above[0],
                about: layout.bounds() + translation,
                pointer: p + translation,
                side: self.side,
            }))),
            _ => None,
        };
        (inner.is_some() || own.is_some())
            .then(|| overlay::Group::with_children(inner.into_iter().chain(own).collect()).overlay())
    }
}

struct Words<'a, 'b, Message> {
    words: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    about: Rectangle,
    pointer: Point,
    side: Side,
}

impl<Message> overlay::Overlay<Message, Theme, Renderer> for Words<'_, '_, Message> {
    fn layout(&mut self, renderer: &Renderer, window: Size) -> layout::Node {
        let most = Size::new((window.width - 2.0 * EDGE).max(0.0), window.height);
        let node = self.words.as_widget_mut().layout(self.tree, renderer, &layout::Limits::new(Size::ZERO, most));
        let at = place(self.side, self.about, self.pointer, node.size(), window);
        node.move_to(at)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.words.as_widget().draw(self.tree, renderer, theme, style, layout, cursor, &layout.bounds());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_or_a_key_takes_the_words_away_until_the_pointer_comes_back() {
        let moved = Event::Mouse(mouse::Event::CursorMoved { position: Point::ORIGIN });
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let up = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        let over = Some(Point::new(5.0, 5.0));
        let shown = State::Idle.after(&moved, over);
        assert_eq!(shown, State::Shown(Point::new(5.0, 5.0)));
        let clicked = shown.after(&press, over);
        assert_eq!(clicked, State::Dismissed);
        assert_eq!(clicked.after(&up, over).after(&moved, over), State::Dismissed);
        // Away (or under an open menu, which hides the pointer from it), then back.
        assert_eq!(clicked.after(&moved, None), State::Idle);
        assert_eq!(clicked.after(&moved, None).after(&moved, over), shown);
    }

    #[test]
    fn words_stay_in_the_window_and_off_what_they_are_about() {
        let window = Size::new(520.0, 360.0);
        let words = Size::new(100.0, 24.0);
        let top = Rectangle::new(Point::new(0.0, 10.0), Size::new(32.0, 32.0));
        assert_eq!(place(Side::Below, top, Point::ORIGIN, words, window), Point::new(4.0, 48.0));
        let bottom = Rectangle::new(Point::new(200.0, 320.0), Size::new(32.0, 32.0));
        assert_eq!(place(Side::Below, bottom, Point::ORIGIN, words, window), Point::new(166.0, 290.0));
    }
}
