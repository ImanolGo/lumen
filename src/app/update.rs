//! What each message does to the window: the body of `App::update`.

use super::*;

impl App {
    pub(super) fn update_inner(&mut self, msg: AppMsg, sender: ComponentSender<Self>) {
        match msg {
            AppMsg::OpenDialog => Self::show_open_dialog(sender),
            AppMsg::Open(paths) => {
                if !self.guard(Pending::Open(paths.clone()), &sender) {
                    return;
                }
                // Only what was opened on purpose is a recent file, not what is flipped past.
                let recent = gtk::RecentManager::default();
                for path in paths.iter().filter(|p| !p.is_dir()) {
                    recent.add_item(&gio::File::for_path(path).uri());
                }
                refresh_recent_menu(&self.recent_menu);
                refresh_recent_row(&self.recent_row, &sender);
                // One picture opened on its own: its folder is listed once it is on screen.
                self.browse_after = match paths.as_slice() {
                    [one]
                        if self.store.borrow().settings.browse_folder
                            && !one.is_dir()
                            && folder::is_picture(one) =>
                    {
                        Some(one.clone())
                    }
                    _ => None,
                };
                self.list_files(paths, false, &sender);
            }
            AppMsg::Add(paths) => {
                let add = !self.list.is_empty();
                self.list_files(paths, add, &sender);
            }
            AppMsg::Listed {
                listing,
                files,
                add,
            } => {
                if listing != self.listing {
                    return;
                }
                if add {
                    // Show the first file that was not there yet; `GoTo` deals with unsaved edits.
                    let mut new: Vec<PathBuf> = files
                        .into_iter()
                        .filter(|f| !self.list.contains(f))
                        .collect();
                    new.dedup();
                    if new.is_empty() {
                        return;
                    }
                    let first = self.list.len();
                    self.list.extend(new);
                    self.refresh_strip();
                    sender.input(AppMsg::GoTo(first));
                } else if files.is_empty() {
                    self.fail(tr("There are no pictures or PDFs to open there."));
                } else {
                    // Files picked together are shown in name order.
                    let mut files = files;
                    files.sort_by(|a, b| folder::natural_cmp(a, b));
                    files.dedup();
                    self.cache.clear();
                    self.list = files;
                    self.refresh_strip();
                    self.show_index(0, &sender);
                    if let Some(picture) = self.browse_after.take()
                        && self.list == [picture.clone()]
                    {
                        let (listing, sender) = (self.listing, sender.clone());
                        relm4::spawn_blocking(move || {
                            let siblings = picture
                                .parent()
                                .map(folder::pictures_in)
                                .unwrap_or_default();
                            sender.input(AppMsg::Siblings {
                                listing,
                                picture,
                                siblings,
                            });
                        });
                    }
                }
            }
            AppMsg::ShowSaved(dest) => {
                self.cache.retain(|(p, _)| *p != dest);
                let index = match self.list.iter().position(|f| *f == dest) {
                    Some(index) => index,
                    None => {
                        let at = (self.index + 1).min(self.list.len());
                        self.list.insert(at, dest);
                        self.refresh_strip();
                        at
                    }
                };
                self.show_index(index, &sender);
            }
            AppMsg::Trash => self.trash_current(&sender),
            AppMsg::ShowInFolder => self.hand_over(true),
            AppMsg::OpenWith => self.hand_over(false),
            AppMsg::Trashed {
                path,
                index,
                result,
            } => self.trashed(path, index, result, &sender),
            AppMsg::Untrash(path, index) => self.untrash(path, index, &sender),
            AppMsg::Untrashed {
                path,
                index,
                result,
            } => match result {
                Ok(()) => {
                    let at = index.min(self.list.len());
                    self.list.insert(at, path);
                    self.refresh_strip();
                    self.show_index(at, &sender);
                }
                Err(message) => edit::show_error(&tr("Could Not Restore the File"), &message),
            },
            AppMsg::FileTouched => {
                self.watch_ticket += 1;
                let (ticket, sender) = (self.watch_ticket, sender.clone());
                glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                    sender.input(AppMsg::FileChanged { ticket });
                });
            }
            AppMsg::FileChanged { ticket } => {
                if ticket == self.watch_ticket {
                    self.file_changed(&sender);
                }
            }
            AppMsg::Palette => {
                if let Some(window) = relm4::main_application()
                    .active_window()
                    .and_downcast::<adw::ApplicationWindow>()
                {
                    shortcuts::show_palette(&window);
                }
            }
            AppMsg::JumpBack => self.jump_along(true),
            AppMsg::JumpForward => self.jump_along(false),
            AppMsg::Siblings {
                listing,
                picture,
                siblings,
            } => {
                // Only if nothing else was opened or added meanwhile.
                if listing == self.listing && self.list == [picture.clone()] {
                    let Some(index) = siblings.iter().position(|p| *p == picture) else {
                        return;
                    };
                    self.list = siblings;
                    self.index = index;
                    self.refresh_strip();
                    self.strip.select(index);
                    self.preload_neighbours(&sender);
                }
            }
            AppMsg::SaveState => {
                self.state_save_pending = false;
                self.store.borrow().save_state();
            }
            AppMsg::ToolKey(tool) => {
                if self.markup.open && self.edit_mode == EditMode::None {
                    self.set_tool(tool);
                }
            }
            AppMsg::Screen(direction) => {
                let at_end = self.pdf.is_some() && !self.canvas.scroll_screen(direction as i32);
                if self.pdf.is_none() || at_end {
                    sender.input(AppMsg::Navigate(direction));
                }
            }
            AppMsg::Edge(last) => {
                if self.pdf.is_some() {
                    self.goto_page(if last { usize::MAX } else { 0 });
                    self.wake_controls(&sender);
                } else if !self.list.is_empty() {
                    let target = if last { self.list.len() - 1 } else { 0 };
                    sender.input(AppMsg::GoTo(target));
                }
            }
            AppMsg::Navigate(delta) => {
                // With an annotation selected, the arrow keys move it instead.
                if self.markup_nudge(delta.signum() as f64, 0.0, &sender) {
                    return;
                }
                if !self.guard(Pending::Navigate(delta), &sender) {
                    return;
                }
                let wrap = self.store.borrow().settings.wrap_around && self.list.len() > 1;
                let target = if wrap {
                    (self.index as isize + delta).rem_euclid(self.list.len() as isize) as usize
                } else {
                    self.index
                        .saturating_add_signed(delta)
                        .min(self.list.len().saturating_sub(1))
                };
                if !self.list.is_empty() && target != self.index {
                    self.show_index(target, &sender);
                }
            }
            AppMsg::GoTo(index) => {
                if index != self.index && !self.guard(Pending::GoTo(index), &sender) {
                    return;
                }
                if index != self.index && index < self.list.len() {
                    self.show_index(index, &sender);
                }
            }
            AppMsg::Loaded { generation, result } => {
                if generation != self.generation {
                    return;
                }
                match result {
                    Ok(opened) => self.apply(opened, &sender),
                    Err(err) => self.open_failed(err, &sender),
                }
            }
            AppMsg::Preloaded { path, result } => {
                self.preloading.remove(&path);
                let wanted = self.awaiting.as_ref() == Some(&path);
                match result {
                    Ok(opened) if wanted => {
                        self.awaiting = None;
                        self.apply(opened, &sender);
                    }
                    Ok(opened) => {
                        let near = self
                            .list
                            .iter()
                            .position(|f| *f == path)
                            .is_some_and(|i| i.abs_diff(self.index) <= PRELOAD_RADIUS);
                        if near {
                            self.cache.push((path, opened));
                        }
                    }
                    Err(err) if wanted => {
                        self.awaiting = None;
                        self.open_failed(err, &sender);
                    }
                    Err(_) => {}
                }
            }
            AppMsg::Thumbnail(path, data) => {
                self.strip.set_thumbnail(&path, &data.into_texture());
            }
            AppMsg::FileStripChanged => {
                // Rows came into view: thumbnail them, replacing whatever was still waiting.
                self.thumbnails.request(self.strip.wanted());
            }
            AppMsg::ViewChanged(info) => {
                let zoom_changed =
                    (info.zoom - self.zoom).abs() > 1e-4 || info.scale_factor != self.scale_factor;
                self.zoom = info.zoom;
                self.scale_factor = info.scale_factor;
                if !self.first_view_logged && (self.image.is_some() || self.pdf.is_some()) {
                    self.first_view_logged = true;
                    if let Some(started) = crate::STARTED.get() {
                        tracing::info!(
                            ms = started.elapsed().as_millis() as u64,
                            "first view on screen"
                        );
                    }
                }
                if let Some(page) = self.pending_goto.take() {
                    self.canvas.scroll_to_page(page);
                }
                self.schedule_sharp(&sender);
                let page_changed = self
                    .pdf
                    .as_ref()
                    .is_some_and(|p| p.page != info.current_page);
                if zoom_changed || page_changed {
                    self.wake_controls(&sender);
                }
                if let Some(pdf) = &mut self.pdf {
                    // `goto_page` has already moved `pdf.page`, so compare with what was remembered.
                    let mut store = self.store.borrow_mut();
                    if store.state.last_page(&pdf.original).unwrap_or(0) != info.current_page {
                        store.state.set_last_page(&pdf.original, info.current_page);
                    }
                    drop(store);
                    pdf.page = info.current_page;
                    self.page_strip.set_current(info.current_page);
                    pdf.visible = (info.first_visible, info.last_visible);
                    let delay = if zoom_changed {
                        RERENDER_DELAY_MS
                    } else {
                        SCROLL_RENDER_DELAY_MS
                    };
                    self.schedule_render(delay, &sender);
                    // Rectangles are only computed near the view, so refresh them when it moves.
                    self.request_selection(false);
                    self.request_links();
                }
            }
            AppMsg::RenderSharp { ticket } => self.render_sharp(ticket, &sender),
            AppMsg::SharpRendered {
                ticket,
                area,
                result,
            } => {
                if ticket == self.sharp_ticket {
                    match result {
                        Ok(tile) => self.canvas.set_sharp(Some((tile, area))),
                        Err(err) => tracing::warn!("cannot render the drawing: {err}"),
                    }
                }
            }
            AppMsg::RenderPages { ticket } => {
                if ticket == self.render_ticket {
                    self.render_visible();
                }
            }
            AppMsg::PageRendered {
                doc_key,
                job,
                result,
            } => {
                let Some(pdf) = &mut self.pdf else { return };
                if doc_key != pdf.doc_key {
                    return;
                }
                match result {
                    Ok(rendered) => {
                        let texture = rendered.into_texture();
                        let (first, last) = pdf.visible;
                        let protect = first.saturating_sub(RENDER_AHEAD)..=last + RENDER_AHEAD;
                        for evicted in pdf.cache.insert(job.page, &texture, job.scale, protect) {
                            self.canvas.set_page_texture(evicted, None);
                        }
                        self.canvas.set_page_texture(job.page, Some(texture));
                    }
                    Err(err) => tracing::warn!(page = job.page, "render failed: {err}"),
                }
            }
            AppMsg::SidebarMode(mode) => self.sidebar_mode = mode,
            AppMsg::GotoTarget(target) => {
                if self.pdf.is_some() {
                    self.jumped_from_here(target.page, &sender);
                    self.canvas.scroll_to_point(target.page, target.y);
                }
            }
            AppMsg::PageStripChanged => {
                let Some(pdf) = &self.pdf else { return };
                let jobs = self.page_strip.wanted().into_iter().map(|page| {
                    let size = pdf.layout.rects[page].size();
                    let scale = THUMB_WIDTH / size.0 * self.scale_factor as f64;
                    RenderJob {
                        page,
                        scale: pdf::effective_scale(size, scale),
                    }
                });
                pdf.thumb_renderer.request(jobs);
            }
            AppMsg::ThumbRendered {
                doc_key,
                job,
                result,
            } => {
                let Some(pdf) = &self.pdf else { return };
                if let (true, Ok(rendered)) = (doc_key == pdf.doc_key, result) {
                    let texture = rendered.into_texture();
                    self.canvas.set_page_stand_in(job.page, texture.clone());
                    self.page_strip.set_thumbnail(job.page, texture);
                }
            }
            AppMsg::PageBy(delta) => {
                if let Some(pdf) = &self.pdf {
                    self.goto_page(pdf.page.saturating_add_signed(delta));
                    self.wake_controls(&sender);
                }
            }
            AppMsg::GotoPage(page) => {
                self.goto_page(page);
                self.wake_controls(&sender);
            }
            AppMsg::GotoPageText(text) => {
                if let Ok(page) = text.trim().parse::<usize>() {
                    self.goto_page(page.saturating_sub(1));
                }
                // Back to the view, so the entry shows the page that was reached.
                self.canvas.grab_focus();
            }
            AppMsg::Select(gesture) => {
                if self.pdf.is_none() {
                    return;
                }
                match gesture {
                    SelectGesture::Start { point, clicks } => {
                        let granularity = match clicks {
                            1 => Granularity::Glyph,
                            2 => Granularity::Word,
                            _ => Granularity::Line,
                        };
                        self.selection = Some(Selection {
                            anchor: point,
                            head: point,
                            granularity,
                        });
                        self.request_selection(false);
                    }
                    SelectGesture::Extend(point) => {
                        if let Some(selection) = &mut self.selection {
                            selection.head = point;
                            self.request_selection(false);
                        }
                    }
                    SelectGesture::Form { page, field } => {
                        self.clear_selection();
                        self.form_clicked(page, field, &sender);
                    }
                    SelectGesture::OpenLink(target) => {
                        self.clear_selection();
                        self.follow_link(target, &sender);
                    }
                    SelectGesture::End { moved } => {
                        // With the highlighter, a dragged selection becomes a highlight once measured.
                        if self.markup_canvas_mode() == crate::widgets::MarkupMode::TextSelect {
                            if moved {
                                self.markup.highlight_pending = true;
                                self.request_selection(false);
                            }
                            return;
                        }
                        // A plain single click just deselects; double and triple clicks keep their word or line.
                        if !moved
                            && self
                                .selection
                                .is_some_and(|s| s.granularity == Granularity::Glyph)
                        {
                            self.clear_selection();
                        }
                    }
                }
            }
            AppMsg::SelectionResult { doc_key, result } => {
                let Some(pdf) = &self.pdf else { return };
                if doc_key != pdf.doc_key {
                    return;
                }
                if result.id == self.selection_id {
                    if self.markup.highlight_pending {
                        let rects = result.rects.clone();
                        self.highlight_selection(&rects, &sender);
                    } else {
                        self.canvas.set_selection(result.rects);
                    }
                }
                if let (Some(text), Some(display)) = (
                    result.text.filter(|t| !t.is_empty()),
                    gdk::Display::default(),
                ) {
                    display.clipboard().set_text(&text);
                }
            }
            AppMsg::LinksLoaded { doc_key, loaded } => {
                let Some(pdf) = &mut self.pdf else { return };
                if doc_key == pdf.doc_key {
                    pdf.links_loaded.insert(loaded.page);
                    self.canvas.set_links(loaded.page, loaded.links);
                }
            }
            AppMsg::Copy => {
                if self.pdf.is_some() {
                    self.request_selection(true);
                } else {
                    self.copy_picture();
                }
            }
            AppMsg::FillForm { id, value } => self.fill_form(id, value, &sender),
            AppMsg::Control { id, method, params } => self.control(id, &method, params, &sender),
            AppMsg::PlaceSignature(index) => self.place_signature(index, &sender),
            AppMsg::NewSignature => self.new_signature(&sender),
            AppMsg::SignatureDrawn(strokes) => self.signature_drawn(strokes, &sender),
            AppMsg::DeleteSignature(index) => self.delete_signature(index, &sender),
            AppMsg::FormFilled { out, result } => self.form_filled(out, result, &sender),
            AppMsg::Edit(op) => {
                // On a PDF the rotate commands turn pages.
                if self.pdf.is_some() {
                    match op {
                        EditOp::RotateCw => self.rotate_pages(90, &sender),
                        EditOp::RotateCcw => self.rotate_pages(-90, &sender),
                        _ => {}
                    }
                } else {
                    self.push_edit(op, &sender);
                }
            }
            AppMsg::Undo => self.undo(&sender),
            AppMsg::Redo => self.redo(&sender),
            AppMsg::EnterMode(mode) => {
                self.enter_mode(mode);
                if mode == EditMode::Crop {
                    self.set_crop_aspect(CropAspect::CHOICES[self.crop_choice].1);
                }
            }
            AppMsg::Confirm => self.apply_mode(&sender),
            AppMsg::Escape => {
                if self.edit_mode != EditMode::None {
                    self.cancel_mode(&sender);
                } else if !self.markup_deselect() {
                    if self.selection.is_some() {
                        self.clear_selection();
                    } else if self.fullscreen || self.slideshow.is_some() {
                        sender.input(AppMsg::LeavePresentation);
                    }
                }
            }
            AppMsg::ToggleFullscreen => {
                if let Some(window) = relm4::main_application().active_window() {
                    if window.is_fullscreen() {
                        window.unfullscreen();
                    } else {
                        window.fullscreen();
                    }
                }
            }
            AppMsg::FullscreenChanged(fullscreen) => {
                self.fullscreen = fullscreen;
                if !fullscreen {
                    self.stop_slideshow();
                }
                self.restart_cursor_timer(&sender);
            }
            AppMsg::ToggleSlideshow => {
                if self.slideshow.is_some() {
                    self.stop_slideshow();
                    if let Some(window) = relm4::main_application().active_window() {
                        window.unfullscreen();
                    }
                } else if self.can_slideshow() {
                    if let Some(window) = relm4::main_application().active_window() {
                        window.fullscreen();
                    }
                    self.start_slideshow_timer(&sender);
                }
            }
            AppMsg::SlideshowTick => {
                if self.is_dirty() {
                    self.stop_slideshow();
                } else if let Some(pdf) = self.pdf.as_ref().filter(|p| p.page + 1 < p.n_pages) {
                    sender.input(AppMsg::GotoPage(pdf.page + 1));
                } else if self.index + 1 < self.list.len() {
                    sender.input(AppMsg::Navigate(1));
                } else {
                    // The last picture has had its turn.
                    self.stop_slideshow();
                }
            }
            AppMsg::PointerMoved => {
                self.wake_controls(&sender);
                if self.fullscreen {
                    self.restart_cursor_timer(&sender);
                }
            }
            AppMsg::HideCursor => {
                if self.fullscreen {
                    self.canvas.set_cursor_hidden(true);
                    self.cursor_hidden = true;
                }
            }
            AppMsg::LeavePresentation => {
                self.stop_slideshow();
                if let Some(window) = relm4::main_application().active_window() {
                    window.unfullscreen();
                }
            }
            AppMsg::Preferences => {
                let sender = sender.clone();
                prefs::show_preferences(&self.store.borrow().settings, move |settings| {
                    sender.input(AppMsg::SettingsChanged(settings));
                });
            }
            AppMsg::SetPdfFit(fit) => {
                self.store.borrow_mut().settings.pdf_fit = fit;
                self.store.borrow().save_settings();
                self.apply_settings();
            }
            AppMsg::SetNightMode(on) => {
                self.store.borrow_mut().settings.night_mode = on;
                self.store.borrow().save_settings();
                self.apply_settings();
            }
            AppMsg::SettingsChanged(settings) => {
                self.store.borrow_mut().settings = settings;
                self.store.borrow().save_settings();
                self.apply_settings();
                if self.slideshow.is_some() {
                    self.start_slideshow_timer(&sender);
                }
            }
            AppMsg::About => prefs::show_about(),
            AppMsg::Shortcuts => shortcuts::show(),
            AppMsg::PageRequest(request) => self.page_request(request, &sender),
            AppMsg::PageOpDone { out, goto, result } => {
                self.page_op_done(out, goto, result, &sender)
            }
            AppMsg::PdfReloaded {
                generation,
                goto,
                path,
                result,
            } => self.pdf_reloaded(generation, goto, path, result, &sender),
            AppMsg::InsertFiles { paths, at } => self.insert_files(paths, at, &sender),
            AppMsg::InsertDialog => self.insert_dialog(&sender),
            AppMsg::DeletePages => self.delete_pages(&sender),
            AppMsg::ExportPagesDialog => self.export_pages_dialog(&sender),
            AppMsg::ExportPagesTo { dest, pages } => self.export_pages(dest, pages, &sender),
            AppMsg::ExportPngDialog => self.export_png_dialog(&sender),
            AppMsg::ExportPngTo { dest } => self.export_png(dest, &sender),
            AppMsg::ExportImagePdfDialog => self.export_image_pdf_dialog(&sender),
            AppMsg::ExportImagePdfTo { dest } => self.export_image_pdf(dest, &sender),
            AppMsg::Exported(result) => self.exported(result),
            AppMsg::Markup(event) => self.markup_event(event, &sender),
            AppMsg::ToggleMarkup => self.toggle_markup(),
            AppMsg::SetTool(tool) => self.set_tool(tool),
            AppMsg::SetColor(color) => {
                // Highlights keep a colour of their own, so choosing one never recolours the pen.
                if self.markup.editor.tool == Tool::Highlight {
                    self.markup.editor.highlight_color = color;
                    self.set_markup_style(|_| {}, &sender);
                } else {
                    self.set_markup_style(|style| style.stroke = color, &sender);
                }
            }
            AppMsg::SetSize(size) => {
                if self.markup.editor.tool == Tool::Text {
                    self.set_markup_style(|style| style.font_size = size, &sender)
                } else {
                    self.set_markup_style(|style| style.width = size, &sender)
                }
            }
            AppMsg::CustomColor => {
                let current = self.markup_color();
                let parent = relm4::main_application().active_window();
                let sender = sender.clone();
                gtk::ColorDialog::new().choose_rgba(
                    parent.as_ref(),
                    Some(&gdk::RGBA::new(current.r, current.g, current.b, current.a)),
                    gio::Cancellable::NONE,
                    move |result| {
                        // Cancelling is reported as an error; nothing to do then.
                        if let Ok(c) = result {
                            sender.input(AppMsg::SetColor(annotate::Color::rgba(
                                c.red(),
                                c.green(),
                                c.blue(),
                                c.alpha(),
                            )));
                        }
                    },
                );
            }
            AppMsg::MarkupDelete => self.markup_delete(&sender),
            AppMsg::MarkupNudge(dx, dy) => {
                self.markup_nudge(dx, dy, &sender);
            }
            AppMsg::MarkupText { page, id, text } => self.markup_text(page, id, &text, &sender),
            AppMsg::CropAspect(index) => {
                self.crop_choice = index.min(CropAspect::CHOICES.len() - 1);
                if self.edit_mode == EditMode::Crop {
                    self.set_crop_aspect(CropAspect::CHOICES[self.crop_choice].1);
                }
            }
            AppMsg::AdjustField(field, value) => self.adjust_field(field, value, &sender),
            AppMsg::ResizeDialog => self.ask_resize(&sender),
            AppMsg::EditRendered { ticket, result } => self.edit_rendered(ticket, result, &sender),
            AppMsg::Save => self.save(&sender),
            AppMsg::SaveConfirmed => self.save_confirmed(&sender),
            AppMsg::SaveCancelled => self.after_save = None,
            AppMsg::SaveAs => self.save_as(&sender),
            AppMsg::SaveTo { dest, format } => self.start_save(dest, format, &sender),
            AppMsg::SaveUnprotected { dest } => {
                self.unprotected_ok = true;
                self.start_save(dest, Format::Png, &sender);
            }
            AppMsg::Saved { dest, result } => self.saved(dest, result, &sender),
            AppMsg::CloseRequested => {
                if self.guard(Pending::Close, &sender) {
                    self.replay(Pending::Close, &sender);
                }
            }
            AppMsg::SaveThen(pending) => {
                // A tool that is still open would hide the edits from the save.
                if self.edit_mode != EditMode::None {
                    self.cancel_mode(&sender);
                }
                self.after_save = Some(pending);
                self.save(&sender);
            }
            AppMsg::Discard(pending) => self.discard(pending, &sender),
            AppMsg::Replay(pending) => self.replay(pending, &sender),
            AppMsg::SearchChanged(text) => {
                self.search_text = text;
                self.search_ticket += 1;
                if self.search_text.is_empty() {
                    self.restart_search();
                } else {
                    // Wait for a pause in typing, so each keystroke does not restart the search.
                    let (ticket, sender) = (self.search_ticket, sender.clone());
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(SEARCH_DELAY_MS),
                        move || sender.input(AppMsg::SearchTyped { ticket }),
                    );
                }
            }
            AppMsg::SearchOptions(options) => {
                self.search_options = options;
                self.restart_search();
            }
            AppMsg::SearchTyped { ticket } => {
                if ticket == self.search_ticket {
                    self.restart_search();
                }
            }
            AppMsg::SearchOpened(open) => {
                self.search_open = open;
                // Closing the bar drops the highlights; reopening searches again.
                self.restart_search();
            }
            AppMsg::Search { doc_key, event } => {
                let Some(pdf) = &self.pdf else { return };
                if doc_key != pdf.doc_key {
                    return;
                }
                match event {
                    SearchEvent::Page { id, page, hits } if id == self.search_id => {
                        self.canvas.add_highlights(page, hits.clone());
                        self.hits.insert(page, hits);
                        // Jump to the first hit found, which is the first one from the viewed page on.
                        if self.current_hit.is_none() {
                            self.select_hit((page, 0));
                        }
                    }
                    SearchEvent::Done { id } if id == self.search_id => {
                        self.searching = false;
                        self.search_finished();
                    }
                    _ => {}
                }
            }
            AppMsg::SearchStep(step) => {
                let positions = self.hit_positions();
                if positions.is_empty() {
                    return;
                }
                let current = self
                    .current_hit
                    .and_then(|c| positions.iter().position(|&p| p == c));
                let next = match current {
                    Some(i) => (i as isize + step).rem_euclid(positions.len() as isize) as usize,
                    None => 0,
                };
                self.select_hit(positions[next]);
            }
            AppMsg::OpenWithPassword(password) => {
                let Some(path) = self.list.get(self.index).cloned() else {
                    return;
                };
                self.generation += 1;
                self.loading = true;
                let generation = self.generation;
                relm4::spawn_blocking(move || {
                    let result = open_logged(&path, Some(&password));
                    sender.input(AppMsg::Loaded { generation, result });
                });
            }
            AppMsg::PasswordCancelled => {
                self.fail(OpenError::PasswordRequired { retry: false }.message())
            }
            AppMsg::Print(export_to) => self.print(export_to),
            AppMsg::ToggleSidebar => {
                self.sidebar_open = !self.sidebar_open;
                self.store.borrow_mut().state.sidebar_open = self.sidebar_open;
            }
            AppMsg::ShowSidebar(show) => {
                self.sidebar_open = show;
                self.store.borrow_mut().state.sidebar_open = show;
            }
            AppMsg::ToggleProperties => self.properties_open = !self.properties_open,
            AppMsg::Hover(true) => self.wake_controls(&sender),
            AppMsg::Hover(false) => self.controls_awake = false,
            AppMsg::ControlsHover(true) => self.controls_hovered += 1,
            AppMsg::ControlsHover(false) => {
                self.controls_hovered = self.controls_hovered.saturating_sub(1)
            }
            AppMsg::ControlsIdle(ticket) => {
                if ticket == self.controls_ticket {
                    self.controls_awake = false;
                }
            }
        }
    }
}
