//! Filling in PDF forms: what a click on a field does, and writing the value.

use std::path::PathBuf;

use relm4::{
    ComponentSender,
    gtk::{self, glib, prelude::*},
};

use super::{App, AppMsg, markup::PdfState};
use crate::{
    i18n::tr,
    pdf::{FieldKind, FieldValue, FormField, PdfSource, forms},
};

impl App {
    /// A field was clicked: toggle it, or ask for its new value next to it.
    pub(super) fn form_clicked(
        &mut self,
        page: usize,
        field: FormField,
        sender: &ComponentSender<App>,
    ) {
        if self.page_busy || !field.editable() {
            return;
        }
        let id = field.id;
        match &field.kind {
            FieldKind::Check { on } => self.fill_form(id, FieldValue::On(!on), sender),
            // A radio button is turned off by choosing another one of its group.
            FieldKind::Radio { on: false } => self.fill_form(id, FieldValue::On(true), sender),
            FieldKind::Radio { on: true } => {}
            FieldKind::Text {
                text,
                multiline,
                password,
                max_len,
            } => {
                let Some(popover) = self.canvas.popup_at(page, &field.area) else {
                    return;
                };
                let sender = sender.clone();
                let done = move |new: String, old: &str| {
                    if new != old {
                        sender.input(AppMsg::FillForm {
                            id,
                            value: FieldValue::Text(new),
                        });
                    }
                };
                text_editor(&popover, text, *multiline, *password, *max_len, done);
            }
            FieldKind::Choice {
                items,
                selected,
                editable,
                text,
            } => {
                let Some(popover) = self.canvas.popup_at(page, &field.area) else {
                    return;
                };
                choice_list(
                    &popover,
                    items,
                    *selected,
                    editable.then_some(text),
                    id,
                    sender,
                );
            }
            FieldKind::Signature => self.sign_field(page, field.area, sender),
            FieldKind::Push => {}
        }
    }

    /// Write a working copy with field `id` set to `value`; it becomes one undo step.
    pub(super) fn fill_form(&mut self, id: i32, value: FieldValue, sender: &ComponentSender<App>) {
        let Some(pdf) = &self.pdf else { return };
        if self.page_busy {
            return;
        }
        let source = PdfSource {
            path: pdf.source.path.clone(),
            password: pdf.source.password.clone(),
        };
        let (out, _) = self.work.next_file();
        self.page_busy = true;
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = forms::set_value(&source, id, &value, &out).map_err(|e| format!("{e:#}"));
            sender.input(AppMsg::FormFilled { out, result });
        });
    }

    pub(super) fn form_filled(
        &mut self,
        out: PathBuf,
        result: Result<(), String>,
        sender: &ComponentSender<App>,
    ) {
        match result {
            Ok(()) => {
                let layers = self.markup.store.borrow().pages.layers.clone();
                self.markup.pdf_history.commit(PdfState {
                    file: out.clone(),
                    layers,
                });
                self.markup.pdf_dirty = true;
                let page = self.pdf.as_ref().map_or(0, |p| p.page);
                self.reload_pdf(out, page, sender);
            }
            Err(message) => {
                self.page_busy = false;
                self.control_finished(super::control::Wait::Reload, Err(message.clone()));
                super::edit::show_error(&tr("Could Not Fill In the Form"), &message);
            }
        }
    }
}

/// Edit `text` in `popover`: one line or several. Enter finishes; in a multi-line field,
/// Shift+Enter starts a new line.
/// `done` gets the new text and the old one when the popover closes.
fn text_editor(
    popover: &gtk::Popover,
    text: &str,
    multiline: bool,
    password: bool,
    max_len: Option<usize>,
    done: impl Fn(String, &str) + 'static,
) {
    let old = text.to_owned();
    if multiline {
        let view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();
        view.buffer().set_text(text);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .min_content_width(300)
            .min_content_height(100)
            .build();
        popover.set_child(Some(&scroller));
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[weak]
            popover,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, state| {
                let enter = key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter;
                if enter && !state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    popover.popdown();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        view.add_controller(keys);
        let typed = view.clone();
        popover.connect_closed(move |_| {
            let buffer = typed.buffer();
            let new = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            done(new.to_string(), &old);
        });
        popover.popup();
        view.grab_focus();
    } else {
        let entry = gtk::Entry::builder()
            .text(text)
            .width_chars(28)
            .visibility(!password)
            .activates_default(false)
            .build();
        if let Some(max) = max_len {
            entry.set_max_length(max as i32);
        }
        popover.set_child(Some(&entry));
        entry.connect_activate(glib::clone!(
            #[weak]
            popover,
            move |_| popover.popdown()
        ));
        let typed = entry.clone();
        popover.connect_closed(move |_| done(typed.text().to_string(), &old));
        popover.popup();
        entry.grab_focus();
    }
}

/// The items of a choice field in `popover`, plus an entry for typed text if it takes any.
fn choice_list(
    popover: &gtk::Popover,
    items: &[String],
    selected: Option<usize>,
    typed: Option<&String>,
    id: i32,
    sender: &ComponentSender<App>,
) {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["navigation-sidebar"])
        .build();
    for (index, item) in items.iter().enumerate() {
        let row = gtk::Box::builder().spacing(12).build();
        row.append(
            &gtk::Label::builder()
                .label(item)
                .xalign(0.0)
                .hexpand(true)
                .build(),
        );
        let check = gtk::Image::from_icon_name("object-select-symbolic");
        check.set_opacity(if selected == Some(index) { 1.0 } else { 0.0 });
        row.append(&check);
        list.append(&row);
    }
    list.connect_row_activated(glib::clone!(
        #[weak]
        popover,
        #[strong]
        sender,
        move |_, row| {
            sender.input(AppMsg::FillForm {
                id,
                value: FieldValue::Item(row.index() as usize),
            });
            popover.popdown();
        }
    ));
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    content.append(&list);
    if let Some(text) = typed {
        let entry = gtk::Entry::builder()
            .text(text.as_str())
            .placeholder_text(tr("Or type a value"))
            .build();
        let sender = sender.clone();
        entry.connect_activate(glib::clone!(
            #[weak]
            popover,
            move |entry| {
                sender.input(AppMsg::FillForm {
                    id,
                    value: FieldValue::Text(entry.text().to_string()),
                });
                popover.popdown();
            }
        ));
        content.append(&entry);
    }
    popover.set_child(Some(&content));
    popover.popup();
}
