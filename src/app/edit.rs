//! Editing in the main window: the edit stack, crop and adjust modes, saving,
//! and the dialogs that guard unsaved work.

use crate::i18n::{tr, tr_fmt};
use std::{path::PathBuf, sync::Arc};

use image::DynamicImage;
use relm4::{
    ComponentSender,
    adw::{self, prelude::*},
    gtk::{self, gdk, gio, glib},
};

use super::{App, AppMsg};
use crate::{
    edit::{self, Adjustments, EditOp, EditStack},
    image::{ImageDocument, Orientation},
    save::{self, Format, SaveRequest},
};

/// The tool that owns the bottom bar, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    None,
    Crop,
    Adjust,
}

/// Edits made to the image that is open.
pub struct EditSession {
    pub path: PathBuf,
    /// EXIF orientation of the file as stored.
    pub base: Orientation,
    pub stack: EditStack,
    /// The stored pixels, decoded the first time an edit needs them.
    pub stored: Option<Arc<DynamicImage>>,
    /// Identifies the newest render request; older results are dropped.
    pub ticket: u64,
    /// The newest request is a preview of an unapplied tool, not the edits on the stack.
    pub previewing: bool,
}

/// Something the user asked for that must wait until unsaved edits are dealt with.
#[derive(Debug, Clone)]
pub enum Pending {
    Open(Vec<PathBuf>),
    Navigate(isize),
    GoTo(usize),
    Close,
}

/// Pixels of an edited image, ready to become a texture on the main thread.
#[derive(Debug)]
pub struct RenderedEdit {
    width: i32,
    height: i32,
    bytes: glib::Bytes,
}

impl RenderedEdit {
    fn new(image: DynamicImage) -> Self {
        let rgba = image.into_rgba8();
        let (width, height) = (rgba.width() as i32, rgba.height() as i32);
        Self {
            width,
            height,
            bytes: glib::Bytes::from_owned(rgba.into_raw()),
        }
    }

    fn into_texture(self) -> gdk::Texture {
        gdk::MemoryTexture::new(
            self.width,
            self.height,
            gdk::MemoryFormat::R8g8b8a8,
            &self.bytes,
            self.width as usize * 4,
        )
        .upcast()
    }
}

/// What a bottom-bar slider changes.
#[derive(Debug, Clone, Copy)]
pub enum AdjustField {
    Exposure,
    Contrast,
    Saturation,
}

impl App {
    /// The image on screen: the edited version if there is one.
    pub(super) fn shown_image(&self) -> Option<&ImageDocument> {
        self.edited.as_ref().or(self.image.as_ref())
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.edit.as_ref().is_some_and(|e| e.stack.is_dirty()) || self.markup.pdf_dirty
    }

    /// Whether the editing tools apply right now.
    pub(super) fn can_edit(&self) -> bool {
        self.image.is_some()
            && self.pdf.is_none()
            && self.edit.is_some()
            && self.edit_mode == EditMode::None
    }

    pub(super) fn can_undo(&self) -> bool {
        if self.pdf.is_some() {
            return self.markup.pdf_history.can_undo() && !self.page_busy;
        }
        self.edit_mode == EditMode::None && self.edit.as_ref().is_some_and(|e| e.stack.can_undo())
    }

    pub(super) fn can_redo(&self) -> bool {
        if self.pdf.is_some() {
            return self.markup.pdf_history.can_redo() && !self.page_busy;
        }
        self.edit_mode == EditMode::None && self.edit.as_ref().is_some_and(|e| e.stack.can_redo())
    }

    pub(super) fn can_save(&self) -> bool {
        if self.pdf.is_some() {
            return self.markup.pdf_dirty && self.edit_mode == EditMode::None;
        }
        self.can_edit() && self.is_dirty()
    }

    /// The file being shown: the image being edited, or the PDF.
    pub(super) fn document_path(&self) -> Option<PathBuf> {
        match (&self.pdf, &self.edit) {
            (Some(pdf), _) => Some(pdf.original.clone()),
            (None, Some(session)) => Some(session.path.clone()),
            _ => None,
        }
    }

