//! Opening and browsing files: listing, loading, preloading, the files sidebar, the trash,
//! and watching the open file for changes.

use super::*;

impl App {
    /// Work out the files behind `paths` off the main thread (folders are listed), then
    /// open them, or add them to the ones being browsed.
    pub(super) fn list_files(
        &mut self,
        paths: Vec<PathBuf>,
        add: bool,
        sender: &ComponentSender<Self>,
    ) {
        self.listing += 1;
        let listing = self.listing;
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            // Files picked on purpose are opened even if their type is unknown, so the
            // error says why; dropped extras that are not pictures or PDFs are skipped.
            let files = folder::expand(&paths, add);
            sender.input(AppMsg::Listed {
                listing,
                files,
                add,
            });
        });
    }

    /// Start showing `list[index]`: from the cache, a pending preload, or a fresh load.
    pub(super) fn show_index(&mut self, index: usize, sender: &ComponentSender<Self>) {
        let Some(path) = self.list.get(index).cloned() else {
            return;
        };
        self.index = index;
        self.generation += 1;
        self.awaiting = None;
        self.strip.select(index);

        if let Some(pos) = self.cache.iter().position(|(p, _)| *p == path) {
            self.canvas.set_placeholder(None);
            let (_, opened) = self.cache.swap_remove(pos);
            self.apply(opened, sender);
        } else if self.preloading.contains(&path) {
            self.loading = true;
            self.awaiting = Some(path.clone());
            self.canvas
                .set_placeholder(crate::thumbnails::cached(&path));
        } else {
            self.canvas
                .set_placeholder(crate::thumbnails::cached(&path));
            self.loading = true;
            self.load_started = Some(std::time::Instant::now());
            let generation = self.generation;
            let sender = sender.clone();
            relm4::spawn_blocking(move || {
                let result = open_logged(&path, None);
                sender.input(AppMsg::Loaded { generation, result });
            });
        }
    }

    /// Decode the files around the current one and drop the ones that are now far away.
    pub(super) fn preload_neighbours(&mut self, sender: &ComponentSender<Self>) {
        let (index, len) = (self.index, self.list.len());
        let near = |i: usize| i.abs_diff(index) <= PRELOAD_RADIUS;
        let list = &self.list;
        self.cache
            .retain(|(p, _)| list.iter().position(|f| f == p).is_some_and(near));

        let lo = index.saturating_sub(PRELOAD_RADIUS);
        let hi = (index + PRELOAD_RADIUS).min(len.saturating_sub(1));
        for i in (lo..=hi).filter(|&i| i != index) {
            let path = self.list[i].clone();
            if self.preloading.contains(&path) || self.cache.iter().any(|(p, _)| *p == path) {
                continue;
            }
            self.preloading.insert(path.clone());
            let sender = sender.clone();
            relm4::spawn_blocking(move || {
                let result = open_logged(&path, None);
                sender.input(AppMsg::Preloaded { path, result });
            });
        }
    }

    /// Show the error page, or ask for the password if that is what is missing.
    pub(super) fn open_failed(&mut self, err: OpenError, sender: &ComponentSender<Self>) {
        match err {
            // Opened for a call: use the password it gave, and never show a dialog.
            OpenError::PasswordRequired { retry } if self.control_loading() => {
                match self.control_password().filter(|_| !retry) {
                    Some(password) => sender.input(AppMsg::OpenWithPassword(password)),
                    None => self.fail(if retry {
                        tr("The password is incorrect.")
                    } else {
                        tr("The PDF is protected; pass its password.")
                    }),
                }
            }
            OpenError::PasswordRequired { retry } => {
                self.loading = false;
                let name = self
                    .list
                    .get(self.index)
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ask_password(&name, retry, sender.clone());
            }
            other => self.fail(other.message()),
        }
    }

    pub(super) fn fail(&mut self, err: String) {
        self.loading = false;
        self.document = None;
        self.image = None;
        self.pdf = None;
        self.reset_edit_session();
        self.load_markup(None);
        self.page_strip.clear();
        self.properties.set_image(None);
        self.control_finished(control::Wait::Load, Err(err.clone()));
        self.error = Some(err);
        self.sync_canvas();
    }

    /// Show the empty window: no file is open.
    pub(super) fn clear_document(&mut self) {
        self.fail(String::new());
        self.error = None;
        self.watcher = None;
    }

    /// Make a decoded file the current document.
    pub(super) fn apply(&mut self, opened: Opened, sender: &ComponentSender<Self>) {
        let info = opened.info;
        let took = self.load_started.take().map(|t| t.elapsed().as_millis());
        tracing::info!(path = %info.path.display(), kind = ?info.kind, load_ms = ?took, "opened");
        self.loading = false;
        self.error = None;
        self.image = opened.image;
        self.reset_edit_session();
        match &opened.pdf {
            Some(pdf) => {
                self.properties
                    .set_pdf(&info.path, &pdf.metadata, pdf.n_pages, pdf.page_sizes[0])
            }
            None => self.properties.set_image(self.image.as_ref()),
        }
        let opened_outline = opened.pdf.as_ref().map(|p| p.outline.clone());
        let opened_annotations = opened.pdf.as_ref().map(|p| p.annotations.clone());
        let opened_forms = opened.pdf.as_ref().map(|p| p.forms.clone());
        self.pdf = opened
            .pdf
            .as_ref()
            .map(|pdf_info| self.make_pdf_view(&info.path, info.path.clone(), pdf_info, sender));
        match &self.pdf {
            Some(pdf) => self.page_strip.set_document(
                &pdf.layout
                    .rects
                    .iter()
                    .map(|r| r.size())
                    .collect::<Vec<_>>(),
            ),
            None => self.page_strip.clear(),
        }
        match &opened_outline {
            Some(items) => self.outline.set_outline(items),
            None => self.outline.set_outline(&[]),
        }
        self.sidebar_mode = if self.pdf.is_some() {
            SidebarMode::Pages
        } else {
            SidebarMode::Files
        };
        self.jump_back.clear();
        self.jump_forward.clear();
        self.watch_file(&info.path, sender);
        self.load_markup(opened_annotations);
        self.unprotected_ok = false;
        self.pending_goto = None;
        if self.store.borrow().settings.remember_state
            && let Some(pdf) = &self.pdf
        {
            let saved = self.store.borrow().state.last_page(&pdf.original);
            self.pending_goto = saved.filter(|&p| p < pdf.n_pages);
        }
        self.save_state_soon(sender);
        self.document = Some(info);
        self.sync_canvas();
        if let Some(forms) = &opened_forms {
            self.canvas.set_forms(forms);
        }
        self.clear_selection();
        self.restart_search();
        self.preload_neighbours(sender);
        self.wake_controls(sender);
        self.control_finished(control::Wait::Load, Ok(()));
    }

    /// The viewer state for a PDF shown from `path` (a working copy of `original`).
    pub(super) fn make_pdf_view(
        &mut self,
        path: &std::path::Path,
        original: PathBuf,
        pdf_info: &pdf::PdfInfo,
        sender: &ComponentSender<Self>,
    ) -> PdfView {
        self.next_key += 1;
        let doc_key = self.next_key;
        let layout = Rc::new(PageLayout::new(&pdf_info.page_sizes));
        let input = sender.input_sender().clone();
        let source = PdfSource::new(path, pdf_info.password.as_deref());
        PdfView {
            original,
            protected: pdf_info.password.is_some(),
            source: source.clone(),
            n_pages: pdf_info.n_pages,
            has_outline: !pdf_info.outline.is_empty(),
            layout,
            page: 0,
            doc_key,
            visible: (0, 0),
            cache: PageCache::new(PAGE_CACHE_BYTES),
            renderer: PageRenderer::new(&source, {
                let input = input.clone();
                move |job, result| {
                    input.emit(AppMsg::PageRendered {
                        doc_key,
                        job,
                        result,
                    })
                }
            }),
            thumb_renderer: PageRenderer::new(&source, {
                let input = input.clone();
                move |job, result| {
                    input.emit(AppMsg::ThumbRendered {
                        doc_key,
                        job,
                        result,
                    })
                }
            }),
            search: SearchWorker::new(&source, {
                let input = input.clone();
                move |event| input.emit(AppMsg::Search { doc_key, event })
            }),
            selector: SelectionWorker::new(&source, {
                let input = input.clone();
                move |result| input.emit(AppMsg::SelectionResult { doc_key, result })
            }),
            links: LinkWorker::new(&source, pdf_info.page_sizes.clone(), move |loaded| {
                input.emit(AppMsg::LinksLoaded { doc_key, loaded })
            }),
            links_loaded: HashSet::new(),
        }
    }

    /// Rebuild the file sidebar. Thumbnails are made for the rows that are on screen,
    /// as they appear (`FileStripChanged`).
    pub(super) fn refresh_strip(&self) {
        self.strip.set_files(&self.list, self.index);
    }

    pub(super) fn show_open_dialog(sender: ComponentSender<Self>) {
        let images = gtk::FileFilter::new();
        images.set_name(Some(&tr("Images")));
        // Any picture: glycin decides what it can read.
        images.add_mime_type("image/*");

        let pdfs = gtk::FileFilter::new();
        pdfs.set_name(Some(&tr("PDF Documents")));
        pdfs.add_mime_type("application/pdf");

        let all = gtk::FileFilter::new();
        all.set_name(Some(&tr("Images and PDFs")));
        all.add_mime_type("image/*");
        all.add_mime_type("application/pdf");

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&all);
        filters.append(&images);
        filters.append(&pdfs);

        let dialog = gtk::FileDialog::builder()
            .title(tr("Open"))
            .modal(true)
            .filters(&filters)
            .default_filter(&all)
            .build();

        let parent = relm4::main_application().active_window();
        dialog.open_multiple(parent.as_ref(), gio::Cancellable::NONE, move |result| {
            // Cancelling the dialog is reported as an error; nothing to do then.
            let Ok(files) = result else { return };
            let paths: Vec<PathBuf> = files
                .iter::<gio::File>()
                .flatten()
                .filter_map(|f| f.path())
                .collect();
            if !paths.is_empty() {
                sender.input(AppMsg::Open(paths));
            }
        });
    }

    /// Move the file on screen to the trash, and offer to bring it back.
    pub(super) fn trash_current(&mut self, sender: &ComponentSender<Self>) {
        if self.is_dirty() || self.loading || self.page_busy || self.saving {
            return;
        }
        let Some(path) = self.list.get(self.index).cloned() else {
            return;
        };
        let (index, sender) = (self.index, sender.input_sender().clone());
        relm4::spawn_blocking(move || {
            let result = gio::File::for_path(&path)
                .trash(gio::Cancellable::NONE)
                .map_err(|err| err.to_string());
            sender.emit(AppMsg::Trashed {
                path,
                index,
                result,
            });
        });
    }

    pub(super) fn trashed(
        &mut self,
        path: PathBuf,
        index: usize,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Err(message) = result {
            edit::show_error(&tr("Could Not Move to Trash"), &message);
            return;
        }
        self.cache.retain(|(p, _)| *p != path);
        let Some(position) = self.list.iter().position(|p| *p == path) else {
            return;
        };
        self.list.remove(position);
        self.refresh_strip();
        if self.list.is_empty() {
            self.clear_document();
        } else {
            self.show_index(position.min(self.list.len() - 1), sender);
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        self.toast(
            &tr_fmt(
                "“{name}” moved to the trash",
                &[("name", &name.unwrap_or_default())],
            ),
            Some((&tr("Undo"), AppMsg::Untrash(path, index), sender)),
        );
    }

    /// Bring a file back from the trash and show it again.
    pub(super) fn untrash(&mut self, path: PathBuf, index: usize, sender: &ComponentSender<Self>) {
        let (sender, wanted) = (sender.input_sender().clone(), path);
        relm4::spawn_blocking(move || {
            let result = restore_from_trash(&wanted);
            sender.emit(AppMsg::Untrashed {
                path: wanted,
                index,
                result,
            });
        });
    }

    /// Put the picture on screen on the clipboard: its pixels (as edited), and the file
    /// itself too when it has no unsaved edits, so a file manager can paste it.
    pub(super) fn copy_picture(&self) {
        let (Some(image), Some(display)) = (self.shown_image(), gdk::Display::default()) else {
            return;
        };
        let mut providers = vec![gdk::ContentProvider::for_value(&image.texture.to_value())];
        if self.edited.is_none() {
            let list = gdk::FileList::from_array(&[gio::File::for_path(&image.path)]);
            providers.push(gdk::ContentProvider::for_value(&list.to_value()));
        }
        let _ = display
            .clipboard()
            .set_content(Some(&gdk::ContentProvider::new_union(&providers)));
        self.toast(&tr("Picture copied"), None);
    }

    /// Give the file on screen to the file manager (`show_folder`) or to a program the
    /// user picks.
    pub(super) fn hand_over(&self, show_folder: bool) {
        let Some(path) = self.list.get(self.index) else {
            return;
        };
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
        let parent = relm4::main_application().active_window();
        let log = |result: Result<(), glib::Error>| {
            if let Err(err) = result {
                tracing::warn!("could not hand the file over: {err}");
            }
        };
        if show_folder {
            launcher.open_containing_folder(parent.as_ref(), gio::Cancellable::NONE, log);
        } else {
            launcher.set_always_ask(true);
            launcher.launch(parent.as_ref(), gio::Cancellable::NONE, log);
        }
    }

    /// Watch the file being shown. When it changes on disk (a document being rebuilt, a
    /// picture saved by another program) it is shown again.
    pub(super) fn watch_file(&mut self, path: &std::path::Path, sender: &ComponentSender<Self>) {
        self.watcher = None;
        let file = gio::File::for_path(path);
        let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            return;
        };
        let input = sender.input_sender().clone();
        monitor.connect_changed(move |_, _, _, event| {
            use gio::FileMonitorEvent as E;
            if matches!(
                event,
                E::Changed | E::ChangesDoneHint | E::Created | E::MovedIn | E::Renamed
            ) {
                input.emit(AppMsg::FileTouched);
            }
        });
        self.watcher = Some(monitor);
    }

    /// The open file changed on disk: show it again, or offer to if there are unsaved edits.
    pub(super) fn file_changed(&mut self, sender: &ComponentSender<Self>) {
        let recently_saved = self
            .last_save
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3));
        if self.saving || recently_saved || self.loading || self.page_busy {
            return;
        }
        let Some(path) = self.list.get(self.index).cloned() else {
            return;
        };
        if self.is_dirty() {
            self.toast(
                &tr("This file changed on disk"),
                Some((
                    &tr("Reload"),
                    AppMsg::Discard(Pending::Reload(path)),
                    sender,
                )),
            );
        } else {
            sender.input(AppMsg::ShowSaved(path));
        }
    }

    /// Write the remembered state a little later, once, however many files are shown meanwhile.
    pub(super) fn save_state_soon(&mut self, sender: &ComponentSender<Self>) {
        if self.state_save_pending {
            return;
        }
        self.state_save_pending = true;
        let sender = sender.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
            sender.input(AppMsg::SaveState);
        });
    }

    pub(super) fn can_go_prev(&self) -> bool {
        self.index > 0 || self.wraps()
    }

    pub(super) fn can_go_next(&self) -> bool {
        self.index + 1 < self.list.len() || self.wraps()
    }

    pub(super) fn wraps(&self) -> bool {
        self.list.len() > 1 && self.store.borrow().settings.wrap_around
    }

    pub(super) fn status_icon(&self) -> &'static str {
        match (&self.error, &self.document) {
            (Some(_), _) => "dialog-error-symbolic",
            (None, Some(d)) if d.kind == DocumentKind::Pdf => "x-office-document-symbolic",
            (None, Some(_)) => "image-x-generic-symbolic",
            (None, None) => crate::APP_ID,
        }
    }

    pub(super) fn status_title(&self) -> String {
        match (&self.error, &self.document) {
            (Some(_), _) => tr("Could Not Open File"),
            (None, Some(d)) => d.file_name(),
            (None, None) => tr("Open a Picture or PDF"),
        }
    }

    pub(super) fn status_description(&self) -> String {
        match (&self.error, &self.document) {
            (Some(err), _) => err.clone(),
            (None, Some(d)) => d.summary.clone(),
            (None, None) => tr("Drop a file here, or press Ctrl+O"),
        }
    }

    pub(super) fn page(&self) -> &'static str {
        if self.image.is_some() || self.pdf.is_some() {
            "image"
        } else {
            "empty"
        }
    }
}
