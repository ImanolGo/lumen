//! Reading a PDF: rendering the pages in view, links and jumps, text selection and search.

use super::*;

impl App {
    pub(super) fn page_text(&self) -> String {
        self.pdf
            .as_ref()
            .map(|p| (p.page + 1).to_string())
            .unwrap_or_default()
    }

    pub(super) fn page_total(&self) -> String {
        self.pdf
            .as_ref()
            .map(|p| tr_fmt("of {total}", &[("total", &p.n_pages.to_string())]))
            .unwrap_or_default()
    }

    pub(super) fn goto_page(&mut self, page: usize) {
        let Some(pdf) = &mut self.pdf else { return };
        pdf.page = page.min(pdf.n_pages.saturating_sub(1));
        self.canvas.scroll_to_page(pdf.page);
    }

    /// Ask the renderer for the pages that are on screen (and next to it) and
    /// not yet drawn at the current zoom.
    pub(super) fn render_visible(&mut self) {
        let Some(pdf) = &mut self.pdf else { return };
        if self.zoom <= 0.0 {
            return;
        }
        let wanted_scale = self.zoom * self.scale_factor as f64;
        let (first, last) = pdf.visible;
        let lo = first.saturating_sub(RENDER_AHEAD);
        let hi = (last + RENDER_AHEAD).min(pdf.n_pages - 1);

        // Pages in view first, nearest to the current one first; read-ahead after.
        let mut pages: Vec<usize> = (lo..=hi).collect();
        pages.sort_by_key(|&p| {
            let in_view = (first..=last).contains(&p);
            (!in_view, p.abs_diff(pdf.page))
        });

        let mut jobs = Vec::new();
        for page in pages {
            pdf.cache.touch(page);
            let scale = pdf::effective_scale(pdf.layout.rects[page].size(), wanted_scale);
            let up_to_date = pdf
                .cache
                .scale(page)
                .is_some_and(|s| (s / scale - 1.0).abs() < 0.02);
            if !up_to_date {
                jobs.push(RenderJob { page, scale });
            }
        }
        pdf.renderer.request(jobs);
    }

    /// The drawing (SVG) on screen, unless it has been edited into pixels.
    pub(super) fn shown_drawing(&self) -> Option<&crate::image::ImageDocument> {
        self.image
            .as_ref()
            .filter(|i| i.vector && self.pdf.is_none() && self.edited.is_none())
    }

