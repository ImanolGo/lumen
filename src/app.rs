//! Root Relm4 component: the main window.

mod control;
mod edit;
mod forms;
mod markup;
mod pages;
mod prefs;
mod shortcuts;
mod signature;

use crate::i18n::{tr, tr_fmt};
use std::{
    cell::{Cell, RefCell},
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
    annotate::{self, Tool},
    document::{self, DocumentInfo, DocumentKind, OpenError, Opened},
    edit::{Adjustments, EditOp},
    folder,
    pdf::{
        self, Granularity, HitRect, LinkTarget, LinkWorker, LinksLoaded, NamedAction,
        OutlineTarget, PageCache, PageLayout, PagePoint, PageRenderer, PdfSource, RenderJob,
        RenderedPage, SearchEvent, SearchWorker, SelectRequest, SelectionResult, SelectionWorker,
    },
    print,
    save::Format,
    settings::{self, DefaultZoom, Settings, Store},
    thumbnails::{ThumbData, ThumbnailLoader},
    widgets::{
        CanvasContent, FileStrip, LumenCanvas, MarkupEvent, OutlinePanel, PageStrip,
        PageStripCallbacks, PropertiesPanel, SelectGesture, THUMB_WIDTH, ViewInfo,
    },
};

/// Action name, the message it sends, and keyboard accelerators.
type NavAction<'a> = (&'a str, fn() -> AppMsg, &'a [&'a str]);

/// Action name, canvas handler and keyboard accelerators.
type ViewAction<'a> = (&'a str, fn(&LumenCanvas), &'a [&'a str]);

use edit::{AdjustField, CropAspect, EditMode, EditSession, Pending, RenderedEdit};
use markup::MarkupState;
use pages::{PageRequest, WorkDir};

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
    /// The file the user opened; saving writes here. `source` is the working copy being shown.
    original: PathBuf,
    /// The original needed a password. Annotations are saved with its protection, but a
    /// rearranged working copy is written without it.
    protected: bool,
    n_pages: usize,
    has_outline: bool,
    layout: Rc<PageLayout>,
    /// Page filling most of the viewport.
    page: usize,
    /// Identifies this document, so late renders of an older one are dropped.
    doc_key: u64,
    visible: (usize, usize),
    source: PdfSource,
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
    if let Some(started) = crate::STARTED.get() {
        tracing::info!(ms = started.elapsed().as_millis() as u64, "open requested");
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
    /// Files being browsed: the ones opened together, plus any dropped in since.
    list: Vec<PathBuf>,
    /// Bumped whenever files are opened or added; older folder listings are dropped.
    listing: u64,
    index: usize,
    /// Bumped on every navigation; results from older requests are dropped.
    generation: u64,
    loading: bool,
    sidebar_open: bool,
    /// The floating controls are awake: something happened recently (see `wake_controls`).
    controls_awake: bool,
    /// Identifies the latest wake-up; older idle timers are ignored.
    controls_ticket: u64,
    /// How many of the floating controls the pointer is over (they stay while it is).
    controls_hovered: u32,
    /// Decoded neighbours of the current file, for instant switching.
    cache: Vec<(PathBuf, Opened)>,
    preloading: HashSet<PathBuf>,
    /// Set when the user navigates to a file that is still being preloaded.
    awaiting: Option<PathBuf>,
    document: Option<DocumentInfo>,
    image: Option<crate::image::ImageDocument>,
    /// The edits of the open image, and the image as edited (if it differs from the file).
    edit: Option<EditSession>,
    edited: Option<crate::image::ImageDocument>,
    edit_mode: EditMode,
    /// Slider values while the colour adjustment bar is open.
    adjust: Adjustments,
    /// Mirrors `is_dirty()` for the window's close handler, which cannot see the model.
    dirty: Rc<Cell<bool>>,
    /// Files the user already agreed to overwrite this session.
    overwrite_ok: HashSet<PathBuf>,
    /// What to do once a save the user asked for has finished.
    after_save: Option<Pending>,
    /// The user agreed to save the open PDF without its password.
    unprotected_ok: bool,
    saving: bool,
    markup: MarkupState,
    /// When the load being waited for began, to report how long it took.
    load_started: Option<std::time::Instant>,
    /// Whether the time to the first picture on screen has been reported.
    first_view_logged: bool,
    fullscreen: bool,
    /// Running while a slideshow is on.
    slideshow: Option<glib::SourceId>,
    /// Hides the pointer a little after it stops moving in fullscreen.
    cursor_timer: Option<glib::SourceId>,
    cursor_hidden: bool,
    /// Preferences and remembered state; shared with the window's close handler.
    store: Rc<RefCell<Store>>,
    /// A page to scroll to as soon as the canvas has a size (the one a PDF was left on).
    pending_goto: Option<usize>,
    /// Working copies of the PDF being rearranged (they are the undo history of page changes).
    work: WorkDir,
    reload_generation: u64,
    /// A page operation or reload is under way.
    page_busy: bool,
    /// Files still to insert, and the page the next one goes in front of.
    insert_queue: Vec<PathBuf>,
    insert_at: usize,
    /// Calls on the control interface that are waiting for work to finish.
    control: control::ControlState,
    /// Saved signatures, and the toolbar button whose menu lists them.
    signatures: crate::signatures::Signatures,
    signature_button: gtk::MenuButton,
    /// A signature field that was clicked: the next signature placed goes into it.
    signature_target: Option<(usize, pdf::HitRect)>,
    /// The colour the style button shows, shared with the circles that draw it.
    style_color: Rc<Cell<annotate::Color>>,
    /// Index into `CropAspect::CHOICES`.
    crop_choice: usize,
    canvas: LumenCanvas,
    pdf: Option<PdfView>,
    next_key: u64,
    /// Zoom and scale factor last reported by the canvas.
    zoom: f64,
    scale_factor: i32,
    /// Bumped on every render request; only the latest one is acted on.
    render_ticket: u64,
    /// Bumped whenever the view of a drawing changes; only the latest sharp render is shown.
    sharp_ticket: u64,
    error: Option<String>,
}

