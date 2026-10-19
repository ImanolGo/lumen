//! Root Relm4 component: the main window.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

use relm4::{
    ComponentParts, ComponentSender, RelmWidgetExt, SimpleComponent,
    adw::{self, prelude::*},
    gtk::{self, gdk, gio, glib},
};

use crate::{
    APP_NAME,
    document::{self, DocumentInfo, DocumentKind, OpenError, Opened},
    folder,
    pdf::{
        self, HitRect, OutlineTarget, PageCache, PageLayout, PageRenderer, RenderJob, RenderedPage,
        LinkTarget, LinkWorker, LinksLoaded, NamedAction, PdfSource, SearchEvent, SearchWorker, Granularity, PagePoint, SelectRequest, SelectionResult, SelectionWorker,
    },
    thumbnails::{ThumbData, ThumbnailLoader},
    widgets::{
        CanvasContent, FileStrip, LumenCanvas, OutlinePanel, PageStrip, PropertiesPanel, SelectGesture,
        THUMB_WIDTH, ViewInfo,
    },
};

/// Action name, the message it sends, and keyboard accelerators.
type NavAction<'a> = (&'a str, fn() -> AppMsg, &'a [&'a str]);

/// Action name, canvas handler and keyboard accelerators.
type ViewAction<'a> = (&'a str, fn(&LumenCanvas), &'a [&'a str]);

const RECENT_LIMIT: usize = 10;
/// How many neighbours on each side of the current file stay decoded.
const PRELOAD_RADIUS: usize = 1;
/// Wait this long after the zoom stops changing before re-rendering pages.
const RERENDER_DELAY_MS: u64 = 150;
/// Scrolling needs no such patience, just a moment to coalesce events.
const SCROLL_RENDER_DELAY_MS: u64 = 20;
/// Memory the rendered pages may use (4 bytes per pixel).
const PAGE_CACHE_BYTES: usize = 160 * 1024 * 1024;
/// Pages beyond the visible ones that are rendered ahead of time.
const RENDER_AHEAD: usize = 1;

/// What the sidebar lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarMode {
    Files,
    Pages,
    Outline,
}

/// Selected text: where the drag started and where it is now.
#[derive(Debug, Clone, Copy)]
struct Selection {
    anchor: PagePoint,
    head: PagePoint,
    granularity: Granularity,
}

/// The PDF being shown as a continuous column of pages.
struct PdfView {
    n_pages: usize,
    has_outline: bool,
    layout: Rc<PageLayout>,
    /// Page filling most of the viewport.
    page: usize,
    /// Identifies this document, so late renders of an older one are dropped.
    doc_key: u64,
    visible: (usize, usize),
    cache: PageCache,
    renderer: PageRenderer,
    /// Renders the sidebar thumbnails; separate so it never delays the pages.
    thumb_renderer: PageRenderer,
    search: SearchWorker,
    selector: SelectionWorker,
    links: LinkWorker,
    /// Pages whose links have been read.
    links_loaded: HashSet<usize>,
}

thread_local! {
    /// Lets `open_files` (called from the GApplication `open` signal) reach the component.
    static INPUT: RefCell<Option<relm4::Sender<AppMsg>>> = const { RefCell::new(None) };
}

/// Open the local files of `files` in the main window.
pub fn open_files(files: &[gio::File]) {
    let paths: Vec<PathBuf> = files.iter().filter_map(gio::File::path).collect();
    if paths.is_empty() {
        return;
    }
    INPUT.with(|input| {
        if let Some(sender) = input.borrow().as_ref() {
            sender.emit(AppMsg::Open(paths));
        }
    });
}

pub struct App {
    recent_menu: gio::Menu,
    strip: FileStrip,
    page_strip: PageStrip,
    sidebar_stack: gtk::Stack,
    outline: OutlinePanel,
    sidebar_mode: SidebarMode,
    selection: Option<Selection>,
    /// Identifies the newest selection request; stale rectangles are dropped.
    selection_id: u64,
    search_open: bool,
    search_text: String,
    /// Identifies the running search; events from older ones are dropped.
    search_id: u64,
    searching: bool,
    hits: BTreeMap<usize, Vec<HitRect>>,
    /// The selected hit: page and index within that page.
    current_hit: Option<(usize, usize)>,
    properties: PropertiesPanel,
    properties_open: bool,
    thumbnails: ThumbnailLoader,
    /// Files being browsed (a whole folder, or the files picked together).
    list: Vec<PathBuf>,
    index: usize,
    /// Bumped on every navigation; results from older requests are dropped.
    generation: u64,
    loading: bool,
    sidebar_open: bool,
    hover: bool,
    /// Decoded neighbours of the current file, for instant switching.
    cache: Vec<(PathBuf, Opened)>,
    preloading: HashSet<PathBuf>,
    /// Set when the user navigates to a file that is still being preloaded.
    awaiting: Option<PathBuf>,
    document: Option<DocumentInfo>,
    image: Option<crate::image::ImageDocument>,
    canvas: LumenCanvas,
    pdf: Option<PdfView>,
    next_key: u64,
    /// Zoom and scale factor last reported by the canvas.
    zoom: f64,
    scale_factor: i32,
    /// Bumped on every render request; only the latest one is acted on.
    render_ticket: u64,
    error: Option<String>,
}

