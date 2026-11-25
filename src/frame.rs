//! A title, a body and buttons, one above the other. When there is not room for all of it the body
//! is made shorter (it scrolls), never the buttons; iced's columns squeeze what comes last.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

pub struct Frame<'a, Message> {
    /// Title, body, buttons.
    parts: [Element<'a, Message>; 3],
    gap: f32,
}

pub fn frame<'a, Message: 'a>(
    title: Element<'a, Message>,
    body: Element<'a, Message>,
    buttons: Element<'a, Message>,
    gap: f32,
) -> Element<'a, Message> {
    Element::new(Frame { parts: [title, body, buttons], gap })
}

/// What is left for the body of a frame at most `max` high.
fn body_room(max: f32, title: f32, buttons: f32, gap: f32) -> f32 {
    (max - title - buttons - 2.0 * gap).max(0.0)
}

impl<Message> Widget<Message, Theme, Renderer> for Frame<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        self.parts.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.parts);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let max = limits.max();
        let mut place = |i: usize, height: f32| {
            let limits = layout::Limits::new(Size::ZERO, Size::new(max.width, height));
            self.parts[i].as_widget_mut().layout(&mut tree.children[i], renderer, &limits)
        };
        let title = place(0, max.height);
        let buttons = place(2, max.height);
        let (above, below) = (title.size().height, buttons.size().height);
        let body = place(1, body_room(max.height, above, below, self.gap));
        let middle = body.size().height;
        let body = body.move_to(Point::new(0.0, above + self.gap));
        let end = above + middle + 2.0 * self.gap;
        let buttons = buttons.move_to(Point::new(0.0, end));
        layout::Node::with_children(Size::new(max.width, end + below), vec![title, body, buttons])
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        for ((part, tree), layout) in self.parts.iter_mut().zip(&mut tree.children).zip(layout.children()) {
            part.as_widget_mut().operate(tree, layout, renderer, operation);
        }
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
        for ((part, tree), layout) in self.parts.iter_mut().zip(&mut tree.children).zip(layout.children()) {
            part.as_widget_mut().update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.parts
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((part, tree), layout)| part.as_widget().mouse_interaction(tree, layout, cursor, viewport, renderer))
            .max()
            .unwrap_or_default()
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
        for ((part, tree), layout) in self.parts.iter().zip(&tree.children).zip(layout.children()) {
            part.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(&mut self.parts, tree, layout, renderer, viewport, translation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_body_gives_way() {
        assert_eq!(body_room(300.0, 24.0, 32.0, 14.0), 216.0);
        assert_eq!(body_room(50.0, 24.0, 32.0, 14.0), 0.0);
    }
}
