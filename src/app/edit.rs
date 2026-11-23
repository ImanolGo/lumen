//! Editing pictures: turning, mirroring, cropping and colour adjustments, with undo.
//!
//! The picture as opened is kept; what is shown is it with the edits applied, worked
//! out on a background thread whenever they change. Markup on the picture follows
//! every turn, mirror and crop. Nothing touches the file until a copy is saved.

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::widget::{button, column, container, row, slider, space, text};
use iced::{Alignment, Color, Element, Font, Length, Padding, Point, Rectangle, Renderer, Size, Task, Theme, Vector};

use super::markup::MarkupMsg;
use super::{App, Doc, Message, TOOLBAR_H, blocking, icon_button, muted, tip};
use crate::edit::crop::{self, Grab};
use crate::edit::{Adjustments, CropRect, Edits, Step};
use crate::raster::Picture;
use crate::{icons, style};

/// The edits of one picture, with their history.
#[derive(Default)]
pub struct PicEdit {
    pub edits: Edits,
    past: Vec<Step>,
    future: Vec<Step>,
    /// What the last saved copy had; nothing is unsaved while the edits equal it.
    pub saved: Edits,
    /// Bumped for every new rendering; older results are dropped.
    seq: u64,
    rendering: bool,
    /// The edits changed while a rendering was running: render again when it is done.
    again: bool,
    /// The adjustments when a slider drag began, for its undo step.
    adjust_from: Option<Edits>,
}

impl PicEdit {
    pub fn is_dirty(&self) -> bool {
        self.edits != self.saved
    }

    pub fn is_busy(&self) -> bool {
        self.rendering || self.again
    }
}

/// Choosing the part of the picture to keep.
pub struct CropState {
    pub rect: crop::Rect,
    pub aspect: usize,
    drag: Option<(Grab, crop::Rect, (f64, f64))>,
}

/// The new size being typed in.
pub struct ResizeForm {
    width: String,
    height: String,
    keep_ratio: bool,
}

/// Aspect ratios offered for cropping; `None` is free, 0.0 means the picture's own.
const ASPECTS: [(&str, Option<f64>); 6] = [
    ("Free", None),
    ("Original", Some(0.0)),
    ("Square", Some(1.0)),
    ("4:3", Some(4.0 / 3.0)),
    ("3:2", Some(1.5)),
    ("16:9", Some(16.0 / 9.0)),
];

#[derive(Debug, Clone)]
pub enum EditMsg {
    Toggle,
    RotateCw,
    RotateCcw,
    FlipH,
    FlipV,
    Undo,
    Redo,
    Reset,
    Rendered(u64, u64, Result<Picture, String>),
    CropStart,
    CropAspect(usize),
    CropPress(f64, f64),
    CropDrag(f64, f64),
    CropRelease,
    CropApply,
    CropCancel,
    AdjustOpen,
    ResizeOpen,
    ResizeWidth(String),
    ResizeHeight(String),
    ResizeKeep(bool),
    ResizePercent(u32),
    ResizeApply,
    Adjust(Adjustments),
    AdjustDone,
}

impl App {
    fn base_size(&self) -> Option<(u32, u32)> {
        match &self.doc {
            Doc::Image(view) => Some((view.original.width, view.original.height)),
            _ => None,
        }
    }

    fn aspect_value(&self, index: usize) -> Option<f64> {
        let Doc::Image(view) = &self.doc else { return None };
        ASPECTS[index].1.map(|a| if a == 0.0 { view.shown.width as f64 / view.shown.height.max(1) as f64 } else { a })
    }

    /// Records `step` and shows its result; markup follows it.
    fn apply_step(&mut self, step: Step) -> Task<Message> {
        if step.before == step.after {
            return Task::none();
        }
        if let Some(marks) = &mut self.marks {
            step.forward.iter().for_each(|c| marks.apply(*c));
        }
        let Doc::Image(view) = &mut self.doc else { return Task::none() };
        view.edit.edits = step.after;
        view.edit.past.push(step);
        view.edit.future.clear();
        self.render_edits()
    }

