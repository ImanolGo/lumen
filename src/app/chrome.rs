//! The window around the document: titles, the sidebar, the floating controls, the slideshow,
//! and the preferences' effect on the view.

use super::*;

impl App {
    /// The file name, after a dot while it has unsaved changes.
    pub(super) fn title(&self) -> String {
        match &self.document {
            Some(document) if self.is_dirty() => format!("• {}", document.file_name()),
            Some(document) => document.file_name(),
            None => APP_NAME.to_owned(),
        }
    }

    /// Where the file is among the ones being browsed. The page of a PDF is in the
    /// floating controls, and the size of a picture in its properties.
    pub(super) fn subtitle(&self) -> String {
        if self.loading {
            tr("Loading…")
        } else if self.list.len() > 1 {
            tr_fmt(
                "{index} of {total}",
                &[
                    ("index", &(self.index + 1).to_string()),
                    ("total", &self.list.len().to_string()),
                ],
            )
        } else {
            String::new()
        }
    }

    /// Show a short message at the bottom of the window. With an `action`, the toast has a
    /// button that sends `message` back.
    pub(super) fn toast(&self, text: &str, action: Option<(&str, AppMsg, &ComponentSender<Self>)>) {
        let Some(toasts) = &self.toasts else { return };
        let toast = adw::Toast::new(text);
        toast.set_timeout(5);
        if let Some((label, message, sender)) = action {
            toast.set_button_label(Some(label));
            let (sender, message) = (
                sender.input_sender().clone(),
                std::cell::RefCell::new(Some(message)),
            );
            toast.connect_button_clicked(move |_| {
                if let Some(message) = message.borrow_mut().take() {
                    sender.emit(message);
                }
            });
        }
        toasts.add_toast(toast);
    }

    /// Show the current document in the canvas (a no-op if nothing changed).
    pub(super) fn sync_canvas(&self) {
        let content = match (&self.pdf, self.shown_image()) {
            (Some(pdf), _) => CanvasContent::Pages {
                key: pdf.doc_key,
                layout: &pdf.layout,
            },
            (None, Some(image)) => CanvasContent::Image(image),
            (None, None) => CanvasContent::None,
        };
        self.canvas.set_content(content);
        self.canvas.set_markup_mode(self.markup_canvas_mode());
    }

    /// Whether there is anything to list in the sidebar.
    pub(super) fn sidebar_available(&self) -> bool {
        !self.fullscreen && (self.list.len() > 1 || self.pdf.is_some())
    }

    pub(super) fn sidebar_visible(&self) -> bool {
        self.sidebar_open && self.sidebar_available()
    }

    /// Whether the floating controls over the view are shown: for a moment after
    /// something happens, and while the pointer is on them.
    pub(super) fn controls_shown(&self) -> bool {
        (self.controls_awake || self.controls_hovered > 0)
            && !self.cursor_hidden
            && self.has_document()
    }

