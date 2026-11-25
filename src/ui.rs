//! Controls that come styled: the views never build a bare input, drop-down, check box, slider or
//! scrolled area, so they all look like one program. (A PDF's form fields belong to the page
//! and keep the styles of their own.)

use iced::widget::{
    Checkbox, PickList, Scrollable, Slider, TextInput, button, checkbox, container, mouse_area, opaque, pick_list, row,
    scrollable, slider, space, stack, svg, text, text_input,
};
use iced::{Alignment, Element, Length, Padding};

use crate::frame::frame;
use crate::{icons, settle, style};

/// A box to type into.
pub fn input<'a, M: Clone + 'a>(placeholder: &str, value: &str) -> TextInput<'a, M> {
    text_input(placeholder, value).size(style::BODY).padding([7, 10]).style(style::input)
}

/// One without a frame of its own, for a panel that is the frame.
pub fn bare_input<'a, M: Clone + 'a>(placeholder: &str, value: &str) -> TextInput<'a, M> {
    text_input(placeholder, value).size(style::BODY).padding([7, 4]).style(style::bare_input)
}

/// A drop-down, as wide as there is room.
pub fn select<'a, T, M>(options: Vec<T>, selected: Option<T>, on_select: impl Fn(T) -> M + 'a) -> Element<'a, M>
where
    T: ToString + PartialEq + Clone + 'a,
    M: Clone + 'a,
{
    let list: PickList<'a, T, Vec<T>, T, M> = pick_list(options, selected, on_select)
        .width(Length::Fill)
        .text_size(style::BODY)
        .padding(Padding { right: 30.0, left: 10.0, top: 7.0, bottom: 7.0 })
        .handle(pick_list::Handle::None)
        .style(style::select)
        .menu_style(style::select_menu);
    let chevron = svg(icons::down()).width(14).height(14).style(style::icon);
    let chevron = container(chevron)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::Center)
        .padding(Padding { right: 10.0, ..Padding::ZERO });
    stack![list, chevron].into()
}

pub fn check<'a, M: 'a>(on: bool) -> Checkbox<'a, M> {
    checkbox(on).size(16).text_size(style::BODY).spacing(8).style(style::check)
}

pub fn fine_slider<'a, M: Clone + 'a>(
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    on_change: impl Fn(f32) -> M + 'a,
) -> Slider<'a, f32, M> {
    slider(range, value, on_change).style(style::slider)
}

/// A list that scrolls, with the thin bar beside its content rather than over it.
pub fn scroll<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let bar = scrollable::Scrollbar::new().width(6).scroller_width(6).spacing(6);
    scrollable(content).direction(scrollable::Direction::Vertical(bar)).style(style::scrollbar)
}

/// One whose bar is so thin that it may lie at the edge (the sidebar's lists).
pub fn thin_scroll<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let bar = scrollable::Scrollbar::new().width(4).scroller_width(4);
    scrollable(content).direction(scrollable::Direction::Vertical(bar)).style(style::scrollbar)
}

/// A row that scrolls sideways, without a bar.
pub fn side_scroll<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let bar = scrollable::Scrollbar::new().width(0).scroller_width(0);
    scrollable(content).direction(scrollable::Direction::Horizontal(bar))
}

/// How wide dialogs are: questions; forms and lists; those with two columns.
pub const ASK: f32 = 360.0;
pub const FORM: f32 = 480.0;
pub const WIDE: f32 = 760.0;

/// What a button of a dialog is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// What the dialog is for.
    Main,
    /// The same, throwing something away.
    Danger,
    Other,
}

pub struct Dialog<'a, M> {
    title: String,
    body: Element<'a, M>,
    width: f32,
    /// At the start of the row of buttons.
    aside: Option<Element<'a, M>>,
    buttons: Vec<(Kind, String, Option<M>)>,
    /// What a press beside the dialog does, if anything.
    cancel: Option<M>,
}