    /// Start a fresh session for a newly opened image (or none, for other documents and
    /// for pictures in a format that cannot be edited).
    pub(super) fn reset_edit_session(&mut self) {
        let editable = self
            .image
            .as_ref()
            .filter(|image| crate::save::can_edit(&image.path));
        self.edit = editable.map(|image| EditSession {
            path: image.path.clone(),
            base: image.orientation,
            stack: EditStack::default(),
            stored: None,
            ticket: 0,
            previewing: false,
        });
        self.edited = None;
        self.edit_mode = EditMode::None;
        self.canvas.end_crop();
        self.dirty.set(false);
    }

    /// Apply an operation on top of the current edits.
    pub(super) fn push_edit(&mut self, op: EditOp, sender: &ComponentSender<App>) {
        if self.edit_mode != EditMode::None {
            return;
        }
        let size = self
            .shown_image()
            .map(|i| i.display_size())
            .map(|(w, h)| (w as f64, h as f64));
        let layer = size.and_then(|size| self.layer_after_geometry(op, size));
        let Some(session) = &mut self.edit else {
            return;
        };
        // Annotations move along with the image, and undo brings them back.
        if let Some(layer) = &layer {
            self.markup
                .store
                .borrow_mut()
                .set_pages(std::collections::BTreeMap::from([(0, (**layer).clone())]));
        }
        session.stack.push_with_layer(op, layer);
        self.refresh_edit(sender, None);
    }

    pub(super) fn undo(&mut self, sender: &ComponentSender<App>) {
        if !self.can_undo() {
            return;
        }
        if self.pdf.is_some() {
            if self.markup.pdf_history.undo().is_some() {
                self.pdf_history_changed(sender);
            }
            return;
        }
        if let Some(session) = &mut self.edit {
            session.stack.undo();
        }
        self.sync_markup_from_history();
        self.refresh_edit(sender, None);
    }

    pub(super) fn redo(&mut self, sender: &ComponentSender<App>) {
        if !self.can_redo() {
            return;
        }
        if self.pdf.is_some() {
            if self.markup.pdf_history.redo().is_some() {
                self.pdf_history_changed(sender);
            }
            return;
        }
        if let Some(session) = &mut self.edit {
            session.stack.redo();
        }
        self.sync_markup_from_history();
        self.refresh_edit(sender, None);
    }