    /// Works out the shown picture from the original and the edits, off the UI thread.
    pub(super) fn render_edits(&mut self) -> Task<Message> {
        let generation = self.generation;
        let Doc::Image(view) = &mut self.doc else { return Task::none() };
        let edit = &mut view.edit;
        if edit.rendering {
            edit.again = true;
            return Task::none();
        }
        edit.rendering = true;
        edit.seq += 1;
        let seq = edit.seq;
        let edits = edit.edits;
        let original = view.original.clone();
        Task::perform(
            blocking(move || {
                let base = image::RgbaImage::from_raw(original.width, original.height, Vec::from(original.rgba))
                    .ok_or_else(|| "the picture is damaged".to_string())?;
                let out = crate::edit::apply(&base, &edits);
                let (width, height) = out.dimensions();
                Ok(Picture { width, height, original: (width, height), rgba: out.into_raw().into() })
            }),
            move |r| Message::Edit(EditMsg::Rendered(generation, seq, r.and_then(|r| r))),
        )
    }

    pub(super) fn edit_update(&mut self, msg: EditMsg) -> Task<Message> {
        let Some(size) = self.base_size() else { return Task::none() };
        let edits = match &self.doc {
            Doc::Image(view) => view.edit.edits,
            _ => return Task::none(),
        };
        match msg {
            EditMsg::Toggle => {
                self.edit_open = !self.edit_open;
                self.crop = None;
                self.adjust_open = false;
                if self.edit_open && self.markup_open {
                    return Task::batch([self.markup_update(MarkupMsg::Toggle)]);
                }
                Task::none()
            }
            EditMsg::RotateCw => self.apply_step(edits.rotate_cw(size)),
            EditMsg::RotateCcw => self.apply_step(edits.rotate_ccw(size)),
            EditMsg::FlipH => self.apply_step(edits.flip_h(size)),
            EditMsg::FlipV => self.apply_step(edits.flip_v(size)),
            EditMsg::Reset => {
                self.crop = None;
                self.apply_step(edits.reset(size))
            }
            EditMsg::Undo | EditMsg::Redo => {
                self.crop = None;
                let undo = matches!(msg, EditMsg::Undo);
                let Doc::Image(view) = &mut self.doc else { return Task::none() };
                let edit = &mut view.edit;
                let Some(step) = (if undo { edit.past.pop() } else { edit.future.pop() }) else { return Task::none() };
                edit.edits = if undo { step.before } else { step.after };
                if let Some(marks) = &mut self.marks {
                    let changes = if undo { &step.back } else { &step.forward };
                    changes.iter().for_each(|c| marks.apply(*c));
                }
                if undo {
                    edit.future.push(step)
                } else {
                    edit.past.push(step)
                }
                self.render_edits()
            }
            EditMsg::Rendered(generation, seq, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                let Doc::Image(view) = &mut self.doc else { return Task::none() };
                view.edit.rendering = false;
                if seq == view.edit.seq {
                    match result {
                        Ok(picture) => view.shown = picture.into(),
                        Err(e) => return self.toast(format!("Could not edit the picture: {e}")),
                    }
                }
                if std::mem::take(&mut view.edit.again) { self.render_edits() } else { Task::none() }
            }
            EditMsg::CropStart => {
                let Doc::Image(view) = &self.doc else { return Task::none() };
                self.adjust_open = false;
                self.resize_form = None;
                let bounds = (view.shown.width as f64, view.shown.height as f64);
                self.crop = Some(CropState { rect: crop::initial(bounds, None, 0.9), aspect: 0, drag: None });
                Task::none()
            }
            EditMsg::CropAspect(i) => {
                let aspect = self.aspect_value(i);
                let Doc::Image(view) = &self.doc else { return Task::none() };
                let bounds = (view.shown.width as f64, view.shown.height as f64);
                if let Some(state) = &mut self.crop {
                    state.aspect = i;
                    state.rect = crop::initial(bounds, aspect, 0.9);
                }
                Task::none()
            }
            EditMsg::CropPress(x, y) => {
                let zoom = self.markup_zoom_for_image();
                if let Some(state) = &mut self.crop {
                    let reach = 10.0 / zoom as f64;
                    state.drag = crop::grab_at(state.rect, x, y, reach).map(|g| (g, state.rect, (x, y)));
                }
                Task::none()
            }
            EditMsg::CropDrag(x, y) => {
                let Doc::Image(view) = &self.doc else { return Task::none() };
                let bounds = (view.shown.width as f64, view.shown.height as f64);
                let aspect = self.crop.as_ref().and_then(|s| self.aspect_value(s.aspect));
                if let Some(state) = &mut self.crop
                    && let Some((grab, start, (sx, sy))) = state.drag
                {
                    state.rect = crop::drag(start, grab, x - sx, y - sy, bounds, aspect);
                }
                Task::none()
            }
            EditMsg::CropRelease => {
                if let Some(state) = &mut self.crop {
                    state.drag = None;
                }
                Task::none()
            }
            EditMsg::CropCancel => {
                self.crop = None;
                Task::none()
            }
            EditMsg::CropApply => {
                let Some(state) = self.crop.take() else { return Task::none() };
                let r = state.rect;
                let (x, y) = (r.x.round().max(0.0) as u32, r.y.round().max(0.0) as u32);
                let rect = CropRect { x, y, w: (r.w.round() as u32).max(1), h: (r.h.round() as u32).max(1) };
                let (w, h) = edits.size(size);
                if rect.x == 0 && rect.y == 0 && rect.w >= w && rect.h >= h {
                    return Task::none();
                }
                let rect = CropRect { w: rect.w.min(w - x.min(w - 1)), h: rect.h.min(h - y.min(h - 1)), ..rect };
                self.apply_step(edits.crop(size, rect))
            }
            EditMsg::AdjustOpen => {
                self.adjust_open = !self.adjust_open;
                self.resize_form = None;
                self.crop = None;
                Task::none()
            }
            EditMsg::ResizeOpen => {
                self.adjust_open = false;
                self.crop = None;
                self.resize_form = match self.resize_form {
                    Some(_) => None,
                    None => {
                        let (w, h) = edits.size(size);
                        Some(ResizeForm { width: w.to_string(), height: h.to_string(), keep_ratio: true })
                    }
                };
                Task::none()
            }
            EditMsg::ResizeWidth(_) | EditMsg::ResizeHeight(_) => {
                let (width, text) = match msg {
                    EditMsg::ResizeWidth(t) => (true, t),
                    EditMsg::ResizeHeight(t) => (false, t),
                    _ => unreachable!(),
                };
                let (sw, sh) = edits.size(size);
                let Some(form) = &mut self.resize_form else { return Task::none() };
                let digits: String = text.chars().filter(char::is_ascii_digit).take(5).collect();
                let value: Option<u32> = digits.parse().ok();
                if width {
                    form.width = digits;
                    if form.keep_ratio
                        && let Some(w) = value
                    {
                        form.height = ((w as f64 * sh as f64 / sw as f64).round().max(1.0) as u32).to_string();
                    }
                } else {
                    form.height = digits;
                    if form.keep_ratio
                        && let Some(h) = value
                    {
                        form.width = ((h as f64 * sw as f64 / sh as f64).round().max(1.0) as u32).to_string();
                    }
                }
                Task::none()
            }
            EditMsg::ResizeKeep(keep) => {
                let (sw, sh) = edits.size(size);
                if let Some(form) = &mut self.resize_form {
                    form.keep_ratio = keep;
                    if keep && let Ok(w) = form.width.parse::<u32>() {
                        form.height = ((w as f64 * sh as f64 / sw as f64).round().max(1.0) as u32).to_string();
                    }
                }
                Task::none()
            }
            EditMsg::ResizePercent(percent) => {
                // Percentages are of the full-size picture, so 100 % undoes a resize.
                let (uw, uh) = edits.unscaled(size);
                if let Some(form) = &mut self.resize_form {
                    form.width = ((uw as u64 * percent as u64 / 100).max(1)).to_string();
                    form.height = ((uh as u64 * percent as u64 / 100).max(1)).to_string();
                }
                Task::none()
            }
            EditMsg::ResizeApply => {
                let Some(form) = &self.resize_form else { return Task::none() };
                let (Ok(w), Ok(h)) = (form.width.parse::<u32>(), form.height.parse::<u32>()) else {
                    return self.toast("Type a width and a height in pixels");
                };
                if w == 0 || h == 0 || w > crate::edit::MAX_SIDE || h > crate::edit::MAX_SIDE {
                    return self.toast(format!("Sizes go from 1 to {} pixels", crate::edit::MAX_SIDE));
                }
                self.resize_form = None;
                if (w, h) == edits.size(size) {
                    return Task::none();
                }
                self.apply_step(edits.resized(size, w, h))
            }
            EditMsg::Adjust(adjust) => {
                let Doc::Image(view) = &mut self.doc else { return Task::none() };
                view.edit.adjust_from.get_or_insert(view.edit.edits);
                view.edit.edits.adjust = adjust;
                self.render_edits()
            }
            EditMsg::AdjustDone => {
                let Doc::Image(view) = &mut self.doc else { return Task::none() };
                let Some(from) = view.edit.adjust_from.take() else { return Task::none() };
                let step = from.adjusted(size, view.edit.edits.adjust);
                if step.before != step.after {
                    view.edit.past.push(step);
                    view.edit.future.clear();
                }
                Task::none()
            }
        }
    }