pub fn dialog<'a, M: Clone + 'a>(title: String, width: f32, body: impl Into<Element<'a, M>>) -> Dialog<'a, M> {
    Dialog { title, body: body.into(), width, aside: None, buttons: Vec::new(), cancel: None }
}

/// A button as dialogs have them.
pub fn action<'a, M: Clone + 'a>(kind: Kind, label: String, message: Option<M>) -> Element<'a, M> {
    let look = match kind {
        Kind::Main => style::primary,
        Kind::Danger => style::danger,
        Kind::Other => style::secondary,
    };
    button(text(label).size(style::BODY)).padding([8, 16]).style(look).on_press_maybe(message).into()
}

/// The order of a dialog's buttons, given with Cancel first and what it is for last: that way round
/// on Linux and macOS, the other on Windows.
fn ordered<T>(mut buttons: Vec<T>, confirm_first: bool) -> Vec<T> {
    if confirm_first {
        buttons.reverse();
    }
    buttons
}

impl<'a, M: Clone + 'a> Dialog<'a, M> {
    pub fn aside(mut self, aside: impl Into<Element<'a, M>>) -> Self {
        self.aside = Some(aside.into());
        self
    }

    /// The button that leaves things as they are, which a press beside the dialog does too.
    pub fn cancel(mut self, label: String, message: M) -> Self {
        self.cancel = Some(message.clone());
        self.buttons.push((Kind::Other, label, Some(message)));
        self
    }

    /// The same, but a press beside the dialog does nothing (it may have been a stroke gone astray).
    pub fn cancel_button(self, label: String, message: M) -> Self {
        self.button(Kind::Other, label, Some(message))
    }

    /// A button; give what the dialog is for last.
    pub fn button(mut self, kind: Kind, label: String, message: Option<M>) -> Self {
        self.buttons.push((kind, label, message));
        self
    }

    /// The dialog over a dimmed window, below the header, its buttons always in view.
    pub fn view(self, header: f32) -> Element<'a, M> {
        let buttons = ordered(self.buttons, crate::platform::caps().confirm_first);
        let buttons = row(buttons.into_iter().map(|(kind, label, message)| action(kind, label, message)))
            .spacing(style::GAP)
            .wrap()
            .align_x(Alignment::End);
        let aside = self.aside.unwrap_or_else(|| space().into());
        let foot = row![aside, container(buttons).width(Length::Fill).align_x(Alignment::End)]
            .spacing(style::GAP)
            .align_y(Alignment::Center);
        let title = text(self.title).size(style::HEADING).font(style::font(iced::font::Weight::Semibold));
        let card = container(frame(title.into(), scroll(self.body).width(Length::Fill).into(), foot.into(), 14.0))
            .max_width(self.width)
            .padding(20)
            .style(style::panel);
        let backdrop = mouse_area(space().width(Length::Fill).height(Length::Fill));
        let backdrop = match self.cancel {
            Some(message) => backdrop.on_press(message),
            None => backdrop,
        };
        let placed = container(settle::down(opaque(card))).center(Length::Fill).padding(Padding {
            top: header + 4.0,
            bottom: 8.0,
            left: 16.0,
            right: 16.0,
        });
        opaque(stack![settle::fade(backdrop, style::scrim_color), placed])
    }
}

/// A shortcut ("Ctrl+F, F3") as keys.
pub fn keys<'a, M: 'a>(shortcut: &str) -> Element<'a, M> {
    let caps = shortcut.split(", ").map(|key| {
        container(text(key.to_owned()).size(style::CAPTION).font(style::font(iced::font::Weight::Medium)))
            .padding([2, 6])
            .style(style::key_cap)
            .into()
    });
    row(caps).spacing(4).into()
}

/// A row of choices, the current one lit.
pub fn choices<'a, M: Clone + 'a>(items: Vec<(String, bool, M)>) -> Element<'a, M> {
    row(items.into_iter().map(|(name, on, message)| {
        button(text(name).size(style::SMALL)).padding([4, 10]).style(style::segment(on)).on_press(message).into()
    }))
    .spacing(2)
    .wrap()
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_button_is_where_the_system_has_it() {
        assert_eq!(ordered(vec!["Cancel", "Copy", "Save"], false), ["Cancel", "Copy", "Save"]);
        assert_eq!(ordered(vec!["Cancel", "Copy", "Save"], true), ["Save", "Copy", "Cancel"]);
    }
}