    /// Bring the displayed image in line with the stack. `preview` is an
    /// operation shown on top of it without being part of the stack yet.
    pub(super) fn refresh_edit(&mut self, sender: &ComponentSender<App>, preview: Option<EditOp>) {
        let (Some(session), Some(original)) = (&mut self.edit, &self.image) else {
            return;
        };
        self.dirty.set(session.stack.is_dirty());

        if preview.is_none() {
            if !session.stack.is_dirty() {
                self.edited = None;
                return;
            }
            // Rotations and mirroring need no pixels: just a different orientation.
            if let Some(orientation) = session.stack.orientation_only(session.base) {
                self.edited = Some(ImageDocument {
                    orientation,
                    ..original.clone()
                });
                return;
            }
        }

        session.ticket += 1;
        session.previewing = preview.is_some();
        let ticket = session.ticket;
        let mut ops = session.stack.active().to_vec();
        ops.extend(preview);
        let (path, base, stored) = (session.path.clone(), session.base, session.stored.clone());
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = (|| {
                let pixels = match &stored {
                    Some(pixels) => Arc::clone(pixels),
                    None => Arc::new(
                        crate::image::decode::pixels(&path)
                            .map_err(|e| format!("cannot read the image: {e:#}"))?,
                    ),
                };
                let rendered = edit::render(&pixels, base, &ops);
                Ok((
                    stored.is_none().then_some(pixels),
                    RenderedEdit::new(rendered),
                ))
            })();
            sender.input(AppMsg::EditRendered { ticket, result });
        });
    }

    /// A render finished: show it unless a newer one has been requested since.
    pub(super) fn edit_rendered(
        &mut self,
        ticket: u64,
        result: Result<(Option<Arc<DynamicImage>>, RenderedEdit), String>,
    ) {
        let (Some(session), Some(original)) = (&mut self.edit, &self.image) else {
            return;
        };
        match result {
            Ok((stored, rendered)) => {
                if let Some(stored) = stored {
                    session.stored = Some(stored);
                }
                if ticket == session.ticket {
                    // Edited pixels are a still picture, even if the original moved.
                    self.edited = Some(ImageDocument {
                        texture: rendered.into_texture(),
                        orientation: Orientation::NORMAL,
                        animation: None,
                        vector: false,
                        ..original.clone()
                    });
                }
            }
            Err(message) => {
                tracing::warn!("edit failed: {message}");
                if ticket == session.ticket {
                    // Take back what could not be applied; a preview was never on the stack.
                    if !session.previewing {
                        session.stack.undo();
                    }
                    session.previewing = false;
                    self.adjust = Adjustments::default();
                    self.edit_mode = EditMode::None;
                    self.canvas.end_crop();
                    self.refresh_after_failure();
                    show_error(&tr("Could Not Edit Image"), &message);
                }
            }
        }
    }

    fn refresh_after_failure(&mut self) {
        self.dirty.set(self.is_dirty());
        if !self.is_dirty() {
            self.edited = None;
        }
    }

    // --- modes -------------------------------------------------------------------

    pub(super) fn enter_mode(&mut self, mode: EditMode) {
        if !self.can_edit() {
            return;
        }
        self.edit_mode = mode;
        match mode {
            EditMode::Crop => self.canvas.begin_crop(None),
            EditMode::Adjust => self.adjust = Adjustments::default(),
            EditMode::None => {}
        }
    }

    pub(super) fn apply_mode(&mut self, sender: &ComponentSender<App>) {
        let mode = std::mem::replace(&mut self.edit_mode, EditMode::None);
        match mode {
            EditMode::Crop => {
                let rect = self.canvas.crop_rect();
                self.canvas.end_crop();
                let full = self.shown_image().map(ImageDocument::display_size);
                if let Some(rect) = rect.filter(|r| full != Some((r.w as i32, r.h as i32))) {
                    self.push_edit(EditOp::Crop(rect), sender);
                }
            }
            EditMode::Adjust => {
                let adjustments = std::mem::take(&mut self.adjust);
                if adjustments.is_neutral() {
                    self.refresh_edit(sender, None);
                } else {
                    self.push_edit(EditOp::Adjust(adjustments), sender);
                }
            }
            EditMode::None => {}
        }
    }

    pub(super) fn cancel_mode(&mut self, sender: &ComponentSender<App>) {
        let mode = std::mem::replace(&mut self.edit_mode, EditMode::None);
        match mode {
            EditMode::Crop => self.canvas.end_crop(),
            EditMode::Adjust => {
                self.adjust = Adjustments::default();
                self.refresh_edit(sender, None);
            }
            EditMode::None => {}
        }
    }

    pub(super) fn set_crop_aspect(&mut self, aspect: CropAspect) {
        let shown = self.shown_image().map(ImageDocument::display_size);
        let ratio = match aspect {
            CropAspect::Free => None,
            CropAspect::Original => shown.map(|(w, h)| w as f64 / h as f64),
            CropAspect::Ratio(w, h) => Some(w / h),
        };
        self.canvas.set_crop_aspect(ratio);
    }

    pub(super) fn adjust_field(
        &mut self,
        field: AdjustField,
        value: f32,
        sender: &ComponentSender<App>,
    ) {
        if self.edit_mode != EditMode::Adjust {
            return;
        }
        match field {
            AdjustField::Exposure => self.adjust.exposure = value,
            AdjustField::Contrast => self.adjust.contrast = value,
            AdjustField::Saturation => self.adjust.saturation = value,
        }
        self.refresh_edit(sender, Some(EditOp::Adjust(self.adjust)));
    }

    // --- resize ------------------------------------------------------------------

    pub(super) fn ask_resize(&self, sender: &ComponentSender<App>) {
        if !self.can_edit() {
            return;
        }
        let Some((w, h)) = self.shown_image().map(ImageDocument::display_size) else {
            return;
        };
        resize_dialog(w as u32, h as u32, sender.clone());
    }

    // --- saving ------------------------------------------------------------------

    /// Ctrl+S: write the changes back to the file, after asking once.
    pub(super) fn save(&mut self, sender: &ComponentSender<App>) {
        if !self.can_save() {
            return;
        }
        let Some(path) = self.document_path() else {
            return;
        };
        // A picture in a format Lumen reads but cannot write (GIF, BMP, TIFF) is saved as a copy.
        if self.pdf.is_none() && Format::from_path(&path).is_none() {
            self.save_as(sender);
            return;
        }
        if self.overwrite_ok.contains(&path) {
            self.start_save(
                path.clone(),
                Format::from_path(&path).unwrap_or(Format::Png),
                sender,
            );
        } else {
            let name = file_name(&path);
            let sender = sender.clone();
            confirm_overwrite(&name, move |choice| match choice {
                OverwriteChoice::Replace => sender.input(AppMsg::SaveConfirmed),
                OverwriteChoice::SaveCopy => sender.input(AppMsg::SaveAs),
                OverwriteChoice::Cancel => sender.input(AppMsg::SaveCancelled),
            });
        }
    }

    pub(super) fn save_confirmed(&mut self, sender: &ComponentSender<App>) {
        let Some(path) = self.document_path() else {
            return;
        };
        self.overwrite_ok.insert(path.clone());
        self.start_save(
            path.clone(),
            Format::from_path(&path).unwrap_or(Format::Png),
            sender,
        );
    }

    /// Ctrl+Shift+S: pick a file name and format, then write there.
    pub(super) fn save_as(&mut self, sender: &ComponentSender<App>) {
        let sender = sender.clone();
        if let Some(pdf) = &self.pdf {
            if !self.markup.pdf_dirty {
                return;
            }
            let source = pdf.source.path.clone();
            choose_pdf_destination(&source, move |dest| match dest {
                Some(dest) => sender.input(AppMsg::SaveTo {
                    dest,
                    format: Format::Png,
                }),
                None => sender.input(AppMsg::SaveCancelled),
            });
            return;
        }
        if !self.can_edit() {
            return;
        }
        let Some(session) = &self.edit else { return };
        let (source, base_format) = (
            session.path.clone(),
            Format::from_path(&session.path).unwrap_or(Format::Png),
        );
        let changes_pixels = session.stack.changes_pixels();
        choose_destination(&source, base_format, move |dest| {
            let Some(dest) = dest else {
                sender.input(AppMsg::SaveCancelled);
                return;
            };
            let format = Format::from_path(&dest).unwrap_or(Format::Png);
            if matches!(format, Format::Jpeg { .. }) && changes_pixels {
                let sender = sender.clone();
                ask_quality(move |quality| {
                    sender.input(AppMsg::SaveTo {
                        dest: dest.clone(),
                        format: Format::Jpeg { quality },
                    });
                });
            } else {
                sender.input(AppMsg::SaveTo { dest, format });
            }
        });
    }

    pub(super) fn start_save(
        &mut self,
        dest: PathBuf,
        format: Format,
        sender: &ComponentSender<App>,
    ) {
        // A PDF: write the annotations into a copy of the file.
        if let Some(pdf) = &self.pdf {
            // Rearranged pages are written by lopdf, which cannot encrypt: say so first.
            if pdf.protected && pdf.source.path != pdf.original && !self.unprotected_ok {
                let sender = sender.clone();
                confirm_unprotected(&file_name(&pdf.original), move |agreed| {
                    sender.input(if agreed {
                        AppMsg::SaveUnprotected { dest }
                    } else {
                        AppMsg::SaveCancelled
                    })
                });
                return;
            }
            let source = pdf.source.clone();
            let layers: std::collections::BTreeMap<_, _> = self
                .markup
                .store
                .borrow()
                .pages
                .layers
                .iter()
                .filter(|(_, layer)| !layer.is_empty())
                .map(|(page, layer)| (*page, layer.clone()))
                .collect();
            self.saving = true;
            let sender = sender.clone();
            relm4::spawn_blocking(move || {
                let result = write_pdf(&source, &layers, &dest).map_err(|err| format!("{err:#}"));
                sender.input(AppMsg::Saved { dest, result });
            });
            return;
        }

        let Some(session) = &self.edit else { return };
        let request = SaveRequest {
            source: session.path.clone(),
            dest: dest.clone(),
            base_orientation: session.base,
            edits: session.stack.clone(),
            format,
        };
        self.saving = true;
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = save::save(&request).map_err(|err| format!("{err:#}"));
            sender.input(AppMsg::Saved { dest, result });
        });
    }

    pub(super) fn saved(
        &mut self,
        dest: PathBuf,
        result: Result<(), String>,
        sender: &ComponentSender<App>,
    ) {
        self.saving = false;
        match result {
            Ok(()) => {
                self.overwrite_ok.insert(dest.clone());
                self.reset_edit_session();
                // What was saved is the new clean state: reopening the saved file below
                // must not ask about unsaved changes.
                let saved_state = self.markup.pdf_history.present().clone();
                self.markup.pdf_history = crate::annotate::History::new(saved_state);
                self.markup.pdf_dirty = false;
                match self.after_save.take() {
                    // The user was on their way somewhere else.
                    Some(pending) => {
                        self.control_finished(super::control::Wait::Save, Ok(()));
                        sender.input(AppMsg::Replay(pending));
                    }
                    // The saved file is opened again; a call answers once it is.
                    None => {
                        self.control_saved_reopens();
                        sender.input(AppMsg::ShowSaved(dest));
                    }
                }
            }
            Err(message) => {
                self.after_save = None;
                self.control_finished(super::control::Wait::Save, Err(message.clone()));
                show_error(&tr("Could Not Save"), &message);
            }
        }
    }

    // --- guarding unsaved work ---------------------------------------------------

    /// Returns true if `pending` may go ahead; otherwise asks what to do with the
    /// unsaved edits and replays `pending` afterwards if the user agrees.
    pub(super) fn guard(&self, pending: Pending, sender: &ComponentSender<App>) -> bool {
        if !self.is_dirty() {
            return true;
        }
        let name = self
            .document_path()
            .map(|p| file_name(&p))
            .unwrap_or_default();
        let sender = sender.clone();
        ask_unsaved(&name, move |choice| match choice {
            UnsavedChoice::Save => sender.input(AppMsg::SaveThen(pending.clone())),
            UnsavedChoice::Discard => sender.input(AppMsg::Discard(pending.clone())),
            UnsavedChoice::Cancel => {}
        });
        false
    }

    pub(super) fn discard(&mut self, pending: Pending, sender: &ComponentSender<App>) {
        self.reset_edit_session();
        if self.pdf.is_some() {
            self.reset_markup();
        } else {
            self.load_markup(None);
        }
        sender.input(AppMsg::Replay(pending));
    }

    pub(super) fn replay(&mut self, pending: Pending, sender: &ComponentSender<App>) {
        match pending {
            Pending::Open(paths) => sender.input(AppMsg::Open(paths)),
            Pending::Navigate(delta) => sender.input(AppMsg::Navigate(delta)),
            Pending::GoTo(index) => sender.input(AppMsg::GoTo(index)),
            Pending::Close => {
                if let Some(window) = relm4::main_application().active_window() {
                    window.close();
                }
            }
        }
    }
}