#[derive(Debug)]
pub enum AppMsg {
    /// Show the file chooser.
    OpenDialog,
    /// Open files (from the dialog, CLI, drag-and-drop, recents).
    Open(Vec<PathBuf>),
    /// Add files (dropped onto the window) to the ones being browsed.
    Add(Vec<PathBuf>),
    /// The files to open or add were worked out (folders listed, unsupported drops left out).
    Listed {
        listing: u64,
        files: Vec<PathBuf>,
        add: bool,
    },
    /// A file was saved: show it again, keeping the files being browsed (a copy is added
    /// after the current file).
    ShowSaved(PathBuf),
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
    /// Rows appeared or disappeared in the file sidebar.
    FileStripChanged,
    /// The canvas zoom or the visible pages changed.
    ViewChanged(ViewInfo),
    /// The view of a drawing settled: render the part in view sharp (request `ticket`).
    RenderSharp {
        ticket: u64,
    },
    SharpRendered {
        ticket: u64,
        area: [f64; 4],
        result: Result<gdk::Texture, String>,
    },
    /// The delay for render request `ticket` has passed.
    RenderPages {
        ticket: u64,
    },
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
    LinksLoaded {
        doc_key: u64,
        loaded: LinksLoaded,
    },
    /// Put the selected text on the clipboard.
    Copy,
    /// A call on the control interface (see `control`); its invocation waits under `id`.
    Control {
        id: u64,
        method: String,
        params: glib::Variant,
    },
    /// Stamp saved signature `index` onto the page.
    PlaceSignature(usize),
    /// Draw a new signature.
    NewSignature,
    /// A new signature was drawn (strokes in the pad's pixels).
    SignatureDrawn(Vec<Vec<(f64, f64)>>),
    DeleteSignature(usize),
    /// Set a form field of the open PDF.
    FillForm {
        id: i32,
        value: pdf::FieldValue,
    },
    /// The working copy with a form field filled in was written.
    FormFilled {
        out: PathBuf,
        result: Result<(), String>,
    },
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
    /// An editing operation on top of the current edits.
    Edit(EditOp),
    Undo,
    Redo,
    EnterMode(EditMode),
    /// Apply (Enter) or cancel (Escape) the open tool.
    Confirm,
    Escape,
    CropAspect(usize),
    AdjustField(AdjustField, f32),
    ResizeDialog,
    EditRendered {
        ticket: u64,
        result: Result<(Option<std::sync::Arc<image::DynamicImage>>, RenderedEdit), String>,
    },
    Save,
    SaveConfirmed,
    SaveCancelled,
    SaveAs,
    SaveTo {
        dest: PathBuf,
        format: Format,
    },
    /// The user agreed to save a rearranged, password-protected PDF without its password.
    SaveUnprotected {
        dest: PathBuf,
    },
    Saved {
        dest: PathBuf,
        result: Result<(), String>,
    },
    ToggleFullscreen,
    FullscreenChanged(bool),
    ToggleSlideshow,
    SlideshowTick,
    PointerMoved,
    HideCursor,
    /// Escape while presenting: end the slideshow and leave fullscreen.
    LeavePresentation,
    Preferences,
    SettingsChanged(Settings),
    About,
    Shortcuts,
    /// A page operation on the open PDF.
    PageRequest(PageRequest),
    PageOpDone {
        out: PathBuf,
        goto: usize,
        result: Result<Vec<pdf::pageops::Entry>, String>,
    },
    PdfReloaded {
        generation: u64,
        goto: usize,
        path: PathBuf,
        result: Result<Opened, OpenError>,
    },
    InsertFiles {
        paths: Vec<PathBuf>,
        at: Option<usize>,
    },
    InsertDialog,
    DeletePages,
    ExportPagesDialog,
    ExportPagesTo {
        dest: PathBuf,
        pages: Vec<usize>,
    },
    ExportPngDialog,
    ExportPngTo {
        dest: PathBuf,
    },
    ExportImagePdfDialog,
    ExportImagePdfTo {
        dest: PathBuf,
    },
    Exported(Result<(), String>),
    /// A pointer event while the markup tools are in use.
    Markup(MarkupEvent),
    ToggleMarkup,
    SetTool(Tool),
    SetColor(annotate::Color),
    /// The size slider: text size for the Text tool, line width for the others.
    SetSize(f64),
    /// Pick a colour that is not among the swatches.
    CustomColor,
    MarkupDelete,
    MarkupNudge(f64, f64),
    MarkupText {
        page: usize,
        id: u64,
        text: String,
    },
    /// The window is closing with unsaved edits.
    CloseRequested,
    /// The user chose to save before going on to `Pending`.
    SaveThen(Pending),
    /// The user chose to discard the edits before going on to `Pending`.
    Discard(Pending),
    /// Carry out something that was waiting for the edits to be dealt with.
    Replay(Pending),
    /// Open the current encrypted file with the password the user typed.
    OpenWithPassword(String),
    /// The password dialog was dismissed.
    PasswordCancelled,
    /// Print the open document (or, with a path, write it to a PDF file without asking).
    Print(Option<PathBuf>),
    ToggleSidebar,
    ShowSidebar(bool),
    ToggleProperties,
    ShowProperties(bool),
    /// The pointer entered (true) or left the view.
    Hover(bool),
    /// The pointer entered (true) or left one of the floating controls.
    ControlsHover(bool),
    /// Nothing happened for a while since wake-up `ticket`.
    ControlsIdle(u64),
}

impl App {
    /// The file name, after a dot while it has unsaved changes.
    fn title(&self) -> String {
        match &self.document {
            Some(document) if self.is_dirty() => format!("• {}", document.file_name()),
            Some(document) => document.file_name(),
            None => APP_NAME.to_owned(),
        }
    }

