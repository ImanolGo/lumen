//! Sidebar with one thumbnail per PDF page.
//!
//! A `gtk::ListView` only creates widgets for the rows on screen, so a
//! 500-page document costs about as much as a 10-page one. Rows tell the
//! owner when they appear or disappear; the owner renders thumbnails for the
//! visible ones and hands them back through `set_thumbnail`.
//!
//! Several pages can be selected (Ctrl and Shift click). Selected pages can be
//! dragged to a new place, and files dropped between pages are inserted there.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
};

use relm4::gtk::{self, gdk, glib, prelude::*};

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
    /// Set while the strip changes its own selection, so that is not mistaken for a click.
    programmatic: Cell<bool>,
}

/// What the strip tells its owner.
pub struct PageStripCallbacks {
    /// A single page was picked: show it.
    pub on_activate: Box<dyn Fn(usize)>,
    /// Rows appeared or disappeared.
    pub on_visible_changed: Box<dyn Fn()>,
    /// Pages were dragged in front of page `before` (the page count means the end).
    pub on_reorder: Box<dyn Fn(Vec<usize>, usize)>,
    /// Files were dropped in front of page `before`.
    pub on_files: Box<dyn Fn(Vec<PathBuf>, usize)>,
    /// The Delete key was pressed on the strip.
    pub on_delete: Box<dyn Fn()>,
}

const DRAG_PREFIX: &str = "lumen-pages:";

pub struct PageStrip {
    root: gtk::ScrolledWindow,
    list: gtk::ListView,
    selection: gtk::MultiSelection,
    model: gtk::StringList,
    state: Rc<State>,
}

impl PageStrip {
    pub fn new(callbacks: PageStripCallbacks) -> Self {
        let state = Rc::new(State::default());
        let PageStripCallbacks { on_activate, on_visible_changed, on_reorder, on_files, on_delete } = callbacks;
        let on_visible_changed: Rc<dyn Fn()> = Rc::from(on_visible_changed);
        let on_reorder: Rc<dyn Fn(Vec<usize>, usize)> = Rc::from(on_reorder);
        let on_files: Rc<dyn Fn(Vec<PathBuf>, usize)> = Rc::from(on_files);

        // The selection model exists before the factory because rows consult it when dragged.
        let model = gtk::StringList::new(&[]);
        let selection = gtk::MultiSelection::new(Some(model.clone()));

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup({
            let selection = selection.clone();
            let on_reorder = Rc::clone(&on_reorder);
            let on_files = Rc::clone(&on_files);
            move |_, object| {
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

            // Drag the selected pages (or just this one) to another place.
            let drag = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
            drag.connect_prepare({
                let (selection, item) = (selection.clone(), item.clone());
                move |source, _, _| {
                    let here = item.position();
                    if !selection.is_selected(here) {
                        selection.select_item(here, true);
                    }
                    let pages = selected_pages(&selection);
                    let list: Vec<String> = pages.iter().map(usize::to_string).collect();
                    // Show the page being dragged under the pointer.
                    if let Some(paintable) = picture_of(&source.widget()).and_then(|p| p.paintable()) {
                        source.set_icon(Some(&paintable), 0, 0);
                    }
                    Some(gdk::ContentProvider::for_value(&format!("{DRAG_PREFIX}{}", list.join(",")).to_value()))
                }
            });
            content.add_controller(drag);

            // Drop pages (to reorder) or files (to insert) before or after this row.
            let drop = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::MOVE | gdk::DragAction::COPY);
            drop.set_types(&[glib::Type::STRING, gdk::FileList::static_type()]);
            drop.connect_drop({
                let (item, on_reorder, on_files) = (item.clone(), Rc::clone(&on_reorder), Rc::clone(&on_files));
                move |target, value, _, y| {
                    let here = item.position() as usize;
                    let lower_half = target.widget().is_some_and(|w| y > w.height() as f64 / 2.0);
                    let before = here + usize::from(lower_half);
                    if let Ok(text) = value.get::<String>() {
                        let pages: Vec<usize> = text
                            .strip_prefix(DRAG_PREFIX)
                            .map(|list| list.split(',').filter_map(|p| p.parse().ok()).collect())
                            .unwrap_or_default();
                        if pages.is_empty() {
                            return false;
                        }
                        on_reorder(pages, before);
                        return true;
                    }
                    if let Ok(files) = value.get::<gdk::FileList>() {
                        let paths: Vec<PathBuf> = files.files().iter().filter_map(|f| f.path()).collect();
                        if !paths.is_empty() {
                            on_files(paths, before);
                            return true;
                        }
                    }
                    false
                }
            });
            content.add_controller(drop);

            item.set_child(Some(&content));
            }
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

        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .css_classes(["navigation-sidebar"])
            .build();

        // Picking exactly one page shows it; a bigger selection is left for page operations.
        selection.connect_selection_changed({
            let state = Rc::clone(&state);
            let selection = selection.clone();
            move |_, _, _| {
                if state.programmatic.get() {
                    return;
                }
                if let [only] = selected_pages(&selection)[..] {
                    on_activate(only);
                }
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Delete {
                on_delete();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        list.add_controller(keys);

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

    /// The selected pages, in order.
    pub fn selected(&self) -> Vec<usize> {
        selected_pages(&self.selection)
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
            // A selection of several pages is the user's to keep while the view scrolls.
            if self.selection.selection().size() <= 1 {
                self.state.programmatic.set(true);
                self.selection.select_item(page as u32, true);
                self.state.programmatic.set(false);
            }
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

fn selected_pages(selection: &gtk::MultiSelection) -> Vec<usize> {
    let set = selection.selection();
    (0..selection.n_items()).filter(|&i| set.contains(i)).map(|i| i as usize).collect()
}

/// The thumbnail inside a row, given the row's content box.
fn picture_of(widget: &Option<gtk::Widget>) -> Option<gtk::Picture> {
    widget.as_ref()?.first_child()?.downcast::<gtk::Picture>().ok()
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
