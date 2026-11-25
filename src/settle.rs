//! Motion: what appears (a menu, a dialog, a toast) slides the last few pixels into place, and the
//! dimming behind a dialog fades in, over 120 ms. Nothing moves with "Reduce motion".

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget, overlay, renderer};
use iced::{Color, Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector, mouse, window};

const TIME: Duration = Duration::from_millis(120);
/// How far a thing slides.
const FAR: f32 = 8.0;

static REDUCED: AtomicBool = AtomicBool::new(false);

/// Whether to do without motion.
pub fn reduced() -> bool {
    REDUCED.load(Ordering::Relaxed)
}

pub fn set_reduced(on: bool) {
    REDUCED.store(on, Ordering::Relaxed);
}

/// How far along something that began at `start` is: 0 to 1, slowing down.
fn along(start: Option<Instant>, now: Instant) -> f32 {
    let t = start.map_or(0.0, |s| now.saturating_duration_since(s).as_secs_f32() / TIME.as_secs_f32()).min(1.0);
    1.0 - (1.0 - t).powi(3)
}

struct State {
    start: Option<Instant>,
    at: f32,
}

#[derive(Clone, Copy)]
enum How {
    /// Comes from above (or from below, when negative).
    Slide(f32),
    /// A colour filling the area behind, fading in.
    Fade(fn(&Theme) -> Color),
}

pub struct Settle<'a, Message> {
    content: Element<'a, Message>,
    how: How,
}

/// `content`, coming down into place.
pub fn down<'a, Message: 'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Settle { content: content.into(), how: How::Slide(-FAR) })
}

/// `content`, coming up into place.
pub fn up<'a, Message: 'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Settle { content: content.into(), how: How::Slide(FAR) })
}

/// `content` on `color`, which fades in.
pub fn fade<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    color: fn(&Theme) -> Color,
) -> Element<'a, Message> {
    Element::new(Settle { content: content.into(), how: How::Fade(color) })
}

impl<Message> Widget<Message, Theme, Renderer> for Settle<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State { start: None, at: if reduced() { 1.0 } else { 0.0 } })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
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
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<State>();
            if state.at < 1.0 {
                let start = *state.start.get_or_insert(*now);
                state.at = along(Some(start), *now);
                shell.request_redraw();
            }
        }
        self.content.as_widget_mut().update(
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
        self.content.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
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
        let at = tree.state.downcast_ref::<State>().at;
        let content = |renderer: &mut Renderer| {
            self.content.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
        };
        match self.how {
            How::Slide(far) if at < 1.0 => {
                renderer.with_translation(Vector::new(0.0, far * (1.0 - at)), content);
            }
            How::Slide(_) => content(renderer),
            How::Fade(color) => {
                let quad = renderer::Quad { bounds: layout.bounds(), ..renderer::Quad::default() };
                renderer.fill_quad(quad, color(theme).scale_alpha(at));
                content(renderer);
            }
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
        self.content.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_ends_after_its_time() {
        let start = Instant::now();
        assert_eq!(along(None, start), 0.0);
        assert_eq!(along(Some(start), start), 0.0);
        let half = along(Some(start), start + TIME / 2);
        assert!(half > 0.5 && half < 1.0, "quick at first: {half}");
        assert_eq!(along(Some(start), start + TIME), 1.0);
        assert_eq!(along(Some(start), start + TIME * 9), 1.0);
    }
}