    fn markup_zoom_for_image(&self) -> f32 {
        match &self.doc {
            Doc::Image(v) => v.zoom.unwrap_or_else(|| self.image_fit_zoom(v)),
            _ => 1.0,
        }
    }

    /// Keys while editing a picture; `None` for the rest.
    pub(super) fn edit_key(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<Task<Message>> {
        use iced::keyboard::{Key, key::Named};
        if !matches!(self.doc, Doc::Image(_)) {
            return None;
        }
        let ctrl = modifiers.command();
        let msg = match key.as_ref() {
            Key::Character(c) => match (c.to_lowercase().as_str(), ctrl, modifiers.shift()) {
                ("e", false, false) => EditMsg::Toggle,
                ("z", true, false) if self.edit_open => EditMsg::Undo,
                ("z", true, true) | ("y", true, false) if self.edit_open => EditMsg::Redo,
                _ => return None,
            },
            Key::Named(Named::Enter) if self.crop.is_some() => EditMsg::CropApply,
            _ => return None,
        };
        Some(self.edit_update(msg))
    }

    /// Escape while editing: drop the crop, then close the panel.
    pub(super) fn edit_escape(&mut self) -> bool {
        self.crop.take().is_some() || self.resize_form.take().is_some() || std::mem::take(&mut self.adjust_open)
    }

    // ------------------------------------------------------------ view

    /// The crop layer over the picture, drawn at `zoom`.
    pub(super) fn crop_layer(&self, zoom: f32, w: f32, h: f32) -> Option<Element<'_, Message>> {
        let state = self.crop.as_ref()?;
        Some(iced::widget::canvas(CropOverlay { rect: state.rect, zoom }).width(w).height(h).into())
    }