#[derive(Debug)]
pub enum AppMsg {
    /// Show the file chooser.
    OpenDialog,
    /// Open files (from the dialog, CLI, drag-and-drop, recents).
    Open(Vec<PathBuf>),
    /// The folder of a single opened file was listed.
    FolderListed { anchor: PathBuf, files: Vec<PathBuf> },
    /// Move by this many files in the list.
    Navigate(isize),
    /// Jump to a list index (sidebar click).
    GoTo(usize),
    /// A background load finished.
    Loaded {
        generation: u64,
        result: Result<Opened, OpenError>,
    },
    /// A neighbour finished decoding in the background.
    Preloaded {
        path: PathBuf,
        result: Result<Opened, OpenError>,
    },
    Thumbnail(PathBuf, ThumbData),
    /// The canvas zoom or the visible pages changed.
    ViewChanged(ViewInfo),
    /// The delay for render request `ticket` has passed.
    RenderPages { ticket: u64 },
    PageRendered {
        doc_key: u64,
        job: RenderJob,
        result: Result<RenderedPage, String>,
    },
    /// Move by this many pages in the open PDF.
    PageBy(isize),
    /// Jump to a page (0-based).
    GotoPage(usize),
    /// Text typed into the page entry (1-based).
    GotoPageText(String),
    SidebarMode(SidebarMode),
    /// Scroll to an outline entry.
    GotoTarget(OutlineTarget),
    /// Rows appeared or disappeared in the thumbnail sidebar.
    PageStripChanged,
    ThumbRendered {
        doc_key: u64,
        job: RenderJob,
        result: Result<RenderedPage, String>,
    },
    /// The pointer selected text on a page.
    Select(SelectGesture),
    SelectionResult {
        doc_key: u64,
        result: SelectionResult,
    },
    /// The links of a page were read.
    LinksLoaded { doc_key: u64, loaded: LinksLoaded },
    /// Put the selected text on the clipboard.
    Copy,
    ClearSelection,
    /// The search text changed.
    SearchChanged(String),
    Search {
        doc_key: u64,
        event: SearchEvent,
    },
    /// Go to the next (1) or previous (-1) match.
    SearchStep(isize),
    /// The search bar was opened or closed.
    SearchOpened(bool),
    /// Open the current encrypted file with the password the user typed.
    OpenWithPassword(String),
    /// The password dialog was dismissed.
    PasswordCancelled,
    ToggleSidebar,
    ToggleProperties,
    Hover(bool),
}

impl App {
    fn title(&self) -> String {
        self.document
            .as_ref()
            .map(DocumentInfo::file_name)
            .unwrap_or_else(|| APP_NAME.to_owned())
    }

    fn subtitle(&self) -> String {
        if self.loading {
            return "Loading…".to_owned();
        }
        let summary = match (&self.pdf, &self.document) {
            (Some(pdf), _) => format!("Page {} of {}", pdf.page + 1, pdf.n_pages),
            (None, Some(d)) => d.summary.clone(),
            (None, None) => String::new(),
        };
        if self.list.len() > 1 {
            format!("{} of {} · {summary}", self.index + 1, self.list.len())
        } else {
            summary
        }
    }

    /// Show the current document in the canvas (a no-op if nothing changed).
    fn sync_canvas(&self) {
        let content = match (&self.pdf, &self.image) {
            (Some(pdf), _) => CanvasContent::Pages {
                key: pdf.doc_key,
                layout: &pdf.layout,
            },
            (None, Some(image)) => CanvasContent::Image(image),
            (None, None) => CanvasContent::None,
        };
        self.canvas.set_content(content);
    }

    fn page_text(&self) -> String {
        self.pdf.as_ref().map(|p| (p.page + 1).to_string()).unwrap_or_default()
    }

    fn page_total(&self) -> String {
        self.pdf.as_ref().map(|p| format!("/ {}", p.n_pages)).unwrap_or_default()
    }

    fn goto_page(&mut self, page: usize) {
        let Some(pdf) = &mut self.pdf else { return };
        pdf.page = page.min(pdf.n_pages.saturating_sub(1));
        self.canvas.scroll_to_page(pdf.page);
    }