    /// Show the floating controls, and hide them again if nothing else happens.
    pub(super) fn wake_controls(&mut self, sender: &ComponentSender<Self>) {
        self.controls_awake = true;
        self.controls_ticket += 1;
        let (ticket, sender) = (self.controls_ticket, sender.clone());
        glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            sender.input(AppMsg::ControlsIdle(ticket));
        });
    }

    pub(super) fn zoom_text(&self) -> String {
        if self.zoom > 0.0 {
            format!("{:.0}%", self.zoom * 100.0)
        } else {
            String::new()
        }
    }

    /// The mode actually shown: the chosen one, unless the document cannot offer it.
    pub(super) fn mode(&self) -> SidebarMode {
        match (&self.pdf, self.sidebar_mode) {
            (None, _) => SidebarMode::Files,
            (Some(pdf), SidebarMode::Outline) if !pdf.has_outline => SidebarMode::Pages,
            (Some(_), SidebarMode::Files) if self.list.len() < 2 => SidebarMode::Pages,
            (Some(_), mode) => mode,
        }
    }

    pub(super) fn sidebar_page_name(&self) -> &'static str {
        match self.mode() {
            SidebarMode::Files => "files",
            SidebarMode::Pages => "pages",
            SidebarMode::Outline => "outline",
        }
    }

    /// Whether a picture or a PDF is open.
    pub(super) fn has_document(&self) -> bool {
        self.image.is_some() || self.pdf.is_some()
    }

    pub(super) fn has_outline(&self) -> bool {
        self.pdf.as_ref().is_some_and(|p| p.has_outline)
    }

    /// Whether more than one sidebar view exists, so a switcher is worth showing.
    pub(super) fn sidebar_switchable(&self) -> bool {
        self.pdf.is_some() && (self.list.len() > 1 || self.has_outline())
    }

    pub(super) fn can_slideshow(&self) -> bool {
        !self.is_dirty()
            && (self.list.len() > 1 || self.pdf.as_ref().is_some_and(|p| p.n_pages > 1))
    }

    pub(super) fn stop_slideshow(&mut self) {
        if let Some(timer) = self.slideshow.take() {
            timer.remove();
        }
    }

    pub(super) fn start_slideshow_timer(&mut self, sender: &ComponentSender<Self>) {
        self.stop_slideshow();
        let seconds = self.store.borrow().settings.slideshow_seconds.max(1);
        let sender = sender.clone();
        self.slideshow = Some(glib::timeout_add_seconds_local(seconds, move || {
            sender.input(AppMsg::SlideshowTick);
            glib::ControlFlow::Continue
        }));
    }

    /// Show the pointer and hide it again if it stays still.
    pub(super) fn restart_cursor_timer(&mut self, sender: &ComponentSender<Self>) {
        if let Some(timer) = self.cursor_timer.take() {
            timer.remove();
        }
        self.canvas.set_cursor_hidden(false);
        self.cursor_hidden = false;
        if self.fullscreen {
            let sender = sender.clone();
            self.cursor_timer = Some(glib::timeout_add_local_once(
                std::time::Duration::from_secs(2),
                move || {
                    sender.input(AppMsg::HideCursor);
                },
            ));
        }
    }

    /// Put the preferences into effect.
    pub(super) fn apply_settings(&self) {
        let store = self.store.borrow();
        self.canvas.set_background(store.settings.background);
        self.canvas.set_smooth_zoom(store.settings.smooth_zoom);
        self.canvas
            .set_start_actual_size(store.settings.default_zoom == DefaultZoom::ActualSize);
        self.canvas
            .set_fit_page(store.settings.pdf_fit == settings::PdfFit::Page);
        self.canvas.set_night(store.settings.night_mode);
    }

    /// Print the current document through the GTK print dialog, or export it to `export_to`.
    pub(super) fn print(&self, export_to: Option<PathBuf>) {
        let job = match (&self.pdf, self.shown_image()) {
            (Some(pdf), _) => print::Job::Pdf {
                source: pdf.source.clone(),
                page_sizes: pdf.layout.rects.iter().map(|r| r.size()).collect(),
                layers: self.markup.store.borrow().pages.layers.clone(),
            },
            (None, Some(image)) => print::Job::Image {
                image: image.clone(),
                layer: self
                    .markup
                    .store
                    .borrow()
                    .pages
                    .layers
                    .get(&0)
                    .cloned()
                    .unwrap_or_default(),
            },
            (None, None) => return,
        };
        let name = self
            .document
            .as_ref()
            .map(DocumentInfo::file_name)
            .unwrap_or_default();
        let parent = relm4::main_application().active_window();
        if let Err(err) = print::print(job, &name, parent.as_ref(), export_to.as_deref()) {
            tracing::warn!("print failed: {err:#}");
            let dialog =
                adw::AlertDialog::new(Some(&tr("Could Not Print")), Some(&format!("{err:#}")));
            dialog.add_response("ok", &tr("_OK"));
            dialog.present(parent.as_ref());
        }
    }
}
