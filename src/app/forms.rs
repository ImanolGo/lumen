//! Filling in PDF forms: an input over each field, and the values until they are saved.

use std::collections::HashMap;
use std::sync::Arc;

use iced::widget::{button, center, container, pick_list, text, text_editor, text_input};
use iced::{Color, Element, Length, Task};

use super::signature::SignMsg;
use super::{App, Doc, Message, placed};
use crate::forms::{Form, Kind, Value};
use crate::style;

/// The form of the PDF on screen, and what has been filled in.
pub struct FormState {
    pub form: Arc<Form>,
    pub values: Vec<Value>,
    /// Multi-line text fields, by field.
    editors: HashMap<usize, text_editor::Content>,
    /// Per page, its widgets as (field, widget) indices.
    by_page: Vec<Vec<(usize, usize)>>,
}

impl FormState {
    pub fn new(form: Arc<Form>, values: Vec<Value>, pages: usize) -> Self {
        let mut by_page = vec![Vec::new(); pages];
        for (f, field) in form.fields.iter().enumerate() {
            for (w, widget) in field.widgets.iter().enumerate() {
                if let Some(list) = by_page.get_mut(widget.page) {
                    list.push((f, w));
                }
            }
        }
        let editors = form
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| matches!(f.kind, Kind::Text { multiline: true, .. }))
            .map(|(i, _)| {
                let t = match &values[i] {
                    Value::Text(t) => t.as_str(),
                    _ => "",
                };
                (i, text_editor::Content::with_text(t))
            })
            .collect();
        Self { form, values, editors, by_page }
    }

    /// Whether anything differs from the file.
    pub fn is_dirty(&self) -> bool {
        self.form.fields.iter().zip(&self.values).any(|(f, v)| f.value != *v)
    }
}

#[derive(Debug, Clone)]
pub enum FormMsg {
    Text(usize, String),
    Edit(usize, text_editor::Action),
    Toggle(usize),
    Radio(usize, String),
    Choose(usize, String),
}

fn ink(c: [f32; 3]) -> Color {
    Color::from_rgb(c[0], c[1], c[2])
}

impl App {
    /// Sets up the form of a PDF that was just opened (or reopened after saving).
    pub(super) fn attach_form(&mut self, path: &std::path::Path, form: Arc<Form>, pages: usize) {
        if form.fields.is_empty() {
            self.form = None;
            return;
        }
        // Reopened (markup turned on, or saved): keep what is filled in.
        let values = match self.form.take() {
            Some(old) if old.values.len() == form.fields.len() && self.form_path.as_deref() == Some(path) => old.values,
            _ => {
                self.stash_forms.remove(path).filter(|v| v.len() == form.fields.len()).unwrap_or_else(|| form.values())
            }
        };
        self.form_path = Some(path.to_owned());
        self.form = Some(FormState::new(form, values, pages));
    }

    /// Puts aside what was filled into the form being left, unless nothing was.
    pub(super) fn detach_form(&mut self) {
        let (Some(state), Some(path)) = (self.form.take(), self.form_path.take()) else { return };
        if state.is_dirty() {
            self.stash_forms.insert(path, state.values);
        }
    }

    pub(super) fn form_update(&mut self, msg: FormMsg) -> Task<Message> {
        let Some(state) = &mut self.form else { return Task::none() };
        let fields = &state.form.fields;
        match msg {
            FormMsg::Text(i, mut t) => {
                if let Some(Kind::Text { max_len: Some(n), .. }) = fields.get(i).map(|f| &f.kind) {
                    t = t.chars().take(*n).collect();
                }
                state.values[i] = Value::Text(t);
            }
            FormMsg::Edit(i, action) => {
                if let Some(content) = state.editors.get_mut(&i) {
                    content.perform(action);
                    let mut t = content.text();
                    if t.ends_with('\n') {
                        t.pop();
                    }
                    state.values[i] = Value::Text(t);
                }
            }
            FormMsg::Toggle(i) => {
                let on = fields[i].widgets.first().and_then(|w| w.on.clone()).unwrap_or_else(|| "Yes".into());
                state.values[i] = match &state.values[i] {
                    Value::State(Some(_)) => Value::State(None),
                    _ => Value::State(Some(on)),
                };
            }
            FormMsg::Radio(i, on) => state.values[i] = Value::State(Some(on)),
            FormMsg::Choose(i, display) => {
                if let Kind::Choice { options } = &fields[i].kind
                    && let Some(o) = options.iter().find(|o| o.display == display)
                {
                    state.values[i] = Value::Text(o.export.clone());
                }
            }
        }
        Task::none()
    }