/// Aspect ratios offered while cropping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CropAspect {
    Free,
    Original,
    Ratio(f64, f64),
}

impl CropAspect {
    /// The choices in the order of the drop-down.
    pub const CHOICES: [(&'static str, CropAspect); 6] = [
        ("Free", Self::Free),
        ("Original", Self::Original),
        ("1:1", Self::Ratio(1.0, 1.0)),
        ("4:3", Self::Ratio(4.0, 3.0)),
        ("3:2", Self::Ratio(3.0, 2.0)),
        ("16:9", Self::Ratio(16.0, 9.0)),
    ];
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// --- dialogs ---------------------------------------------------------------------

pub(super) fn main_window() -> Option<gtk::Window> {
    relm4::main_application().active_window()
}

pub(super) fn show_error(heading: &str, message: &str) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(message));
    dialog.add_response("ok", &tr("_OK"));
    dialog.present(main_window().as_ref());
}

#[derive(Debug, Clone, Copy)]
enum UnsavedChoice {
    Save,
    Discard,
    Cancel,
}

fn ask_unsaved(name: &str, then: impl FnOnce(UnsavedChoice) + 'static) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Save Changes?")),
        Some(&tr_fmt(
            "“{name}” has edits that have not been saved. They will be lost if you discard them.",
            &[("name", name)],
        )),
    );
    crate::i18n::add_responses(
        &dialog,
        &[
            ("cancel", crate::i18n::n_("_Cancel")),
            ("discard", crate::i18n::n_("_Discard")),
            ("save", crate::i18n::n_("_Save")),
        ],
    );
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    dialog.choose(
        main_window().as_ref(),
        gio::Cancellable::NONE,
        move |response| {
            then(match response.as_str() {
                "save" => UnsavedChoice::Save,
                "discard" => UnsavedChoice::Discard,
                _ => UnsavedChoice::Cancel,
            });
        },
    );
}