    /// Where the file is among the ones being browsed. The page of a PDF is in the
    /// floating controls, and the size of a picture in its properties.
    fn subtitle(&self) -> String {
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

    /// Show the current document in the canvas (a no-op if nothing changed).
    fn sync_canvas(&self) {
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

    fn page_text(&self) -> String {
        self.pdf
            .as_ref()
            .map(|p| (p.page + 1).to_string())
            .unwrap_or_default()
    }

    fn page_total(&self) -> String {
        self.pdf
            .as_ref()
            .map(|p| tr_fmt("of {total}", &[("total", &p.n_pages.to_string())]))
            .unwrap_or_default()
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
    fn shown_drawing(&self) -> Option<&crate::image::ImageDocument> {
        self.image
            .as_ref()
            .filter(|i| i.vector && self.pdf.is_none() && self.edited.is_none())
    }

    /// A drawing is shown sharp at any zoom: once the view settles, the part in view is
    /// rendered again at that zoom (see `render_sharp`).
    fn schedule_sharp(&mut self, sender: &ComponentSender<Self>) {
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

    fn render_sharp(&mut self, ticket: u64, sender: &ComponentSender<Self>) {
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
    fn schedule_render(&mut self, delay_ms: u64, sender: &ComponentSender<Self>) {
        self.render_ticket += 1;
        let ticket = self.render_ticket;
        let sender = sender.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay_ms), move || {
            sender.input(AppMsg::RenderPages { ticket });
        });
    }

    /// Whether there is anything to list in the sidebar.
    fn sidebar_available(&self) -> bool {
        !self.fullscreen && (self.list.len() > 1 || self.pdf.is_some())
    }

    fn sidebar_visible(&self) -> bool {
        self.sidebar_open && self.sidebar_available()
    }

    /// Whether the floating controls over the view are shown: for a moment after
    /// something happens, and while the pointer is on them.
    fn controls_shown(&self) -> bool {
        (self.controls_awake || self.controls_hovered > 0)
            && !self.cursor_hidden
            && self.has_document()
    }

    /// Show the floating controls, and hide them again if nothing else happens.
    fn wake_controls(&mut self, sender: &ComponentSender<Self>) {
        self.controls_awake = true;
        self.controls_ticket += 1;
        let (ticket, sender) = (self.controls_ticket, sender.clone());
        glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            sender.input(AppMsg::ControlsIdle(ticket));
        });
    }

    fn zoom_text(&self) -> String {
        if self.zoom > 0.0 {
            format!("{:.0}%", self.zoom * 100.0)
        } else {
            String::new()
        }
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
        let wanted =
            first.saturating_sub(RENDER_AHEAD)..=(last + RENDER_AHEAD).min(pdf.n_pages - 1);
        pdf.links
            .request(wanted.filter(|p| !pdf.links_loaded.contains(p)));
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

    /// Print the current document through the GTK print dialog, or export it to `export_to`.
    fn print(&self, export_to: Option<PathBuf>) {
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

    fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_id += 1;
        self.canvas.set_selection(Vec::new());
    }

    /// Ask the selection worker for the rectangles (and optionally the text) of the selection.
    fn request_selection(&mut self, want_text: bool) {
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
    fn restart_search(&mut self) {
        self.search_id += 1;
        self.hits.clear();
        self.current_hit = None;
        self.canvas.clear_highlights();
        self.searching = false;

        let Some(pdf) = &self.pdf else { return };
        if self.search_open && !self.search_text.is_empty() {
            self.searching = true;
            pdf.search
                .start(self.search_id, self.search_text.clone(), pdf.page);
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

    /// Whether a picture or a PDF is open.
    fn has_document(&self) -> bool {
        self.image.is_some() || self.pdf.is_some()
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

    /// Work out the files behind `paths` off the main thread (folders are listed), then
    /// open them, or add them to the ones being browsed.
    fn list_files(&mut self, paths: Vec<PathBuf>, add: bool, sender: &ComponentSender<Self>) {
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
            self.load_started = Some(std::time::Instant::now());
            let generation = self.generation;
            let sender = sender.clone();
            relm4::spawn_blocking(move || {
                let result = open_logged(&path, None);
                sender.input(AppMsg::Loaded { generation, result });
            });
        }
    }

    fn can_slideshow(&self) -> bool {
        !self.is_dirty()
            && (self.list.len() > 1 || self.pdf.as_ref().is_some_and(|p| p.n_pages > 1))
    }

    fn stop_slideshow(&mut self) {
        if let Some(timer) = self.slideshow.take() {
            timer.remove();
        }
    }

    fn start_slideshow_timer(&mut self, sender: &ComponentSender<Self>) {
        self.stop_slideshow();
        let seconds = self.store.borrow().settings.slideshow_seconds.max(1);
        let sender = sender.clone();
        self.slideshow = Some(glib::timeout_add_seconds_local(seconds, move || {
            sender.input(AppMsg::SlideshowTick);
            glib::ControlFlow::Continue
        }));
    }

    /// Show the pointer and hide it again if it stays still.
    fn restart_cursor_timer(&mut self, sender: &ComponentSender<Self>) {
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
    fn apply_settings(&self) {
        let store = self.store.borrow();
        self.canvas.set_background(store.settings.background);
        self.canvas.set_smooth_zoom(store.settings.smooth_zoom);
        self.canvas
            .set_start_actual_size(store.settings.default_zoom == DefaultZoom::ActualSize);
    }

    /// The viewer state for a PDF shown from `path` (a working copy of `original`).
    fn make_pdf_view(
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

    /// Make a decoded file the current document.
    fn apply(&mut self, opened: Opened, sender: &ComponentSender<Self>) {
        let info = opened.info;
        let took = self.load_started.take().map(|t| t.elapsed().as_millis());
        tracing::info!(path = %info.path.display(), kind = ?info.kind, load_ms = ?took, "opened");
        gtk::RecentManager::default().add_item(&gio::File::for_path(&info.path).uri());
        refresh_recent_menu(&self.recent_menu);
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
        self.load_markup(opened_annotations);
        self.unprotected_ok = false;
        self.pending_goto = None;
        if self.store.borrow().settings.remember_state
            && let Some(pdf) = &self.pdf
        {
            let saved = self.store.borrow().state.last_page(&pdf.original);
            self.pending_goto = saved.filter(|&p| p < pdf.n_pages);
        }
        self.store.borrow().save_state();
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

    fn fail(&mut self, err: String) {
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

    /// Rebuild the file sidebar. Thumbnails are made for the rows that are on screen,
    /// as they appear (`FileStripChanged`).
    fn refresh_strip(&self) {
        self.strip.set_files(&self.list, self.index);
    }

    fn page(&self) -> &'static str {
        if self.image.is_some() || self.pdf.is_some() {
            "image"
        } else {
            "empty"
        }
    }

    fn status_icon(&self) -> &'static str {
        match (&self.error, &self.document) {
            (Some(_), _) => "dialog-error-symbolic",
            (None, Some(d)) if d.kind == DocumentKind::Pdf => "x-office-document-symbolic",
            (None, Some(_)) => "image-x-generic-symbolic",
            (None, None) => crate::APP_ID,
        }
    }

    fn status_title(&self) -> String {
        match (&self.error, &self.document) {
            (Some(_), _) => tr("Could Not Open File"),
            (None, Some(d)) => d.file_name(),
            (None, None) => tr("Open a Picture or PDF"),
        }
    }

    fn status_description(&self) -> String {
        match (&self.error, &self.document) {
            (Some(err), _) => err.clone(),
            (None, Some(d)) => d.summary.clone(),
            (None, None) => tr("Drop a file here, or press Ctrl+O"),
        }
    }

    fn show_open_dialog(sender: ComponentSender<Self>) {
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
}

#[relm4::component(pub)]
impl SimpleComponent for App {
    type Init = ();
    type Input = AppMsg;
    type Output = ();

    view! {
        main_window = adw::ApplicationWindow {
            set_default_size: (initial_size.0, initial_size.1),
            #[watch]
            set_title: Some(&model.title()),
            connect_fullscreened_notify[sender] => move |window| {
                sender.input(AppMsg::FullscreenChanged(window.is_fullscreen()));
            },
            connect_close_request[sender, dirty, store] => move |window| {
                // Remember how the window was left.
                {
                    let mut store = store.borrow_mut();
                    let (width, height) = (window.width(), window.height());
                    store.state.maximized = window.is_maximized();
                    if !window.is_maximized() && width > 0 && height > 0 {
                        store.state.window_width = width;
                        store.state.window_height = height;
                    }
                    store.save_state();
                }
                // Unsaved edits: ask first, and close only once they are dealt with.
                if dirty.get() {
                    sender.input(AppMsg::CloseRequested);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            },

            #[wrap(Some)]
            set_content = &adw::ToolbarView {
                #[watch]
                set_reveal_top_bars: !model.fullscreen,
                #[watch]
                set_reveal_bottom_bars: !model.fullscreen || model.edit_mode != EditMode::None,
                add_top_bar = &adw::HeaderBar {
                    pack_start = &gtk::ToggleButton {
                        set_icon_name: "sidebar-show-symbolic",
                        set_tooltip_text: Some(&tr("Sidebar (F9)")),
                        #[watch]
                        set_active: model.sidebar_visible(),
                        #[watch]
                        set_visible: model.sidebar_available(),
                        connect_clicked[sender] => move |button| {
                            sender.input(AppMsg::ShowSidebar(button.is_active()));
                        },
                    },

                    pack_start = &adw::SplitButton {
                        set_icon_name: "document-open-symbolic",
                        set_tooltip_text: Some(&tr("Open… (Ctrl+O)")),
                        set_dropdown_tooltip: &tr("Recent Files"),
                        set_action_name: Some("win.open"),
                        set_menu_model: Some(&recent_menu),
                    },

                    pack_end = &gtk::MenuButton {
                        set_icon_name: "open-menu-symbolic",
                        set_tooltip_text: Some(&tr("Main Menu")),
                        set_primary: true,
                        set_menu_model: Some(&main_menu),
                    },

                    pack_end = &gtk::MenuButton {
                        set_icon_name: "view-paged-symbolic",
                        set_tooltip_text: Some(&tr("Pages")),
                        set_menu_model: Some(&pages_menu),
                        #[watch]
                        set_visible: model.pdf.is_some(),
                    },

                    pack_end = &gtk::MenuButton {
                        set_icon_name: "lumen-image-edit-symbolic",
                        set_tooltip_text: Some(&tr("Edit")),
                        set_menu_model: Some(&edit_menu),
                        #[watch]
                        set_visible: model.image.is_some() && model.edit.is_some(),
                    },

                    pack_end = &gtk::ToggleButton {
                        set_icon_name: "document-edit-symbolic",
                        set_tooltip_text: Some(&tr("Markup (Ctrl+Shift+A)")),
                        #[watch]
                        set_active: model.markup.open,
                        #[watch]
                        set_visible: model.pdf.is_some() || model.edit.is_some(),
                        connect_clicked[sender] => move |_| sender.input(AppMsg::ToggleMarkup),
                    },

                    pack_end = &gtk::ToggleButton {
                        set_icon_name: "info-outline-symbolic",
                        set_tooltip_text: Some(&tr("Properties (Alt+Enter)")),
                        #[watch]
                        set_active: model.properties_open,
                        #[watch]
                        set_visible: model.has_document(),
                        connect_clicked[sender] => move |button| {
                            sender.input(AppMsg::ShowProperties(button.is_active()));
                        },
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
                            set_placeholder_text: Some(&tr("Find in document")),
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
                            set_tooltip_text: Some(&tr("Previous Match (Shift+Enter)")),
                            set_action_name: Some("win.find-prev"),
                        },
                        gtk::Button {
                            set_icon_name: "go-down-symbolic",
                            set_tooltip_text: Some(&tr("Next Match (Enter)")),
                            set_action_name: Some("win.find-next"),
                        },
                    },
                },

                add_top_bar = &gtk::Box {
                    set_spacing: 6,
                    set_halign: gtk::Align::Center,
                    add_css_class: "toolbar",
                    #[watch]
                    set_visible: model.markup.open,

                    gtk::Box {
                        set_spacing: 2,
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-select-symbolic",
                            set_tooltip_text: Some(&tr("Select")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Select,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Select)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-rectangle-symbolic",
                            set_tooltip_text: Some(&tr("Rectangle")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Rectangle,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Rectangle)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-ellipse-symbolic",
                            set_tooltip_text: Some(&tr("Ellipse")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Ellipse,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Ellipse)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-arrow-symbolic",
                            set_tooltip_text: Some(&tr("Arrow")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Arrow,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Arrow)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-line-symbolic",
                            set_tooltip_text: Some(&tr("Line")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Line,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Line)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-pen-symbolic",
                            set_tooltip_text: Some(&tr("Pen")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Pen,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Pen)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-text-symbolic",
                            set_tooltip_text: Some(&tr("Text")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Text,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Text)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-highlight-symbolic",
                            set_tooltip_text: Some(&tr("Highlight")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Highlight,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Highlight)),
                        },
                    },

                    gtk::Separator {},

                    #[name = "style_button"]
                    gtk::MenuButton {
                        set_tooltip_text: Some(&tr("Colour and Size")),
                        set_always_show_arrow: true,

                        #[wrap(Some)]
                        set_popover = &gtk::Popover {
                            #[wrap(Some)]
                            set_child = &gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 12,
                                set_margin_all: 6,

                                #[local_ref]
                                swatches -> gtk::Grid {
                                    set_row_spacing: 6,
                                    set_column_spacing: 6,
                                    set_halign: gtk::Align::Center,
                                },

                                gtk::Button {
                                    set_label: &tr("Custom Colour…"),
                                    add_css_class: "flat",
                                    connect_clicked[sender] => move |_| sender.input(AppMsg::CustomColor),
                                },

                                gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 2,
                                    #[watch]
                                    set_visible: model.markup_has_size(),

                                    gtk::Label {
                                        set_halign: gtk::Align::Start,
                                        set_margin_start: 6,
                                        add_css_class: "dim-label",
                                        add_css_class: "caption-heading",
                                        #[watch]
                                        set_label: &model.markup_size_label(),
                                    },
                                    gtk::Scale {
                                        set_width_request: 220,
                                        set_draw_value: true,
                                        set_digits: 0,
                                        set_value_pos: gtk::PositionType::Right,
                                        #[watch]
                                        #[block_signal(size_changed)]
                                        set_range: (model.markup_size_range().0, model.markup_size_range().1),
                                        #[watch]
                                        #[block_signal(size_changed)]
                                        set_value: model.markup_size(),
                                        connect_value_changed[sender] => move |scale| {
                                            sender.input(AppMsg::SetSize(scale.value().round()));
                                        } @size_changed,
                                    },
                                },
                            },
                        },
                    },

                    #[local_ref]
                    signature_button -> gtk::MenuButton {},

                    gtk::Separator {},

                    gtk::Button {
                        set_icon_name: "edit-delete-symbolic",
                        set_tooltip_text: Some(&tr("Delete the selected annotation (Delete)")),
                        connect_clicked[sender] => move |_| sender.input(AppMsg::MarkupDelete),
                    },
                },

                add_bottom_bar = &gtk::Box {
                    set_spacing: 12,
                    add_css_class: "toolbar",
                    #[watch]
                    set_visible: model.edit_mode != EditMode::None,

                    gtk::Box {
                        set_spacing: 8,
                        set_valign: gtk::Align::Center,
                        #[watch]
                        set_visible: model.edit_mode == EditMode::Crop,

                        gtk::Label { set_label: &tr("Aspect ratio") },
                        gtk::DropDown::from_strings(&[&tr("Free"), &tr("Original"), &tr("1:1"), &tr("4:3"), &tr("3:2"), &tr("16:9")]) {
                            #[watch]
                            set_selected: model.crop_choice as u32,
                            connect_selected_notify[sender] => move |dropdown| {
                                sender.input(AppMsg::CropAspect(dropdown.selected() as usize));
                            },
                        },
                    },

                    gtk::Box {
                        set_spacing: 12,
                        set_hexpand: true,
                        set_valign: gtk::Align::Center,
                        #[watch]
                        set_visible: model.edit_mode == EditMode::Adjust,

                        gtk::Label { set_label: &tr("Exposure") },
                        gtk::Scale {
                            set_range: (-2.0, 2.0),
                            set_hexpand: true,
                            set_draw_value: false,
                            #[watch]
                            set_value: model.adjust.exposure as f64,
                            connect_value_changed[sender] => move |scale| {
                                sender.input(AppMsg::AdjustField(AdjustField::Exposure, scale.value() as f32));
                            },
                        },
                        gtk::Label { set_label: &tr("Contrast") },
                        gtk::Scale {
                            set_range: (-1.0, 1.0),
                            set_hexpand: true,
                            set_draw_value: false,
                            #[watch]
                            set_value: model.adjust.contrast as f64,
                            connect_value_changed[sender] => move |scale| {
                                sender.input(AppMsg::AdjustField(AdjustField::Contrast, scale.value() as f32));
                            },
                        },
                        gtk::Label { set_label: &tr("Saturation") },
                        gtk::Scale {
                            set_range: (-1.0, 1.0),
                            set_hexpand: true,
                            set_draw_value: false,
                            #[watch]
                            set_value: model.adjust.saturation as f64,
                            connect_value_changed[sender] => move |scale| {
                                sender.input(AppMsg::AdjustField(AdjustField::Saturation, scale.value() as f32));
                            },
                        },
                    },

                    gtk::Box { set_hexpand: true },

                    gtk::Button {
                        set_label: &tr("Cancel"),
                        set_tooltip_text: Some(&tr("Cancel (Esc)")),
                        connect_clicked[sender] => move |_| sender.input(AppMsg::Escape),
                    },
                    gtk::Button {
                        set_label: &tr("Apply"),
                        set_tooltip_text: Some(&tr("Apply (Enter)")),
                        add_css_class: "suggested-action",
                        connect_clicked[sender] => move |_| sender.input(AppMsg::Confirm),
                    },
                },

                #[wrap(Some)]
                set_content = &gtk::Box {
                    append = &gtk::Revealer {
                        set_transition_type: gtk::RevealerTransitionType::SlideRight,
                        #[watch]
                        set_reveal_child: model.sidebar_visible(),

                        #[wrap(Some)]
                        set_child = &gtk::Box {
                            add_css_class: "sidebar-pane",

                            gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,

                                gtk::Box {
                                    add_css_class: "linked",
                                    set_margin_all: 6,
                                    set_halign: gtk::Align::Center,
                                    #[watch]
                                    set_visible: model.sidebar_switchable(),

                                    gtk::ToggleButton {
                                        set_icon_name: "folder-pictures-symbolic",
                                        set_tooltip_text: Some(&tr("Files")),
                                        #[watch]
                                        set_visible: model.list.len() > 1,
                                        #[watch]
                                        set_active: model.mode() == SidebarMode::Files,
                                        connect_clicked[sender] => move |_| sender.input(AppMsg::SidebarMode(SidebarMode::Files)),
                                    },
                                    gtk::ToggleButton {
                                        set_icon_name: "view-paged-symbolic",
                                        set_tooltip_text: Some(&tr("Pages")),
                                        #[watch]
                                        set_active: model.mode() == SidebarMode::Pages,
                                        connect_clicked[sender] => move |_| sender.input(AppMsg::SidebarMode(SidebarMode::Pages)),
                                    },
                                    gtk::ToggleButton {
                                        set_icon_name: "view-list-bullet-symbolic",
                                        set_tooltip_text: Some(&tr("Outline")),
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
                                    set_transition_type: gtk::StackTransitionType::Crossfade,
                                    #[watch]
                                    set_visible_child_name: model.sidebar_page_name(),
                                },
                            },

                            gtk::Separator {},
                        },
                    },

                    append = &gtk::Overlay {
                        set_hexpand: true,
                        #[name = "overlay"]
                        #[wrap(Some)]
                        set_child = &gtk::Stack {
                            set_transition_type: gtk::StackTransitionType::Crossfade,

                            add_named[Some("empty")] = &adw::StatusPage {
                                #[watch]
                                set_icon_name: Some(model.status_icon()),
                                #[watch]
                                set_title: &model.status_title(),
                                #[watch]
                                set_description: Some(&model.status_description()),

                                #[wrap(Some)]
                                set_child = &gtk::Button {
                                    set_label: &tr("Open…"),
                                    set_halign: gtk::Align::Center,
                                    set_action_name: Some("win.open"),
                                    add_css_class: "pill",
                                    add_css_class: "suggested-action",
                                    #[watch]
                                    set_visible: model.document.is_none() && !model.loading,
                                },
                            },

                            #[name = "scroller"]
                            add_named[Some("image")] = &gtk::ScrolledWindow {},

                            #[watch]
                            set_visible_child_name: model.page(),
                        },

                        add_overlay = &adw::Spinner {
                            set_halign: gtk::Align::Center,
                            set_valign: gtk::Align::Center,
                            set_width_request: 48,
                            set_height_request: 48,
                            set_can_target: false,
                            #[watch]
                            set_visible: model.loading,
                        },

                        add_overlay = &gtk::Revealer {
                            set_halign: gtk::Align::Start,
                            set_valign: gtk::Align::Center,
                            set_transition_type: gtk::RevealerTransitionType::Crossfade,
                            #[watch]
                            set_reveal_child: model.controls_shown() && model.can_go_prev(),
                            #[wrap(Some)]
                            #[name = "prev_button"]
                            set_child = &gtk::Button {
                                set_icon_name: "go-previous-symbolic",
                                set_tooltip_text: Some(&tr("Previous (Left)")),
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
                            set_reveal_child: model.controls_shown() && model.can_go_next(),
                            #[wrap(Some)]
                            #[name = "next_button"]
                            set_child = &gtk::Button {
                                set_icon_name: "go-next-symbolic",
                                set_tooltip_text: Some(&tr("Next (Right)")),
                                set_action_name: Some("win.next"),
                                set_margin_all: 12,
                                add_css_class: "osd",
                                add_css_class: "circular",
                            },
                        },

                        // Zoom, and the page of a PDF, floating at the bottom of the view.
                        add_overlay = &gtk::Revealer {
                            set_halign: gtk::Align::Center,
                            set_valign: gtk::Align::End,
                            set_transition_type: gtk::RevealerTransitionType::Crossfade,
                            #[watch]
                            set_reveal_child: model.controls_shown() && model.edit_mode == EditMode::None,
                            #[wrap(Some)]
                            #[name = "view_controls"]
                            set_child = &gtk::Box {
                                set_margin_all: 18,
                                add_css_class: "osd",
                                add_css_class: "toolbar",
                                add_css_class: "view-controls",

                                gtk::Box {
                                    #[watch]
                                    set_visible: model.pdf.is_some(),

                                    gtk::Button {
                                        add_css_class: "flat",
                                        set_icon_name: "go-up-symbolic",
                                        set_tooltip_text: Some(&tr("Previous Page (Page Up)")),
                                        set_action_name: Some("win.page-prev"),
                                    },
                                    #[name = "page_entry"]
                                    gtk::Entry {
                                        set_width_chars: 3,
                                        set_max_width_chars: 5,
                                        gtk::prelude::EditableExt::set_alignment: 1.0,
                                        set_valign: gtk::Align::Center,
                                        set_tooltip_text: Some(&tr("Go to page (Ctrl+G)")),
                                        #[watch]
                                        set_text: &model.page_text(),
                                        connect_activate[sender] => move |entry| {
                                            sender.input(AppMsg::GotoPageText(entry.text().to_string()));
                                        },
                                    },
                                    gtk::Label {
                                        add_css_class: "page-total",
                                        #[watch]
                                        set_label: &model.page_total(),
                                    },
                                    gtk::Button {
                                        add_css_class: "flat",
                                        set_icon_name: "go-down-symbolic",
                                        set_tooltip_text: Some(&tr("Next Page (Page Down)")),
                                        set_action_name: Some("win.page-next"),
                                    },
                                    gtk::Separator {
                                        set_margin_start: 4,
                                        set_margin_end: 4,
                                    },
                                },

                                gtk::Button {
                                    add_css_class: "flat",
                                    set_icon_name: "zoom-out-symbolic",
                                    set_tooltip_text: Some(&tr("Zoom Out (Ctrl+−)")),
                                    set_action_name: Some("win.zoom-out"),
                                },
                                gtk::Button {
                                    add_css_class: "flat",
                                    add_css_class: "zoom-level",
                                    set_tooltip_text: Some(&tr("Fit to Window (Ctrl+0)")),
                                    set_action_name: Some("win.best-fit"),
                                    #[watch]
                                    set_label: &model.zoom_text(),
                                },
                                gtk::Button {
                                    add_css_class: "flat",
                                    set_icon_name: "zoom-in-symbolic",
                                    set_tooltip_text: Some(&tr("Zoom In (Ctrl+=)")),
                                    set_action_name: Some("win.zoom-in"),
                                },
                            },
                        },
                    },

                    append = &gtk::Revealer {
                        set_transition_type: gtk::RevealerTransitionType::SlideLeft,
                        #[watch]
                        set_reveal_child: model.properties_open && model.has_document() && !model.fullscreen,

                        #[wrap(Some)]
                        set_child = &gtk::Box {
                            add_css_class: "sidebar-pane",
                            gtk::Separator {},
                            #[local_ref]
                            properties_root -> gtk::ScrolledWindow {},
                        },
                    },
                },
            },
        }
    }

    fn post_view() {
        let color = model.markup_color();
        if model.style_color.get() != color {
            model.style_color.set(color);
            if let Some(dot) = widgets.style_button.child() {
                dot.queue_draw();
            }
            let mut child = widgets.swatches.first_child();
            while let Some(button) = child {
                if let Some(circle) = button.first_child() {
                    circle.queue_draw();
                }
                child = button.next_sibling();
            }
        }
        // Grey out menu items and shortcuts that do not apply right now.
        let states = [
            ("undo", model.can_undo()),
            ("redo", model.can_redo()),
            ("rotate-right", model.can_edit() || model.can_pages()),
            ("rotate-left", model.can_edit() || model.can_pages()),
            ("flip-h", model.can_edit()),
            ("flip-v", model.can_edit()),
            ("crop", model.can_edit()),
            ("resize", model.can_edit()),
            ("adjust", model.can_edit()),
            ("save", model.can_save()),
            ("save-as", model.can_edit() || model.can_save()),
            ("markup", model.can_markup() || model.markup.open),
            (
                "leave-presentation",
                model.fullscreen || model.slideshow.is_some(),
            ),
            (
                "slideshow",
                model.slideshow.is_some() || model.can_slideshow(),
            ),
            ("delete-pages", model.can_pages()),
            ("insert-pages", model.can_pages()),
            ("export-pages", model.can_pages()),
            ("export-png", model.can_pages()),
            ("export-image-pdf", model.can_edit()),
            ("print", model.has_document()),
        ];
        for (name, enabled) in states {
            if let Some(action) = widgets
                .main_window
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                action.set_enabled(enabled);
            }
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        INPUT.with(|input| *input.borrow_mut() = Some(sender.input_sender().clone()));

        let dirty = Rc::new(Cell::new(false));
        let store = Rc::new(RefCell::new(Store::load(settings::Paths::user())));
        let initial_size = {
            let store = store.borrow();
            if store.settings.remember_state {
                (
                    store.state.window_width.max(400),
                    store.state.window_height.max(300),
                )
            } else {
                (1000, 720)
            }
        };

        let edit_menu = gio::Menu::new();
        let history = gio::Menu::new();
        history.append(Some(&tr("Undo")), Some("win.undo"));
        history.append(Some(&tr("Redo")), Some("win.redo"));
        edit_menu.append_section(None, &history);
        let geometry = gio::Menu::new();
        geometry.append(Some(&tr("Rotate Right")), Some("win.rotate-right"));
        geometry.append(Some(&tr("Rotate Left")), Some("win.rotate-left"));
        geometry.append(Some(&tr("Flip Horizontally")), Some("win.flip-h"));
        geometry.append(Some(&tr("Flip Vertically")), Some("win.flip-v"));
        edit_menu.append_section(None, &geometry);
        let tools = gio::Menu::new();
        tools.append(Some(&tr("Crop…")), Some("win.crop"));
        tools.append(Some(&tr("Resize…")), Some("win.resize"));
        tools.append(Some(&tr("Adjust Colour…")), Some("win.adjust"));
        edit_menu.append_section(None, &tools);
        let export_section = gio::Menu::new();
        export_section.append(Some(&tr("Export as PDF…")), Some("win.export-image-pdf"));
        edit_menu.append_section(None, &export_section);
        let markup_section = gio::Menu::new();
        markup_section.append(Some(&tr("Markup")), Some("win.markup"));
        edit_menu.append_section(None, &markup_section);
        let saving = gio::Menu::new();
        saving.append(Some(&tr("Save")), Some("win.save"));
        saving.append(Some(&tr("Save As…")), Some("win.save-as"));
        edit_menu.append_section(None, &saving);

        let pages_menu = gio::Menu::new();
        pages_menu.append_section(None, &history);
        let page_turns = gio::Menu::new();
        page_turns.append(Some(&tr("Rotate Right")), Some("win.rotate-right"));
        page_turns.append(Some(&tr("Rotate Left")), Some("win.rotate-left"));
        pages_menu.append_section(None, &page_turns);
        let page_edits = gio::Menu::new();
        page_edits.append(Some(&tr("Delete Pages")), Some("win.delete-pages"));
        page_edits.append(
            Some(&tr("Insert Pages from File…")),
            Some("win.insert-pages"),
        );
        pages_menu.append_section(None, &page_edits);
        let page_exports = gio::Menu::new();
        page_exports.append(
            Some(&tr("Export Selected Pages…")),
            Some("win.export-pages"),
        );
        page_exports.append(Some(&tr("Export Page as PNG…")), Some("win.export-png"));
        pages_menu.append_section(None, &page_exports);
        pages_menu.append_section(None, &saving);

        let main_menu = gio::Menu::new();
        let print_section = gio::Menu::new();
        print_section.append(Some(&tr("Print…")), Some("win.print"));
        let paper_size = gio::Menu::new();
        paper_size.append(Some(&tr("Fit to Page")), Some("win.print-scaling::fit"));
        paper_size.append(Some(&tr("Actual Size")), Some("win.print-scaling::actual"));
        print_section.append_submenu(Some(&tr("Size on Paper")), &paper_size);
        main_menu.append_section(None, &print_section);
        let view_section = gio::Menu::new();
        view_section.append(Some(&tr("Fullscreen")), Some("win.fullscreen"));
        view_section.append(Some(&tr("Slideshow")), Some("win.slideshow"));
        main_menu.append_section(None, &view_section);
        let prefs_section = gio::Menu::new();
        prefs_section.append(Some(&tr("Preferences")), Some("win.preferences"));
        main_menu.append_section(None, &prefs_section);
        let about_section = gio::Menu::new();
        about_section.append(Some(&tr("Keyboard Shortcuts")), Some("win.shortcuts"));
        about_section.append(Some(&tr("About Lumen")), Some("win.about"));
        main_menu.append_section(None, &about_section);

        let recent_menu = gio::Menu::new();
        refresh_recent_menu(&recent_menu);
        let strip = FileStrip::new(
            {
                let sender = sender.clone();
                move |index| sender.input(AppMsg::GoTo(index))
            },
            {
                let sender = sender.input_sender().clone();
                move || sender.emit(AppMsg::FileStripChanged)
            },
        );
        let thumbnails = ThumbnailLoader::new({
            let sender = sender.input_sender().clone();
            move |path, data| sender.emit(AppMsg::Thumbnail(path, data))
        });
        let model = App {
            recent_menu: recent_menu.clone(),
            strip,
            page_strip: PageStrip::new(PageStripCallbacks {
                on_activate: Box::new({
                    let sender = sender.clone();
                    move |page| sender.input(AppMsg::GotoPage(page))
                }),
                on_visible_changed: Box::new({
                    let sender = sender.input_sender().clone();
                    move || sender.emit(AppMsg::PageStripChanged)
                }),
                on_reorder: Box::new({
                    let sender = sender.clone();
                    move |pages, before| {
                        sender.input(AppMsg::PageRequest(PageRequest::Move { pages, before }))
                    }
                }),
                on_files: Box::new({
                    let sender = sender.clone();
                    move |paths, before| {
                        sender.input(AppMsg::InsertFiles {
                            paths,
                            at: Some(before),
                        })
                    }
                }),
                on_delete: Box::new({
                    let sender = sender.clone();
                    move || sender.input(AppMsg::DeletePages)
                }),
            }),
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
            listing: 0,
            index: 0,
            generation: 0,
            loading: false,
            sidebar_open: store.borrow().settings.remember_state
                && store.borrow().state.sidebar_open
                || !store.borrow().settings.remember_state,
            controls_awake: false,
            controls_ticket: 0,
            controls_hovered: 0,
            cache: Vec::new(),
            preloading: HashSet::new(),
            awaiting: None,
            canvas: LumenCanvas::default(),
            pdf: None,
            next_key: 0,
            zoom: 0.0,
            scale_factor: 1,
            render_ticket: 0,
            sharp_ticket: 0,
            document: None,
            image: None,
            edit: None,
            edited: None,
            edit_mode: EditMode::None,
            adjust: Adjustments::default(),
            dirty: dirty.clone(),
            overwrite_ok: HashSet::new(),
            after_save: None,
            unprotected_ok: false,
            saving: false,
            markup: MarkupState::default(),
            load_started: None,
            first_view_logged: false,
            fullscreen: false,
            slideshow: None,
            cursor_timer: None,
            cursor_hidden: false,
            store: store.clone(),
            pending_goto: None,
            work: WorkDir::default(),
            reload_generation: 0,
            page_busy: false,
            insert_queue: Vec::new(),
            insert_at: 0,
            crop_choice: 0,
            style_color: Rc::new(Cell::new(annotate::Style::default().stroke)),
            control: control::ControlState::default(),
            signatures: crate::signatures::Signatures::user(),
            signature_button: gtk::MenuButton::builder()
                .icon_name("lumen-signature-symbolic")
                .tooltip_text(tr("Signature"))
                .build(),
            signature_target: None,
            error: None,
        };
        model.apply_settings();
        let model_canvas = model.canvas.clone();
        let model_markup_store = model.markup.store.clone();
        // The style button shows the current colour; the swatches ring the one in use.
        let color_dot = color_circle(None, &model.style_color, 16);
        let swatches = gtk::Grid::new();
        for (i, (name, color)) in markup::SWATCHES.iter().enumerate() {
            let button = gtk::Button::builder()
                .child(&color_circle(Some(*color), &model.style_color, 28))
                .tooltip_text(tr(name))
                .css_classes(["flat", "circular"])
                .build();
            let (sender, color) = (sender.clone(), *color);
            button.connect_clicked(move |_| sender.input(AppMsg::SetColor(color)));
            swatches.attach(&button, (i % 4) as i32, (i / 4) as i32, 1, 1);
        }
        let signature_button = model.signature_button.clone();
        model.refresh_signature_menu(&sender);
        let sidebar_stack = model.sidebar_stack.clone();
        sidebar_stack.add_named(model.strip.root(), Some("files"));
        sidebar_stack.add_named(model.page_strip.root(), Some("pages"));
        sidebar_stack.add_named(model.outline.root(), Some("outline"));
        let properties_root = model.properties.root().clone();
        let widgets = view_output!();
        if store.borrow().settings.remember_state && store.borrow().state.maximized {
            widgets.main_window.maximize();
        }
        if let Some(started) = crate::STARTED.get() {
            tracing::info!(ms = started.elapsed().as_millis() as u64, "window built");
        }
        widgets.style_button.set_child(Some(&color_dot));
        label_icon_buttons(widgets.main_window.upcast_ref());
        for (entry, label) in [
            (
                widgets.page_entry.upcast_ref::<gtk::Widget>(),
                tr("Page number"),
            ),
            (
                widgets.search_entry.upcast_ref::<gtk::Widget>(),
                tr("Search in the document"),
            ),
        ] {
            entry.update_property(&[gtk::accessible::Property::Label(&label)]);
        }

        // Window actions + keyboard shortcuts.
        let open_action = gio::SimpleAction::new("open", None);
        let open_sender = sender.clone();
        open_action.connect_activate(move |_, _| open_sender.input(AppMsg::OpenDialog));
        widgets.main_window.add_action(&open_action);

        let canvas = model_canvas;
        widgets.scroller.set_child(Some(&canvas));
        canvas.set_markup_store(model_markup_store);
        canvas.connect_markup({
            let sender = sender.clone();
            move |event| sender.input(AppMsg::Markup(event))
        });
        canvas.connect_select({
            let sender = sender.clone();
            move |gesture| sender.input(AppMsg::Select(gesture))
        });
        // Ctrl+C copies the selection, but only while the page view has focus, so
        // it never steals the shortcut from a text entry.
        let shortcuts = gtk::ShortcutController::new();
        for (accel, make_msg) in [
            ("<Control>c", (|| AppMsg::Copy) as fn() -> AppMsg),
            ("Escape", || AppMsg::Escape),
            ("Return", || AppMsg::Confirm),
            ("Delete", || AppMsg::MarkupDelete),
            ("Up", || AppMsg::MarkupNudge(0.0, -1.0)),
            ("Down", || AppMsg::MarkupNudge(0.0, 1.0)),
            ("<Shift>Up", || AppMsg::MarkupNudge(0.0, -10.0)),
            ("<Shift>Down", || AppMsg::MarkupNudge(0.0, 10.0)),
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

        let nav_actions: &[NavAction] = &[
            ("page-next", || AppMsg::PageBy(1), &["Page_Down"]),
            ("page-prev", || AppMsg::PageBy(-1), &["Page_Up"]),
            ("page-first", || AppMsg::GotoPage(0), &["Home"]),
            ("page-last", || AppMsg::GotoPage(usize::MAX), &["End"]),
            ("next", || AppMsg::Navigate(1), &["Right", "space"]),
            ("prev", || AppMsg::Navigate(-1), &["Left", "BackSpace"]),
            ("toggle-sidebar", || AppMsg::ToggleSidebar, &["F9"]),
            (
                "toggle-properties",
                || AppMsg::ToggleProperties,
                &["<Alt>Return"],
            ),
            ("find-next", || AppMsg::SearchStep(1), &["F3"]),
            ("find-prev", || AppMsg::SearchStep(-1), &["<Shift>F3"]),
            ("find-close", || AppMsg::SearchOpened(false), &[]),
            ("copy", || AppMsg::Copy, &[]),
            ("print", || AppMsg::Print(None), &["<Control>p"]),
            ("undo", || AppMsg::Undo, &["<Control>z"]),
            (
                "redo",
                || AppMsg::Redo,
                &["<Control><Shift>z", "<Control>y"],
            ),
            (
                "rotate-right",
                || AppMsg::Edit(EditOp::RotateCw),
                &["<Control>r"],
            ),
            (
                "rotate-left",
                || AppMsg::Edit(EditOp::RotateCcw),
                &["<Control><Shift>r"],
            ),
            ("flip-h", || AppMsg::Edit(EditOp::FlipH), &[]),
            ("flip-v", || AppMsg::Edit(EditOp::FlipV), &[]),
            (
                "crop",
                || AppMsg::EnterMode(EditMode::Crop),
                &["<Control>k"],
            ),
            ("resize", || AppMsg::ResizeDialog, &["<Control><Alt>r"]),
            (
                "adjust",
                || AppMsg::EnterMode(EditMode::Adjust),
                &["<Control><Alt>c"],
            ),
            ("save", || AppMsg::Save, &["<Control>s"]),
            ("save-as", || AppMsg::SaveAs, &["<Control><Shift>s"]),
            ("apply-tool", || AppMsg::Confirm, &[]),
            ("markup", || AppMsg::ToggleMarkup, &["<Control><Shift>a"]),
            ("preferences", || AppMsg::Preferences, &["<Control>comma"]),
            ("fullscreen", || AppMsg::ToggleFullscreen, &["F11"]),
            ("slideshow", || AppMsg::ToggleSlideshow, &["F5"]),
            ("leave-presentation", || AppMsg::LeavePresentation, &[]),
            ("about", || AppMsg::About, &[]),
            (
                "shortcuts",
                || AppMsg::Shortcuts,
                &["<Control>question", "<Control>slash"],
            ),
            ("delete-pages", || AppMsg::DeletePages, &[]),
            ("insert-pages", || AppMsg::InsertDialog, &[]),
            ("export-pages", || AppMsg::ExportPagesDialog, &[]),
            ("export-png", || AppMsg::ExportPngDialog, &[]),
            ("export-image-pdf", || AppMsg::ExportImagePdfDialog, &[]),
        ];
        let app = relm4::main_application();
        // Application accelerators run before the focused widget sees the key, so a plain
        // key such as Backspace or Space would never reach a text entry. Those go through
        // a shortcut controller that steps aside while the user is typing or in a dialog.
        let plain_keys = gtk::ShortcutController::new();
        plain_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        for &(name, make_msg, accels) in nav_actions {
            let action = gio::SimpleAction::new(name, None);
            let sender = sender.clone();
            action.connect_activate(move |_, _| sender.input(make_msg()));
            widgets.main_window.add_action(&action);
            let (plain, chords): (Vec<&str>, Vec<&str>) =
                accels.iter().partition(|accel| is_plain_key(accel));
            app.set_accels_for_action(&format!("win.{name}"), &chords);
            for accel in plain {
                let name = name.to_owned();
                plain_keys.add_shortcut(gtk::Shortcut::new(
                    gtk::ShortcutTrigger::parse_string(accel),
                    Some(gtk::CallbackAction::new(move |widget, _| {
                        let Some(window) = widget.downcast_ref::<gtk::ApplicationWindow>() else {
                            return glib::Propagation::Proceed;
                        };
                        match window.lookup_action(&name) {
                            Some(action) if action.is_enabled() && !typing(window) => {
                                action.activate(None);
                                glib::Propagation::Stop
                            }
                            _ => glib::Propagation::Proceed,
                        }
                    })),
                ));
            }
        }
        widgets.main_window.add_controller(plain_keys);

        // Escape that nothing else wanted (the view handles it first when focused):
        // leave fullscreen or the slideshow.
        let escape = gtk::ShortcutController::new();
        escape.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::CallbackAction::new({
                let sender = sender.clone();
                move |_, _| {
                    sender.input(AppMsg::Escape);
                    glib::Propagation::Stop
                }
            })),
        ));
        widgets.main_window.add_controller(escape);

        // Ctrl+F opens the search bar, but only for PDFs.
        let find_action = gio::SimpleAction::new("find", None);
        {
            let (bar, entry, canvas) = (
                widgets.search_bar.clone(),
                widgets.search_entry.clone(),
                canvas.clone(),
            );
            find_action.connect_activate(move |_, _| {
                if canvas.has_pages() {
                    bar.set_search_mode(true);
                    entry.grab_focus();
                    entry.select_region(0, -1);
                }
            });
        }
        widgets.main_window.add_action(&find_action);

        // Radio items in the print menu: how big the content is on paper.
        let scaling_action = gio::SimpleAction::new_stateful(
            "print-scaling",
            Some(glib::VariantTy::STRING),
            &"fit".to_variant(),
        );
        scaling_action.connect_activate(|action, choice| {
            if let Some(choice) = choice.and_then(|c| c.str()) {
                print::set_actual_size(choice == "actual");
                action.set_state(&choice.to_variant());
            }
        });
        widgets.main_window.add_action(&scaling_action);

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
                let step = if state.contains(gdk::ModifierType::SHIFT_MASK) {
                    -1
                } else {
                    1
                };
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
        for control in [
            widgets.view_controls.upcast_ref::<gtk::Widget>(),
            widgets.prev_button.upcast_ref(),
            widgets.next_button.upcast_ref(),
        ] {
            let motion = gtk::EventControllerMotion::new();
            let enter_sender = sender.clone();
            motion.connect_enter(move |_, _, _| enter_sender.input(AppMsg::ControlsHover(true)));
            let leave_sender = sender.clone();
            motion.connect_leave(move |_| leave_sender.input(AppMsg::ControlsHover(false)));
            control.add_controller(motion);
        }

        // Any pointer movement brings the cursor back in fullscreen.
        let pointer = gtk::EventControllerMotion::new();
        pointer.set_propagation_phase(gtk::PropagationPhase::Capture);
        pointer.connect_motion({
            let sender = sender.clone();
            // Every message updates the whole view, so a few per second are plenty.
            let last = Cell::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
            move |_, _, _| {
                if last.get().elapsed() >= std::time::Duration::from_millis(200) {
                    last.set(std::time::Instant::now());
                    sender.input(AppMsg::PointerMoved);
                }
            }
        });
        widgets.main_window.add_controller(pointer);

        let view_actions: [ViewAction; 4] = [
            (
                "zoom-in",
                LumenCanvas::zoom_in,
                &["<Control>plus", "<Control>equal"],
            ),
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

        control::register(&sender);

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
            sender.input(AppMsg::Add(paths));
            true
        });
        widgets.main_window.add_controller(drop);

        // GTK puts the window's actions on D-Bus (where scripts and tests reach them) on
        // Wayland and X11 only. Do the same on Broadway, so the UI can be driven headless.
        if WidgetExt::display(&widgets.main_window).type_().name() == "GdkBroadwayDisplay"
            && let (Some(bus), Some(path)) = (app.dbus_connection(), app.dbus_object_path())
            && let Err(err) =
                bus.export_action_group(&format!("{path}/window/1"), &widgets.main_window)
        {
            tracing::warn!("could not export the window actions: {err}");
        }

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        self.update_inner(msg, sender);
        // A PDF is edited as long as its history can go back; undoing everything makes it clean again.
        self.markup.pdf_dirty = self.pdf.is_some() && self.markup.pdf_history.can_undo();
        self.dirty.set(self.is_dirty());
        // Whatever the message did, the canvas must show the current document.
        self.sync_canvas();
    }
}

impl App {
    fn update_inner(&mut self, msg: AppMsg, sender: ComponentSender<Self>) {
        match msg {
            AppMsg::OpenDialog => Self::show_open_dialog(sender),
            AppMsg::Open(paths) => {
                if !self.guard(Pending::Open(paths.clone()), &sender) {
                    return;
                }
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
            AppMsg::Navigate(delta) => {
                // With an annotation selected, the arrow keys move it instead.
                if self.markup_nudge(delta.signum() as f64, 0.0, &sender) {
                    return;
                }
                if !self.guard(Pending::Navigate(delta), &sender) {
                    return;
                }
                let target = self
                    .index
                    .saturating_add_signed(delta)
                    .min(self.list.len().saturating_sub(1));
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
                    self.page_strip
                        .set_thumbnail(job.page, rendered.into_texture());
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
                        self.follow_link(target);
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
            AppMsg::Copy => self.request_selection(true),
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
            AppMsg::EditRendered { ticket, result } => self.edit_rendered(ticket, result),
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
            AppMsg::ShowProperties(show) => self.properties_open = show,
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

/// Ask for the password of an encrypted PDF; the answer comes back as a message.
fn ask_password(name: &str, retry: bool, sender: ComponentSender<App>) {
    let body = if retry {
        tr_fmt(
            "The password for “{name}” was incorrect. Try again.",
            &[("name", name)],
        )
    } else {
        tr_fmt(
            "“{name}” is protected. Enter its password to open it.",
            &[("name", name)],
        )
    };
    let dialog = adw::AlertDialog::new(Some(&tr("Password Required")), Some(&body));
    crate::i18n::add_responses(
        &dialog,
        &[
            ("cancel", crate::i18n::n_("_Cancel")),
            ("open", crate::i18n::n_("_Open")),
        ],
    );
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

/// Rebuild the "Recent Files" menu from the desktop-wide recent list.
fn refresh_recent_menu(menu: &gio::Menu) {
    menu.remove_all();

    let mut items: Vec<_> = gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter(|item| {
            item.is_local() && item.exists() && DocumentKind::from_mime(&item.mime_type()).is_some()
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
        menu.append(Some(&tr("No Recent Files")), Some("win.none"));
    }
}

/// A circle of `color` (or of the shared current colour, for `None`), `size` pixels wide.
/// A fixed colour gets a ring around it while it is the current one.
fn color_circle(
    color: Option<annotate::Color>,
    current: &Rc<Cell<annotate::Color>>,
    size: i32,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(size)
        .content_height(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let current = Rc::clone(current);
    area.set_draw_func(move |area, cr, w, h| {
        let shown = color.unwrap_or_else(|| current.get());
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let ring = color.is_some_and(|c| c == current.get());
        let radius = cx.min(cy) - if color.is_some() { 4.0 } else { 1.0 };
        cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(
            shown.r as f64,
            shown.g as f64,
            shown.b as f64,
            shown.a as f64,
        );
        let _ = cr.fill_preserve();
        // A faint edge, so white and black still read on any background.
        let fg = area.color();
        cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.2);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        if ring {
            cr.arc(cx, cy, radius + 2.5, 0.0, std::f64::consts::TAU);
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.9);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
    });
    area
}

/// Render the window, as it is on screen, into a PNG file.
fn screenshot(window: &adw::ApplicationWindow, path: &str) -> Result<(), String> {
    let (width, height) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot.to_node().ok_or("the window has nothing to draw")?;
    let renderer = window.renderer().ok_or("the window is not realized")?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    renderer
        .render_texture(node, Some(&bounds))
        .save_to_png(path)
        .map_err(|err| err.to_string())
}

/// Whether `accel` is a key without modifiers that text entries also use (not a function key).
fn is_plain_key(accel: &str) -> bool {
    !accel.starts_with('<') && !(accel.starts_with('F') && accel[1..].parse::<u8>().is_ok())
}

/// Whether keys should go to something other than the view: a text field, a dialog
/// shown over the window, or an open menu.
fn typing(window: &gtk::ApplicationWindow) -> bool {
    let Some(focus) = gtk::prelude::RootExt::focus(window) else {
        return false;
    };
    let in_window = focus.native().is_some_and(|native| {
        native.upcast_ref::<gtk::Widget>() == window.upcast_ref::<gtk::Widget>()
    });
    focus.is::<gtk::Text>()
        || focus.is::<gtk::TextView>()
        || focus.ancestor(adw::Dialog::static_type()).is_some()
        || !in_window
}

/// Give buttons that show only an icon an accessible name, taken from their tooltip, so
/// screen readers announce "Print" instead of nothing. Called once on the finished window.
fn label_icon_buttons(widget: &gtk::Widget) {
    let icon_only = widget
        .downcast_ref::<gtk::Button>()
        .is_some_and(|b| b.label().is_none())
        || widget.is::<gtk::MenuButton>()
        || widget.is::<gtk::ColorDialogButton>();
    if let (true, Some(tooltip)) = (icon_only, widget.tooltip_text()) {
        widget.update_property(&[gtk::accessible::Property::Label(&tooltip)]);
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        label_icon_buttons(&current);
        child = current.next_sibling();
    }
}

#[cfg(test)]
mod tests {
    use super::is_plain_key;

    #[test]
    fn plain_keys_are_those_text_entries_use() {
        for key in ["Right", "space", "BackSpace", "Page_Down", "Home", "Escape"] {
            assert!(is_plain_key(key), "{key}");
        }
        for key in ["<Control>p", "<Shift>F3", "F3", "F11", "<Alt>Return"] {
            assert!(!is_plain_key(key), "{key}");
        }
    }
}
