//! Signing: a pad to draw a signature on, the menu of saved ones, and placing one.

use std::{cell::RefCell, rc::Rc};

use relm4::{
    ComponentSender,
    adw::{self, prelude::*},
    gtk::{self, glib},
};

use super::{App, AppMsg};
use crate::{
    annotate::{Color, Rect, Style},
    i18n::tr,
    signatures::Signature,
};

/// Strokes as drawn on the pad, in its pixels.
type Strokes = Vec<Vec<(f64, f64)>>;

/// Signatures are stamped in dark blue ink, whatever the markup colour is.
const INK: Color = Color::rgba(0.11, 0.16, 0.36, 1.0);

impl App {
    /// Fill the signature menu with the saved signatures and the button to draw a new one.
    pub(super) fn refresh_signature_menu(&self, sender: &ComponentSender<App>) {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        for (index, signature) in self.signatures.items.iter().enumerate() {
            let row = gtk::Box::builder().spacing(6).build();
            let pick = gtk::Button::builder()
                .child(&preview(signature, 220, 64))
                .tooltip_text(tr("Place This Signature"))
                .css_classes(["flat"])
                .build();
            let sender_pick = sender.clone();
            pick.connect_clicked(move |_| sender_pick.input(AppMsg::PlaceSignature(index)));
            let delete = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Delete This Signature"))
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build();
            let sender_delete = sender.clone();
            delete.connect_clicked(move |_| sender_delete.input(AppMsg::DeleteSignature(index)));
            row.append(&pick);
            row.append(&delete);
            content.append(&row);
        }
        if !self.signatures.items.is_empty() {
            content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let new = gtk::Button::builder()
            .label(tr("New Signature…"))
            .css_classes(["flat"])
            .build();
        let sender_new = sender.clone();
        new.connect_clicked(move |_| sender_new.input(AppMsg::NewSignature));
        content.append(&new);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        self.signature_button.set_popover(Some(&popover));
    }

    /// Put signature `index` on the page, where the reader is looking (or into the
    /// signature field that was clicked), selected so it can be moved and resized.
    pub(super) fn place_signature(&mut self, index: usize, sender: &ComponentSender<App>) {
        self.signature_button.popdown();
        let Some(signature) = self.signatures.items.get(index).cloned() else {
            return;
        };
        if !self.can_markup() {
            return;
        }
        let Some((page, target)) = self.signature_box() else {
            return;
        };
        // Newest first: the one just used comes up first next time.
        self.signatures.add(signature.clone());
        self.refresh_signature_menu(sender);
        let style = Style {
            stroke: INK,
            fill: None,
            width: (target.h * 0.035).clamp(0.6, 4.0),
            ..Style::default()
        };
        let outcome = {
            let mut store = self.markup.store.borrow_mut();
            self.markup
                .editor
                .place(&mut store, page, signature.shape_in(target), style)
        };
        self.markup.open = true;
        self.finish_markup(outcome, sender);
    }

    /// Where a signature goes: the field that was clicked, or a third of the page's
    /// width, centred on what is in view.
    fn signature_box(&mut self) -> Option<(usize, Rect)> {
        if let Some((page, area)) = self.signature_target.take() {
            let inset = 2.0;
            return Some((
                page,
                Rect::new(
                    area.x1 + inset,
                    area.y1 + inset,
                    (area.x2 - area.x1 - 2.0 * inset).max(1.0),
                    (area.y2 - area.y1 - 2.0 * inset).max(1.0),
                ),
            ));
        }
        let (page, (page_w, page_h)) = match &self.pdf {
            Some(pdf) => (pdf.page, pdf.layout.rects[pdf.page].size()),
            None => {
                let (w, h) = self.shown_image()?.display_size();
                (0, (w as f64, h as f64))
            }
        };
        let (w, h) = (page_w * 0.32, page_w * 0.32 / 3.0);
        let center = self
            .canvas
            .view_center(page)
            .unwrap_or(crate::annotate::Point::new(page_w / 2.0, page_h / 2.0));
        let x = (center.x - w / 2.0).clamp(0.0, (page_w - w).max(0.0));
        let y = (center.y - h / 2.0).clamp(0.0, (page_h - h).max(0.0));
        Some((page, Rect::new(x, y, w, h)))
    }

    pub(super) fn new_signature(&self, sender: &ComponentSender<App>) {
        self.signature_button.popdown();
        let sender = sender.clone();
        signature_pad(move |strokes| sender.input(AppMsg::SignatureDrawn(strokes)));
    }

    pub(super) fn signature_drawn(&mut self, strokes: Strokes, sender: &ComponentSender<App>) {
        let Some(signature) = Signature::from_strokes(&strokes) else {
            return;
        };
        self.signatures.add(signature);
        self.refresh_signature_menu(sender);
        self.place_signature(0, sender);
    }

    pub(super) fn delete_signature(&mut self, index: usize, sender: &ComponentSender<App>) {
        self.signatures.remove(index);
        self.refresh_signature_menu(sender);
    }

    /// A signature field of the form was clicked: sign into it.
    pub(super) fn sign_field(
        &mut self,
        page: usize,
        area: crate::pdf::HitRect,
        sender: &ComponentSender<App>,
    ) {
        self.signature_target = Some((page, area));
        if self.signatures.items.is_empty() {
            self.new_signature(sender);
        } else {
            self.place_signature(0, sender);
        }
    }
}

/// `signature` drawn to fit in `width` × `height` pixels, in the text colour.
fn preview(signature: &Signature, width: i32, height: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(width)
        .content_height(height)
        .build();
    let signature = signature.clone();
    area.set_draw_func(move |area, cr, w, h| {
        let margin = 6.0;
        let box_ = Rect::new(
            margin,
            margin,
            w as f64 - 2.0 * margin,
            h as f64 - 2.0 * margin,
        );
        let crate::annotate::Shape::Ink(strokes) = signature.shape_in(box_) else {
            return;
        };
        draw_strokes(area, cr, &strokes, 2.0);
    });
    area
}

fn draw_strokes(
    widget: &impl IsA<gtk::Widget>,
    cr: &gtk::cairo::Context,
    strokes: &[Vec<crate::annotate::Point>],
    width: f64,
) {
    let color = widget.as_ref().color();
    cr.set_source_rgba(
        color.red() as f64,
        color.green() as f64,
        color.blue() as f64,
        color.alpha() as f64,
    );
    cr.set_line_width(width);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    for stroke in strokes {
        if let Some((first, rest)) = stroke.split_first() {
            cr.move_to(first.x, first.y);
            if rest.is_empty() {
                cr.line_to(first.x, first.y);
            }
            for p in rest {
                cr.line_to(p.x, p.y);
            }
        }
    }
    let _ = cr.stroke();
}

/// A dialog to sign in, with a mouse, touchpad or pen. `on_save` gets the strokes.
fn signature_pad(on_save: impl Fn(Strokes) + 'static) {
    let strokes: Rc<RefCell<Strokes>> = Rc::default();

    let pad = gtk::DrawingArea::builder()
        .content_width(520)
        .content_height(200)
        .hexpand(true)
        .vexpand(true)
        .cursor(&gtk::gdk::Cursor::from_name("crosshair", None).expect("a standard cursor"))
        .build();
    pad.update_property(&[gtk::accessible::Property::Label(&tr("Signing area"))]);
    pad.set_draw_func({
        let strokes = Rc::clone(&strokes);
        move |pad, cr, w, h| {
            // A line to sign on.
            let color = pad.color();
            cr.set_source_rgba(
                color.red() as f64,
                color.green() as f64,
                color.blue() as f64,
                0.2,
            );
            cr.set_line_width(1.0);
            let base = (h as f64 * 0.72).round() + 0.5;
            cr.move_to(24.0, base);
            cr.line_to(w as f64 - 24.0, base);
            let _ = cr.stroke();
            let points: Vec<Vec<crate::annotate::Point>> = strokes
                .borrow()
                .iter()
                .map(|s| {
                    s.iter()
                        .map(|&(x, y)| crate::annotate::Point::new(x, y))
                        .collect()
                })
                .collect();
            draw_strokes(pad, cr, &points, 2.5);
        }
    });

    let save = gtk::Button::builder()
        .label(tr("_Save"))
        .use_underline(true)
        .sensitive(false)
        .css_classes(["suggested-action"])
        .build();
    let clear = gtk::Button::builder()
        .label(tr("C_lear"))
        .use_underline(true)
        .halign(gtk::Align::End)
        .sensitive(false)
        .build();

    let drag = gtk::GestureDrag::new();
    let start = Rc::new(std::cell::Cell::new((0.0, 0.0)));
    drag.connect_drag_begin({
        let (strokes, start, pad) = (Rc::clone(&strokes), Rc::clone(&start), pad.clone());
        let (save, clear) = (save.clone(), clear.clone());
        move |_, x, y| {
            start.set((x, y));
            strokes.borrow_mut().push(vec![(x, y)]);
            save.set_sensitive(true);
            clear.set_sensitive(true);
            pad.queue_draw();
        }
    });
    drag.connect_drag_update({
        let (strokes, start, pad) = (Rc::clone(&strokes), Rc::clone(&start), pad.clone());
        move |_, dx, dy| {
            let (x, y) = start.get();
            if let Some(stroke) = strokes.borrow_mut().last_mut() {
                stroke.push((x + dx, y + dy));
            }
            pad.queue_draw();
        }
    });
    pad.add_controller(drag);

    clear.connect_clicked({
        let (strokes, pad, save) = (Rc::clone(&strokes), pad.clone(), save.clone());
        move |clear| {
            strokes.borrow_mut().clear();
            save.set_sensitive(false);
            clear.set_sensitive(false);
            pad.queue_draw();
        }
    });

    let frame = gtk::Frame::builder()
        .child(&pad)
        .css_classes(["view"])
        .build();
    let hint = gtk::Label::builder()
        .label(tr(
            "Sign with your mouse, touchpad or pen. Lumen keeps the signature for next time.",
        ))
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(6)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    body.append(&hint);
    body.append(&frame);
    body.append(&clear);

    let cancel = gtk::Button::builder()
        .label(tr("_Cancel"))
        .use_underline(true)
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));

    let dialog = adw::Dialog::builder()
        .title(tr("New Signature"))
        .content_width(580)
        .child(&toolbar)
        .build();
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    save.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            on_save(strokes.borrow().clone());
            dialog.close();
        }
    ));
    dialog.present(relm4::main_application().active_window().as_ref());
}
