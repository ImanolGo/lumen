//! Side panel listing the file's properties and EXIF metadata.

use crate::i18n::tr;
use relm4::{
    adw::{self, prelude::*},
    gtk::{self, glib},
};

use crate::image::ImageDocument;

pub struct PropertiesPanel {
    root: gtk::ScrolledWindow,
    group: adw::PreferencesGroup,
    rows: std::cell::RefCell<Vec<adw::ActionRow>>,
}

impl PropertiesPanel {
    pub fn new() -> Self {
        let group = adw::PreferencesGroup::builder()
            .title(tr("Properties"))
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(300)
            .hexpand(false)
            .child(&group)
            .build();
        Self {
            root,
            group,
            rows: Default::default(),
        }
    }

    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    pub fn set_image(&self, image: Option<&ImageDocument>) {
        let mut rows = self.rows.borrow_mut();
        for row in rows.drain(..) {
            self.group.remove(&row);
        }
        let Some(image) = image else { return };

        let (w, h) = image.display_size();
        let meta = &image.metadata;
        let folder = image.path.parent().map(|p| p.display().to_string());
        let name = image
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());

        let entries = [
            (tr("File"), name),
            (tr("Folder"), folder),
            (
                tr("Size on disk"),
                meta.file_size.map(|b| glib::format_size(b).to_string()),
            ),
            (tr("Dimensions"), Some(format!("{w} × {h} px"))),
            (tr("Camera"), meta.camera.clone()),
            (tr("Date taken"), meta.date_taken.clone()),
            (tr("Exposure"), meta.exposure.clone()),
            (tr("Location"), meta.location.clone()),
        ];
        for (title, value) in entries {
            let Some(value) = value else { continue };
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(glib::markup_escape_text(&value))
                .subtitle_selectable(true)
                .css_classes(["property"])
                .build();
            self.group.add(&row);
            rows.push(row);
        }
    }
}