#[derive(Debug, Clone, Copy)]
enum OverwriteChoice {
    Replace,
    SaveCopy,
    Cancel,
}

/// Warn that a rearranged, password-protected PDF will be saved without its password.
fn confirm_unprotected(name: &str, then: impl FnOnce(bool) + 'static) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Save Without Password?")),
        Some(&tr_fmt(
            "“{name}” is protected with a password. After its pages have been rearranged, Lumen can only save it without the password, so anyone will be able to open the saved file.",
            &[("name", name)],
        )),
    );
    crate::i18n::add_responses(
        &dialog,
        &[
            ("cancel", crate::i18n::n_("_Cancel")),
            ("save", crate::i18n::n_("Save _Without Password")),
        ],
    );
    dialog.set_response_appearance("save", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.choose(
        main_window().as_ref(),
        gio::Cancellable::NONE,
        move |response| then(response == "save"),
    );
}

fn confirm_overwrite(name: &str, then: impl FnOnce(OverwriteChoice) + 'static) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Replace the Original?")),
        Some(&tr_fmt(
            "Saving writes your edits to “{name}”. You can also keep the original and save a copy.",
            &[("name", name)],
        )),
    );
    crate::i18n::add_responses(
        &dialog,
        &[
            ("cancel", crate::i18n::n_("_Cancel")),
            ("copy", crate::i18n::n_("Save as _Copy…")),
            ("replace", crate::i18n::n_("_Replace")),
        ],
    );
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("copy"));
    dialog.set_close_response("cancel");
    dialog.choose(
        main_window().as_ref(),
        gio::Cancellable::NONE,
        move |response| {
            then(match response.as_str() {
                "replace" => OverwriteChoice::Replace,
                "copy" => OverwriteChoice::SaveCopy,
                _ => OverwriteChoice::Cancel,
            });
        },
    );
}