    /// A drawing is shown sharp at any zoom: once the view settles, the part in view is
    /// rendered again at that zoom (see `render_sharp`).
    pub(super) fn schedule_sharp(&mut self, sender: &ComponentSender<Self>) {
        self.sharp_ticket += 1;
        if self.shown_drawing().is_none() {
            return;
        }
        let (ticket, sender) = (self.sharp_ticket, sender.clone());
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(RERENDER_DELAY_MS),
            move || sender.input(AppMsg::RenderSharp { ticket }),
        );
    }

    pub(super) fn render_sharp(&mut self, ticket: u64, sender: &ComponentSender<Self>) {
        if ticket != self.sharp_ticket {
            return;
        }
        let Some(drawing) = self.shown_drawing() else {
            return;
        };
        let scale = self.zoom * self.scale_factor as f64;
        // At its natural size or smaller, the decoded pixels are already exact.
        if scale <= 1.001 {
            self.canvas.set_sharp(None);
            return;
        }
        let Some([x, y, w, h]) = self.canvas.visible_image_area(0.25) else {
            return;
        };
        let clip = (
            (x * scale).floor() as u32,
            (y * scale).floor() as u32,
            (w * scale).ceil() as u32,
            (h * scale).ceil() as u32,
        );
        // The area the whole pixels of the clip cover, in the drawing's own pixels.
        let area = [
            clip.0 as f64 / scale,
            clip.1 as f64 / scale,
            clip.2 as f64 / scale,
            clip.3 as f64 / scale,
        ];
        let (path, sender) = (drawing.path.clone(), sender.clone());
        relm4::spawn_blocking(move || {
            let result = crate::image::decode::render_region(&path, scale, clip)
                .map_err(|e| format!("{e:#}"));
            sender.input(AppMsg::SharpRendered {
                ticket,
                area,
                result,
            });
        });
    }

    /// Re-run `render_visible` once `delay_ms` has passed without a newer request.
    pub(super) fn schedule_render(&mut self, delay_ms: u64, sender: &ComponentSender<Self>) {
        self.render_ticket += 1;
        let ticket = self.render_ticket;
        let sender = sender.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay_ms), move || {
            sender.input(AppMsg::RenderPages { ticket });
        });
    }

    /// Read the links of the pages in and next to the view, once per page.
    pub(super) fn request_links(&mut self) {
        let Some(pdf) = &self.pdf else { return };
        let (first, last) = pdf.visible;
        let wanted =
            first.saturating_sub(RENDER_AHEAD)..=(last + RENDER_AHEAD).min(pdf.n_pages - 1);
        pdf.links
            .request(wanted.filter(|p| !pdf.links_loaded.contains(p)));
    }

    /// Remember where the reader is before a jump (a link or the outline), so Alt+Left can
    /// come back, and say so.
    pub(super) fn jumped_from_here(&mut self, page: usize, sender: &ComponentSender<Self>) {
        let Some(position) = self.canvas.scroll_position() else {
            return;
        };
        self.jump_back.push(position);
        self.jump_forward.clear();
        self.toast(
            &tr_fmt("Went to page {page}", &[("page", &(page + 1).to_string())]),
            Some((&tr("Back"), AppMsg::JumpBack, sender)),
        );
    }

    /// Go back (or forward) along the jumps made with links and the outline.
    pub(super) fn jump_along(&mut self, back: bool) {
        let Some(here) = self.canvas.scroll_position() else {
            return;
        };
        let (from, to) = if back {
            (&mut self.jump_back, &mut self.jump_forward)
        } else {
            (&mut self.jump_forward, &mut self.jump_back)
        };
        if let Some(position) = from.pop() {
            to.push(here);
            self.canvas.restore_scroll(position);
        }
    }

    pub(super) fn follow_link(&mut self, target: LinkTarget, sender: &ComponentSender<Self>) {
        match target {
            LinkTarget::Page { page, y } => {
                self.jumped_from_here(page, sender);
                self.canvas.scroll_to_point(page, y);
            }
            LinkTarget::Named(action) => {
                let Some(pdf) = &self.pdf else { return };
                let page = match action {
                    NamedAction::Next => pdf.page.saturating_add(1),
                    NamedAction::Prev => pdf.page.saturating_sub(1),
                    NamedAction::First => 0,
                    NamedAction::Last => usize::MAX,
                };
                self.goto_page(page);
            }
            LinkTarget::Uri(uri) if pdf::is_safe_uri(&uri) => {
                let parent = relm4::main_application().active_window();
                gtk::UriLauncher::new(&uri).launch(
                    parent.as_ref(),
                    gio::Cancellable::NONE,
                    move |result| {
                        if let Err(err) = result {
                            tracing::warn!(uri, "could not open link: {err}");
                        }
                    },
                );
            }
            LinkTarget::Uri(uri) => tracing::warn!(uri, "ignored link with an unsupported scheme"),
        }
    }

    pub(super) fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_id += 1;
        self.canvas.set_selection(Vec::new());
    }

    /// Ask the selection worker for the rectangles (and optionally the text) of the selection.
    pub(super) fn request_selection(&mut self, want_text: bool) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some(pdf) = &self.pdf else { return };
        self.selection_id += 1;
        pdf.selector.request(SelectRequest {
            id: self.selection_id,
            anchor: selection.anchor,
            head: selection.head,
            granularity: selection.granularity,
            visible: pdf.visible,
            want_text,
        });
    }

    /// Forget all hits and, if a search is wanted, start it over on the current document.
    pub(super) fn restart_search(&mut self) {
        self.search_id += 1;
        self.hits.clear();
        self.current_hit = None;
        self.canvas.clear_highlights();
        self.searching = false;

        let Some(pdf) = &self.pdf else { return };
        if self.search_open && !self.search_text.is_empty() {
            self.searching = true;
            pdf.search.start(
                self.search_id,
                self.search_text.clone(),
                pdf.page,
                self.search_options,
            );
        } else {
            pdf.search.stop(self.search_id);
        }
    }

    /// Hits in reading order as (page, index within page).
    pub(super) fn hit_positions(&self) -> Vec<(usize, usize)> {
        self.hits
            .iter()
            .flat_map(|(&page, hits)| (0..hits.len()).map(move |i| (page, i)))
            .collect()
    }

    pub(super) fn select_hit(&mut self, hit: (usize, usize)) {
        self.current_hit = Some(hit);
        self.canvas.set_current_hit(self.current_hit);
        if let Some(rect) = self.hits.get(&hit.0).and_then(|h| h.get(hit.1)) {
            self.canvas.reveal(hit.0, *rect);
        }
    }

    pub(super) fn search_status(&self) -> String {
        if self.search_text.is_empty() {
            return String::new();
        }
        // Called on every update, so count instead of listing every hit.
        let total: usize = self.hits.values().map(Vec::len).sum();
        let more = if self.searching { "…" } else { "" };
        let current = self
            .current_hit
            .filter(|(page, i)| self.hits.get(page).is_some_and(|hits| *i < hits.len()));
        match current {
            Some((page, i)) => format!(
                "{}{more}",
                tr_fmt(
                    "{current} of {total}",
                    &[
                        (
                            "current",
                            &(self.hits.range(..page).map(|(_, h)| h.len()).sum::<usize>() + i + 1)
                                .to_string()
                        ),
                        ("total", &total.to_string())
                    ]
                )
            ),
            None if self.searching => tr("Searching…"),
            None => tr("No results"),
        }
    }
}
