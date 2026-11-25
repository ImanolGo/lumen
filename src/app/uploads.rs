//! Puts pictures on the GPU of the window they are for. iced's own task does that for the first
//! window of the program only; a widget is given the renderer of the window it is in.

use std::cell::RefCell;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::Tree;
use iced::advanced::{Renderer as _, Widget, renderer};
use iced::widget::image;
use iced::{Element, Length, Rectangle, Renderer, Size, Theme, mouse};
use iced_runtime::image::{Allocation, Error};
use tokio::sync::oneshot;

type Answer = oneshot::Sender<Result<Allocation, Error>>;

/// What is waiting for the window to be drawn.
#[derive(Default)]
pub struct Uploads(RefCell<Vec<(image::Handle, Answer)>>);

impl Uploads {
    /// Asks for `handle` to be uploaded; answered once it is, or dropped with the window.
    pub fn ask(&self, handle: image::Handle) -> oneshot::Receiver<Result<Allocation, Error>> {
        let (tx, rx) = oneshot::channel();
        self.0.borrow_mut().push((handle, tx));
        rx
    }

    /// The widget that does it, a single transparent pixel: what has no size is not drawn.
    pub fn widget<'a, Message: 'a>(&'a self) -> Element<'a, Message> {
        Element::new(Uploader(self))
    }
}

struct Uploader<'a>(&'a Uploads);

impl<Message> Widget<Message, Theme, Renderer> for Uploader<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(1.0), Length::Fixed(1.0))
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, _limits: &layout::Limits) -> layout::Node {
        layout::Node::new(Size::new(1.0, 1.0))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        for (handle, answer) in self.0.0.take() {
            renderer.allocate_image(&handle, move |done| {
                let _ = answer.send(done);
            });
        }
    }
}
