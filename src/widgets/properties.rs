//! Side panel listing the file's properties: EXIF metadata for pictures, document details for PDFs.

use crate::i18n::tr;
use relm4::{
    adw::{self, prelude::*},
    gtk::{self, glib},
};

use std::path::Path;

use crate::{image::ImageDocument, pdf::PdfMetadata};

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
        let Some(image) = image else {
            self.show(Vec::new());
            return;
        };
        let (w, h) = image.display_size();
        let meta = &image.metadata;
        let mut entries = file_entries(&image.path, meta.file_size);
        entries.extend([
            (tr("Dimensions"), Some(format!("{w} × {h} px"))),
            (tr("Camera"), meta.camera.clone()),
            (tr("Date taken"), meta.date_taken.clone()),
            (tr("Exposure"), meta.exposure.clone()),
            (tr("Location"), meta.location.clone()),
        ]);
        self.show(entries);
    }

    /// Show what the PDF at `path` says about itself. `page_size` is the first page's, in points.
    pub fn set_pdf(&self, path: &Path, info: &PdfMetadata, n_pages: usize, page_size: (f64, f64)) {
        let file_size = std::fs::metadata(path).ok().map(|m| m.len());
        let mm = |points: f64| (points * 25.4 / 72.0).round();
        let mut entries = file_entries(path, file_size);
        entries.extend([
            (tr("Title"), info.title.clone()),
            (tr("Author"), info.author.clone()),
            (tr("Subject"), info.subject.clone()),
            (tr("Pages"), Some(n_pages.to_string())),
            (
                tr("Page size"),
                Some(format!("{} × {} mm", mm(page_size.0), mm(page_size.1))),
            ),
            (tr("Created"), info.created.clone()),
            (tr("Made with"), info.creator.clone()),
            (tr("PDF producer"), info.producer.clone()),
            (tr("PDF version"), info.version.clone()),
        ]);
        self.show(entries);
    }

    /// Replace the rows with `entries`, skipping the ones without a value.
    fn show(&self, entries: Vec<(String, Option<String>)>) {
        let mut rows = self.rows.borrow_mut();
        for row in rows.drain(..) {
            self.group.remove(&row);
        }
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

/// Name, folder and size on disk: the rows every kind of file has.
fn file_entries(path: &Path, file_size: Option<u64>) -> Vec<(String, Option<String>)> {
    vec![
        (
            tr("File"),
            path.file_name().map(|n| n.to_string_lossy().into_owned()),
        ),
        (tr("Folder"), path.parent().map(|p| p.display().to_string())),
        (
            tr("Size on disk"),
            file_size.map(|b| glib::format_size(b).to_string()),
        ),
    ]
}