    pub(super) fn edit_bar(&self) -> Element<'_, Message> {
        let Doc::Image(view) = &self.doc else { return space().into() };
        let e = |m: EditMsg| Some(Message::Edit(m));
        let content: Element<'_, Message> = if let Some(state) = &self.crop {
            let mut chips =
                row![text("Crop").size(13).font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT })]
                    .spacing(4)
                    .align_y(Alignment::Center);
            for (i, (label, _)) in ASPECTS.iter().enumerate() {
                chips = chips.push(
                    button(text(*label).size(13))
                        .padding([5, 10])
                        .style(style::segment(state.aspect == i))
                        .on_press(Message::Edit(EditMsg::CropAspect(i))),
                );
            }
            let px = format!("{} × {}", state.rect.w.round(), state.rect.h.round());
            row![
                chips,
                space().width(Length::Fill),
                text(px).size(12).style(muted),
                button(text("Cancel").size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press(Message::Edit(EditMsg::CropCancel)),
                button(text("Crop").size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press(Message::Edit(EditMsg::CropApply)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            let edit = &view.edit;
            let busy = edit.is_busy();
            let can_save = !busy && (edit.is_dirty() || self.marks.as_ref().is_some_and(|m| m.has_items()));
            let save = button(text("Save a Copy…").size(13))
                .padding([6, 14])
                .style(style::primary)
                .on_press_maybe(can_save.then_some(Message::Markup(MarkupMsg::SaveAs)));
            let status = if busy {
                "Working…"
            } else if edit.is_dirty() {
                "Unsaved"
            } else {
                ""
            };
            row![
                icon_button(icons::rotate_left(), "Rotate left (Shift+R)", e(EditMsg::RotateCcw), false),
                icon_button(icons::rotate_right(), "Rotate right (R)", e(EditMsg::RotateCw), false),
                icon_button(icons::flip_h(), "Mirror left to right", e(EditMsg::FlipH), false),
                icon_button(icons::flip_v(), "Mirror top to bottom", e(EditMsg::FlipV), false),
                super::separator(),
                icon_button(icons::crop(), "Crop", e(EditMsg::CropStart), false),
                icon_button(icons::resize(), "Resize", e(EditMsg::ResizeOpen), self.resize_form.is_some()),
                icon_button(icons::adjust(), "Light and colour", e(EditMsg::AdjustOpen), self.adjust_open),
                super::separator(),
                icon_button(
                    icons::undo(),
                    "Undo (Ctrl+Z)",
                    (!edit.past.is_empty()).then_some(Message::Edit(EditMsg::Undo)),
                    false
                ),
                icon_button(
                    icons::redo(),
                    "Redo (Ctrl+Shift+Z)",
                    (!edit.future.is_empty()).then_some(Message::Edit(EditMsg::Redo)),
                    false
                ),
                icon_button(
                    icons::reset(),
                    "Back to the original",
                    (!edit.edits.is_none()).then_some(Message::Edit(EditMsg::Reset)),
                    false
                ),
                space().width(Length::Fill),
                text(status).size(12).style(muted),
                tip(save.into(), "Save the edited picture as a new file (Ctrl+S)"),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into()
        };
        container(container(content).padding([0, 8])).height(46).align_y(Alignment::Center).style(style::bar).into()
    }

    pub(super) fn adjust_panel(&self) -> Element<'_, Message> {
        let Doc::Image(view) = &self.doc else { return space().into() };
        let a = view.edit.edits.adjust;
        let line = |label: &'static str,
                    value: f32,
                    range: std::ops::RangeInclusive<f32>,
                    set: fn(Adjustments, f32) -> Adjustments| {
            column![
                row![
                    text(label).size(13),
                    space().width(Length::Fill),
                    text(format!("{value:+.2}")).size(12).style(muted)
                ],
                slider(range, value, move |v| Message::Edit(EditMsg::Adjust(set(a, v))))
                    .step(0.01_f32)
                    .on_release(Message::Edit(EditMsg::AdjustDone)),
            ]
            .spacing(4)
        };
        let panel = container(
            column![
                text("Light and colour").size(14).font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT }),
                line("Exposure", a.exposure, -2.0..=2.0, |a, v| Adjustments { exposure: v, ..a }),
                line("Contrast", a.contrast, -1.0..=1.0, |a, v| Adjustments { contrast: v, ..a }),
                line("Saturation", a.saturation, -1.0..=1.0, |a, v| Adjustments { saturation: v, ..a }),
                button(text("Reset").size(13)).padding([5, 12]).style(style::secondary).on_press_maybe(
                    (!a.is_neutral()).then(|| { Message::Edit(EditMsg::Adjust(Adjustments::default())) })
                ),
            ]
            .spacing(12)
            .width(240),
        )
        .padding(14)
        .style(style::panel);
        container(panel)
            .width(Length::Fill)
            .align_x(Alignment::End)
            .padding(Padding { top: TOOLBAR_H + 54.0, right: 16.0, ..Padding::ZERO })
            .into()
    }
}