/// Write annotations into a copy of the PDF next to `dest`, then move it into place.
fn write_pdf(
    source: &crate::pdf::PdfSource,
    layers: &std::collections::BTreeMap<usize, crate::annotate::AnnotationLayer>,
    dest: &std::path::Path,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let name = dest
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = dest.with_file_name(format!(".{name}.lumen-tmp.pdf"));
    let written = crate::pdf::annots::write(source, layers, &temp);
    match written {
        Ok(()) => crate::fsutil::replace_file(&temp, dest)
            .with_context(|| format!("cannot replace {}", dest.display())),
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            Err(err)
        }
    }
}

fn choose_pdf_destination(source: &std::path::Path, then: impl FnOnce(Option<PathBuf>) + 'static) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&tr("PDF Documents")));
    filter.add_mime_type("application/pdf");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dialog = gtk::FileDialog::builder()
        .title(tr("Save As"))
        .modal(true)
        .filters(&filters)
        .initial_name(format!("{stem}-annotated.pdf"))
        .build();
    if let Some(folder) = source.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    dialog.save(
        main_window().as_ref(),
        gio::Cancellable::NONE,
        move |result| {
            then(result.ok().and_then(|file| file.path()));
        },
    );
}

fn choose_destination(
    source: &std::path::Path,
    format: Format,
    then: impl FnOnce(Option<PathBuf>) + 'static,
) {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    for (name, mime) in [
        ("PNG", "image/png"),
        ("JPEG", "image/jpeg"),
        ("WebP", "image/webp"),
    ] {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(name));
        filter.add_mime_type(mime);
        filters.append(&filter);
    }
    let dialog = gtk::FileDialog::builder()
        .title(tr("Save As"))
        .modal(true)
        .filters(&filters)
        .initial_name(save::suggested_name(source, format))
        .build();
    if let Some(folder) = source.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    dialog.save(
        main_window().as_ref(),
        gio::Cancellable::NONE,
        move |result| {
            then(result.ok().and_then(|file| file.path()));
        },
    );
}