    /// Ask the renderer for the pages that are on screen (and next to it) and
    /// not yet drawn at the current zoom.
    fn render_visible(&mut self) {
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
            let up_to_date = pdf.cache.scale(page).is_some_and(|s| (s / scale - 1.0).abs() < 0.02);
            if !up_to_date {
                jobs.push(RenderJob { page, scale });
            }
        }
        pdf.renderer.request(jobs);
    }

    /// Re-run `render_visible` once `delay_ms` has passed without a newer request.
    fn schedule_render(&mut self, delay_ms: u64, sender: &ComponentSender<Self>) {
        self.render_ticket += 1;
        let ticket = self.render_ticket;
        let sender = sender.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay_ms), move || {
            sender.input(AppMsg::RenderPages { ticket });
        });
    }

    fn sidebar_visible(&self) -> bool {
        self.sidebar_open && (self.list.len() > 1 || self.pdf.is_some())
    }

    /// The mode actually shown: the chosen one, unless the document cannot offer it.
    fn mode(&self) -> SidebarMode {
        match (&self.pdf, self.sidebar_mode) {
            (None, _) => SidebarMode::Files,
            (Some(pdf), SidebarMode::Outline) if !pdf.has_outline => SidebarMode::Pages,
            (Some(_), SidebarMode::Files) if self.list.len() < 2 => SidebarMode::Pages,
            (Some(_), mode) => mode,
        }
    }

    fn sidebar_page_name(&self) -> &'static str {
        match self.mode() {
            SidebarMode::Files => "files",
            SidebarMode::Pages => "pages",
            SidebarMode::Outline => "outline",
        }
    }

    /// Read the links of the pages in and next to the view, once per page.
    fn request_links(&mut self) {
        let Some(pdf) = &self.pdf else { return };
        let (first, last) = pdf.visible;
        let wanted = first.saturating_sub(RENDER_AHEAD)..=(last + RENDER_AHEAD).min(pdf.n_pages - 1);
        pdf.links.request(wanted.filter(|p| !pdf.links_loaded.contains(p)));
    }

    fn follow_link(&mut self, target: LinkTarget) {
        match target {
            LinkTarget::Page { page, y } => self.canvas.scroll_to_point(page, y),
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
                gtk::UriLauncher::new(&uri).launch(parent.as_ref(), gio::Cancellable::NONE, move |result| {
                    if let Err(err) = result {
                        tracing::warn!(uri, "could not open link: {err}");
                    }
                });
            }
            LinkTarget::Uri(uri) => tracing::warn!(uri, "ignored link with an unsupported scheme"),
        }
    }

    fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_id += 1;
        self.canvas.set_selection(Vec::new());
    }

    /// Ask the selection worker for the rectangles (and optionally the text) of the selection.
    fn request_selection(&mut self, want_text: bool) {
        let Some(selection) = self.selection else { return };
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
    fn restart_search(&mut self) {
        self.search_id += 1;
        self.hits.clear();
        self.current_hit = None;
        self.canvas.clear_highlights();
        self.searching = false;

        let Some(pdf) = &self.pdf else { return };
        if self.search_open && !self.search_text.is_empty() {
            self.searching = true;
            pdf.search.start(self.search_id, self.search_text.clone(), pdf.page);
        } else {
            pdf.search.stop(self.search_id);
        }
    }

    /// Hits in reading order as (page, index within page).
    fn hit_positions(&self) -> Vec<(usize, usize)> {
        self.hits
            .iter()
            .flat_map(|(&page, hits)| (0..hits.len()).map(move |i| (page, i)))
            .collect()
    }

    fn select_hit(&mut self, hit: (usize, usize)) {
        self.current_hit = Some(hit);
        self.canvas.set_current_hit(self.current_hit);
        if let Some(rect) = self.hits.get(&hit.0).and_then(|h| h.get(hit.1)) {
            self.canvas.reveal(hit.0, *rect);
        }
    }

    fn search_status(&self) -> String {
        if self.search_text.is_empty() {
            return String::new();
        }
        let positions = self.hit_positions();
        let more = if self.searching { "…" } else { "" };
        match self.current_hit.and_then(|c| positions.iter().position(|&p| p == c)) {
            Some(i) => format!("{} of {}{more}", i + 1, positions.len()),
            None if self.searching => "Searching…".to_owned(),
            None => "No results".to_owned(),
        }
    }

    fn has_outline(&self) -> bool {
        self.pdf.as_ref().is_some_and(|p| p.has_outline)
    }

    /// Whether more than one sidebar view exists, so a switcher is worth showing.
    fn sidebar_switchable(&self) -> bool {
        self.pdf.is_some() && (self.list.len() > 1 || self.has_outline())
    }

    fn can_go_prev(&self) -> bool {
        self.index > 0
    }

    fn can_go_next(&self) -> bool {
        self.index + 1 < self.list.len()
    }

    /// Start showing `list[index]`: from the cache, a pending preload, or a fresh load.
    fn show_index(&mut self, index: usize, sender: &ComponentSender<Self>) {
        let Some(path) = self.list.get(index).cloned() else {
            return;
        };
        self.index = index;
        self.generation += 1;
        self.awaiting = None;
        self.strip.select(index);

        if let Some(pos) = self.cache.iter().position(|(p, _)| *p == path) {
            let (_, opened) = self.cache.swap_remove(pos);
            self.apply(opened, sender);
        } else if self.preloading.contains(&path) {
            self.loading = true;
            self.awaiting = Some(path);
        } else {
            self.loading = true;
            let generation = self.generation;
            let sender = sender.clone();
            relm4::spawn_blocking(move || {
                let result = open_logged(&path, None);
                sender.input(AppMsg::Loaded { generation, result });
            });
        }
    }

    /// Make a decoded file the current document.
    fn apply(&mut self, opened: Opened, sender: &ComponentSender<Self>) {
        let info = opened.info;
        tracing::info!(path = %info.path.display(), kind = ?info.kind, "opened");
        gtk::RecentManager::default().add_item(&gio::File::for_path(&info.path).uri());
        refresh_recent_menu(&self.recent_menu);
        self.loading = false;
        self.error = None;
        self.image = opened.image;
        self.properties.set_image(self.image.as_ref());
        let opened_outline = opened.pdf.as_ref().map(|p| p.outline.clone());
        self.pdf = opened.pdf.map(|pdf_info| {
            self.next_key += 1;
            let doc_key = self.next_key;
            let layout = Rc::new(PageLayout::new(&pdf_info.page_sizes));
            let input = sender.input_sender().clone();
            let source = PdfSource::new(&info.path, pdf_info.password.as_deref());
            PdfView {
                n_pages: pdf_info.n_pages,
                has_outline: !pdf_info.outline.is_empty(),
                layout,
                page: 0,
                doc_key,
                visible: (0, 0),
                cache: PageCache::new(PAGE_CACHE_BYTES),
                renderer: PageRenderer::new(&source, {
                    let input = input.clone();
                    move |job, result| input.emit(AppMsg::PageRendered { doc_key, job, result })
                }),
                thumb_renderer: PageRenderer::new(&source, {
                    let input = input.clone();
                    move |job, result| input.emit(AppMsg::ThumbRendered { doc_key, job, result })
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
        });
        match &self.pdf {
            Some(pdf) => self.page_strip.set_document(&pdf.layout.rects.iter().map(|r| r.size()).collect::<Vec<_>>()),
            None => self.page_strip.clear(),
        }
        match &opened_outline {
            Some(items) => self.outline.set_outline(items),
            None => self.outline.set_outline(&[]),
        }
        self.sidebar_mode = if self.pdf.is_some() { SidebarMode::Pages } else { SidebarMode::Files };
        self.document = Some(info);
        self.sync_canvas();
        self.clear_selection();
        self.restart_search();
        self.preload_neighbours(sender);
    }

    /// Decode the files around the current one and drop the ones that are now far away.
    fn preload_neighbours(&mut self, sender: &ComponentSender<Self>) {
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
    fn open_failed(&mut self, err: OpenError, sender: &ComponentSender<Self>) {
        match err {
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

    fn fail(&mut self, err: String) {
        self.loading = false;
        self.document = None;
        self.image = None;
        self.pdf = None;
        self.page_strip.clear();
        self.properties.set_image(None);
        self.error = Some(err);
        self.sync_canvas();
    }

    /// Rebuild the sidebar and queue thumbnails, nearest to the current file first.
    fn refresh_strip(&self) {
        self.strip.set_files(&self.list, self.index);
        let mut order: Vec<usize> = (0..self.list.len()).collect();
        order.sort_by_key(|&i| i.abs_diff(self.index));
        self.thumbnails.request(
            order
                .into_iter()
                .map(|i| self.list[i].clone())
                .filter(|p| !is_pdf(p)),
        );
    }

    fn page(&self) -> &'static str {
        if self.image.is_some() || self.pdf.is_some() { "image" } else { "empty" }
    }

    fn status_icon(&self) -> &'static str {
        match (&self.error, &self.document) {
            (Some(_), _) => "dialog-error-symbolic",
            (None, Some(d)) if d.kind == DocumentKind::Pdf => "x-office-document-symbolic",
            (None, Some(_)) => "image-x-generic-symbolic",
            (None, None) => "document-open-symbolic",
        }
    }

    fn status_title(&self) -> String {
        match (&self.error, &self.document) {
            (Some(_), _) => "Could Not Open File".to_owned(),
            (None, Some(d)) => d.file_name(),
            (None, None) => "Open a Picture or PDF".to_owned(),
        }
    }

    fn status_description(&self) -> String {
        match (&self.error, &self.document) {
            (Some(err), _) => err.clone(),
            // Placeholder until the real viewers land in Phases 1 and 2.
            (None, Some(d)) => format!("{} — viewer coming in the next step", d.summary),
            (None, None) => "Press Ctrl+O or pass a file on the command line".to_owned(),
        }
    }

    fn show_open_dialog(sender: ComponentSender<Self>) {
        let images = gtk::FileFilter::new();
        images.set_name(Some("Images"));
        images.add_pixbuf_formats();

        let pdfs = gtk::FileFilter::new();
        pdfs.set_name(Some("PDF Documents"));
        pdfs.add_mime_type("application/pdf");

        let all = gtk::FileFilter::new();
        all.set_name(Some("Images and PDFs"));
        all.add_pixbuf_formats();
        all.add_mime_type("application/pdf");

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&all);
        filters.append(&images);
        filters.append(&pdfs);

        let dialog = gtk::FileDialog::builder()
            .title("Open")
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
}

#[relm4::component(pub)]
impl SimpleComponent for App {
    type Init = ();
    type Input = AppMsg;
    type Output = ();

    view! {
        main_window = adw::ApplicationWindow {
            set_default_size: (1000, 720),
            #[watch]
            set_title: Some(&model.title()),

            #[wrap(Some)]
            set_content = &adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
                    pack_start = &gtk::Button {
                        set_icon_name: "document-open-symbolic",
                        set_tooltip_text: Some("Open… (Ctrl+O)"),
                        set_action_name: Some("win.open"),
                    },

                    pack_start = &gtk::MenuButton {
                        set_icon_name: "document-open-recent-symbolic",
                        set_tooltip_text: Some("Recent Files"),
                        set_menu_model: Some(&recent_menu),
                    },

                    pack_end = &gtk::Box {
                        set_spacing: 6,
                        #[watch]
                        set_visible: model.pdf.is_some(),

                        #[name = "page_entry"]
                        gtk::Entry {
                            set_width_chars: 4,
                            set_max_width_chars: 6,
                            gtk::prelude::EditableExt::set_alignment: 1.0,
                            set_valign: gtk::Align::Center,
                            set_tooltip_text: Some("Go to page (Ctrl+G)"),
                            #[watch]
                            set_text: &model.page_text(),
                            connect_activate[sender] => move |entry| {
                                sender.input(AppMsg::GotoPageText(entry.text().to_string()));
                            },
                        },

                        gtk::Label {
                            add_css_class: "dim-label",
                            #[watch]
                            set_label: &model.page_total(),
                        },
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "info-outline-symbolic",
                        set_tooltip_text: Some("Properties (Alt+Enter)"),
                        set_action_name: Some("win.toggle-properties"),
                        #[watch]
                        set_visible: model.image.is_some(),
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "sidebar-show-symbolic",
                        set_tooltip_text: Some("Toggle Sidebar (F9)"),
                        set_action_name: Some("win.toggle-sidebar"),
                        #[watch]
                        set_visible: model.list.len() > 1,
                    },

                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        #[watch]
                        set_title: &model.title(),
                        #[watch]
                        set_subtitle: &model.subtitle(),
                    },
                },

                #[name = "search_bar"]
                add_top_bar = &gtk::SearchBar {
                    set_show_close_button: true,
                    connect_search_mode_enabled_notify[sender] => move |bar| {
                        sender.input(AppMsg::SearchOpened(bar.is_search_mode()));
                    },

                    #[wrap(Some)]
                    set_child = &gtk::Box {
                        set_spacing: 6,

                        #[name = "search_entry"]
                        gtk::SearchEntry {
                            set_hexpand: true,
                            set_max_width_chars: 40,
                            set_placeholder_text: Some("Find in document"),
                            connect_search_changed[sender] => move |entry| {
                                sender.input(AppMsg::SearchChanged(entry.text().to_string()));
                            },
                        },

                        gtk::Label {
                            add_css_class: "dim-label",
                            set_width_chars: 12,
                            #[watch]
                            set_label: &model.search_status(),
                        },

                        gtk::Button {
                            set_icon_name: "go-up-symbolic",
                            set_tooltip_text: Some("Previous Match (Shift+Enter)"),
                            set_action_name: Some("win.find-prev"),
                        },
                        gtk::Button {
                            set_icon_name: "go-down-symbolic",
                            set_tooltip_text: Some("Next Match (Enter)"),
                            set_action_name: Some("win.find-next"),
                        },
                    },
                },

                #[wrap(Some)]
                set_content = &gtk::Box {
                    append = &gtk::Box {
                        #[watch]
                        set_visible: model.sidebar_visible(),

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Box {
                                add_css_class: "linked",
                                set_margin_all: 6,
                                set_homogeneous: true,
                                #[watch]
                                set_visible: model.sidebar_switchable(),

                                gtk::ToggleButton {
                                    set_label: "Files",
                                    #[watch]
                                    set_visible: model.list.len() > 1,
                                    #[watch]
                                    set_active: model.mode() == SidebarMode::Files,
                                    connect_clicked[sender] => move |_| sender.input(AppMsg::SidebarMode(SidebarMode::Files)),
                                },
                                gtk::ToggleButton {
                                    set_label: "Pages",
                                    #[watch]
                                    set_active: model.mode() == SidebarMode::Pages,
                                    connect_clicked[sender] => move |_| sender.input(AppMsg::SidebarMode(SidebarMode::Pages)),
                                },
                                gtk::ToggleButton {
                                    set_label: "Outline",
                                    #[watch]
                                    set_visible: model.has_outline(),
                                    #[watch]
                                    set_active: model.mode() == SidebarMode::Outline,
                                    connect_clicked[sender] => move |_| sender.input(AppMsg::SidebarMode(SidebarMode::Outline)),
                                },
                            },

                            #[local_ref]
                            sidebar_stack -> gtk::Stack {
                                set_vexpand: true,
                                #[watch]
                                set_visible_child_name: model.sidebar_page_name(),
                            },
                        },

                        gtk::Separator {},
                    },

                    append = &gtk::Overlay {
                        set_hexpand: true,
                        #[name = "overlay"]
                        #[wrap(Some)]
                        set_child = &gtk::Stack {
                            add_named[Some("empty")] = &adw::StatusPage {
                                #[watch]
                                set_icon_name: Some(model.status_icon()),
                                #[watch]
                                set_title: &model.status_title(),
                                #[watch]
                                set_description: Some(&model.status_description()),
                            },

                            #[name = "scroller"]
                            add_named[Some("image")] = &gtk::ScrolledWindow {},

                            #[watch]
                            set_visible_child_name: model.page(),
                        },

                        add_overlay = &gtk::Spinner {
                            set_halign: gtk::Align::Center,
                            set_valign: gtk::Align::Center,
                            set_width_request: 48,
                            set_height_request: 48,
                            #[watch]
                            set_visible: model.loading,
                            #[watch]
                            set_spinning: model.loading,
                        },

                        add_overlay = &gtk::Revealer {
                            set_halign: gtk::Align::Start,
                            set_valign: gtk::Align::Center,
                            set_transition_type: gtk::RevealerTransitionType::Crossfade,
                            #[watch]
                            set_reveal_child: model.hover && model.can_go_prev(),
                            #[wrap(Some)]
                            set_child = &gtk::Button {
                                set_icon_name: "go-previous-symbolic",
                                set_tooltip_text: Some("Previous (Left)"),
                                set_action_name: Some("win.prev"),
                                set_margin_all: 12,
                                add_css_class: "osd",
                                add_css_class: "circular",
                            },
                        },

                        add_overlay = &gtk::Revealer {
                            set_halign: gtk::Align::End,
                            set_valign: gtk::Align::Center,
                            set_transition_type: gtk::RevealerTransitionType::Crossfade,
                            #[watch]
                            set_reveal_child: model.hover && model.can_go_next(),
                            #[wrap(Some)]
                            set_child = &gtk::Button {
                                set_icon_name: "go-next-symbolic",
                                set_tooltip_text: Some("Next (Right)"),
                                set_action_name: Some("win.next"),
                                set_margin_all: 12,
                                add_css_class: "osd",
                                add_css_class: "circular",
                            },
                        },
                    },

                    append = &gtk::Box {
                        #[watch]
                        set_visible: model.properties_open && model.image.is_some(),

                        gtk::Separator {},

                        #[local_ref]
                        properties_root -> gtk::ScrolledWindow {},
                    },
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        INPUT.with(|input| *input.borrow_mut() = Some(sender.input_sender().clone()));

        let recent_menu = gio::Menu::new();
        refresh_recent_menu(&recent_menu);
        let strip = FileStrip::new({
            let sender = sender.clone();
            move |index| sender.input(AppMsg::GoTo(index))
        });
        let thumbnails = ThumbnailLoader::new({
            let sender = sender.input_sender().clone();
            move |path, data| sender.emit(AppMsg::Thumbnail(path, data))
        });
        let model = App {
            recent_menu: recent_menu.clone(),
            strip,
            page_strip: PageStrip::new(
                {
                    let sender = sender.clone();
                    move |page| sender.input(AppMsg::GotoPage(page))
                },
                {
                    let sender = sender.input_sender().clone();
                    move || sender.emit(AppMsg::PageStripChanged)
                },
            ),
            sidebar_stack: gtk::Stack::new(),
            outline: OutlinePanel::new({
                let sender = sender.clone();
                move |target| sender.input(AppMsg::GotoTarget(target))
            }),
            sidebar_mode: SidebarMode::Files,
            selection: None,
            selection_id: 0,
            search_open: false,
            search_text: String::new(),
            search_id: 0,
            searching: false,
            hits: BTreeMap::new(),
            current_hit: None,
            properties: PropertiesPanel::new(),
            properties_open: false,
            thumbnails,
            list: Vec::new(),
            index: 0,
            generation: 0,
            loading: false,
            sidebar_open: true,
            hover: false,
            cache: Vec::new(),
            preloading: HashSet::new(),
            awaiting: None,
            canvas: LumenCanvas::default(),
            pdf: None,
            next_key: 0,
            zoom: 0.0,
            scale_factor: 1,
            render_ticket: 0,
            document: None,
            image: None,
            error: None,
        };
        let model_canvas = model.canvas.clone();
        let sidebar_stack = model.sidebar_stack.clone();
        sidebar_stack.add_named(model.strip.root(), Some("files"));
        sidebar_stack.add_named(model.page_strip.root(), Some("pages"));
        sidebar_stack.add_named(model.outline.root(), Some("outline"));
        let properties_root = model.properties.root().clone();
        let widgets = view_output!();

        // Window actions + keyboard shortcuts.
        let open_action = gio::SimpleAction::new("open", None);
        let open_sender = sender.clone();
        open_action.connect_activate(move |_, _| open_sender.input(AppMsg::OpenDialog));
        widgets.main_window.add_action(&open_action);

        let canvas = model_canvas;
        widgets.scroller.set_child(Some(&canvas));
        canvas.connect_select({
            let sender = sender.clone();
            move |gesture| sender.input(AppMsg::Select(gesture))
        });
        // Ctrl+C copies the selection, but only while the page view has focus, so
        // it never steals the shortcut from a text entry.
        let shortcuts = gtk::ShortcutController::new();
        for (accel, make_msg) in [
            ("<Control>c", (|| AppMsg::Copy) as fn() -> AppMsg),
            ("Escape", || AppMsg::ClearSelection),
        ] {
            let sender = sender.clone();
            shortcuts.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(accel),
                Some(gtk::CallbackAction::new(move |_, _| {
                    sender.input(make_msg());
                    glib::Propagation::Stop
                })),
            ));
        }
        canvas.add_controller(shortcuts);
        canvas.connect_view_changed({
            let sender = sender.clone();
            move |info| sender.input(AppMsg::ViewChanged(info))
        });

        // `win.sidebar-mode` takes "files", "pages" or "outline".
        let mode_action = gio::SimpleAction::new("sidebar-mode", Some(glib::VariantTy::STRING));
        let mode_sender = sender.clone();
        mode_action.connect_activate(move |_, name| {
            let mode = match name.and_then(|n| n.str()) {
                Some("files") => SidebarMode::Files,
                Some("pages") => SidebarMode::Pages,
                Some("outline") => SidebarMode::Outline,
                _ => return,
            };
            mode_sender.input(AppMsg::SidebarMode(mode));
        });
        widgets.main_window.add_action(&mode_action);

        let goto_action = gio::SimpleAction::new("goto-page", None);
        let entry = widgets.page_entry.clone();
        goto_action.connect_activate(move |_, _| {
            entry.grab_focus();
            entry.select_region(0, -1);
        });
        widgets.main_window.add_action(&goto_action);
        relm4::main_application().set_accels_for_action("win.goto-page", &["<Control>g"]);

        let nav_actions: [NavAction; 12] = [
            ("page-next", || AppMsg::PageBy(1), &["Page_Down"]),
            ("page-prev", || AppMsg::PageBy(-1), &["Page_Up"]),
            ("page-first", || AppMsg::GotoPage(0), &["Home"]),
            ("page-last", || AppMsg::GotoPage(usize::MAX), &["End"]),
            ("next", || AppMsg::Navigate(1), &["Right", "space"]),
            ("prev", || AppMsg::Navigate(-1), &["Left", "BackSpace"]),
            ("toggle-sidebar", || AppMsg::ToggleSidebar, &["F9"]),
            ("toggle-properties", || AppMsg::ToggleProperties, &["<Alt>Return"]),
            ("find-next", || AppMsg::SearchStep(1), &["F3"]),
            ("find-prev", || AppMsg::SearchStep(-1), &["<Shift>F3"]),
            ("find-close", || AppMsg::SearchOpened(false), &[]),
            ("copy", || AppMsg::Copy, &[]),
        ];
        let app = relm4::main_application();
        for (name, make_msg, accels) in nav_actions {
            let action = gio::SimpleAction::new(name, None);
            let sender = sender.clone();
            action.connect_activate(move |_, _| sender.input(make_msg()));
            widgets.main_window.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }

        // Ctrl+F opens the search bar, but only for PDFs.
        let find_action = gio::SimpleAction::new("find", None);
        {
            let (bar, entry, canvas) = (widgets.search_bar.clone(), widgets.search_entry.clone(), canvas.clone());
            find_action.connect_activate(move |_, _| {
                if canvas.has_pages() {
                    bar.set_search_mode(true);
                    entry.grab_focus();
                    entry.select_region(0, -1);
                }
            });
        }
        widgets.main_window.add_action(&find_action);

        // `win.select-text` takes (page, x1, y1, x2, y2) in points from the page's top-left
        // and selects the text between those two points, as a drag would.
        let select_action = gio::SimpleAction::new(
            "select-text",
            Some(&<(u32, f64, f64, f64, f64)>::static_variant_type()),
        );
        {
            let sender = sender.clone();
            select_action.connect_activate(move |_, args| {
                let Some((page, x1, y1, x2, y2)) = args.and_then(|a| a.get::<(u32, f64, f64, f64, f64)>()) else {
                    return;
                };
                let page = page as usize;
                for gesture in [
                    SelectGesture::Start { point: PagePoint { page, x: x1, y: y1 }, clicks: 1 },
                    SelectGesture::Extend(PagePoint { page, x: x2, y: y2 }),
                    SelectGesture::End { moved: true },
                ] {
                    sender.input(AppMsg::Select(gesture));
                }
            });
        }
        widgets.main_window.add_action(&select_action);

        // `win.password` answers the password dialog without typing (for scripts and tests).
        let password_action = gio::SimpleAction::new("password", Some(glib::VariantTy::STRING));
        {
            let sender = sender.clone();
            password_action.connect_activate(move |_, text| {
                if let Some(text) = text.and_then(|t| t.str()) {
                    sender.input(AppMsg::OpenWithPassword(text.to_owned()));
                }
            });
        }
        widgets.main_window.add_action(&password_action);

        // `win.search` takes the text to look for: opens the bar and fills it in.
        let search_action = gio::SimpleAction::new("search", Some(glib::VariantTy::STRING));
        {
            let (bar, entry, canvas) = (widgets.search_bar.clone(), widgets.search_entry.clone(), canvas.clone());
            search_action.connect_activate(move |_, text| {
                if let (true, Some(text)) = (canvas.has_pages(), text.and_then(|t| t.str())) {
                    bar.set_search_mode(true);
                    entry.set_text(text);
                }
            });
        }
        widgets.main_window.add_action(&search_action);
        app.set_accels_for_action("win.find", &["<Control>f"]);
        widgets.search_bar.connect_entry(&widgets.search_entry);

        // Shift+Enter goes to the previous match; plain Enter to the next.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let sender = sender.clone();
            move |_, key, _, state| {
                if key != gdk::Key::Return && key != gdk::Key::KP_Enter {
                    return glib::Propagation::Proceed;
                }
                let step = if state.contains(gdk::ModifierType::SHIFT_MASK) { -1 } else { 1 };
                sender.input(AppMsg::SearchStep(step));
                glib::Propagation::Stop
            }
        });
        widgets.search_entry.add_controller(keys);

        let motion = gtk::EventControllerMotion::new();
        let enter_sender = sender.clone();
        motion.connect_enter(move |_, _, _| enter_sender.input(AppMsg::Hover(true)));
        let leave_sender = sender.clone();
        motion.connect_leave(move |_| leave_sender.input(AppMsg::Hover(false)));
        widgets.overlay.add_controller(motion);

        let view_actions: [ViewAction; 4] = [
            ("zoom-in", LumenCanvas::zoom_in, &["<Control>plus", "<Control>equal"]),
            ("zoom-out", LumenCanvas::zoom_out, &["<Control>minus"]),
            ("best-fit", LumenCanvas::best_fit, &["<Control>0"]),
            ("actual-size", LumenCanvas::actual_size, &["<Control>1"]),
        ];
        for (name, handler, accels) in view_actions {
            let action = gio::SimpleAction::new(name, None);
            let canvas = canvas.clone();
            action.connect_activate(move |_, _| handler(&canvas));
            widgets.main_window.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }
        app.set_accels_for_action("win.open", &["<Control>o"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);

        let open_recent = gio::SimpleAction::new("open-recent", Some(glib::VariantTy::STRING));
        let recent_sender = sender.clone();
        open_recent.connect_activate(move |_, uri| {
            let path = uri
                .and_then(|u| u.str())
                .and_then(|u| gio::File::for_uri(u).path());
            if let Some(path) = path {
                recent_sender.input(AppMsg::Open(vec![path]));
            }
        });
        widgets.main_window.add_action(&open_recent);

        // Drop files onto the window to open them.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop.connect_drop(move |_, value, _, _| {
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            let paths: Vec<PathBuf> = list.files().iter().filter_map(gio::File::path).collect();
            if paths.is_empty() {
                return false;
            }
            sender.input(AppMsg::Open(paths));
            true
        });
        widgets.main_window.add_controller(drop);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            AppMsg::OpenDialog => Self::show_open_dialog(sender),
            AppMsg::Open(mut paths) => {
                if paths.len() == 1 {
                    // Browse the whole folder once it has been listed.
                    let anchor = paths[0].clone();
                    let sender = sender.clone();
                    relm4::spawn_blocking(move || {
                        let files = folder::siblings(&anchor);
                        sender.input(AppMsg::FolderListed { anchor, files });
                    });
                } else {
                    paths.sort_by(|a, b| folder::natural_cmp(a, b));
                    paths.dedup();
                }
                self.cache.clear();
                self.list = paths;
                self.refresh_strip();
                self.show_index(0, &sender);
            }
            AppMsg::FolderListed { anchor, files } => {
                // Ignore stale answers if the user already opened something else.
                if self.list.len() == 1 && self.list[0] == anchor {
                    self.index = files.iter().position(|f| *f == anchor).unwrap_or(0);
                    self.list = files;
                    self.refresh_strip();
                    self.preload_neighbours(&sender);
                }
            }
            AppMsg::Navigate(delta) => {
                let target = self.index.saturating_add_signed(delta).min(self.list.len().saturating_sub(1));
                if !self.list.is_empty() && target != self.index {
                    self.show_index(target, &sender);
                }
            }
            AppMsg::GoTo(index) => {
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
            AppMsg::ViewChanged(info) => {
                let zoom_changed =
                    (info.zoom - self.zoom).abs() > 1e-4 || info.scale_factor != self.scale_factor;
                self.zoom = info.zoom;
                self.scale_factor = info.scale_factor;
                if let Some(pdf) = &mut self.pdf {
                    pdf.page = info.current_page;
                    self.page_strip.set_current(info.current_page);
                    pdf.visible = (info.first_visible, info.last_visible);
                    let delay = if zoom_changed { RERENDER_DELAY_MS } else { SCROLL_RENDER_DELAY_MS };
                    self.schedule_render(delay, &sender);
                    // Rectangles are only computed near the view, so refresh them when it moves.
                    self.request_selection(false);
                    self.request_links();
                }
            }
            AppMsg::RenderPages { ticket } => {
                if ticket == self.render_ticket {
                    self.render_visible();
                }
            }
            AppMsg::PageRendered { doc_key, job, result } => {
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
                    self.canvas.scroll_to_point(target.page, target.y);
                }
            }
            AppMsg::PageStripChanged => {
                let Some(pdf) = &self.pdf else { return };
                let jobs = self.page_strip.wanted().into_iter().map(|page| {
                    let size = pdf.layout.rects[page].size();
                    let scale = THUMB_WIDTH / size.0 * self.scale_factor as f64;
                    RenderJob { page, scale: pdf::effective_scale(size, scale) }
                });
                pdf.thumb_renderer.request(jobs);
            }
            AppMsg::ThumbRendered { doc_key, job, result } => {
                let Some(pdf) = &self.pdf else { return };
                if let (true, Ok(rendered)) = (doc_key == pdf.doc_key, result) {
                    self.page_strip.set_thumbnail(job.page, rendered.into_texture());
                }
            }
            AppMsg::PageBy(delta) => {
                if let Some(pdf) = &self.pdf {
                    self.goto_page(pdf.page.saturating_add_signed(delta));
                }
            }
            AppMsg::GotoPage(page) => self.goto_page(page),
            AppMsg::GotoPageText(text) => {
                if let Ok(page) = text.trim().parse::<usize>() {
                    self.goto_page(page.saturating_sub(1));
                }
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
                        self.selection = Some(Selection { anchor: point, head: point, granularity });
                        self.request_selection(false);
                    }
                    SelectGesture::Extend(point) => {
                        if let Some(selection) = &mut self.selection {
                            selection.head = point;
                            self.request_selection(false);
                        }
                    }
                    SelectGesture::OpenLink(target) => {
                        self.clear_selection();
                        self.follow_link(target);
                    }
                    SelectGesture::End { moved } => {
                        // A plain single click just deselects; double and triple clicks keep their word or line.
                        if !moved && self.selection.is_some_and(|s| s.granularity == Granularity::Glyph) {
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
                    self.canvas.set_selection(result.rects);
                }
                if let (Some(text), Some(display)) =
                    (result.text.filter(|t| !t.is_empty()), gdk::Display::default())
                {
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
            AppMsg::Copy => self.request_selection(true),
            AppMsg::ClearSelection => self.clear_selection(),
            AppMsg::SearchChanged(text) => {
                self.search_text = text;
                self.restart_search();
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
                    SearchEvent::Done { id } if id == self.search_id => self.searching = false,
                    _ => {}
                }
            }
            AppMsg::SearchStep(step) => {
                let positions = self.hit_positions();
                if positions.is_empty() {
                    return;
                }
                let current = self.current_hit.and_then(|c| positions.iter().position(|&p| p == c));
                let next = match current {
                    Some(i) => (i as isize + step).rem_euclid(positions.len() as isize) as usize,
                    None => 0,
                };
                self.select_hit(positions[next]);
            }
            AppMsg::OpenWithPassword(password) => {
                let Some(path) = self.list.get(self.index).cloned() else { return };
                self.generation += 1;
                self.loading = true;
                let generation = self.generation;
                relm4::spawn_blocking(move || {
                    let result = open_logged(&path, Some(&password));
                    sender.input(AppMsg::Loaded { generation, result });
                });
            }
            AppMsg::PasswordCancelled => self.fail(OpenError::PasswordRequired { retry: false }.message()),
            AppMsg::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            AppMsg::ToggleProperties => self.properties_open = !self.properties_open,
            AppMsg::Hover(hover) => self.hover = hover,
        }
    }
}