impl App {
    pub(super) fn resize_panel(&self) -> Element<'_, Message> {
        let (Some(form), Doc::Image(view)) = (&self.resize_form, &self.doc) else { return space().into() };
        let (uw, uh) = view.edit.edits.unscaled((view.original.width, view.original.height));
        let field = |value: &str, on: fn(String) -> EditMsg| {
            iced::widget::text_input("", value)
                .on_input(move |t| Message::Edit(on(t)))
                .on_submit(Message::Edit(EditMsg::ResizeApply))
                .size(14)
                .padding([6, 8])
                .width(80)
        };
        let mut presets = row![].spacing(4);
        for percent in [25, 50, 75, 100] {
            presets = presets.push(
                button(text(format!("{percent} %")).size(12))
                    .padding([4, 8])
                    .style(style::secondary)
                    .on_press(Message::Edit(EditMsg::ResizePercent(percent))),
            );
        }
        let ok = form.width.parse::<u32>().is_ok_and(|w| w > 0) && form.height.parse::<u32>().is_ok_and(|h| h > 0);
        let panel = container(
            column![
                text("Resize").size(14).font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT }),
                row![
                    field(&form.width, EditMsg::ResizeWidth),
                    text("×").size(14),
                    field(&form.height, EditMsg::ResizeHeight),
                    text("px").size(13).style(muted)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                iced::widget::checkbox(form.keep_ratio)
                    .label("Keep proportions")
                    .on_toggle(|k| Message::Edit(EditMsg::ResizeKeep(k)))
                    .size(16)
                    .text_size(13),
                presets,
                text(format!("Full size: {uw} × {uh}")).size(12).style(muted),
                row![
                    space().width(Length::Fill),
                    button(text("Resize").size(13))
                        .padding([6, 14])
                        .style(style::primary)
                        .on_press_maybe(ok.then_some(Message::Edit(EditMsg::ResizeApply))),
                ],
            ]
            .spacing(10)
            .width(250),
        )
        .padding(14)
        .style(style::panel);
        container(panel)
            .width(Length::Fill)
            .align_x(Alignment::End)
            .padding(Padding { top: TOOLBAR_H + 54.0, right: 16.0, ..Padding::ZERO })
            .into()
    }
}

