//! Sidebar with one thumbnail per PDF page.
//!
//! A `gtk::ListView` only creates widgets for the rows on screen, so a
//! 500-page document costs about as much as a 10-page one. Rows tell the
//! owner when they appear or disappear; the owner renders thumbnails for the
//! visible ones and hands them back through `set_thumbnail`.

use std::{cell::{Cell, RefCell}, collections::HashMap, rc::Rc};

use relm4::gtk::{self, gdk, prelude::*};

/// Width of a thumbnail in points; the height follows the page's aspect ratio.
pub const THUMB_WIDTH: f64 = 140.0;
/// Rendered thumbnails kept around beyond the visible ones.
const KEEP_THUMBNAILS: usize = 300;

#[derive(Default)]
struct State {
    sizes: RefCell<Vec<(f64, f64)>>,
    thumbs: RefCell<HashMap<usize, gdk::Texture>>,
    /// Rows currently on screen, by page index.
    bound: RefCell<HashMap<usize, gtk::Picture>>,
    last_current: Cell<Option<usize>>,
}

pub struct PageStrip {
    root: gtk::ScrolledWindow,
    list: gtk::ListView,
    selection: gtk::SingleSelection,
    model: gtk::StringList,
    state: Rc<State>,
}

impl PageStrip {
    /// `on_activate` gets the page the user clicked; `on_visible_changed` is
    /// called whenever rows appear or disappear.
    pub fn new(
        on_activate: impl Fn(usize) + 'static,
        on_visible_changed: impl Fn() + 'static,
    ) -> Self {
        let state = Rc::new(State::default());
        let on_visible_changed = Rc::new(on_visible_changed);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().expect("a list item");
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .halign(gtk::Align::Center)
                .css_classes(["card"])
                .build();
            let label = gtk::Label::builder().css_classes(["caption"]).build();
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .margin_top(6)
                .margin_bottom(6)
                .build();
            content.append(&picture);
            content.append(&label);
            item.set_child(Some(&content));
        });
        factory.connect_bind({
            let state = Rc::clone(&state);
            let changed = Rc::clone(&on_visible_changed);
            move |_, object| {
                let item = object.downcast_ref::<gtk::ListItem>().expect("a list item");
                let page = item.position() as usize;
                let (picture, label) = row_widgets(item);

                let (w, h) = state.sizes.borrow().get(page).copied().unwrap_or((1.0, 1.4));
                picture.set_size_request(THUMB_WIDTH as i32, (THUMB_WIDTH * h / w).round() as i32);
                picture.set_paintable(state.thumbs.borrow().get(&page));
                label.set_label(&(page + 1).to_string());

                state.bound.borrow_mut().insert(page, picture);
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

    /// Show a new document with pages of the given sizes (in points).
    pub fn set_document(&self, sizes: &[(f64, f64)]) {
        self.clear();
        *self.state.sizes.borrow_mut() = sizes.to_vec();
        let rows = vec![""; sizes.len()];
        self.model.splice(0, 0, &rows);
        self.state.last_current.set(None);
    }

    pub fn clear(&self) {
        self.model.splice(0, self.model.n_items(), &[]);
        self.state.sizes.borrow_mut().clear();
        self.state.thumbs.borrow_mut().clear();
        self.state.bound.borrow_mut().clear();
    }

    /// Highlight `page` and scroll it into view.
    pub fn set_current(&self, page: usize) {
        if self.state.last_current.replace(Some(page)) == Some(page) {
            return;
        }
        if (page as u32) < self.model.n_items() {
            self.selection.set_selected(page as u32);
            self.list.scroll_to(page as u32, gtk::ListScrollFlags::NONE, None);
        }
    }

    /// Pages on screen that still lack a thumbnail, top to bottom.
    pub fn wanted(&self) -> Vec<usize> {
        let thumbs = self.state.thumbs.borrow();
        let mut pages: Vec<usize> = self
            .state
            .bound
            .borrow()
            .keys()
            .copied()
            .filter(|p| !thumbs.contains_key(p))
            .collect();
        pages.sort_unstable();
        pages
    }

    pub fn set_thumbnail(&self, page: usize, texture: gdk::Texture) {
        if let Some(picture) = self.state.bound.borrow().get(&page) {
            picture.set_paintable(Some(&texture));
        }
        let mut thumbs = self.state.thumbs.borrow_mut();
        thumbs.insert(page, texture);
        if thumbs.len() > KEEP_THUMBNAILS {
            let bound = self.state.bound.borrow();
            thumbs.retain(|p, _| bound.contains_key(p));
        }
    }
}

fn row_widgets(item: &gtk::ListItem) -> (gtk::Picture, gtk::Label) {
    let content = item
        .child()
        .and_then(|c| c.downcast::<gtk::Box>().ok())
        .expect("row content is a box");
    let picture = content
        .first_child()
        .and_then(|c| c.downcast::<gtk::Picture>().ok())
        .expect("first child is the picture");
    let label = content
        .last_child()
        .and_then(|c| c.downcast::<gtk::Label>().ok())
        .expect("last child is the label");
    (picture, label)
}