/// Ask for the password of an encrypted PDF; the answer comes back as a message.
fn ask_password(name: &str, retry: bool, sender: ComponentSender<App>) {
    let body = if retry {
        format!("The password for “{name}” was incorrect. Try again.")
    } else {
        format!("“{name}” is protected. Enter its password to open it.")
    };
    let dialog = adw::AlertDialog::new(Some("Password Required"), Some(&body));
    dialog.add_responses(&[("cancel", "_Cancel"), ("open", "_Open")]);
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("open"));
    dialog.set_close_response("cancel");

    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));

    let parent = relm4::main_application().active_window();
    entry.grab_focus();
    dialog.choose(parent.as_ref(), gio::Cancellable::NONE, move |response| {
        if response == "open" {
            sender.input(AppMsg::OpenWithPassword(entry.text().to_string()));
        } else {
            sender.input(AppMsg::PasswordCancelled);
        }
    });
}

fn open_logged(path: &std::path::Path, password: Option<&str>) -> Result<Opened, OpenError> {
    document::open(path, password).inspect_err(|err| {
        // Needing a password is expected, not a failure worth logging.
        if let OpenError::Other(message) = err {
            tracing::warn!(path = %path.display(), "open failed: {message}");
        }
    })
}

fn is_pdf(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Rebuild the "Recent Files" menu from the desktop-wide recent list.
fn refresh_recent_menu(menu: &gio::Menu) {
    menu.remove_all();

    let mut items: Vec<_> = gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter(|item| {
            item.is_local()
                && item.exists()
                && DocumentKind::from_mime(&item.mime_type()).is_some()
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.modified().to_unix()));

    for info in items.into_iter().take(RECENT_LIMIT) {
        let item = gio::MenuItem::new(Some(&info.display_name()), None);
        item.set_action_and_target_value(Some("win.open-recent"), Some(&info.uri().to_variant()));
        menu.append_item(&item);
    }

    if menu.n_items() == 0 {
        // No such action, so the entry shows up disabled.
        menu.append(Some("No Recent Files"), Some("win.none"));
    }
}
