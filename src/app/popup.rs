//! A panel that opens under a button or at the pointer, over everything else, and always inside
//! the window: it is moved and made shorter as needed (what is in it scrolls).

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

/// Kept free around a panel, and between it and its button.
const EDGE: f32 = 8.0;
const GAP: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    /// Under the base, their left or right edges in line.
    Start,
    End,
    /// With its corner there.
    At(Point),
}

pub struct Popup<'a, Message> {
    /// What is always there, and the panel.
    parts: [Element<'a, Message>; 2],
    open: Option<(Anchor, Message)>,
    on_right_press: Option<fn(Point) -> Message>,
}

/// `base`, with a panel over it if there is one; `close` is sent by a press anywhere else.
pub fn popup<'a, Message: 'a>(
    base: impl Into<Element<'a, Message>>,
    panel: Option<Element<'a, Message>>,
    anchor: Anchor,
    close: Message,
) -> Popup<'a, Message> {
    let open = panel.is_some().then_some((anchor, close));
    let panel = panel.unwrap_or_else(|| iced::widget::space().into());
    Popup { parts: [base.into(), panel], open, on_right_press: None }
}

impl<Message> Popup<'_, Message> {
    /// Says where the right button goes down, before anything in the base hears of it.
    pub fn on_right_press(mut self, message: fn(Point) -> Message) -> Self {
        self.on_right_press = Some(message);
        self
    }
}

/// Where a panel goes: its position, and the most it can be. `base` is the button's bounds.
fn room(anchor: Anchor, base: Rectangle, window: Size) -> Size {
    let top = match anchor {
        Anchor::At(_) => EDGE,
        _ => base.y + base.height + GAP,
    };
    Size::new((window.width - 2.0 * EDGE).max(0.0), (window.height - top - EDGE).max(0.0))
}

fn place(anchor: Anchor, base: Rectangle, panel: Size, window: Size) -> Point {
    let at = match anchor {
        Anchor::Start => Point::new(base.x, base.y + base.height + GAP),
        Anchor::End => Point::new(base.x + base.width - panel.width, base.y + base.height + GAP),
        Anchor::At(p) => p,
    };
    let within = |v: f32, size: f32, all: f32| v.min(all - size - EDGE).max(EDGE);
    Point::new(within(at.x, panel.width, window.width), within(at.y, panel.height, window.height))
}

impl<Message: Clone> Widget<Message, Theme, Renderer> for Popup<'_, Message> {
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
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) = event
            && let (Some(message), Some(p)) = (self.on_right_press, cursor.position_over(layout.bounds()))
        {
            shell.publish(message(p));
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
        let [base, panel] = &mut self.parts;
        let (below, above) = tree.children.split_at_mut(1);
        let inner = base.as_widget_mut().overlay(&mut below[0], layout, renderer, viewport, translation);
        let own = self.open.as_ref().map(|(anchor, close)| {
            overlay::Element::new(Box::new(Panel {
                panel,
                tree: &mut above[0],
                base: layout.bounds() + translation,
                anchor: *anchor,
                close: close.clone(),
            }))
        });
        (inner.is_some() || own.is_some())
            .then(|| overlay::Group::with_children(inner.into_iter().chain(own).collect()).overlay())
    }
}

struct Panel<'a, 'b, Message> {
    panel: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    base: Rectangle,
    anchor: Anchor,
    close: Message,
}

impl<Message: Clone> overlay::Overlay<Message, Theme, Renderer> for Panel<'_, '_, Message> {
    fn layout(&mut self, renderer: &Renderer, window: Size) -> layout::Node {
        let limits = layout::Limits::new(Size::ZERO, room(self.anchor, self.base, window));
        let node = self.panel.as_widget_mut().layout(self.tree, renderer, &limits);
        let at = place(self.anchor, self.base, node.size(), window);
        node.move_to(at)
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.panel.as_widget_mut().operate(self.tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();
        self.panel.as_widget_mut().update(self.tree, event, layout, cursor, renderer, clipboard, shell, &bounds);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(_)) => {
                if !cursor.is_over(bounds) {
                    shell.publish(self.close.clone());
                }
                shell.capture_event();
            }
            // What is underneath does not move meanwhile.
            Event::Mouse(mouse::Event::WheelScrolled { .. }) => shell.capture_event(),
            _ => {}
        }
    }

    fn mouse_interaction(&self, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &Renderer) -> mouse::Interaction {
        let bounds = layout.bounds();
        let inner = self.panel.as_widget().mouse_interaction(self.tree, layout, cursor, &bounds, renderer);
        // While it is open nothing under it answers to the pointer, hints neither.
        if inner == mouse::Interaction::None { mouse::Interaction::Idle } else { inner }
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.panel.as_widget().draw(self.tree, renderer, theme, style, layout, cursor, &layout.bounds());
    }
}

impl<'a, Message: Clone + 'a> From<Popup<'a, Message>> for Element<'a, Message> {
    fn from(p: Popup<'a, Message>) -> Self {
        Element::new(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Size = Size::new(520.0, 360.0);

    #[test]
    fn a_panel_hangs_from_its_button_and_stays_in_the_window() {
        let button = Rectangle::new(Point::new(470.0, 10.0), Size::new(32.0, 32.0));
        let panel = Size::new(250.0, 200.0);
        assert_eq!(place(Anchor::End, button, panel, WINDOW), Point::new(252.0, 46.0));
        // From its left edge it would leave the window.
        assert_eq!(place(Anchor::Start, button, panel, WINDOW), Point::new(262.0, 46.0));
        // A tall one gets what there is below the button.
        assert_eq!(room(Anchor::End, button, WINDOW), Size::new(504.0, 306.0));
        // At the pointer, near the corner: moved back inside.
        assert_eq!(place(Anchor::At(Point::new(510.0, 350.0)), button, panel, WINDOW), Point::new(262.0, 152.0));
        assert_eq!(place(Anchor::At(Point::new(40.0, 50.0)), button, panel, WINDOW), Point::new(40.0, 50.0));
    }
}
