//! Sidebar listing the files being browsed, with lazily filled thumbnails.
//!
//! Like the page strip it is a `gtk::ListView`, which only builds rows for what
//! is on screen: a folder of ten thousand pictures costs no more than one of ten.
//! Rows tell the owner when they appear or disappear, and the owner renders
//! thumbnails for the visible ones only.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
};

use relm4::gtk::{self, gdk, pango, prelude::*};

use crate::thumbnails::THUMB_SIZE;

#[derive(Default)]
struct State {
    paths: RefCell<Vec<PathBuf>>,
    thumbs: RefCell<HashMap<PathBuf, gdk::Texture>>,
    /// Rows on screen, by position.
    bound: RefCell<HashMap<usize, (gtk::Stack, gtk::Picture)>>,
}

pub struct FileStrip {
    root: gtk::ScrolledWindow,
    list: gtk::ListView,
    selection: gtk::SingleSelection,
    model: gtk::StringList,
    state: Rc<State>,
}

impl FileStrip {
    /// `on_activate` receives the index of the row the user picked; `on_visible_changed`
    /// is called whenever rows appear or disappear.
    pub fn new(
        on_activate: impl Fn(usize) + 'static,
        on_visible_changed: impl Fn() + 'static,
    ) -> Self {
        let state = Rc::new(State::default());
        let on_visible_changed: Rc<dyn Fn()> = Rc::new(on_visible_changed);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().expect("a list item");
            // The icon stands in until the thumbnail arrives. A `GtkImage` would draw a
            // paintable at icon size, so the thumbnail uses a `GtkPicture`.
            let icon = gtk::Image::builder().pixel_size(48).build();
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
            item.set_child(Some(&content));
        });
        factory.connect_bind({
            let state = Rc::clone(&state);
            let changed = Rc::clone(&on_visible_changed);
            move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().expect("a list item");
                let position = item.position() as usize;
                let Some(path) = state.paths.borrow().get(position).cloned() else {
                    return;
                };
                let (stack, picture, icon, label) = row_widgets(item);

                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                label.set_label(&name);
                label.set_tooltip_text(Some(&name));
                icon.set_icon_name(Some(if is_pdf(&path) {
                    "x-office-document-symbolic"
                } else {
                    "image-x-generic-symbolic"
                }));
                match state.thumbs.borrow().get(&path) {
                    Some(texture) => {
                        picture.set_paintable(Some(texture));
                        stack.set_visible_child_name("thumb");
                    }
                    None => {
                        picture.set_paintable(gdk::Paintable::NONE);
                        stack.set_visible_child_name("icon");
                    }
                }
                state.bound.borrow_mut().insert(position, (stack, picture));
                changed();
            }
        });
        factory.connect_unbind({
            let state = Rc::clone(&state);
            let changed = Rc::clone(&on_visible_changed);
            move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().expect("a list item");
                state.bound.borrow_mut().remove(&(item.position() as usize));
                changed();
            }
        });

        let model = gtk::StringList::new(&[]);
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .single_click_activate(true)
            .css_classes(["navigation-sidebar"])
            .build();
        list.connect_activate(move |_, position| on_activate(position as usize));

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(180)
            .child(&list)
            .build();

        Self {
            root,
            list,
            selection,
            model,
            state,
        }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    pub fn set_files(&self, files: &[PathBuf], selected: usize) {
        *self.state.paths.borrow_mut() = files.to_vec();
        // Thumbnails of files that are still in the list stay valid; drop the rest.
        self.state
            .thumbs
            .borrow_mut()
            .retain(|path, _| files.contains(path));
        let rows = vec![""; files.len()];
        self.model.splice(0, self.model.n_items(), &rows);
        self.select(selected);
    }

    pub fn select(&self, index: usize) {
        if (index as u32) < self.model.n_items() {
            self.selection.set_selected(index as u32);
            self.list
                .scroll_to(index as u32, gtk::ListScrollFlags::NONE, None);
        }
    }

    /// Files on screen that still lack a thumbnail, top to bottom (PDFs show an icon instead).
    pub fn wanted(&self) -> Vec<PathBuf> {
        let paths = self.state.paths.borrow();
        let thumbs = self.state.thumbs.borrow();
        let mut positions: Vec<usize> = self.state.bound.borrow().keys().copied().collect();
        positions.sort_unstable();
        positions
            .into_iter()
            .filter_map(|p| paths.get(p))
            .filter(|path| !is_pdf(path) && !thumbs.contains_key(*path))
            .cloned()
            .collect()
    }

    pub fn set_thumbnail(&self, path: &Path, texture: &gdk::Texture) {
        self.state
            .thumbs
            .borrow_mut()
            .insert(path.to_owned(), texture.clone());
        let paths = self.state.paths.borrow();
        for (position, (stack, picture)) in self.state.bound.borrow().iter() {
            if paths.get(*position).is_some_and(|p| p == path) {
                picture.set_paintable(Some(texture));
                stack.set_visible_child_name("thumb");
            }
        }
    }
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// The widgets of a row, in the order stack, picture, icon, label.
fn row_widgets(item: &gtk::ListItem) -> (gtk::Stack, gtk::Picture, gtk::Image, gtk::Label) {
    let content = item
        .child()
        .and_then(|c| c.downcast::<gtk::Box>().ok())
        .expect("row content is a box");
    let stack = content
        .first_child()
        .and_then(|c| c.downcast::<gtk::Stack>().ok())
        .expect("first child is the stack");
    let icon = stack
        .child_by_name("icon")
        .and_then(|c| c.downcast::<gtk::Image>().ok())
        .expect("icon page");
    let picture = stack
        .child_by_name("thumb")
        .and_then(|c| c.downcast::<gtk::Picture>().ok())
        .expect("thumb page");
    let label = content
        .last_child()
        .and_then(|c| c.downcast::<gtk::Label>().ok())
        .expect("last child is the label");
    (stack, picture, icon, label)
}