/// The crop rectangle over the picture: the rest dimmed, rule-of-thirds lines, handles.
struct CropOverlay {
    rect: crop::Rect,
    zoom: f32,
}

#[derive(Default)]
struct CropDrag {
    down: bool,
}

impl CropOverlay {
    fn to_doc(&self, p: Point) -> (f64, f64) {
        ((p.x / self.zoom) as f64, (p.y / self.zoom) as f64)
    }
}

impl canvas::Program<Message> for CropOverlay {
    type State = CropDrag;

    fn update(
        &self,
        state: &mut CropDrag,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<Message>> {
        let canvas::Event::Mouse(event) = event else { return None };
        match event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let at = cursor.position_in(bounds)?;
                let (x, y) = self.to_doc(at);
                crop::grab_at(self.rect, x, y, (10.0 / self.zoom) as f64)?;
                state.down = true;
                Some(canvas::Action::publish(Message::Edit(EditMsg::CropPress(x, y))).and_capture())
            }
            mouse::Event::CursorMoved { position } if state.down => {
                let (x, y) = self.to_doc(*position - Vector::new(bounds.x, bounds.y));
                Some(canvas::Action::publish(Message::Edit(EditMsg::CropDrag(x, y))))
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) if state.down => {
                state.down = false;
                Some(canvas::Action::publish(Message::Edit(EditMsg::CropRelease)))
            }
            _ => None,
        }
    }

    fn draw(&self, _: &CropDrag, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let z = self.zoom;
        let (x, y, w, h) =
            (self.rect.x as f32 * z, self.rect.y as f32 * z, self.rect.w as f32 * z, self.rect.h as f32 * z);
        let shade = Color::from_rgba(0.0, 0.0, 0.0, 0.55);
        let (bw, bh) = (bounds.width, bounds.height);
        frame.fill_rectangle(Point::ORIGIN, Size::new(bw, y), shade);
        frame.fill_rectangle(Point::new(0.0, y + h), Size::new(bw, (bh - y - h).max(0.0)), shade);
        frame.fill_rectangle(Point::new(0.0, y), Size::new(x, h), shade);
        frame.fill_rectangle(Point::new(x + w, y), Size::new((bw - x - w).max(0.0), h), shade);
        let thin = Stroke::default().with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.45)).with_width(1.0);
        for k in [1.0, 2.0] {
            frame.stroke(&Path::line(Point::new(x + w * k / 3.0, y), Point::new(x + w * k / 3.0, y + h)), thin);
            frame.stroke(&Path::line(Point::new(x, y + h * k / 3.0), Point::new(x + w, y + h * k / 3.0)), thin);
        }
        frame.stroke(
            &Path::rectangle(Point::new(x, y), Size::new(w, h)),
            Stroke::default().with_color(Color::WHITE).with_width(1.5),
        );
        let corner = 16.0_f32.min(w / 3.0).min(h / 3.0);
        let thick = Stroke::default().with_color(Color::WHITE).with_width(4.0);
        for (cx, cy, sx, sy) in
            [(x, y, 1.0, 1.0), (x + w, y, -1.0, 1.0), (x, y + h, 1.0, -1.0), (x + w, y + h, -1.0, -1.0)]
        {
            frame.stroke(
                &Path::new(|b| {
                    b.move_to(Point::new(cx + sx * corner, cy));
                    b.line_to(Point::new(cx, cy));
                    b.line_to(Point::new(cx, cy + sy * corner));
                }),
                thick,
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &CropDrag, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        let Some(at) = cursor.position_in(bounds) else { return mouse::Interaction::None };
        let (x, y) = self.to_doc(at);
        match crop::grab_at(self.rect, x, y, (10.0 / self.zoom) as f64) {
            Some(Grab::Move) if state.down => mouse::Interaction::Grabbing,
            Some(Grab::Move) => mouse::Interaction::Grab,
            Some(Grab::Edges { left, right, top, bottom }) => match (left || right, top || bottom) {
                (true, false) => mouse::Interaction::ResizingHorizontally,
                (false, true) => mouse::Interaction::ResizingVertically,
                _ if (left && top) || (right && bottom) => mouse::Interaction::ResizingDiagonallyDown,
                _ => mouse::Interaction::ResizingDiagonallyUp,
            },
            None => mouse::Interaction::Crosshair,
        }
    }
}
