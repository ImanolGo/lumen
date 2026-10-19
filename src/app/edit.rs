//! Editing in the main window: the edit stack, crop and adjust modes, saving,
//! and the dialogs that guard unsaved work.

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
        self.edit.as_ref().is_some_and(|e| e.stack.is_dirty())
    }

    /// Whether the editing tools apply right now.
    pub(super) fn can_edit(&self) -> bool {
        self.image.is_some() && self.pdf.is_none() && self.edit.is_some() && self.edit_mode == EditMode::None
    }

    pub(super) fn can_undo(&self) -> bool {
        self.edit_mode == EditMode::None && self.edit.as_ref().is_some_and(|e| e.stack.can_undo())
    }

    pub(super) fn can_redo(&self) -> bool {
        self.edit_mode == EditMode::None && self.edit.as_ref().is_some_and(|e| e.stack.can_redo())
    }

    pub(super) fn can_save(&self) -> bool {
        self.can_edit() && self.is_dirty()
    }

    /// Start a fresh session for a newly opened image (or none, for other documents).
    pub(super) fn reset_edit_session(&mut self) {
        self.edit = self.image.as_ref().map(|image| EditSession {
            path: image.path.clone(),
            base: image.orientation,
            stack: EditStack::default(),
            stored: None,
            ticket: 0,
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
        let Some(session) = &mut self.edit else { return };
        session.stack.push(op);
        self.refresh_edit(sender, None);
    }

    pub(super) fn undo(&mut self, sender: &ComponentSender<App>) {
        if self.can_undo() {
            if let Some(session) = &mut self.edit {
                session.stack.undo();
            }
            self.refresh_edit(sender, None);
        }
    }

    pub(super) fn redo(&mut self, sender: &ComponentSender<App>) {
        if self.can_redo() {
            if let Some(session) = &mut self.edit {
                session.stack.redo();
            }
            self.refresh_edit(sender, None);
        }
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
        let ticket = session.ticket;
        let mut ops = session.stack.active().to_vec();
        ops.extend(preview);
        let (path, base, stored) = (session.path.clone(), session.base, session.stored.clone());
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = (|| {
                let pixels = match &stored {
                    Some(pixels) => Arc::clone(pixels),
                    None => Arc::new(image::open(&path).map_err(|e| format!("cannot read the image: {e}"))?),
                };
                let rendered = edit::render(&pixels, base, &ops);
                Ok((stored.is_none().then_some(pixels), RenderedEdit::new(rendered)))
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
                    self.edited = Some(ImageDocument {
                        texture: rendered.into_texture(),
                        orientation: Orientation::NORMAL,
                        ..original.clone()
                    });
                }
            }
            Err(message) => {
                tracing::warn!("edit failed: {message}");
                if ticket == session.ticket {
                    // Take back what could not be applied.
                    session.stack.undo();
                    self.edit_mode = EditMode::None;
                    self.canvas.end_crop();
                    self.refresh_after_failure();
                    show_error("Could Not Edit Image", &message);
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

    pub(super) fn adjust_field(&mut self, field: AdjustField, value: f32, sender: &ComponentSender<App>) {
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
        let Some(session) = &self.edit else { return };
        let path = session.path.clone();
        if self.overwrite_ok.contains(&path) {
            let format = Format::from_path(&path).unwrap_or(Format::Png);
            self.start_save(path, format, sender);
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
        let Some(session) = &self.edit else { return };
        let path = session.path.clone();
        self.overwrite_ok.insert(path.clone());
        let format = Format::from_path(&path).unwrap_or(Format::Png);
        self.start_save(path, format, sender);
    }

    /// Ctrl+Shift+S: pick a file name and format, then write there.
    pub(super) fn save_as(&mut self, sender: &ComponentSender<App>) {
        if !self.can_edit() {
            return;
        }
        let Some(session) = &self.edit else { return };
        let (source, base_format) = (session.path.clone(), Format::from_path(&session.path).unwrap_or(Format::Png));
        let changes_pixels = session.stack.changes_pixels();
        let sender = sender.clone();
        choose_destination(&source, base_format, move |dest| {
            let Some(dest) = dest else {
                sender.input(AppMsg::SaveCancelled);
                return;
            };
            let format = Format::from_path(&dest).unwrap_or(Format::Png);
            if matches!(format, Format::Jpeg { .. }) && changes_pixels {
                let sender = sender.clone();
                ask_quality(move |quality| {
                    sender.input(AppMsg::SaveTo { dest: dest.clone(), format: Format::Jpeg { quality } });
                });
            } else {
                sender.input(AppMsg::SaveTo { dest, format });
            }
        });
    }

    pub(super) fn start_save(&mut self, dest: PathBuf, format: Format, sender: &ComponentSender<App>) {
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

    pub(super) fn saved(&mut self, dest: PathBuf, result: Result<(), String>, sender: &ComponentSender<App>) {
        self.saving = false;
        match result {
            Ok(()) => {
                self.overwrite_ok.insert(dest.clone());
                self.reset_edit_session();
                match self.after_save.take() {
                    // The user was on their way somewhere else.
                    Some(pending) => sender.input(AppMsg::Replay(pending)),
                    None => sender.input(AppMsg::Open(vec![dest])),
                }
            }
            Err(message) => {
                self.after_save = None;
                show_error("Could Not Save", &message);
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
        let name = self.edit.as_ref().map(|e| file_name(&e.path)).unwrap_or_default();
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
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

// --- dialogs ---------------------------------------------------------------------

fn main_window() -> Option<gtk::Window> {
    relm4::main_application().active_window()
}

fn show_error(heading: &str, message: &str) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(message));
    dialog.add_response("ok", "_OK");
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
        Some("Save Changes?"),
        Some(&format!("“{name}” has edits that have not been saved. They will be lost if you discard them.")),
    );
    dialog.add_responses(&[("cancel", "_Cancel"), ("discard", "_Discard"), ("save", "_Save")]);
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    dialog.choose(main_window().as_ref(), gio::Cancellable::NONE, move |response| {
        then(match response.as_str() {
            "save" => UnsavedChoice::Save,
            "discard" => UnsavedChoice::Discard,
            _ => UnsavedChoice::Cancel,
        });
    });
}

#[derive(Debug, Clone, Copy)]
enum OverwriteChoice {
    Replace,
    SaveCopy,
    Cancel,
}

fn confirm_overwrite(name: &str, then: impl FnOnce(OverwriteChoice) + 'static) {
    let dialog = adw::AlertDialog::new(
        Some("Replace the Original?"),
        Some(&format!("Saving writes your edits to “{name}”. You can also keep the original and save a copy.")),
    );
    dialog.add_responses(&[("cancel", "_Cancel"), ("copy", "Save as _Copy…"), ("replace", "_Replace")]);
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("copy"));
    dialog.set_close_response("cancel");
    dialog.choose(main_window().as_ref(), gio::Cancellable::NONE, move |response| {
        then(match response.as_str() {
            "replace" => OverwriteChoice::Replace,
            "copy" => OverwriteChoice::SaveCopy,
            _ => OverwriteChoice::Cancel,
        });
    });
}

fn choose_destination(source: &std::path::Path, format: Format, then: impl FnOnce(Option<PathBuf>) + 'static) {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    for (name, mime) in [("PNG", "image/png"), ("JPEG", "image/jpeg"), ("WebP", "image/webp")] {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(name));
        filter.add_mime_type(mime);
        filters.append(&filter);
    }
    let dialog = gtk::FileDialog::builder()
        .title("Save As")
        .modal(true)
        .filters(&filters)
        .initial_name(save::suggested_name(source, format))
        .build();
    if let Some(folder) = source.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    dialog.save(main_window().as_ref(), gio::Cancellable::NONE, move |result| {
        then(result.ok().and_then(|file| file.path()));
    });
}

fn ask_quality(then: impl FnOnce(u8) + 'static) {
    let dialog = adw::AlertDialog::new(Some("JPEG Quality"), Some("Higher quality makes a larger file."));
    dialog.add_responses(&[("cancel", "_Cancel"), ("save", "_Save")]);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 50.0, 100.0, 1.0);
    scale.set_value(save::DEFAULT_JPEG_QUALITY as f64);
    scale.set_draw_value(true);
    scale.set_width_request(260);
    dialog.set_extra_child(Some(&scale));
    dialog.choose(main_window().as_ref(), gio::Cancellable::NONE, move |response| {
        if response == "save" {
            then(scale.value().round() as u8);
        }
    });
}

fn resize_dialog(width: u32, height: u32, sender: ComponentSender<App>) {
    let dialog = adw::AlertDialog::new(Some("Resize Image"), Some(&format!("Currently {width} × {height} px.")));
    dialog.add_responses(&[("cancel", "_Cancel"), ("resize", "_Resize")]);
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
    let keep = gtk::CheckButton::with_label("Keep aspect ratio");
    keep.set_active(true);

    let grid = gtk::Grid::builder().row_spacing(8).column_spacing(12).halign(gtk::Align::Center).build();
    for (row, (label, widget)) in [("Width (px)", &w), ("Height (px)", &h), ("Scale (%)", &percent)].into_iter().enumerate() {
        let label = gtk::Label::builder().label(label).xalign(1.0).build();
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
        let (h, percent, keep, syncing) = (h.clone(), percent.clone(), keep.clone(), syncing.clone());
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
        let (w, percent, keep, syncing) = (w.clone(), percent.clone(), keep.clone(), syncing.clone());
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

    dialog.choose(main_window().as_ref(), gio::Cancellable::NONE, move |response| {
        if response == "resize" {
            let (w, h) = (w.value() as u32, h.value() as u32);
            if (w, h) != (width, height) {
                sender.input(AppMsg::Edit(EditOp::Resize { w, h }));
            }
        }
    });
}
