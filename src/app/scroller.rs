//! The scrolled view of a picture or a PDF. Unlike iced's `scrollable`, which keeps its position to
//! itself and is told about changes one step late, this one is drawn where the app says: a zoom
//! and the position that goes with it are always seen together, so nothing jumps while zooming.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget, overlay};
use iced::{Border, Element, Event, Length, Point, Rectangle, Renderer, Shadow, Size, Theme, Vector, keyboard, mouse};

/// Pixels a notch of the wheel scrolls.
const LINE: f32 = 60.0;
/// Thickness of a scrollbar and of the strip it takes presses in, and its shortest thumb.
const BAR: f32 = 6.0;
const STRIP: f32 = 12.0;
const MIN_THUMB: f32 = 28.0;

#[derive(Default)]
struct State {
    modifiers: keyboard::Modifiers,
    /// A scrollbar being dragged: which, and where on its thumb.
    drag: Option<(Axis, f32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

/// `content` seen through a window at `offset`.
pub struct Scroller<'a, Message> {
    content: Element<'a, Message>,
    offset: Vector,
    on_scroll: fn(Vector, Size) -> Message,
}

pub fn scroller<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    offset: Vector,
    on_scroll: fn(Vector, Size) -> Message,
) -> Scroller<'a, Message> {
    Scroller { content: content.into(), offset, on_scroll }
}

/// `offset` kept so that the view does not leave the content.
pub fn clamped(offset: Vector, content: Size, view: Size) -> Vector {
    let axis = |v: f32, content: f32, view: f32| v.clamp(0.0, (content - view).max(0.0));
    Vector::new(axis(offset.x, content.width, view.width), axis(offset.y, content.height, view.height))
}

/// Where a scrollbar's thumb starts and how long it is, along a track as long as the view; none
/// when there is nothing to scroll.
fn thumb(offset: f32, content: f32, view: f32) -> Option<(f32, f32)> {
    if content <= view + 1.0 {
        return None;
    }
    let length = (view * view / content).max(MIN_THUMB).min(view);
    Some((offset / (content - view) * (view - length), length))
}

/// The offset that puts the thumb at `start`.
fn offset_for(start: f32, content: f32, view: f32) -> f32 {
    match thumb(0.0, content, view) {
        Some((_, length)) if view > length => (start / (view - length)).clamp(0.0, 1.0) * (content - view),
        _ => 0.0,
    }
}

impl<Message> Scroller<'_, Message> {
    fn sizes(&self, layout: Layout<'_>) -> (Rectangle, Size, Vector) {
        let bounds = layout.bounds();
        let content = layout.children().next().map_or(Size::ZERO, |c| c.bounds().size());
        (bounds, content, clamped(self.offset, content, bounds.size()))
    }

    /// The thumbs, in window coordinates.
    fn thumbs(bounds: Rectangle, content: Size, offset: Vector) -> [(Axis, Option<Rectangle>); 2] {
        let y = thumb(offset.y, content.height, bounds.height).map(|(start, length)| {
            Rectangle::new(Point::new(bounds.x + bounds.width - BAR - 2.0, bounds.y + start), Size::new(BAR, length))
        });
        let x = thumb(offset.x, content.width, bounds.width).map(|(start, length)| {
            Rectangle::new(Point::new(bounds.x + start, bounds.y + bounds.height - BAR - 2.0), Size::new(length, BAR))
        });
        [(Axis::Y, y), (Axis::X, x)]
    }

    /// The scrollbar whose strip is under `p`.
    fn bar_at(bounds: Rectangle, content: Size, p: Point) -> Option<Axis> {
        if !bounds.contains(p) {
            return None;
        }
        if content.height > bounds.height + 1.0 && p.x > bounds.x + bounds.width - STRIP {
            Some(Axis::Y)
        } else if content.width > bounds.width + 1.0 && p.y > bounds.y + bounds.height - STRIP {
            Some(Axis::X)
        } else {
            None
        }
    }

    /// The pointer as the content sees it: moved with it, and only hovering over it when it is in view.
    fn inside(bounds: Rectangle, content: Size, offset: Vector, cursor: mouse::Cursor) -> mouse::Cursor {
        match cursor.position_over(bounds) {
            Some(p) if Self::bar_at(bounds, content, p).is_none() => mouse::Cursor::Available(p + offset),
            _ => cursor.levitate() + offset,
        }
    }
}

