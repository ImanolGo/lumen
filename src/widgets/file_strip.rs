//! Sidebar listing the files being browsed, with lazily filled thumbnails.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

use relm4::gtk::{self, gdk, glib, pango, prelude::*};

use crate::thumbnails::THUMB_SIZE;

pub struct FileStrip {
    root: gtk::ScrolledWindow,
    list: gtk::ListBox,
    images: RefCell<HashMap<PathBuf, (gtk::Stack, gtk::Picture)>>,
}

impl FileStrip {
    /// `on_activate` receives the index of the row the user picked.
    pub fn new(on_activate: impl Fn(usize) + 'static) -> Self {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar"])
            .build();
        list.connect_row_activated(move |_, row| on_activate(row.index() as usize));

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(180)
            .child(&list)
            .build();

        Self {
            root,
            list,
            images: RefCell::default(),
        }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    pub fn set_files(&self, files: &[PathBuf], selected: usize) {
        self.list.remove_all();
        let mut images = self.images.borrow_mut();
        images.clear();
        for path in files {
            let (row, image) = build_row(path);
            self.list.append(&row);
            images.insert(path.clone(), image);
        }
        drop(images);
        self.select(selected);
    }

    pub fn select(&self, index: usize) {
        let Some(row) = self.list.row_at_index(index as i32) else {
            return;
        };
        self.list.select_row(Some(&row));

        // Rows may not be allocated yet right after `set_files`.
        let scroller = self.root.clone();
        let list = self.list.clone();
        glib::idle_add_local_once(move || {
            let adj = scroller.vadjustment();
            let Some(top) = row.compute_point(&list, &gtk::graphene::Point::zero()) else {
                return;
            };
            let (top, bottom) = (top.y() as f64, (top.y() + row.height() as f32) as f64);
            if top < adj.value() {
                adj.set_value(top);
            } else if bottom > adj.value() + adj.page_size() {
                adj.set_value(bottom - adj.page_size());
            }
        });
    }

    pub fn set_thumbnail(&self, path: &Path, texture: &gdk::Texture) {
        if let Some((stack, picture)) = self.images.borrow().get(path) {
            picture.set_paintable(Some(texture));
            stack.set_visible_child_name("thumb");
        }
    }
}

fn build_row(path: &Path) -> (gtk::ListBoxRow, (gtk::Stack, gtk::Picture)) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_pdf = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    let icon = if is_pdf {
        "x-office-document-symbolic"
    } else {
        "image-x-generic-symbolic"
    };

    // The icon stands in until the thumbnail arrives. A `GtkImage` would draw
    // the paintable at icon size, so the thumbnail uses a `GtkPicture`.
    let icon = gtk::Image::builder().icon_name(icon).pixel_size(48).build();
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .can_shrink(true)
        .build();
    let frame = gtk::Stack::builder()
        .width_request(THUMB_SIZE)
        .height_request(THUMB_SIZE * 3 / 4)
        .halign(gtk::Align::Center)
        .build();
    frame.add_named(&icon, Some("icon"));
    frame.add_named(&picture, Some("thumb"));

    let label = gtk::Label::builder()
        .label(&name)
        .ellipsize(pango::EllipsizeMode::Middle)
        .max_width_chars(16)
        .css_classes(["caption"])
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    content.append(&frame);
    content.append(&label);

    let row = gtk::ListBoxRow::builder()
        .child(&content)
        .tooltip_text(&name)
        .build();
    (row, (frame, picture))
}
