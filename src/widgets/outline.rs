//! Sidebar tree showing a PDF's outline (table of contents).

use std::{cell::RefCell, sync::Arc};

use relm4::gtk::{self, gio, glib, pango, prelude::*, subclass::prelude::*};

use crate::pdf::{OutlineItem, OutlineTarget};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct OutlineNode {
        pub item: RefCell<Option<Arc<OutlineItem>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OutlineNode {
        const NAME: &'static str = "LumenOutlineNode";
        type Type = super::OutlineNode;
    }

    impl ObjectImpl for OutlineNode {}
}

glib::wrapper! {
    /// One outline entry as a GObject, so it can live in a list model.
    pub struct OutlineNode(ObjectSubclass<imp::OutlineNode>);
}

impl OutlineNode {
    fn new(item: Arc<OutlineItem>) -> Self {
        let node: Self = glib::Object::new();
        *node.imp().item.borrow_mut() = Some(item);
        node
    }

    fn item(&self) -> Arc<OutlineItem> {
        self.imp().item.borrow().clone().expect("node always has an item")
    }
}

fn store_of(items: &[Arc<OutlineItem>]) -> gio::ListStore {
    let store = gio::ListStore::new::<OutlineNode>();
    store.extend_from_slice(&items.iter().map(|i| OutlineNode::new(Arc::clone(i))).collect::<Vec<_>>());
    store
}

pub struct OutlinePanel {
    root: gtk::ScrolledWindow,
    top_level: RefCell<gio::ListStore>,
}

impl OutlinePanel {
    /// `on_activate` receives the target of the entry the user clicked.
    pub fn new(on_activate: impl Fn(OutlineTarget) + 'static) -> Self {
        let top_level = store_of(&[]);
        let tree = gtk::TreeListModel::new(top_level.clone(), false, false, |object| {
            let node = object.downcast_ref::<OutlineNode>()?;
            let children = &node.item().children;
            (!children.is_empty()).then(|| store_of(children).upcast())
        });

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().expect("a list item");
            let title = gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(pango::EllipsizeMode::End)
                .build();
            let page = gtk::Label::builder().css_classes(["dim-label", "caption"]).build();
            let row = gtk::Box::builder().spacing(8).build();
            row.append(&title);
            row.append(&page);
            let expander = gtk::TreeExpander::builder().child(&row).build();
            item.set_child(Some(&expander));
        });
        factory.connect_bind(|_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().expect("a list item");
            let Some(list_row) = item.item().and_downcast::<gtk::TreeListRow>() else {
                return;
            };
            let Some(node) = list_row.item().and_downcast::<OutlineNode>() else {
                return;
            };
            let expander = item.child().and_downcast::<gtk::TreeExpander>().expect("an expander");
            expander.set_list_row(Some(&list_row));

            let row = expander.child().and_downcast::<gtk::Box>().expect("a box");
            let title = row.first_child().and_downcast::<gtk::Label>().expect("title label");
            let page = row.last_child().and_downcast::<gtk::Label>().expect("page label");
            let node = node.item();
            title.set_label(&node.title);
            title.set_tooltip_text(Some(&node.title));
            page.set_label(&node.target.map(|t| (t.page + 1).to_string()).unwrap_or_default());
        });

        let selection = gtk::NoSelection::new(Some(tree));
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .single_click_activate(true)
            .css_classes(["navigation-sidebar"])
            .build();
        list.connect_activate({
            let selection = selection.clone();
            move |_, position| {
                let target = selection
                    .item(position)
                    .and_downcast::<gtk::TreeListRow>()
                    .and_then(|row| row.item().and_downcast::<OutlineNode>())
                    .and_then(|node| node.item().target);
                if let Some(target) = target {
                    on_activate(target);
                }
            }
        });

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(180)
            .child(&list)
            .build();

        Self {
            root,
            top_level: RefCell::new(top_level),
        }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    pub fn set_outline(&self, items: &[Arc<OutlineItem>]) {
        let store = self.top_level.borrow();
        store.remove_all();
        store.extend_from_slice(&items.iter().map(|i| OutlineNode::new(Arc::clone(i))).collect::<Vec<_>>());
    }
}