    /// Whether ink (a signature) has been put inside `rect` on `page`.
    fn is_signed(&self, page: usize, rect: &hayro::kurbo::Rect) -> bool {
        let Some(layer) = self.marks.as_ref().and_then(|m| m.store.layer(page)) else { return false };
        layer.items.iter().any(|a| {
            let c = a.bounds().center();
            matches!(a.shape, crate::annotate::Shape::Ink(_)) && rect.contains(hayro::kurbo::Point::new(c.x, c.y))
        })
    }

    /// The inputs over the fields of `page`, drawn at `zoom`.
    pub(super) fn form_layers(&self, page: usize, zoom: f32) -> Vec<Element<'_, Message>> {
        let (Some(state), Doc::Pdf(_)) = (&self.form, &self.doc) else { return Vec::new() };
        let Some(widgets) = state.by_page.get(page) else { return Vec::new() };
        let mut out = Vec::new();
        for &(f, w) in widgets {
            let field = &state.form.fields[f];
            let widget = &field.widgets[w];
            let r = widget.rect;
            let (width, height) = (r.width() as f32 * zoom, r.height() as f32 * zoom);
            let size = (field.text_size(r.height().min(r.width())) as f32 * zoom).max(6.0);
            let color = ink(field.color());
            let element: Element<'_, Message> = match (&field.kind, &state.values[f]) {
                _ if field.read_only => continue,
                (Kind::Text { multiline: true, .. }, _) => match state.editors.get(&f) {
                    Some(content) => text_editor(content)
                        .on_action(move |a| Message::Form(FormMsg::Edit(f, a)))
                        .size(size)
                        .padding(2.0 * zoom)
                        .height(height)
                        .style(style::form_text_area(color))
                        .into(),
                    None => continue,
                },
                (Kind::Text { password, .. }, value) => {
                    let value = match value {
                        Value::Text(t) => t.as_str(),
                        _ => "",
                    };
                    let input = text_input("", value)
                        .on_input(move |t| Message::Form(FormMsg::Text(f, t)))
                        .secure(*password)
                        .size(size)
                        .padding([0.0, 2.0 * zoom])
                        .style(style::form_input(color));
                    container(input).height(height).center_y(height).into()
                }
                (Kind::Check | Kind::Radio, value) => {
                    let on = widget.on.clone().unwrap_or_else(|| "Yes".into());
                    let checked = matches!(value, Value::State(Some(s)) if *s == on);
                    let radio = field.kind == Kind::Radio;
                    let mark = match (checked, radio) {
                        (false, _) => "",
                        (true, false) => "✓",
                        (true, true) => "●",
                    };
                    let glyph = text(mark).size(height.min(width) * if radio { 0.6 } else { 0.85 }).color(color);
                    let msg = if radio { FormMsg::Radio(f, on) } else { FormMsg::Toggle(f) };
                    button(center(glyph)).padding(0).style(style::form_box).on_press(Message::Form(msg)).into()
                }
                (Kind::Choice { options }, value) => {
                    let shown: Vec<String> = options.iter().map(|o| o.display.clone()).collect();
                    let selected = match value {
                        Value::Text(t) => options.iter().find(|o| o.export == *t).map(|o| o.display.clone()),
                        _ => None,
                    };
                    let list = pick_list(shown, selected, move |d| Message::Form(FormMsg::Choose(f, d)))
                        .text_size(size)
                        .padding([0.0, 3.0 * zoom])
                        .width(Length::Fill)
                        .style(style::form_choice(color));
                    container(list).height(height).center_y(height).into()
                }
                (Kind::Signature, _) if self.is_signed(page, &r) => continue,
                (Kind::Signature, _) => {
                    let label = text("Sign here").size((height * 0.35).clamp(9.0, 16.0));
                    let rect = [r.x0, r.y0, r.width(), r.height()];
                    button(center(label))
                        .padding(0)
                        .style(style::form_box)
                        .on_press(Message::Sign(SignMsg::Field { page, rect }))
                        .into()
                }
                (Kind::Push, _) => continue,
            };
            let sized = container(element).width(width).height(height);
            out.push(placed(&r, zoom, sized.into()));
        }
        out
    }
}
