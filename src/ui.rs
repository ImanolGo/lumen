//! Controls that come styled: the views never build a bare input, drop-down, check box, slider or
//! scrolled area, so they all look like one program. (A PDF's form fields belong to the page
//! and keep the styles of their own.)

use iced::widget::{
    Checkbox, PickList, Scrollable, Slider, TextInput, checkbox, container, pick_list, scrollable, slider, stack, svg,
    text_input,
};
use iced::{Alignment, Element, Length, Padding};

use crate::{icons, style};

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