fn ask_quality(then: impl FnOnce(u8) + 'static) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("JPEG Quality")),
        Some(&tr("Higher quality makes a larger file.")),
    );
    crate::i18n::add_responses(
        &dialog,
        &[
            ("cancel", crate::i18n::n_("_Cancel")),
            ("save", crate::i18n::n_("_Save")),
        ],
    );
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 50.0, 100.0, 1.0);
    scale.set_value(save::DEFAULT_JPEG_QUALITY as f64);
    scale.set_draw_value(true);
    scale.set_width_request(260);
    dialog.set_extra_child(Some(&scale));
    dialog.choose(
        main_window().as_ref(),
        gio::Cancellable::NONE,
        move |response| {
            if response == "save" {
                then(scale.value().round() as u8);
            }
        },
    );
}

fn resize_dialog(width: u32, height: u32, sender: ComponentSender<App>) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Resize Image")),
        Some(&tr_fmt(
            "Currently {width} × {height} px.",
            &[
                ("width", &width.to_string()),
                ("height", &height.to_string()),
            ],
        )),
    );
    crate::i18n::add_responses(
        &dialog,
        &[
            ("cancel", crate::i18n::n_("_Cancel")),
            ("resize", crate::i18n::n_("_Resize")),
        ],
    );
    dialog.set_response_appearance("resize", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("resize"));
    dialog.set_close_response("cancel");

    let spin = |value: f64, max: f64| {
        let spin = gtk::SpinButton::with_range(1.0, max, 1.0);
        spin.set_value(value);
        spin.set_activates_default(true);
        spin
    };
    let w = spin(width as f64, 30_000.0);
    let h = spin(height as f64, 30_000.0);
    let percent = spin(100.0, 1_000.0);
    let keep = gtk::CheckButton::with_label(&tr("Keep aspect ratio"));
    keep.set_active(true);

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(12)
        .halign(gtk::Align::Center)
        .build();
    for (row, (label, widget)) in [
        (tr("Width (px)"), &w),
        (tr("Height (px)"), &h),
        (tr("Scale (%)"), &percent),
    ]
    .into_iter()
    .enumerate()
    {
        let label = gtk::Label::builder().label(&label).xalign(1.0).build();
        grid.attach(&label, 0, row as i32, 1, 1);
        grid.attach(widget, 1, row as i32, 1, 1);
    }
    grid.attach(&keep, 1, 3, 1, 1);
    dialog.set_extra_child(Some(&grid));

    // The three fields describe one size; changing one updates the others.
    let ratio = width as f64 / height as f64;
    let syncing = std::rc::Rc::new(std::cell::Cell::new(false));
    let sync = |syncing: &std::rc::Rc<std::cell::Cell<bool>>, f: &dyn Fn()| {
        if !syncing.replace(true) {
            f();
            syncing.set(false);
        }
    };
    w.connect_value_changed({
        let (h, percent, keep, syncing) =
            (h.clone(), percent.clone(), keep.clone(), syncing.clone());
        move |w| {
            let syncing2 = syncing.clone();
            sync(&syncing2, &|| {
                if keep.is_active() {
                    h.set_value((w.value() / ratio).round().max(1.0));
                }
                percent.set_value((w.value() / width as f64 * 100.0).round().max(1.0));
            });
        }
    });
    h.connect_value_changed({
        let (w, percent, keep, syncing) =
            (w.clone(), percent.clone(), keep.clone(), syncing.clone());
        move |h| {
            let syncing2 = syncing.clone();
            sync(&syncing2, &|| {
                if keep.is_active() {
                    w.set_value((h.value() * ratio).round().max(1.0));
                }
                percent.set_value((h.value() / height as f64 * 100.0).round().max(1.0));
            });
        }
    });
    percent.connect_value_changed({
        let (w, h, syncing) = (w.clone(), h.clone(), syncing.clone());
        move |p| {
            let syncing2 = syncing.clone();
            sync(&syncing2, &|| {
                w.set_value((width as f64 * p.value() / 100.0).round().max(1.0));
                h.set_value((height as f64 * p.value() / 100.0).round().max(1.0));
            });
        }
    });

    dialog.choose(
        main_window().as_ref(),
        gio::Cancellable::NONE,
        move |response| {
            if response == "resize" {
                let (w, h) = (w.value() as u32, h.value() as u32);
                if (w, h) != (width, height) {
                    sender.input(AppMsg::Edit(EditOp::Resize { w, h }));
                }
            }
        },
    );
}