impl<Message> Widget<Message, Theme, Renderer> for Scroller<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let size = limits.max();
        let free = layout::Limits::new(Size::ZERO, Size::INFINITE);
        let content = self.content.as_widget_mut().layout(&mut tree.children[0], renderer, &free);
        layout::Node::with_children(size, vec![content])
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        if let Some(content) = layout.children().next() {
            self.content.as_widget_mut().operate(&mut tree.children[0], content, renderer, operation);
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
        _viewport: &Rectangle,
    ) {
        let (bounds, content, offset) = self.sizes(layout);
        let tell = |shell: &mut Shell<'_, Message>, to: Vector| {
            let to = clamped(to, content, bounds.size());
            shell.publish((self.on_scroll)(to, bounds.size()));
        };
        let state = tree.state.downcast_mut::<State>();
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => state.modifiers = *m,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position()
                    && let Some(axis) = Self::bar_at(bounds, content, p)
                {
                    let (at, view, total, now) = match axis {
                        Axis::Y => (p.y - bounds.y, bounds.height, content.height, offset.y),
                        Axis::X => (p.x - bounds.x, bounds.width, content.width, offset.x),
                    };
                    let Some((start, length)) = thumb(now, total, view) else { return };
                    // A press beside the thumb brings its middle there.
                    let grab = if (start..start + length).contains(&at) { at - start } else { length / 2.0 };
                    state.drag = Some((axis, grab));
                    let to = offset_for(at - grab, total, view);
                    tell(shell, if axis == Axis::Y { Vector::new(offset.x, to) } else { Vector::new(to, offset.y) });
                    shell.capture_event();
                    return;
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some((axis, grab)) = state.drag {
                    let to = match axis {
                        Axis::Y => {
                            let y = offset_for(position.y - bounds.y - grab, content.height, bounds.height);
                            Vector::new(offset.x, y)
                        }
                        Axis::X => {
                            let x = offset_for(position.x - bounds.x - grab, content.width, bounds.width);
                            Vector::new(x, offset.y)
                        }
                    };
                    tell(shell, to);
                    shell.capture_event();
                    return;
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.drag.take().is_some() => {
                shell.capture_event();
                return;
            }
            _ => {}
        }

        let Some(inner) = layout.children().next() else { return };
        let seen = Rectangle { x: bounds.x + offset.x, y: bounds.y + offset.y, ..bounds };
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            inner,
            Self::inside(bounds, content, offset, cursor),
            renderer,
            clipboard,
            shell,
            &seen,
        );
        if shell.is_event_captured() {
            return;
        }

        if let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event
            && cursor.is_over(bounds)
        {
            let state = tree.state.downcast_ref::<State>();
            // With Ctrl it zooms, which is the app's business.
            if state.modifiers.control() {
                shell.capture_event();
                return;
            }
            let moved = match *delta {
                mouse::ScrollDelta::Lines { x, y } => {
                    // A Mac turns the axes itself when Shift is held.
                    if state.modifiers.shift() && !cfg!(target_os = "macos") {
                        Vector::new(y, x) * LINE
                    } else {
                        Vector::new(x, y) * LINE
                    }
                }
                mouse::ScrollDelta::Pixels { x, y } => Vector::new(x, y),
            };
            let to = clamped(offset - moved, content, bounds.size());
            if to != offset {
                tell(shell, to);
                shell.capture_event();
            }
        }
        // The content or the view shrank: the app should know where it really is.
        if matches!(event, Event::Window(iced::window::Event::RedrawRequested(_)))
            && ((offset.x - self.offset.x).abs() > 0.5 || (offset.y - self.offset.y).abs() > 0.5)
        {
            tell(shell, offset);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let (bounds, content, offset) = self.sizes(layout);
        if tree.state.downcast_ref::<State>().drag.is_some() {
            return mouse::Interaction::None;
        }
        let Some(inner) = layout.children().next() else { return mouse::Interaction::None };
        let seen = Rectangle { x: bounds.x + offset.x, y: bounds.y + offset.y, ..bounds };
        let cursor = Self::inside(bounds, content, offset, cursor);
        self.content.as_widget().mouse_interaction(&tree.children[0], inner, cursor, &seen, renderer)
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
        let (bounds, content, offset) = self.sizes(layout);
        let Some(clip) = bounds.intersection(viewport) else { return };
        let Some(inner) = layout.children().next() else { return };
        let seen = Rectangle { x: clip.x + offset.x, y: clip.y + offset.y, ..clip };
        let pointer = Self::inside(bounds, content, offset, cursor);
        renderer.with_layer(clip, |renderer| {
            renderer.with_translation(Vector::new(-offset.x, -offset.y), |renderer| {
                self.content.as_widget().draw(&tree.children[0], renderer, theme, style, inner, pointer, &seen);
            });
        });
        let dragged = tree.state.downcast_ref::<State>().drag.map(|(axis, _)| axis);
        let hovered = cursor.position().and_then(|p| Self::bar_at(bounds, content, p));
        renderer.with_layer(clip, |renderer| {
            for (axis, bar) in Self::thumbs(bounds, content, offset) {
                let Some(bar) = bar else { continue };
                let active = dragged == Some(axis) || hovered == Some(axis);
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: bar,
                        border: Border { radius: 3.0.into(), ..Border::default() },
                        shadow: Shadow::default(),
                        ..renderer::Quad::default()
                    },
                    crate::style::scroll_thumb(theme, active),
                );
            }
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let (_, _, offset) = self.sizes(layout);
        let inner = layout.children().next()?;
        let translation = translation - offset;
        self.content.as_widget_mut().overlay(&mut tree.children[0], inner, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a> From<Scroller<'a, Message>> for Element<'a, Message> {
    fn from(s: Scroller<'a, Message>) -> Self {
        Element::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_stays_on_the_content() {
        let (content, view) = (Size::new(1000.0, 300.0), Size::new(400.0, 400.0));
        assert_eq!(clamped(Vector::new(-5.0, 50.0), content, view), Vector::new(0.0, 0.0));
        assert_eq!(clamped(Vector::new(9999.0, 0.0), content, view), Vector::new(600.0, 0.0));
        assert_eq!(clamped(Vector::new(120.0, 0.0), content, view), Vector::new(120.0, 0.0));
    }

    #[test]
    fn thumbs_show_where_the_view_is_and_lead_back_to_it() {
        assert_eq!(thumb(0.0, 400.0, 400.0), None);
        let (start, length) = thumb(600.0, 1000.0, 400.0).unwrap();
        assert!((start + length - 400.0).abs() < 1e-3, "the end is at the end");
        for offset in [0.0, 150.0, 600.0] {
            let (start, _) = thumb(offset, 1000.0, 400.0).unwrap();
            assert!((offset_for(start, 1000.0, 400.0) - offset).abs() < 0.01);
        }
        // A very long document still has a thumb to hold.
        assert_eq!(thumb(0.0, 1e6, 400.0).unwrap().1, MIN_THUMB);
    }
}
