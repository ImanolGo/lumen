//! The application: state, messages, update and view (Elm architecture, via iced).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::mouse::{self, ScrollDelta};
use iced::theme::Mode;
use iced::widget::{
    Id, button, center, column, container, image, mouse_area, operation, row, scrollable, sensor, space, stack, svg,
    text, text_input, tooltip,
};
use iced::{
    Alignment, ContentFit, Element, Event, Font, Length, Padding, Point, Size, Subscription, Task, Theme, event,
    system, window,
};

mod anim;
mod chooser;
mod commands;
mod control;
mod edit;
mod export;
mod forms;
mod markup;
mod menu;
mod overlay;
mod pagelist;
mod pages;
mod palette;
mod prefs;
mod printing;
mod signature;
mod slideshow;
#[cfg(test)]
mod tests;

use anim::Anim;
use chooser::{Chooser, ChooserMsg, Purpose};
use edit::{CropState, EditMsg, PicEdit, ResizeForm};
use export::{ExportJob, ExportMsg};
use forms::{FormMsg, FormState};
use markup::{DocMarkup, MarkupMsg, TextEdit};
use menu::{Dialog, Menu};
use pagelist::PageThumbs;
use pages::{Organizer, PagesMsg};
use printing::{PrintDialog, PrintMsg};
use signature::{Pad, SignMsg};
use slideshow::Slideshow;

use crate::annotate::{self, Editor};
use crate::i18n::tr;
use crate::library::{self, Kind};
use crate::pdf;
use crate::print::{self, Outcome};
use crate::raster::{self, Picture};
use crate::structure::{PageText, Target};
use crate::{icons, style};
use hayro::kurbo::{Point as PagePoint, Rect};

// ---------------------------------------------------------------- constants

const TOOLBAR_H: f32 = 48.0;
const SIDEBAR_W: f32 = 196.0;
/// Height of one sidebar row; rows are fixed-height so the list can be virtualised.
const ROW_H: f32 = 140.0;
/// Height of the picture in a sidebar row; the file name goes underneath.
const ROW_THUMB: f32 = 102.0;
/// How long a zoom change takes to ease in.
/// How many recent files the empty window lists.
const RECENT_SHOWN: usize = 5;

/// How often an open file is looked at when it cannot be watched.
const FALLBACK_LOOK: Duration = Duration::from_secs(3);

const ZOOM_EASE: Duration = Duration::from_millis(170);
/// How long the floating controls stay after the pointer stops moving.
const CONTROLS_LINGER: Duration = Duration::from_millis(1800);
/// Height of one table-of-contents row.
const TOC_ROW_H: f32 = 30.0;
/// How many thumbnails are decoded at once.
const THUMB_PARALLEL: usize = 4;
/// How far either side of the current file thumbnails are prepared.
const THUMB_RADIUS: usize = 40;
/// Decoded pictures kept around for instant back/forward (at most this many, and within the cache size in the preferences).
const PICTURE_CACHE: usize = 5;
/// Threads for decoding and other blocking work.
const BLOCKING_THREADS: usize = 3;
const PAGE_GAP: f32 = 16.0;
const PAGE_PAD: f32 = 24.0;
/// 100 % zoom for PDFs: 96 logical pixels per inch.
const PT_TO_PX: f32 = 96.0 / 72.0;
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 32.0;
const ZOOM_STEP: f32 = 1.25;

fn scroll_id() -> Id {
    Id::new("content")
}
fn sidebar_id() -> Id {
    Id::new("sidebar")
}
fn contents_id() -> Id {
    Id::new("contents")
}
fn search_input_id() -> Id {
    Id::new("search")
}
fn password_input_id() -> Id {
    Id::new("password")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Contents,
    Pages,
}

/// One search match: the boxes (one per line) it covers on its page, in page points.
struct Hit {
    page: usize,
    rects: Vec<Rect>,
}

#[derive(Default)]
struct Search {
    query: String,
    /// Sorted by page, then by position on the page.
    hits: Vec<Hit>,
    current: Option<usize>,
    /// Pages searched so far, and whether all are done.
    scanned: usize,
    done: bool,
    seq: u64,
    /// Kept when the query changes.
    options: crate::structure::SearchOptions,
    /// Aborts the running scan when replaced or dropped.
    _scan: Option<iced::task::Handle>,
}

// ---------------------------------------------------------------- state

pub struct App {
    mode: Mode,
    /// What the desktop says; `mode` is that unless the preferences or `LUMEN_THEME` say otherwise.
    system_mode: Mode,
    /// The preferences, kept in `~/.config/lumen-rs/settings.toml`.
    prefs: crate::settings::Settings,
    theme: Theme,
    window: Option<window::Id>,
    scale_factor: f32,
    fullscreen: bool,
    show_sidebar: bool,
    /// Scroll offset and height of the sidebar list (logical px).
    sidebar_scroll: (f32, f32),
    /// The tab the user picked; until then a PDF's contents are preferred over the file list.
    sidebar_tab: Option<SidebarTab>,
    contents_scroll: (f32, f32),
    /// Thumbnails for the Pages tab.
    page_thumbs: PageThumbs,
    /// The go-to-page box, with what has been typed.
    goto: Option<String>,
    /// Night mode for PDF pages (this session only until there are preferences).
    night: bool,
    search: Option<Search>,
    search_seq: u64,
    selection: Option<TextSel>,
    /// A drag that extends the selection is in progress.
    selecting: bool,
    /// The page under the pointer and the pointer's position on it, in page points.
    hover: Option<(usize, PagePoint)>,
    hover_text: bool,
    /// When the file now loading was asked for, for the `opened` timing log.
    load_started: Option<Instant>,
    first_view_logged: bool,
    /// Size of the area pictures and pages are drawn in (logical px).
    area: Size,
    viewport: Viewport,
    /// Where the PDF was before each link was followed, and where Back came from.
    links: LinkHistory,
    /// The open file's size and modification time as last seen, to notice another program
    /// changing it. `None` until the next check, which then takes the file as it is.
    disk: Option<(PathBuf, Option<Stamp>)>,
    /// Recent files, PDF pages and the window size, kept between runs.
    state: crate::state::State,
    state_save_pending: bool,
    /// The file cannot be watched with inotify, so it is looked at now and then instead.
    watch_failed: bool,
    /// Where to put a PDF back after it is reloaded.
    restore: Option<Place>,
    modifiers: Modifiers,
    pan: Option<Option<Point>>,

    files: Vec<PathBuf>,
    current: usize,
    /// Bumped whenever the shown file changes, so late results are ignored.
    generation: u64,
    doc: Doc,

    pictures: VecDeque<(PathBuf, Loaded)>,
    preloading: HashSet<PathBuf>,
    thumbs: HashMap<PathBuf, Option<image::Handle>>,
    /// Password-protected PDFs whose thumbnail waits for the password.
    locked_thumbs: HashSet<PathBuf>,
    thumbs_running: usize,

    pending_drops: Vec<PathBuf>,
    drop_hover: bool,
    /// A message at the bottom of the window, and whether it reports a problem.
    toast: Option<(u64, String, bool)>,
    toast_seq: u64,
    printing: bool,

    /// The markup bar is showing and the pointer draws.
    markup_open: bool,
    editor: Editor,
    swatch: usize,
    /// Line width and text size for new markup, in [`DocMarkup::unit`]s.
    line_width: f64,
    text_size: f64,
    /// The markup style menu, its colour picker, and what is typed in it.
    style_menu: bool,
    picker_open: bool,
    picker_hue: f32,
    hex_input: String,
    size_input: String,
    /// The annotations before a live style change, to record it as one undo step.
    style_from: Option<markup::Layers>,
    /// Markup of the file on screen.
    marks: Option<DocMarkup>,
    /// Unsaved markup of other files, until they are shown again.
    stash: HashMap<PathBuf, DocMarkup>,
    editing: Option<TextEdit>,
    /// Preparing a PDF for markup, or saving: what to say meanwhile.
    markup_busy: Option<&'static str>,
    last_drag: Option<(f64, f64)>,
    confirm_quit: bool,
    /// Passwords of the protected PDFs opened this session, kept in memory only.
    passwords: HashMap<PathBuf, String>,
    password_input: String,

    /// The form of the PDF on screen, and the file it belongs to.
    form: Option<FormState>,
    form_path: Option<PathBuf>,
    /// Unsaved form values of other files.
    stash_forms: HashMap<PathBuf, Vec<crate::forms::Value>>,
    signatures: crate::signatures::Signatures,
    sign_menu: bool,
    pad: Option<Pad>,
    /// The signature field being signed.
    sign_target: Option<(usize, annotate::Rect)>,
    /// A signature to place once the PDF is ready for markup.
    pending_sign: Option<(crate::signatures::Signature, usize, annotate::Rect)>,
    /// The picture editing bar is showing.
    edit_open: bool,
    crop: Option<CropState>,
    adjust_open: bool,
    resize_form: Option<ResizeForm>,
    /// Unsaved edits of other pictures.
    stash_edits: HashMap<PathBuf, PicEdit>,
    /// The page organiser of the open PDF.
    organizer: Option<Organizer>,
    /// The floating zoom and previous/next controls fade in while the pointer moves over the view.
    controls: iced::Animation<bool>,
    now: Instant,
    last_pointer: Instant,
    hide_pending: bool,
    over_controls: bool,
    zoom_ease: Option<ZoomEase>,
    /// Where the pointer is over the view (logical px from its top-left), to zoom around it.
    pointer_at: Option<Point>,
    /// The scale of the touchpad pinch in progress, since it began.
    pinch_scale: f32,
    menu: Option<Menu>,
    dialog: Option<Dialog>,
    /// What the Properties dialog shows; none while it is being read.
    properties: Option<Result<crate::properties::Sheet, String>>,
    /// What the file chooser's answer will export.
    export_job: Option<ExportJob>,
    /// Exports started through the control interface that have not finished.
    export_running: usize,
    /// The last thing a toast said (kept for the control interface).
    last_toast: Option<(String, bool)>,
    /// A file is being found for the control interface's `Open`.
    control_opening: bool,
    /// The file last moved to the trash (and where it sat in the list), while its toast offers Undo: the toast's id comes first.
    /// The slideshow, while one is running.
    slideshow: Option<Slideshow>,
    trash_undo: Option<(u64, usize, crate::trash::Trashed)>,
    /// The toast that offers to undo deleting pages in the organiser.
    pages_undo: Option<u64>,
    /// The command palette, while it is open.
    palette: Option<commands::Palette>,
    /// Lumen's own file chooser and print dialog, when the desktop has no portal for them.
    chooser: Option<Chooser>,
    print_dialog: Option<PrintDialog>,
}

/// A zoom change easing in: from one zoom to another, and what to settle on.
#[derive(Debug, Clone, Copy)]
struct ZoomEase {
    from: f32,
    to: f32,
    start: Instant,
    end: ZoomEnd,
    /// The point of the view that stays put; the middle when `None`.
    anchor: Option<Point>,
}

#[derive(Debug, Clone, Copy)]
enum ZoomEnd {
    /// A picture's zoom; `None` fits it to the window.
    Image(Option<f32>),
    Pdf(PdfFit),
}

/// A file's modification time and size; if either differs, the file changed.
type Stamp = (std::time::SystemTime, u64);

/// The file as it is now, to tell later whether another program changed it.
fn disk_baseline(path: &Path) -> (PathBuf, Option<Stamp>) {
    (path.to_path_buf(), std::fs::metadata(path).and_then(|m| Ok((m.modified()?, m.len()))).ok())
}

/// A place in a PDF that survives zooming: a page, how far down it, and how far across.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    page: usize,
    /// Points from the top of the page to the top of the view (negative above the page).
    down: f32,
    across: f32,
}

/// Back and forward after following a link or a contents entry (Alt+Left, Alt+Right).
#[derive(Debug, Default)]
struct LinkHistory {
    back: Vec<Place>,
    forward: Vec<Place>,
}

impl LinkHistory {
    /// Remembers `here` before a jump; whatever was undone by Back is forgotten.
    fn leave(&mut self, here: Place) {
        if self.back.last() != Some(&here) {
            self.back.push(here);
        }
        self.forward.clear();
    }

    /// The place to go back to, remembering `here` for Forward.
    fn back(&mut self, here: Place) -> Option<Place> {
        let to = self.back.pop()?;
        self.forward.push(here);
        Some(to)
    }

    fn forward(&mut self, here: Place) -> Option<Place> {
        let to = self.forward.pop()?;
        self.back.push(here);
        Some(to)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Viewport {
    x: f32,
    y: f32,
    height: f32,
}

/// A decoded picture plus its GPU-ready handle (kept so re-showing it is free).
#[derive(Debug, Clone)]
struct Loaded {
    handle: image::Handle,
    width: u32,
    height: u32,
    picture: Picture,
}

impl From<Picture> for Loaded {
    fn from(p: Picture) -> Self {
        let handle = image::Handle::from_rgba(p.width, p.height, p.rgba.clone());
        Self { handle, width: p.width, height: p.height, picture: p }
    }
}

enum Doc {
    Empty,
    Loading(PathBuf),
    Failed {
        path: PathBuf,
        error: String,
    },
    /// A password-protected PDF waiting for its password.
    Locked {
        path: PathBuf,
        wrong: bool,
    },
    Image(Box<ImageView>),
    Pdf(PdfView),
}

struct ImageView {
    path: PathBuf,
    /// The picture as opened; `shown` is it with `edit` applied.
    original: Picture,
    shown: Loaded,
    /// `None` fits the picture to the window (never enlarging it).
    zoom: Option<f32>,
    edit: PicEdit,
    /// Playing, for an animated GIF, APNG or WebP: the first frame stays in `shown` for the tools.
    anim: Option<Anim>,
    /// Whether the file holds several frames (from its header; `anim` only learns that by playing).
    animated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdfFit {
    Width,
    Page,
    Manual,
}

struct PdfView {
    doc: pdf::Document,
    /// Logical pixels per PDF point.
    zoom: f32,
    fit: PdfFit,
    /// Top of each page in content coordinates, plus the total height at the end.
    tops: Vec<f32>,
    slots: Vec<Slot>,
    current_page: usize,
}

#[derive(Default)]
struct Slot {
    shown: Option<(image::Handle, f32)>,
    pending: Option<f32>,
    /// The page's text, for selection; asked for once the page comes into view.
    text: Option<Arc<PageText>>,
    text_pending: bool,
}

/// A text selection in a PDF: two carets, each a page and a position between glyphs on it.
#[derive(Debug, Clone, Copy)]
struct TextSel {
    anchor: (usize, usize),
    head: (usize, usize),
}

impl TextSel {
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The caret range selected on `page`, if any.
    fn on_page(&self, page: usize, len: usize) -> Option<std::ops::Range<usize>> {
        let (a, b) = self.ordered();
        if page < a.0 || page > b.0 {
            return None;
        }
        let start = if page == a.0 { a.1.min(len) } else { 0 };
        let end = if page == b.0 { b.1.min(len) } else { len };
        (start < end).then_some(start..end)
    }
}

impl PdfView {
    fn layout(&mut self) {
        let mut y = PAGE_PAD;
        self.tops.clear();
        for &(_, h) in self.doc.pages() {
            self.tops.push(y);
            y += h * self.zoom + PAGE_GAP;
        }
        self.tops.push(y - PAGE_GAP + PAGE_PAD);
    }

    fn content_width(&self) -> f32 {
        let widest = self.doc.pages().iter().map(|p| p.0).fold(0.0, f32::max);
        widest * self.zoom + 2.0 * PAGE_PAD
    }

    fn page_at(&self, y: f32) -> usize {
        let n = self.doc.page_count();
        match self.tops[..n].binary_search_by(|t| t.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Less)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
        .min(n - 1)
    }

    fn visible(&self, vp: &Viewport) -> std::ops::RangeInclusive<usize> {
        let h = if vp.height > 0.0 { vp.height } else { 1000.0 };
        self.page_at(vp.y)..=self.page_at(vp.y + h)
    }
}

// ---------------------------------------------------------------- messages

#[derive(Debug, Clone)]
pub enum Message {
    SystemTheme(Mode),
    Pref(prefs::PrefMsg),
    Control(crate::control::Shared),
    WindowEvent(window::Id, window::Event),
    ScaleFactor(f32),
    Key(keyboard::Event),
    Wheel(ScrollDelta),
    AreaResized(Size),
    Scrolled(Viewport),

    Open {
        folder: bool,
    },
    Chosen(Result<Vec<PathBuf>, String>),
    Expanded {
        files: Vec<PathBuf>,
        start: usize,
        append: bool,
    },
    FlushDrops,
    Select(usize),
    Next,
    Prev,
    First,
    Last,

    PictureLoaded(u64, PathBuf, Result<Picture, String>),
    Preloaded(PathBuf, Result<Picture, String>),
    PdfOpened(u64, PathBuf, Result<pdf::Opened, pdf::OpenError>),
    PasswordInput(String),
    Unlock,
    /// A rendered page: generation, page, scale, night mode it was drawn for, bitmap.
    PageRendered(u64, usize, f32, bool, Result<Picture, String>),
    ToggleNight,
    /// A thumbnail, or none; `true` when the file is password-protected and its password not known.
    Thumb(PathBuf, Option<Picture>, bool),
    SidebarScrolled(f32, f32),
    SidebarTab(SidebarTab),
    ContentsScrolled(f32, f32),
    PageThumbsScrolled(f32, f32),
    PageThumb(u64, usize, bool, Result<Picture, String>),
    GoToPage(usize),
    GoToOpen,
    GoToInput(String),
    GoToSubmit,
    GoToClose,
    GoTo(Target),
    LinkOpened(Result<(), String>),

    LinkBack,
    LinkForward,
    CopyPicture,
    PictureCopied(Result<(), String>),
    Palette(commands::PaletteMsg),
    /// Time to see whether another program changed the open file.
    CheckFile,
    /// The watcher's word: something happened to the file (`Some`), or it cannot be watched (`None`).
    FileEvent(Option<()>),
    FileStat(PathBuf, Option<Stamp>),
    /// Write the remembered state to disk (a moment after it changed).
    SaveState,
    OpenRecent(PathBuf),
    ClearRecent,
    SearchOpen,
    SearchClose,
    SearchInput(String),
    SearchSubmit,
    SearchMatchCase,
    SearchWholeWords,
    SearchNext,
    SearchPrev,
    SearchHits(u64, usize, Vec<Vec<Rect>>),
    SearchDone(u64),
    Escape,

    PageText(u64, usize, Arc<PageText>),
    PageHover(usize, Point),
    PagePress(usize),
    PageDoubleClick(usize),
    MouseUp,
    Copy,
    SelectAll,
    Copied(String, bool),

    ZoomIn,
    ZoomOut,
    ZoomFit,
    ZoomFitWidth,
    ZoomActual,
    Rotate(i8),
    PageNext,
    PagePrev,

    PanStart,
    PanMove(Point),
    PanEnd,

    Print,
    /// A print finished, or could not start; with what was being printed.
    Printed(PathBuf, Result<Outcome, String>),
    ToggleFullscreen,
    ToggleSidebar,
    DismissToast(u64),
    ShowInFolder,
    Slideshow,
    SlideshowPause,
    SlideshowTick(Instant),
    AnimTick(Instant),
    OpenWith,
    Trash,
    Trashed(usize, Result<crate::trash::Trashed, String>),
    UndoTrash,
    Restored(usize, Result<PathBuf, String>),
    FileActionDone(Result<(), String>),
    Quit,
    Markup(MarkupMsg),
    Form(FormMsg),
    Sign(SignMsg),
    Edit(EditMsg),
    Pages(PagesMsg),
    /// The pointer moved over the view, or left it.
    Pointer(Point),
    PointerLeft,
    /// A touchpad pinch changed its scale by this much (0 when one begins or ends).
    Pinch(f32),
    /// The Delete key, whatever has focus; and the modifier keys, likewise.
    DeleteKey,
    Modifiers(Modifiers),
    /// Open a header menu (or close it with `None`), or pick one of its items.
    Menu(Option<Menu>),
    MenuPick(Box<Message>),
    Dialog(Option<Dialog>),
    ShowProperties,
    Export(ExportMsg),
    PropertiesRead(Result<crate::properties::Sheet, String>),
    Chooser(ChooserMsg),
    Printing(PrintMsg),
    ControlsCheck,
    OverControls(bool),
    Frame(Instant),
}

fn err_string(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Runs CPU-bound or blocking work off the UI thread. A panic in there (say, a
/// decoder choking on a hostile file) becomes an error instead of a crash.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    worker_pool().send(Box::new(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let _ = tx.send(result);
    }));
    rx.await.map_err(|e| e.to_string())?.map_err(|_| "the file could not be decoded".to_string())
}

type Job = Box<dyn FnOnce() + Send>;

/// A few long-lived threads for blocking work. Unlike tokio's blocking pool, which
/// starts a new thread for every burst, this keeps the number of threads (and with
/// it glibc's per-thread malloc arenas, which hold on to freed pixels) small.
struct Pool(std::sync::mpsc::Sender<Job>);

impl Pool {
    fn send(&self, job: Job) {
        let _ = self.0.send(job);
    }
}

fn worker_pool() -> &'static Pool {
    static POOL: std::sync::OnceLock<Pool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        for i in 0..BLOCKING_THREADS {
            let rx = rx.clone();
            let _ = std::thread::Builder::new().name(format!("lumen-worker-{i}")).spawn(move || {
                loop {
                    let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                }
            });
        }
        Pool(tx)
    })
}

// ---------------------------------------------------------------- boot

impl App {
    pub fn new(paths: Vec<PathBuf>) -> (Self, Task<Message>) {
        let app = Self {
            mode: Mode::Light,
            system_mode: Mode::Light,
            prefs: crate::settings::Settings::default(),
            theme: style::theme(Mode::Light),
            window: None,
            scale_factor: 1.0,
            fullscreen: false,
            show_sidebar: true,
            sidebar_scroll: (0.0, 0.0),
            sidebar_tab: None,
            contents_scroll: (0.0, 0.0),
            page_thumbs: PageThumbs::default(),
            goto: None,
            night: false,
            search: None,
            search_seq: 0,
            selection: None,
            selecting: false,
            hover: None,
            hover_text: false,
            load_started: None,
            first_view_logged: false,
            area: Size::new(1100.0, 700.0),
            viewport: Viewport::default(),
            links: LinkHistory::default(),
            disk: None,
            state: if cfg!(test) { crate::state::State::default() } else { crate::state::State::load() },
            state_save_pending: false,
            watch_failed: false,
            restore: None,
            modifiers: Modifiers::default(),
            pan: None,
            files: Vec::new(),
            current: 0,
            generation: 0,
            doc: Doc::Empty,
            pictures: VecDeque::new(),
            preloading: HashSet::new(),
            thumbs: HashMap::new(),
            locked_thumbs: HashSet::new(),
            thumbs_running: 0,
            pending_drops: Vec::new(),
            drop_hover: false,
            toast: None,
            toast_seq: 0,
            printing: false,
            markup_open: false,
            editor: Editor::default(),
            swatch: 0,
            line_width: 3.0,
            text_size: 18.0,
            style_menu: false,
            picker_open: false,
            picker_hue: 0.0,
            hex_input: String::new(),
            size_input: String::new(),
            style_from: None,
            marks: None,
            stash: HashMap::new(),
            editing: None,
            markup_busy: None,
            last_drag: None,
            confirm_quit: false,
            passwords: HashMap::new(),
            password_input: String::new(),
            form: None,
            form_path: None,
            stash_forms: HashMap::new(),
            signatures: crate::signatures::Signatures::user(),
            sign_menu: false,
            pad: None,
            sign_target: None,
            pending_sign: None,
            edit_open: false,
            crop: None,
            adjust_open: false,
            resize_form: None,
            stash_edits: HashMap::new(),
            organizer: None,
            controls: iced::Animation::new(false).duration(Duration::from_millis(180)),
            now: Instant::now(),
            last_pointer: Instant::now(),
            hide_pending: false,
            over_controls: false,
            zoom_ease: None,
            pointer_at: None,
            pinch_scale: 1.0,
            menu: None,
            dialog: None,
            properties: None,
            export_job: None,
            export_running: 0,
            last_toast: None,
            control_opening: false,
            trash_undo: None,
            pages_undo: None,
            palette: None,
            slideshow: None,
            chooser: None,
            print_dialog: None,
        };
        let mut app = app;
        app.prefs = if cfg!(test) { crate::settings::Settings::default() } else { crate::settings::Settings::load() };
        crate::settings::set_portals(app.prefs.portals);
        crate::sandbox::set_enabled(app.prefs.sandbox);
        app.show_sidebar = app.prefs.sidebar_open;
        app.night = app.prefs.night_mode;
        app.apply_theme();
        let mut tasks = vec![system::theme().map(Message::SystemTheme)];
        if !paths.is_empty() {
            tracing::info!(ms = crate::since_start(), "open requested");
            tasks.push(expand(paths, false));
        }
        (app, Task::batch(tasks))
    }

    pub fn title(&self) -> String {
        let unsaved = if self.has_unsaved_changes() { "• " } else { "" };
        match self.current_path() {
            Some(p) => format!("{unsaved}{} — Lumen", file_name(p)),
            None => "Lumen".into(),
        }
    }

    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let fading = if self.controls.is_animating(self.now) || self.zoom_ease.is_some() {
            window::frames().map(Message::Frame)
        } else {
            Subscription::none()
        };
        Subscription::batch([
            fading,
            self.slideshow_subscription(),
            self.anim_subscription(),
            if crate::control::enabled() {
                Subscription::run(crate::control::requests).map(Message::Control)
            } else {
                Subscription::none()
            },
            system::theme_changes().map(Message::SystemTheme),
            window::events().map(|(id, e)| Message::WindowEvent(id, e)),
            keyboard::listen().map(Message::Key),
            // Another program may change the open file: inotify tells us (nothing is polled).
            match self.current_path().map(Path::to_path_buf) {
                Some(path) if matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) => {
                    Subscription::run_with(path, |path| crate::watch::changes(path.clone())).map(Message::FileEvent)
                }
                _ => Subscription::none(),
            },
            // Where inotify cannot be had (a network folder, too many watches), look now and then.
            if self.watch_failed && matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) {
                iced::time::every(FALLBACK_LOOK).map(|_| Message::CheckFile)
            } else {
                Subscription::none()
            },
            event::listen_with(|event, _status, _id| match event {
                Event::Mouse(mouse::Event::WheelScrolled { delta }) => Some(Message::Wheel(delta)),
                Event::Mouse(mouse::Event::Pinched { delta }) => Some(Message::Pinch(delta)),
                Event::Keyboard(keyboard::Event::KeyPressed { key: Key::Named(Named::Delete), .. }) => {
                    Some(Message::DeleteKey)
                }
                // Modifiers are tracked whatever has focus, so Ctrl is never thought held down.
                Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => Some(Message::Modifiers(m)),
                Event::Keyboard(keyboard::Event::KeyPressed { key: Key::Named(Named::Escape), .. }) => {
                    Some(Message::Escape)
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::MouseUp),
                _ => None,
            }),
        ])
    }

    fn current_path(&self) -> Option<&Path> {
        self.files.get(self.current).map(PathBuf::as_path)
    }

    /// Reports a problem at the bottom of the window.
    fn toast(&mut self, msg: impl Into<String>) -> Task<Message> {
        self.show_toast(msg.into(), true)
    }

    /// Says something went fine (saved, sent to the printer).
    fn notice(&mut self, msg: impl Into<String>) -> Task<Message> {
        self.show_toast(msg.into(), false)
    }

    fn show_toast(&mut self, msg: String, error: bool) -> Task<Message> {
        self.show_toast_for(msg, error, if error { 6 } else { 3 })
    }

    fn show_toast_for(&mut self, msg: String, error: bool, seconds: u64) -> Task<Message> {
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.last_toast = Some((msg.clone(), error));
        self.toast = Some((id, msg, error));
        Task::perform(tokio::time::sleep(Duration::from_secs(seconds)), move |_| Message::DismissToast(id))
    }

    /// The file on screen, if there is one to act on.
    fn current_file(&self) -> Option<PathBuf> {
        self.files.get(self.current).cloned().filter(|p| p.exists())
    }
}

fn expand(paths: Vec<PathBuf>, append: bool) -> Task<Message> {
    Task::perform(blocking(move || library::expand(&paths)), move |r| {
        let (files, start) = r.unwrap_or_default();
        Message::Expanded { files, start, append }
    })
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

// ---------------------------------------------------------------- update

impl App {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SystemTheme(mode) => {
                self.system_mode = mode;
                self.apply_theme();
                Task::none()
            }
            Message::Pref(msg) => self.prefs_update(msg),
            Message::WindowEvent(id, event) => self.window_event(id, event),
            Message::ScaleFactor(f) => {
                self.scale_factor = f.max(0.5);
                self.request_pages()
            }
            Message::Key(event) => self.key(event),
            Message::Wheel(delta) => {
                if !self.modifiers.control() {
                    return Task::none();
                }
                let y = match delta {
                    ScrollDelta::Lines { y, .. } => y,
                    ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                // Ctrl+scroll zooms around the pointer.
                let anchor = self.pointer_at;
                if y > 0.0 {
                    self.zoom_by_at(ZOOM_STEP.powf(y.min(3.0)), anchor)
                } else if y < 0.0 {
                    self.zoom_by_at(1.0 / ZOOM_STEP.powf((-y).min(3.0)), anchor)
                } else {
                    Task::none()
                }
            }
            Message::AreaResized(size) => {
                self.area = size;
                self.refit()
            }
            Message::Scrolled(vp) => {
                self.viewport = vp;
                let mut page_changed = false;
                if let Doc::Pdf(view) = &mut self.doc {
                    let page = view.page_at(vp.y + vp.height * 0.4);
                    page_changed = page != view.current_page;
                    view.current_page = page;
                }
                let follow = if page_changed { self.follow_in_sidebar() } else { Task::none() };
                Task::batch([self.request_pages(), follow])
            }

            Message::Open { folder } => {
                App::ask_for_files(if folder { Purpose::OpenFolder } else { Purpose::OpenFiles }, None)
            }
            Message::Chooser(msg) => self.chooser_update(msg),
            Message::Printing(msg) => self.printing_update(msg),
            Message::Chosen(Ok(paths)) if paths.is_empty() => Task::none(),
            Message::Chosen(Ok(paths)) => expand(paths, false),
            Message::Chosen(Err(e)) => self.toast(e),
            Message::Expanded { files, start, append } => {
                self.control_opening = false;
                self.set_files(files, start, append)
            }
            Message::Control(request) => self.control_update(request),
            Message::FlushDrops => {
                let paths = std::mem::take(&mut self.pending_drops);
                if paths.is_empty() { Task::none() } else { expand(paths, true) }
            }
            Message::Select(i) => self.show(i),
            Message::Next if self.prefs.wrap_around && self.current + 1 >= self.files.len() => self.show(0),
            Message::Next => self.show(self.current + 1),
            Message::Prev => match self.current.checked_sub(1) {
                Some(i) => self.show(i),
                None if self.prefs.wrap_around => self.show(self.files.len().saturating_sub(1)),
                None => Task::none(),
            },
            Message::First => self.show(0),
            Message::Last => self.show(self.files.len().saturating_sub(1)),

            Message::PictureLoaded(generation, path, result) => {
                if generation != self.generation {
                    if let Ok(p) = result {
                        self.remember(path, p.into());
                    }
                    return Task::none();
                }
                match result {
                    Ok(p) => {
                        self.log_opened(&path, "picture");
                        let loaded: Loaded = p.into();
                        self.remember(path.clone(), loaded.clone());
                        let shown = self.show_picture(path, loaded);
                        Task::batch([shown, self.attach_markup(), self.preload_neighbours()])
                    }
                    Err(error) => {
                        self.state.forget(&path);
                        self.doc = Doc::Failed { path, error };
                        Task::none()
                    }
                }
            }
            Message::Preloaded(path, result) => {
                self.preloading.remove(&path);
                if let Ok(p) = result {
                    self.remember(path, p.into());
                }
                Task::none()
            }
            Message::PdfOpened(generation, path, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                match result {
                    Ok(opened) => {
                        let thumb = self.unlocked_thumb(&path);
                        Task::batch([self.install_opened(path, opened), thumb])
                    }
                    Err(error) => self.pdf_failed(path, error),
                }
            }
            Message::ToggleNight => {
                self.night = !self.night;
                self.prefs.night_mode = self.night;
                self.prefs.save();
                if let Doc::Pdf(view) = &mut self.doc {
                    for slot in &mut view.slots {
                        slot.shown = None;
                        slot.pending = None;
                    }
                }
                self.reset_page_thumbs();
                Task::batch([self.request_pages(), self.request_page_thumbs()])
            }
            Message::PageRendered(generation, page, scale, night, result) => {
                if generation != self.generation || night != self.night {
                    return Task::none();
                }
                let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
                let Some(slot) = view.slots.get_mut(page) else { return Task::none() };
                if slot.pending == Some(scale) {
                    slot.pending = None;
                }
                match result {
                    Ok(p) => {
                        if !self.first_view_logged {
                            self.first_view_logged = true;
                            tracing::info!(ms = crate::since_start(), page = page + 1, "first view on screen");
                        }
                        slot.shown = Some((image::Handle::from_rgba(p.width, p.height, p.rgba.clone()), scale))
                    }
                    Err(e) => tracing::warn!("page {}: {e}", page + 1),
                }
                self.request_pages()
            }
            Message::Thumb(path, picture, locked) => {
                self.thumbs_running = self.thumbs_running.saturating_sub(1);
                if locked {
                    self.locked_thumbs.insert(path.clone());
                }
                let handle = picture.map(|p| image::Handle::from_rgba(p.width, p.height, p.rgba.clone()));
                self.thumbs.insert(path, handle);
                self.request_thumbs()
            }

            Message::SidebarScrolled(offset, height) => {
                self.sidebar_scroll = (offset, height);
                self.request_thumbs()
            }
            Message::SidebarTab(tab) => {
                self.sidebar_tab = Some(tab);
                match tab {
                    SidebarTab::Files => Task::batch([self.request_thumbs(), self.reveal_in_sidebar()]),
                    SidebarTab::Contents => self.reveal_in_contents(),
                    SidebarTab::Pages => Task::batch([self.request_page_thumbs(), self.reveal_page_thumb()]),
                }
            }
            Message::PageThumbsScrolled(offset, height) => self.page_thumbs_scrolled(offset, height),
            Message::PageThumb(generation, page, night, result) => {
                if night == self.night {
                    self.page_thumb_ready(generation, page, result);
                }
                Task::none()
            }
            Message::GoToPage(page) => self.go_to_page(page),
            Message::GoToOpen => self.goto_open(),
            Message::GoToInput(typed) => {
                self.goto = Some(typed);
                Task::none()
            }
            Message::GoToSubmit => self.goto_submit(),
            Message::GoToClose => {
                self.goto = None;
                Task::none()
            }
            Message::ContentsScrolled(offset, height) => {
                self.contents_scroll = (offset, height);
                Task::none()
            }
            Message::GoTo(Target::Page { page, top }) => {
                if let Some(here) = self.place() {
                    self.links.leave(here);
                }
                self.go_to_point(page, top.map(|t| (0.0, t)))
            }
            Message::CheckFile => {
                let Some(path) = self.current_path().map(Path::to_path_buf) else { return Task::none() };
                if self.markup_busy.is_some() {
                    return Task::none();
                }
                Task::perform(
                    blocking(move || {
                        let stamp = std::fs::metadata(&path).and_then(|m| Ok((m.modified()?, m.len()))).ok();
                        (path, stamp)
                    }),
                    |r| match r {
                        Ok((path, stamp)) => Message::FileStat(path, stamp),
                        Err(_) => Message::LinkOpened(Ok(())),
                    },
                )
            }
            Message::FileEvent(Some(())) => self.update(Message::CheckFile),
            Message::FileEvent(None) => {
                self.watch_failed = true;
                Task::none()
            }
            Message::FileStat(path, stamp) => self.file_stat(path, stamp),
            Message::CopyPicture => {
                let Doc::Image(view) = &self.doc else { return Task::none() };
                let picture = view.shown.picture.clone();
                Task::perform(
                    blocking(move || {
                        let png = crate::clipboard::encode_png(picture.width, picture.height, &picture.rgba)?;
                        crate::clipboard::copy_png(png)
                    }),
                    |r| Message::PictureCopied(r.and_then(|r| r)),
                )
            }
            Message::PictureCopied(Ok(())) => self.notice(tr("Copied the picture")),
            Message::PictureCopied(Err(e)) => self.toast(trf!("Could not copy the picture: {}", e)),
            Message::Palette(msg) => self.palette_update(msg),
            Message::SaveState => {
                self.state_save_pending = false;
                self.state.save();
                Task::none()
            }
            Message::OpenRecent(path) => expand(vec![path], false),
            Message::ClearRecent => {
                self.state.clear_recent();
                self.state.save();
                Task::none()
            }
            Message::LinkBack | Message::LinkForward => {
                let Some(here) = self.place() else { return Task::none() };
                let to =
                    if matches!(message, Message::LinkBack) { self.links.back(here) } else { self.links.forward(here) };
                match to {
                    Some(to) => self.go_to_place(to),
                    None => Task::none(),
                }
            }
            Message::GoTo(Target::Uri(uri)) => Task::perform(
                async move { crate::portal::open_uri(uri).await.map_err(err_string) },
                Message::LinkOpened,
            ),
            Message::LinkOpened(Ok(())) => Task::none(),
            Message::LinkOpened(Err(e)) => self.toast(trf!("Could not open the link: {}", e)),

            Message::PageText(generation, page, text) => {
                if generation != self.generation {
                    return Task::none();
                }
                if let Doc::Pdf(view) = &mut self.doc
                    && let Some(slot) = view.slots.get_mut(page)
                {
                    slot.text = Some(text);
                    slot.text_pending = false;
                }
                Task::none()
            }
            Message::PageHover(page, position) => {
                let Doc::Pdf(view) = &self.doc else { return Task::none() };
                let point = PagePoint::new((position.x / view.zoom) as f64, (position.y / view.zoom) as f64);
                let text = view.slots.get(page).and_then(|s| s.text.clone());
                self.hover = Some((page, point));
                self.hover_text = text.as_ref().is_some_and(|t| t.is_over_text(point));
                if self.selecting
                    && let (Some(sel), Some(text)) = (&mut self.selection, text)
                {
                    sel.head = (page, text.caret_at(point));
                }
                Task::none()
            }
            Message::PagePress(page) => {
                let done = self.markup_page_press();
                let text = match (&self.doc, self.hover) {
                    (Doc::Pdf(view), Some((p, _))) if p == page => view.slots[page].text.clone(),
                    _ => None,
                };
                match (text, self.hover) {
                    (Some(text), Some((_, point))) if self.hover_text || self.modifiers.shift() => {
                        let caret = (page, text.caret_at(point));
                        match &mut self.selection {
                            // Shift+click extends the selection.
                            Some(sel) if self.modifiers.shift() => sel.head = caret,
                            _ => self.selection = Some(TextSel { anchor: caret, head: caret }),
                        }
                        self.selecting = true;
                    }
                    _ => {
                        // Not on text: drag to pan, as on the rest of the canvas.
                        self.selection = None;
                        self.pan = Some(None);
                    }
                }
                done
            }
            Message::PageDoubleClick(page) => {
                let Doc::Pdf(view) = &self.doc else { return Task::none() };
                if let (Some(text), Some((p, point))) = (&view.slots[page].text, self.hover)
                    && p == page
                    && self.hover_text
                {
                    let word = text.word_at(point);
                    self.selection = Some(TextSel { anchor: (page, word.start), head: (page, word.end) });
                    self.selecting = false;
                    return self.copy_selection(true);
                }
                Task::none()
            }
            Message::MouseUp => {
                if self.pan.is_some() {
                    self.pan = None;
                }
                if self.pages_mouse_up() {
                    return Task::none();
                }
                if let Some(task) = self.markup_mouse_up() {
                    return task;
                }
                if std::mem::take(&mut self.selecting) {
                    // Like other Linux apps, a finished selection becomes the primary selection (middle-click paste).
                    return self.copy_selection(true);
                }
                Task::none()
            }
            Message::Copy => self.copy_selection(false),
            Message::SelectAll => {
                let Doc::Pdf(view) = &self.doc else { return Task::none() };
                let last = view.doc.page_count() - 1;
                self.selection = Some(TextSel { anchor: (0, 0), head: (last, usize::MAX) });
                self.selecting = false;
                Task::none()
            }
            Message::Copied(text, primary) => {
                if primary {
                    iced::clipboard::write_primary(text)
                } else {
                    iced::clipboard::write(text)
                }
            }

            Message::SearchOpen => {
                if !matches!(self.doc, Doc::Pdf(_)) {
                    return Task::none();
                }
                if self.search.is_none() {
                    self.search = Some(Search::default());
                }
                Task::batch([operation::focus(search_input_id()), operation::select_all(search_input_id())])
            }
            Message::SearchClose => {
                self.search = None;
                Task::none()
            }
            Message::Escape => {
                if self.chooser.take().is_some()
                    || self.print_dialog.take().is_some()
                    || self.palette.take().is_some()
                    || self.menu_escape()
                    || self.sign_escape()
                    || self.pages_escape()
                    || self.edit_escape()
                {
                    Task::none()
                } else if let Some(task) = self.markup_escape() {
                    task
                } else if self.goto.take().is_some() {
                    Task::none()
                } else if self.search.is_some() {
                    self.search = None;
                    Task::none()
                } else if self.selection.take().is_some() {
                    Task::none()
                } else if self.slideshow.is_some() {
                    self.slideshow_stop()
                } else if self.fullscreen {
                    self.set_fullscreen(false)
                } else {
                    Task::none()
                }
            }
            Message::SearchInput(query) => {
                let Some(search) = &mut self.search else { return Task::none() };
                search.query = query;
                self.start_search()
            }
            Message::SearchMatchCase | Message::SearchWholeWords => {
                let Some(search) = &mut self.search else { return Task::none() };
                if matches!(message, Message::SearchMatchCase) {
                    search.options.match_case = !search.options.match_case;
                } else {
                    search.options.whole_words = !search.options.whole_words;
                }
                self.start_search()
            }
            Message::SearchSubmit => {
                self.update(if self.modifiers.shift() { Message::SearchPrev } else { Message::SearchNext })
            }
            Message::SearchNext | Message::SearchPrev => {
                let forward = matches!(message, Message::SearchNext);
                let Some(search) = &mut self.search else { return self.update(Message::SearchOpen) };
                let n = search.hits.len();
                if n == 0 {
                    return Task::none();
                }
                search.current = Some(match search.current {
                    Some(c) if forward => (c + 1) % n,
                    Some(c) => (c + n - 1) % n,
                    None => 0,
                });
                self.reveal_hit()
            }
            Message::SearchHits(seq, page, found) => {
                let Some(search) = &mut self.search else { return Task::none() };
                if seq != search.seq {
                    return Task::none();
                }
                search.scanned += 1;
                if found.is_empty() {
                    return Task::none();
                }
                let at = search.hits.partition_point(|h| h.page <= page);
                let count = found.len();
                search.hits.splice(at..at, found.into_iter().map(|rects| Hit { page, rects }));
                match &mut search.current {
                    Some(c) if *c >= at => *c += count,
                    Some(_) => {}
                    None => {
                        // The scan starts at the page in view, so the first match found is the nearest one.
                        search.current = Some(at);
                        return self.reveal_hit();
                    }
                }
                Task::none()
            }
            Message::SearchDone(seq) => {
                if let Some(search) = &mut self.search
                    && search.seq == seq
                {
                    search.done = true;
                }
                Task::none()
            }

            Message::ZoomIn => self.zoom_by(ZOOM_STEP),
            Message::ZoomOut => self.zoom_by(1.0 / ZOOM_STEP),
            Message::ZoomFit => self.zoom_fit(false),
            Message::ZoomFitWidth => self.zoom_fit(true),
            Message::ZoomActual => match &self.doc {
                Doc::Image(_) => {
                    let z = 1.0 / self.scale_factor;
                    self.ease_zoom(z, ZoomEnd::Image(Some(z)))
                }
                Doc::Pdf(_) => self.ease_zoom(PT_TO_PX, ZoomEnd::Pdf(PdfFit::Manual)),
                _ => Task::none(),
            },
            Message::Rotate(dir) => self.edit_update(if dir > 0 { EditMsg::RotateCw } else { EditMsg::RotateCcw }),
            Message::PageNext | Message::PagePrev => {
                let Doc::Pdf(view) = &self.doc else { return Task::none() };
                let last = view.doc.page_count() - 1;
                let target = if matches!(message, Message::PageNext) {
                    (view.current_page + 1).min(last)
                } else {
                    view.current_page.saturating_sub(1)
                };
                self.go_to_page(target)
            }

            Message::PanStart => {
                self.pan = Some(None);
                Task::none()
            }
            Message::PanMove(p) => {
                let Some(last) = &mut self.pan else { return Task::none() };
                let task = match *last {
                    Some(prev) => operation::scroll_by(
                        scroll_id(),
                        scrollable::AbsoluteOffset { x: prev.x - p.x, y: prev.y - p.y },
                    ),
                    None => Task::none(),
                };
                *last = Some(p);
                task
            }
            Message::PanEnd => {
                self.pan = None;
                Task::none()
            }

            Message::Print => {
                let Some(path) = self.current_path().map(Path::to_owned) else { return Task::none() };
                if self.printing {
                    return Task::none();
                }
                self.printing = true;
                if let Some(copy) = self.printable_copy() {
                    // Print what is on screen, unsaved markup included.
                    let original = path.clone();
                    return copy.then(move |copy| match copy {
                        Ok(copy) => Task::perform(
                            {
                                let copy = copy.clone();
                                async move { print::print(copy).await.map_err(err_string) }
                            },
                            move |r| Message::Printed(copy.clone(), r),
                        ),
                        Err(e) => Task::done(Message::Printed(original.clone(), Err(e))),
                    });
                }
                Task::perform(
                    {
                        let path = path.clone();
                        async move { print::print(path).await.map_err(err_string) }
                    },
                    move |r| Message::Printed(path.clone(), r),
                )
            }
            Message::Printed(path, result) => {
                self.printing = false;
                match result {
                    Ok(Outcome::Sent) => self.notice(tr("Sent to the printer")),
                    Ok(Outcome::Cancelled) => Task::none(),
                    Ok(Outcome::NoPortal) => self.open_print_dialog(path),
                    Err(e) => self.toast(trf!("Could not print: {}", e)),
                }
            }
            Message::ToggleFullscreen => self.set_fullscreen(!self.fullscreen),
            Message::ToggleSidebar => {
                self.show_sidebar = !self.show_sidebar;
                if self.show_sidebar {
                    Task::batch([self.reveal_in_sidebar(), self.request_page_thumbs()])
                } else {
                    Task::none()
                }
            }
            Message::DismissToast(id) => {
                if self.toast.as_ref().is_some_and(|(t, ..)| *t == id) {
                    self.toast = None;
                }
                if self.trash_undo.as_ref().is_some_and(|(t, ..)| *t == id) {
                    self.trash_undo = None;
                }
                if self.pages_undo == Some(id) {
                    self.pages_undo = None;
                }
                Task::none()
            }
            Message::Slideshow => self.slideshow_toggle(),
            Message::SlideshowPause => self.slideshow_pause(),
            Message::SlideshowTick(now) => self.slideshow_tick(now),
            Message::AnimTick(now) => {
                self.anim_tick(now);
                Task::none()
            }
            Message::ShowInFolder => {
                self.menu = None;
                let Some(path) = self.current_file() else { return Task::none() };
                Task::perform(async move { crate::portal::show_in_folder(path).await.map_err(err_string) }, |r| {
                    Message::FileActionDone(r)
                })
            }
            Message::OpenWith => {
                self.menu = None;
                let Some(path) = self.current_file() else { return Task::none() };
                Task::perform(async move { crate::portal::open_with(path).await.map_err(err_string) }, |r| {
                    Message::FileActionDone(r)
                })
            }
            Message::FileActionDone(Ok(())) => Task::none(),
            Message::FileActionDone(Err(e)) => self.toast(e),
            Message::Trash => {
                self.menu = None;
                let Some(path) = self.current_file() else { return Task::none() };
                let index = self.current;
                Task::perform(blocking(move || crate::trash::trash(&path).map_err(err_string)), move |r| {
                    Message::Trashed(index, r.and_then(|r| r))
                })
            }
            Message::Trashed(_, Err(e)) => self.toast(trf!("Could not move it to the trash: {}", e)),
            Message::Trashed(index, Ok(item)) => {
                let path = item.original.clone();
                // Only forget the file if it is still where it was in the list.
                let Some(at) = self
                    .files
                    .iter()
                    .position(|p| *p == path)
                    .or((self.files.get(index) == Some(&path)).then_some(index))
                else {
                    return self.notice(tr("Moved to the trash"));
                };
                self.files.remove(at);
                self.thumbs.remove(&path);
                self.locked_thumbs.remove(&path);
                self.stash.remove(&path);
                self.stash_forms.remove(&path);
                self.stash_edits.remove(&path);
                self.passwords.remove(&path);
                self.pictures.retain(|(p, _)| *p != path);
                let name = file_name(&path);
                let show = if self.files.is_empty() {
                    self.organizer = None;
                    self.detach_form();
                    self.detach_markup();
                    self.generation += 1;
                    self.doc = Doc::Empty;
                    self.current = usize::MAX;
                    Task::none()
                } else {
                    let next = at.min(self.files.len() - 1);
                    self.current = usize::MAX; // force the next file to load
                    self.show(next)
                };
                let toast = self.show_toast_for(trf!("Moved “{}” to the trash", name), false, 8);
                self.trash_undo = Some((self.toast_seq, at, item));
                Task::batch([show, toast, self.request_thumbs()])
            }
            Message::UndoTrash => {
                let Some((_, index, item)) = self.trash_undo.take() else { return Task::none() };
                self.toast = None;
                Task::perform(
                    blocking(move || crate::trash::restore(&item).map(|()| item.original).map_err(err_string)),
                    move |r| Message::Restored(index, r.and_then(|r| r)),
                )
            }
            Message::Restored(_, Err(e)) => self.toast(trf!("Could not restore it: {}", e)),
            Message::Restored(index, Ok(path)) => {
                let at = index.min(self.files.len());
                self.files.insert(at, path.clone());
                self.current = usize::MAX;
                let shown = self.show(at);
                Task::batch([shown, self.notice(trf!("Restored “{}”", file_name(&path))), self.request_thumbs()])
            }
            Message::Quit => {
                if self.unsaved_files() > 0 && !self.confirm_quit {
                    let done = self.finish_text();
                    self.confirm_quit = true;
                    return done;
                }
                self.remember_place();
                self.state.save();
                iced::exit()
            }
            Message::PasswordInput(text) => {
                self.password_input = text;
                Task::none()
            }
            Message::Unlock => {
                let Doc::Locked { path, .. } = &self.doc else { return Task::none() };
                if self.password_input.is_empty() {
                    return Task::none();
                }
                let path = path.clone();
                // Kept in memory only, so flipping back to the file does not ask again.
                self.passwords.insert(path.clone(), std::mem::take(&mut self.password_input));
                self.load_started = Some(Instant::now());
                let markup = self.opens_for_markup(&path);
                self.load_pdf(path, markup)
            }
            Message::Markup(msg) => self.markup_update(msg),
            Message::Form(msg) => self.form_update(msg),
            Message::Sign(msg) => self.sign_update(msg),
            Message::Edit(msg) => self.edit_update(msg),
            Message::Pages(msg) => self.pages_update(msg),
            Message::Export(msg) => self.export_update(msg),
            Message::Pointer(at) => {
                self.pointer_at = Some(at);
                let now = Instant::now();
                self.now = now;
                self.last_pointer = now;
                if !self.controls.value() {
                    self.controls.go_mut(true, now);
                }
                if std::mem::replace(&mut self.hide_pending, true) {
                    Task::none()
                } else {
                    Task::perform(tokio::time::sleep(CONTROLS_LINGER), |_| Message::ControlsCheck)
                }
            }
            Message::ControlsCheck => {
                self.hide_pending = false;
                let now = Instant::now();
                let still = now.duration_since(self.last_pointer);
                if self.over_controls || still < CONTROLS_LINGER {
                    // Keep them while a control is under the pointer, or it moved meanwhile.
                    self.hide_pending = true;
                    let wait = CONTROLS_LINGER.saturating_sub(still).max(Duration::from_millis(200));
                    return Task::perform(tokio::time::sleep(wait), |_| Message::ControlsCheck);
                }
                self.now = now;
                self.controls.go_mut(false, now);
                Task::none()
            }
            Message::Pinch(delta) => self.pinch(delta),
            Message::DeleteKey => self.markup_delete_key(),
            Message::Modifiers(m) => {
                self.modifiers = m;
                Task::none()
            }
            Message::Menu(menu) => self.menu_update(menu),
            Message::MenuPick(msg) => self.menu_pick(*msg),
            Message::ShowProperties => {
                let (path, password) = match &self.doc {
                    Doc::Pdf(v) => (v.doc.path().to_path_buf(), v.doc.password().map(str::to_string)),
                    Doc::Image(v) => (v.path.clone(), None),
                    _ => return Task::none(),
                };
                self.properties = None;
                self.dialog = Some(Dialog::Properties);
                Task::perform(
                    blocking(move || crate::properties::read(&path, password.as_deref()).map_err(err_string)),
                    |r| Message::PropertiesRead(r.and_then(|r| r)),
                )
            }
            Message::PropertiesRead(result) => {
                if self.dialog == Some(Dialog::Properties) {
                    self.properties = Some(result);
                }
                Task::none()
            }
            Message::Dialog(dialog) => {
                self.menu = None;
                self.dialog = dialog;
                Task::none()
            }
            Message::PointerLeft => {
                self.pointer_at = None;
                if !self.over_controls {
                    let now = Instant::now();
                    self.now = now;
                    self.controls.go_mut(false, now);
                }
                Task::none()
            }
            Message::OverControls(over) => {
                self.over_controls = over;
                Task::none()
            }
            Message::Frame(now) => {
                self.now = now;
                self.step_zoom(now)
            }
        }
    }

    fn window_event(&mut self, id: window::Id, event: window::Event) -> Task<Message> {
        match event {
            window::Event::Opened { .. } => {
                tracing::info!(ms = crate::since_start(), "window built");
                self.window = Some(id);
                window::scale_factor(id).map(Message::ScaleFactor)
            }
            window::Event::CloseRequested => self.update(Message::Quit),
            window::Event::Resized(size) if !self.fullscreen => {
                self.state.window = Some((size.width.round() as u32, size.height.round() as u32));
                Task::none()
            }
            window::Event::Rescaled(f) => {
                self.scale_factor = f.max(0.5);
                self.request_pages()
            }
            window::Event::FileHovered(_) => {
                self.drop_hover = true;
                Task::none()
            }
            window::Event::FilesHoveredLeft => {
                self.drop_hover = false;
                Task::none()
            }
            window::Event::FileDropped(path) => {
                self.drop_hover = false;
                let first = self.pending_drops.is_empty();
                self.pending_drops.push(path);
                if first {
                    // Several files arrive as separate events; gather them first.
                    Task::perform(tokio::time::sleep(Duration::from_millis(60)), |_| Message::FlushDrops)
                } else {
                    Task::none()
                }
            }
            _ => Task::none(),
        }
    }

    fn key(&mut self, event: keyboard::Event) -> Task<Message> {
        let (key, modifiers) = match event {
            keyboard::Event::ModifiersChanged(m) => {
                self.modifiers = m;
                return Task::none();
            }
            keyboard::Event::KeyPressed { key, modifiers, .. } => (key, modifiers),
            _ => return Task::none(),
        };
        self.modifiers = modifiers;
        if let Some(task) = self.chooser_key(&key) {
            return task;
        }
        if self.confirm_quit || self.pad.is_some() || self.print_dialog.is_some() {
            return Task::none();
        }
        if let Some(task) = self.palette_key(&key) {
            return task;
        }
        if let Some(task) = self.pages_key(&key, modifiers) {
            return task;
        }
        if let Some(task) = self.edit_key(&key, modifiers) {
            return task;
        }
        if let Some(task) = self.markup_key(&key, modifiers) {
            return task;
        }
        let ctrl = modifiers.command();
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let msg = match key.as_ref() {
            Key::Character(c) => match (c.to_lowercase().as_str(), ctrl, modifiers.shift()) {
                ("o", true, false) => Message::Open { folder: false },
                ("o", true, true) => Message::Open { folder: true },
                ("p", true, true) => Message::Palette(commands::PaletteMsg::Open),
                ("p", true, _) => Message::Print,
                ("f", true, _) if is_pdf => Message::SearchOpen,
                ("c", true, _) if is_pdf => Message::Copy,
                ("c", true, _) if matches!(self.doc, Doc::Image(_)) => Message::CopyPicture,
                ("a", true, _) if is_pdf => Message::SelectAll,
                ("g", true, _) if is_pdf => Message::GoToOpen,
                ("n", true, true) if is_pdf => Message::ToggleNight,
                ("q" | "w", true, _) => Message::Quit,
                ("+" | "=", _, _) => Message::ZoomIn,
                ("-" | "_", _, _) => Message::ZoomOut,
                ("0", _, _) => Message::ZoomFit,
                ("1", _, _) => Message::ZoomActual,
                ("w", false, _) if is_pdf => Message::ZoomFitWidth,
                ("r", false, false) => Message::Rotate(1),
                ("r", false, true) => Message::Rotate(-1),
                ("f", false, _) => Message::ToggleFullscreen,
                ("t", false, _) => Message::ToggleSidebar,
                ("g", false, _) if is_pdf => Message::Pages(PagesMsg::Open),
                _ => return Task::none(),
            },
            Key::Named(named) => match named {
                Named::ArrowLeft if modifiers.alt() && is_pdf => Message::LinkBack,
                Named::ArrowRight if modifiers.alt() && is_pdf => Message::LinkForward,
                Named::ArrowRight if !ctrl => Message::Next,
                Named::ArrowLeft if !ctrl => Message::Prev,
                Named::Space if self.slideshow.is_some() => Message::SlideshowPause,
                Named::F5 => Message::Slideshow,
                Named::Space if !is_pdf => Message::Next,
                // A screen at a time, keeping a little of the last one in view.
                Named::Space if is_pdf => {
                    let screen = self.area.height * 0.9;
                    let y = if modifiers.shift() { -screen } else { screen };
                    return operation::scroll_by(scroll_id(), scrollable::AbsoluteOffset { x: 0.0, y });
                }
                Named::Enter if modifiers.alt() => Message::ShowProperties,
                Named::Backspace if !is_pdf => Message::Prev,
                Named::PageDown if is_pdf => Message::PageNext,
                Named::PageUp if is_pdf => Message::PagePrev,
                Named::PageDown => Message::Next,
                Named::PageUp => Message::Prev,
                Named::Home if is_pdf => return self.go_to_page(0),
                Named::End if is_pdf => {
                    let last = if let Doc::Pdf(v) = &self.doc { v.doc.page_count() - 1 } else { 0 };
                    return self.go_to_page(last);
                }
                Named::Home => Message::First,
                Named::End => Message::Last,
                Named::ArrowDown => {
                    return operation::scroll_by(scroll_id(), scrollable::AbsoluteOffset { x: 0.0, y: 64.0 });
                }
                Named::ArrowUp => {
                    return operation::scroll_by(scroll_id(), scrollable::AbsoluteOffset { x: 0.0, y: -64.0 });
                }
                Named::F11 => Message::ToggleFullscreen,
                Named::F3 if is_pdf => {
                    if modifiers.shift() {
                        Message::SearchPrev
                    } else {
                        Message::SearchNext
                    }
                }
                _ => return Task::none(),
            },
            _ => return Task::none(),
        };
        self.update(msg)
    }

    fn set_fullscreen(&mut self, on: bool) -> Task<Message> {
        self.fullscreen = on;
        match self.window {
            Some(id) => window::set_mode(id, if on { window::Mode::Fullscreen } else { window::Mode::Windowed }),
            None => Task::none(),
        }
    }

    // ------------------------------------------------------------ files

    fn set_files(&mut self, files: Vec<PathBuf>, start: usize, append: bool) -> Task<Message> {
        if files.is_empty() {
            return if append { Task::none() } else { self.toast(tr("Nothing to show: no pictures or PDFs there")) };
        }
        let index = if append && !self.files.is_empty() {
            let mut first = None;
            for f in files {
                let i = match self.files.iter().position(|e| *e == f) {
                    Some(i) => i,
                    None => {
                        self.files.push(f);
                        self.files.len() - 1
                    }
                };
                first.get_or_insert(i);
            }
            first.unwrap_or(self.current)
        } else {
            self.files = files;
            self.thumbs.clear();
            self.locked_thumbs.clear();
            self.thumbs_running = 0;
            start
        };
        self.current = usize::MAX; // force a reload
        self.show(index)
    }

    fn show(&mut self, index: usize) -> Task<Message> {
        if index >= self.files.len() || index == self.current {
            return Task::none();
        }
        self.show_forced(index)
    }

    /// Shows file `index` even when it is the one already shown (to load it afresh).
    fn show_forced(&mut self, index: usize) -> Task<Message> {
        // How the file is as it starts to load: a change after this is another program's.
        self.disk = self.files.get(index).map(|path| disk_baseline(path));
        self.remember_place();
        self.organizer = None;
        self.zoom_ease = None;
        self.detach_form();
        self.detach_markup();
        if let Doc::Image(view) = &mut self.doc
            && view.edit.is_dirty()
        {
            let edit = std::mem::take(&mut view.edit);
            self.stash_edits.insert(view.path.clone(), edit);
        }
        self.crop = None;
        self.resize_form = None;
        self.pending_sign = None;
        self.sign_target = None;
        self.current = index;
        self.generation += 1;
        self.load_started = Some(Instant::now());
        self.selection = None;
        self.selecting = false;
        self.hover = None;
        self.hover_text = false;
        if let Some(search) = &mut self.search {
            // Results belong to the old file; the scan restarts once the new one is open.
            *search = Search { query: std::mem::take(&mut search.query), ..Search::default() };
        }
        self.pan = None;
        let generation = self.generation;
        let path = self.files[index].clone();
        self.state.touch(&path);

        let load = match Kind::of(&path) {
            Some(Kind::Pdf) => {
                let markup = self.opens_for_markup(&path);
                self.load_pdf(path.clone(), markup)
            }
            _ => {
                if let Some(loaded) = self.cached(&path) {
                    self.log_opened(&path, "cached");
                    let shown = self.show_picture(path, loaded);
                    Task::batch([shown, self.attach_markup(), self.preload_neighbours()])
                } else {
                    // Keep showing the previous picture until the next one is ready, unless
                    // it takes a while; avoids a flash of the spinner when flipping quickly.
                    if !matches!(self.doc, Doc::Image(_)) {
                        self.doc = Doc::Loading(path.clone());
                    }
                    Task::perform(
                        {
                            let path = path.clone();
                            blocking(move || raster::load(&path).map_err(err_string))
                        },
                        move |r| Message::PictureLoaded(generation, path.clone(), r.and_then(|r| r)),
                    )
                }
            }
        };
        let reset = operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x: 0.0, y: 0.0 });
        Task::batch([load, reset, self.request_thumbs(), self.reveal_in_sidebar(), self.state_changed()])
    }

    /// Opens the PDF at `path` off the UI thread, with the password given for it earlier, if any;
    /// with `markup` it is made ready for editing too.
    fn load_pdf(&mut self, path: PathBuf, markup: bool) -> Task<Message> {
        self.doc = Doc::Loading(path.clone());
        if markup {
            self.markup_busy = Some("Preparing…");
        }
        let generation = self.generation;
        let password = self.passwords.get(&path).cloned();
        Task::perform(
            {
                let path = path.clone();
                blocking(move || pdf::Document::open_full(&path, markup, password.as_deref()))
            },
            move |r| {
                let r = r.map_err(pdf::OpenError::Failed).and_then(|r| r.map_err(pdf::OpenError::from));
                if markup {
                    Message::Markup(MarkupMsg::Prepared(generation, path.clone(), r))
                } else {
                    Message::PdfOpened(generation, path.clone(), r)
                }
            },
        )
    }

    /// A protected PDF was opened with its password: its thumbnail can be drawn now.
    fn unlocked_thumb(&mut self, path: &Path) -> Task<Message> {
        if self.locked_thumbs.remove(path) && self.thumbs.get(path).is_some_and(Option::is_none) {
            self.thumbs.remove(path);
            return self.request_thumbs();
        }
        Task::none()
    }

    /// A PDF did not open: ask for its password, or say why.
    fn pdf_failed(&mut self, path: PathBuf, error: pdf::OpenError) -> Task<Message> {
        self.markup_busy = None;
        match error {
            pdf::OpenError::Password { wrong } => {
                if wrong {
                    self.passwords.remove(&path);
                }
                self.log_opened(&path, "locked");
                self.doc = Doc::Locked { path, wrong };
                self.password_input.clear();
                operation::focus(password_input_id())
            }
            pdf::OpenError::Failed(error) => {
                self.state.forget(&path);
                self.doc = Doc::Failed { path, error };
                Task::none()
            }
        }
    }

    fn open_pdf_view(&mut self, path: PathBuf, doc: pdf::Document) -> Task<Message> {
        self.log_opened(&path, "pdf");
        self.links = LinkHistory::default();
        let pages = doc.page_count();
        let slots = (0..doc.page_count()).map(|_| Slot::default()).collect();
        let fit = match self.prefs.pdf_fit {
            crate::settings::PdfFit::Width => PdfFit::Width,
            crate::settings::PdfFit::Page => PdfFit::Page,
        };
        let mut view = PdfView { doc, zoom: 1.0, fit, tops: Vec::new(), slots, current_page: 0 };
        view.zoom = self.pdf_fit_zoom(&view, fit).min(1.5 * PT_TO_PX);
        view.layout();
        self.doc = Doc::Pdf(view);
        self.viewport.y = 0.0;
        self.viewport.x = 0.0;
        self.contents_scroll = (0.0, 0.0);
        self.reset_page_thumbs();
        let restored = match (self.restore.take(), self.state.page_of(&path)) {
            (Some(place), _) => self.go_to_place(place),
            // A PDF opened again comes back at the page it was left at.
            (None, Some(page)) if page > 0 && page < pages => self.go_to_page(page),
            _ => Task::none(),
        };
        Task::batch([
            operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }),
            operation::scroll_to(contents_id(), scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }),
            operation::scroll_to(pagelist::list_id(), scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }),
            restored,
            self.request_page_thumbs(),
            self.request_pages(),
            self.start_search(),
        ])
    }

    /// Shows a decoded picture, with any edits it had when it was left.
    fn show_picture(&mut self, path: PathBuf, loaded: Loaded) -> Task<Message> {
        let edit = self.stash_edits.remove(&path).unwrap_or_default();
        let edited = !edit.edits.is_none();
        let original = loaded.picture.clone();
        let anim = Anim::start(&path);
        let animated = anim.is_some() && crate::animation::looks_animated(&path);
        self.doc = Doc::Image(Box::new(ImageView { path, original, shown: loaded, zoom: None, edit, anim, animated }));
        if edited { self.render_edits() } else { Task::none() }
    }

    /// Logs how long the current file took to open, like the GTK Lumen's `opened load_ms`.
    fn log_opened(&mut self, path: &Path, kind: &str) {
        let load_ms = self.load_started.take().map(|t| t.elapsed().as_millis() as u64);
        tracing::info!(path = %path.display(), kind, load_ms, "opened");
        if !self.first_view_logged && kind != "pdf" {
            self.first_view_logged = true;
            tracing::info!(ms = crate::since_start(), "first view on screen");
        }
    }

    fn cached(&mut self, path: &Path) -> Option<Loaded> {
        let i = self.pictures.iter().position(|(p, _)| p == path)?;
        let entry = self.pictures.remove(i)?;
        let loaded = entry.1.clone();
        self.pictures.push_back(entry);
        Some(loaded)
    }

    fn remember(&mut self, path: PathBuf, loaded: Loaded) {
        self.pictures.retain(|(p, _)| *p != path);
        self.pictures.push_back((path, loaded));
        let bytes = |pics: &VecDeque<(PathBuf, Loaded)>| pics.iter().map(|(_, l)| l.picture.rgba.len()).sum::<usize>();
        let budget = self.prefs.cache_bytes();
        while self.pictures.len() > PICTURE_CACHE || (self.pictures.len() > 1 && bytes(&self.pictures) > budget) {
            self.pictures.pop_front();
        }
    }

    fn preload_neighbours(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for i in [self.current + 1, self.current.wrapping_sub(1)] {
            let Some(path) = self.files.get(i).cloned() else { continue };
            if !matches!(Kind::of(&path), Some(Kind::Image | Kind::Svg))
                || self.pictures.iter().any(|(p, _)| *p == path)
                || !self.preloading.insert(path.clone())
            {
                continue;
            }
            let max_pixels = self.prefs.preload_pixels();
            tasks.push(Task::perform(
                {
                    let path = path.clone();
                    blocking(move || match raster::pixel_count(&path) {
                        Some(n) if n > max_pixels => Err("too big to preload".to_string()),
                        _ => raster::load(&path).map_err(err_string),
                    })
                },
                move |r| Message::Preloaded(path.clone(), r.and_then(|r| r)),
            ));
        }
        Task::batch(tasks)
    }

    /// Sidebar rows on screen, as an index range.
    fn sidebar_rows(&self) -> std::ops::Range<usize> {
        let (offset, height) = self.sidebar_scroll;
        let height = if height > 0.0 { height } else { self.area.height.max(ROW_H) };
        let first = (offset / ROW_H).floor().max(0.0) as usize;
        let last = ((offset + height) / ROW_H).ceil() as usize + 1;
        first.min(self.files.len())..last.min(self.files.len())
    }

    fn request_thumbs(&mut self) -> Task<Message> {
        if self.files.len() < 2 {
            return Task::none();
        }
        // Rows on screen first (top to bottom), then the files you are likely to flip to next.
        let files_shown = self.show_sidebar && !self.fullscreen && self.sidebar_shows() == Some(SidebarTab::Files);
        let visible = if files_shown { self.sidebar_rows() } else { 0..0 };
        let near = (0..=THUMB_RADIUS).flat_map(|d| [self.current + d, self.current.wrapping_sub(d)]);
        let order: Vec<usize> = visible.clone().chain(near).filter(|&i| i < self.files.len()).collect();

        // Keep memory flat in huge folders: forget thumbnails far from both the view and the current file.
        if self.thumbs.len() > 6 * THUMB_RADIUS {
            let keep_from = |c: usize| c.saturating_sub(2 * THUMB_RADIUS)..c + 2 * THUMB_RADIUS;
            let (a, b) = (keep_from(visible.start), keep_from(self.current));
            let keep: HashSet<&PathBuf> = a.chain(b).filter_map(|i| self.files.get(i)).collect();
            self.thumbs.retain(|p, t| t.is_none() || keep.contains(p));
        }

        let mut tasks = Vec::new();
        for i in order {
            if self.thumbs_running >= THUMB_PARALLEL {
                break;
            }
            let path = self.files[i].clone();
            if self.thumbs.contains_key(&path) {
                continue;
            }
            self.thumbs.insert(path.clone(), None);
            self.thumbs_running += 1;
            let password = self.passwords.get(&path).cloned();
            let cache = if self.prefs.thumbnail_cache && !cfg!(test) { crate::thumbcache::base_dir() } else { None };
            tasks.push(Task::perform(
                {
                    let path = path.clone();
                    blocking(move || match Kind::of(&path) {
                        Some(Kind::Pdf) => match pdf::thumbnail(&path, raster::THUMB_SIDE, password.as_deref()) {
                            Ok(p) => (Some(p), false),
                            Err(e) => (None, e.downcast_ref::<pdf::PasswordNeeded>().is_some()),
                        },
                        _ => (cached_thumbnail(cache.as_deref(), &path), false),
                    })
                },
                move |r| {
                    let (picture, locked) = r.unwrap_or((None, false));
                    Message::Thumb(path.clone(), picture, locked)
                },
            ));
        }
        Task::batch(tasks)
    }

    /// Which outline entry the page in view belongs to.
    fn current_section(&self) -> Option<usize> {
        let Doc::Pdf(view) = &self.doc else { return None };
        let page = view.current_page;
        view.doc
            .outline()
            .iter()
            .rposition(|item| matches!(item.target, Some(Target::Page { page: p, .. }) if p <= page))
    }

    fn reveal_in_contents(&self) -> Task<Message> {
        let Some(i) = self.current_section() else { return Task::none() };
        let (offset, height) = self.contents_scroll;
        let height = if height > 0.0 { height } else { self.area.height };
        let top = i as f32 * TOC_ROW_H;
        if top >= offset && top + TOC_ROW_H <= offset + height {
            return Task::none();
        }
        let y = (top - height / 3.0).max(0.0);
        operation::scroll_to(contents_id(), scrollable::AbsoluteOffset { x: None, y: Some(y) })
    }

    fn has_contents(&self) -> bool {
        matches!(&self.doc, Doc::Pdf(v) if !v.doc.outline().is_empty())
    }

    /// The sidebar tab actually shown: the chosen one if it has anything to show.
    fn sidebar_shows(&self) -> Option<SidebarTab> {
        let available = |tab| self.sidebar_tabs().contains(&tab);
        match self.sidebar_tab {
            Some(tab) if available(tab) => Some(tab),
            _ => [SidebarTab::Contents, SidebarTab::Files, SidebarTab::Pages].into_iter().find(|t| available(*t)),
        }
    }

    /// The tabs there is something to show in, in display order.
    fn sidebar_tabs(&self) -> Vec<SidebarTab> {
        let mut tabs = Vec::new();
        if self.files.len() > 1 {
            tabs.push(SidebarTab::Files);
        }
        if self.has_contents() {
            tabs.push(SidebarTab::Contents);
        }
        if matches!(&self.doc, Doc::Pdf(v) if v.doc.page_count() > 1) {
            tabs.push(SidebarTab::Pages);
        }
        tabs
    }

    /// Scrolls the sidebar just enough to show the current file.
    fn reveal_in_sidebar(&self) -> Task<Message> {
        let (offset, height) = self.sidebar_scroll;
        let height = if height > 0.0 { height } else { self.area.height };
        let top = self.current as f32 * ROW_H;
        let y = if top < offset {
            top
        } else if top + ROW_H > offset + height {
            top + ROW_H - height
        } else {
            return Task::none();
        };
        operation::scroll_to(sidebar_id(), scrollable::AbsoluteOffset { x: None, y: Some(y.max(0.0)) })
    }

    // ------------------------------------------------------------ zoom

    fn image_fit_zoom(&self, view: &ImageView) -> f32 {
        let (w, h) = (view.shown.width as f32, view.shown.height as f32);
        ((self.area.width - 24.0) / w).min((self.area.height - 24.0) / h).min(1.0 / self.scale_factor).max(MIN_ZOOM)
    }

    fn pdf_fit_zoom(&self, view: &PdfView, fit: PdfFit) -> f32 {
        let widest = view.doc.pages().iter().map(|p| p.0).fold(1.0, f32::max);
        let by_width = (self.area.width - 2.0 * PAGE_PAD - 12.0) / widest;
        let z = match fit {
            PdfFit::Page => {
                let (_, h) = view.doc.page_size(view.current_page);
                by_width.min((self.area.height - 2.0 * PAGE_PAD) / h)
            }
            _ => by_width,
        };
        z.clamp(MIN_ZOOM, MAX_ZOOM)
    }

    /// Current zoom in percent, as a person reads it (100 % = actual size).
    fn zoom_percent(&self) -> Option<f32> {
        match &self.doc {
            Doc::Image(v) => Some(v.zoom.unwrap_or_else(|| self.image_fit_zoom(v)) * self.scale_factor * 100.0),
            Doc::Pdf(v) => Some(v.zoom / PT_TO_PX * 100.0),
            _ => None,
        }
    }

    /// The zoom on screen right now (logical pixels per picture pixel or per PDF point).
    fn shown_zoom(&self) -> Option<f32> {
        match &self.doc {
            Doc::Image(v) => Some(v.zoom.unwrap_or_else(|| self.image_fit_zoom(v))),
            Doc::Pdf(v) => Some(v.zoom),
            _ => None,
        }
    }

    fn zoom_by(&mut self, factor: f32) -> Task<Message> {
        self.zoom_by_at(factor, None)
    }

    /// Zooms by `factor` keeping `anchor` (a point of the view; the middle when `None`) in place.
    fn zoom_by_at(&mut self, factor: f32, anchor: Option<Point>) -> Task<Message> {
        // Steps taken while easing add up from where it is heading, so fast scrolling stays smooth.
        let Some(base) = self.zoom_ease.map(|e| e.to).or_else(|| self.shown_zoom()) else { return Task::none() };
        let z = (base * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let end = match &self.doc {
            Doc::Image(_) => ZoomEnd::Image(Some(z)),
            Doc::Pdf(_) => ZoomEnd::Pdf(PdfFit::Manual),
            _ => return Task::none(),
        };
        let task = self.ease_zoom(z, end);
        if let Some(ease) = &mut self.zoom_ease {
            ease.anchor = anchor;
        }
        task
    }

    /// A touchpad pinch: the zoom follows the fingers directly, around the pointer.
    fn pinch(&mut self, delta: f32) -> Task<Message> {
        if delta == 0.0 {
            // A pinch began or ended; once it ends, PDF pages are drawn sharp again.
            self.pinch_scale = 1.0;
            return if matches!(self.doc, Doc::Pdf(_)) { self.request_pages() } else { Task::none() };
        }
        let before = self.pinch_scale;
        self.pinch_scale = (before + delta).max(0.05);
        let factor = self.pinch_scale / before;
        let Some(zoom) = self.shown_zoom() else { return Task::none() };
        self.zoom_ease = None;
        let z = (zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let anchor = self.pointer_at;
        match &self.doc {
            Doc::Image(_) => self.set_image_zoom_at(Some(z), anchor),
            Doc::Pdf(_) => self.set_pdf_zoom_at(z, PdfFit::Manual, false, anchor),
            _ => Task::none(),
        }
    }

    fn zoom_fit(&mut self, width: bool) -> Task<Message> {
        match &self.doc {
            Doc::Image(v) => {
                let z = self.image_fit_zoom(v);
                self.ease_zoom(z, ZoomEnd::Image(None))
            }
            Doc::Pdf(v) => {
                let fit = if width { PdfFit::Width } else { PdfFit::Page };
                let z = self.pdf_fit_zoom(v, fit);
                self.ease_zoom(z, ZoomEnd::Pdf(fit))
            }
            _ => Task::none(),
        }
    }

    /// Starts easing the zoom towards `to`; frames carry it there (see [`App::step_zoom`]).
    fn ease_zoom(&mut self, to: f32, end: ZoomEnd) -> Task<Message> {
        let Some(from) = self.shown_zoom() else { return Task::none() };
        let to = to.clamp(MIN_ZOOM, MAX_ZOOM);
        if (to / from - 1.0).abs() < 1e-3 {
            self.zoom_ease = None;
            return self.settle_zoom(end, to);
        }
        let now = Instant::now();
        self.now = now;
        self.zoom_ease = Some(ZoomEase { from, to, start: now, end, anchor: None });
        Task::none()
    }

    /// One frame of an easing zoom: ease-out, in proportion (so in and out feel the same).
    fn step_zoom(&mut self, now: Instant) -> Task<Message> {
        let Some(ease) = self.zoom_ease else { return Task::none() };
        let t = (now.saturating_duration_since(ease.start).as_secs_f32() / ZOOM_EASE.as_secs_f32()).min(1.0);
        if t >= 1.0 {
            self.zoom_ease = None;
            return match ease.end {
                ZoomEnd::Image(zoom) => self.set_image_zoom_at(zoom, ease.anchor),
                ZoomEnd::Pdf(fit) => self.set_pdf_zoom_at(ease.to, fit, true, ease.anchor),
            };
        }
        let eased = 1.0 - (1.0 - t).powi(3);
        let z = ease.from * (ease.to / ease.from).powf(eased);
        match &self.doc {
            Doc::Image(_) => self.set_image_zoom_at(Some(z), ease.anchor),
            // Pages are stretched while easing and drawn sharp once it settles.
            Doc::Pdf(_) => self.set_pdf_zoom_at(z, PdfFit::Manual, false, ease.anchor),
            _ => Task::none(),
        }
    }

    fn settle_zoom(&mut self, end: ZoomEnd, to: f32) -> Task<Message> {
        match end {
            ZoomEnd::Image(zoom) => self.set_image_zoom(zoom),
            ZoomEnd::Pdf(fit) => self.set_pdf_zoom(to, fit),
        }
    }

    fn set_image_zoom(&mut self, zoom: Option<f32>) -> Task<Message> {
        self.set_image_zoom_at(zoom, None)
    }

    /// Sets the picture's zoom (`None` fits it) keeping the spot of the picture under `anchor`
    /// (a point of the view; its middle when `None`) where it is.
    fn set_image_zoom_at(&mut self, zoom: Option<f32>, anchor: Option<Point>) -> Task<Message> {
        let vp = self.viewport;
        let area = self.area;
        let Doc::Image(view) = &self.doc else { return Task::none() };
        let fit = self.image_fit_zoom(view);
        let Doc::Image(view) = &mut self.doc else { return Task::none() };
        let old = view.zoom;
        let zoom = zoom.map(|z| z.clamp(MIN_ZOOM, MAX_ZOOM));
        view.zoom = zoom;
        let (Some(new), w, h) = (zoom, view.shown.width as f32, view.shown.height as f32) else {
            return Task::none();
        };
        let a = anchor.unwrap_or(Point::new(area.width / 2.0, area.height / 2.0));
        // The spot of the picture under the anchor now, in picture pixels. The picture is
        // centred in the view when it is smaller than it.
        let (px, py) = match old {
            Some(z) => {
                let left = ((area.width - w * z) / 2.0).max(0.0);
                let top = ((area.height - h * z) / 2.0).max(0.0);
                ((vp.x + a.x - left) / z, (vp.y + a.y - top) / z)
            }
            None => ((a.x - (area.width - w * fit) / 2.0) / fit, (a.y - (area.height - h * fit) / 2.0) / fit),
        };
        let left = ((area.width - w * new) / 2.0).max(0.0);
        let top = ((area.height - h * new) / 2.0).max(0.0);
        // Where the view is going, known now, so the next step of a zoom builds on it.
        let x = (px * new + left - a.x).clamp(0.0, (w * new - area.width).max(0.0));
        let y = (py * new + top - a.y).clamp(0.0, (h * new - area.height).max(0.0));
        self.viewport.x = x;
        self.viewport.y = y;
        operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x, y })
    }

    fn set_pdf_zoom(&mut self, zoom: f32, fit: PdfFit) -> Task<Message> {
        self.set_pdf_zoom_at(zoom, fit, true, None)
    }

    /// Sets the PDF's zoom keeping the spot under `anchor` (a point of the view; its middle when
    /// `None`) in place; `render` asks for sharp pages.
    fn set_pdf_zoom_at(&mut self, zoom: f32, fit: PdfFit, render: bool, anchor: Option<Point>) -> Task<Message> {
        let vp = self.viewport;
        let area = self.area;
        let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let a = anchor.unwrap_or(Point::new(area.width / 2.0, area.height / 2.0));
        // The page and the spot inside it under the anchor.
        let at_y = vp.y + a.y;
        let page = view.page_at(at_y);
        let frac = (at_y - view.tops[page]) / view.zoom;
        let old_cx = (vp.x + a.x) / view.content_width().max(area.width).max(1.0);

        view.zoom = zoom;
        view.fit = fit;
        view.layout();
        let y = (view.tops[page] + frac * zoom - a.y).max(0.0);
        let x = (old_cx * view.content_width().max(area.width) - a.x).max(0.0);
        self.viewport.y = y;
        self.viewport.x = x;
        let pages = if render { self.request_pages() } else { Task::none() };
        Task::batch([operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x, y }), pages])
    }

    /// Re-applies "fit" zoom after the window or the chrome changed size.
    fn refit(&mut self) -> Task<Message> {
        match &self.doc {
            Doc::Pdf(v) if v.fit != PdfFit::Manual => {
                let fit = v.fit;
                let z = self.pdf_fit_zoom(v, fit);
                if (z - v.zoom).abs() > 0.001 { self.set_pdf_zoom(z, fit) } else { Task::none() }
            }
            _ => Task::none(),
        }
    }

    fn go_to_page(&mut self, page: usize) -> Task<Message> {
        let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
        let page = page.min(view.doc.page_count() - 1);
        view.current_page = page;
        let y = (view.tops[page] - PAGE_GAP / 2.0).max(0.0);
        self.viewport.y = y;
        Task::batch([
            operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x: None, y: Some(y) }),
            self.request_pages(),
            self.follow_in_sidebar(),
        ])
    }

    /// Keeps the sidebar's contents or page list on the page in view.
    fn follow_in_sidebar(&self) -> Task<Message> {
        match self.sidebar_shows() {
            Some(SidebarTab::Contents) => self.reveal_in_contents(),
            Some(SidebarTab::Pages) => self.reveal_page_thumb(),
            _ => Task::none(),
        }
    }

    /// Scrolls so that `point` (page points from the page's top-left) is near the top-left of the view.
    /// Notes the page the open PDF is at, for next time.
    fn remember_place(&mut self) {
        if let Doc::Pdf(view) = &self.doc {
            let path = view.doc.path().to_path_buf();
            self.state.set_page(&path, view.current_page);
        }
    }

    /// The remembered state changed: write it soon (not at every flip through a folder).
    fn state_changed(&mut self) -> Task<Message> {
        if self.state_save_pending || cfg!(test) {
            return Task::none();
        }
        self.state_save_pending = true;
        Task::perform(tokio::time::sleep(Duration::from_secs(2)), |_| Message::SaveState)
    }

    /// The result of looking at the open file on disk: reload it if another program changed it.
    fn file_stat(&mut self, path: PathBuf, stamp: Option<Stamp>) -> Task<Message> {
        if self.current_path() != Some(path.as_path()) {
            return Task::none();
        }
        // The first look just notes how the file is; a file that vanished is left on screen.
        let Some(now) = stamp else { return Task::none() };
        let known = match &self.disk {
            Some((p, Some(known))) if *p == path => *known,
            _ => {
                self.disk = Some((path, Some(now)));
                return Task::none();
            }
        };
        if known == now {
            return Task::none();
        }
        self.disk = Some((path.clone(), Some(now)));
        if self.has_unsaved_changes() {
            return self.toast(trf!(
                "“{}” changed on disk. Your unsaved changes are kept; saving will overwrite it.",
                file_name(&path)
            ));
        }
        let place = self.place();
        self.pictures.retain(|(p, _)| *p != path);
        self.thumbs.remove(&path);
        self.restore = place;
        let reloaded = self.show_forced(self.current);
        self.disk = Some((path.clone(), Some(now)));
        Task::batch([reloaded, self.notice(trf!("Reloaded “{}”: it changed on disk", file_name(&path)))])
    }

    /// Where the PDF view is now, in terms that survive a change of zoom.
    fn place(&self) -> Option<Place> {
        let Doc::Pdf(view) = &self.doc else { return None };
        let page = view.page_at(self.viewport.y + self.viewport.height * 0.4);
        let down = (self.viewport.y - view.tops[page]) / view.zoom;
        Some(Place { page, down, across: self.viewport.x / view.zoom })
    }

    fn go_to_place(&mut self, to: Place) -> Task<Message> {
        let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
        let page = to.page.min(view.doc.page_count() - 1);
        view.current_page = page;
        let y = (view.tops[page] + to.down * view.zoom).max(0.0);
        let x = (to.across * view.zoom).max(0.0);
        self.viewport.y = y;
        self.viewport.x = x;
        Task::batch([
            operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x, y }),
            self.request_pages(),
            self.follow_in_sidebar(),
        ])
    }

    fn go_to_point(&mut self, page: usize, point: Option<(f32, f32)>) -> Task<Message> {
        let Some((x, top)) = point else { return self.go_to_page(page) };
        let area = self.area;
        let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
        let page = page.min(view.doc.page_count() - 1);
        view.current_page = page;
        let y = (view.tops[page] + top * view.zoom - 16.0).max(0.0);
        let left = (view.content_width() - view.doc.page_size(page).0 * view.zoom) / 2.0;
        let x = (left + x * view.zoom - area.width / 2.0).max(0.0);
        self.viewport.y = y;
        self.viewport.x = x;
        Task::batch([
            operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x, y }),
            self.request_pages(),
            self.follow_in_sidebar(),
        ])
    }

    /// Copies the selected text to the clipboard (or the primary selection). Pages whose text
    /// is not loaded yet (after Select All, say) are read first, off the UI thread.
    fn copy_selection(&self, primary: bool) -> Task<Message> {
        let (Doc::Pdf(view), Some(sel)) = (&self.doc, self.selection) else { return Task::none() };
        if sel.is_empty() {
            return Task::none();
        }
        let doc = view.doc.clone();
        let (a, b) = sel.ordered();
        Task::perform(
            async move {
                let mut out = String::new();
                for page in a.0..=b.0 {
                    let text = doc.text(page).await;
                    let Some(range) = sel.on_page(page, text.len()) else { continue };
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&text.text(range));
                }
                out
            },
            move |text| Message::Copied(text, primary),
        )
    }

    /// (Re)starts searching the open PDF for the current query, from the page in view onwards.
    fn start_search(&mut self) -> Task<Message> {
        self.search_seq += 1;
        let seq = self.search_seq;
        let Some(search) = &mut self.search else { return Task::none() };
        *search =
            Search { query: std::mem::take(&mut search.query), options: search.options, seq, ..Search::default() };
        let Doc::Pdf(view) = &self.doc else { return Task::none() };
        let query = search.query.trim().to_string();
        let options = search.options;
        if query.is_empty() {
            search.done = true;
            return Task::none();
        }
        let doc = view.doc.clone();
        let n = doc.page_count();
        let start = view.current_page;
        let pages = iced::futures::stream::unfold(0usize, move |k| {
            let (doc, query) = (doc.clone(), query.clone());
            async move {
                if k >= n {
                    return None;
                }
                let page = (start + k) % n;
                let found = doc.text(page).await.find_with(&query, options);
                Some((Message::SearchHits(seq, page, found), k + 1))
            }
        });
        let (task, handle) = Task::stream(pages).chain(Task::done(Message::SearchDone(seq))).abortable();
        search._scan = Some(handle.abort_on_drop());
        task
    }

    /// Scrolls the current search match into view.
    fn reveal_hit(&mut self) -> Task<Message> {
        let Some(search) = &self.search else { return Task::none() };
        let Some(hit) = search.current.and_then(|c| search.hits.get(c)) else { return Task::none() };
        let (page, rect) = (hit.page, hit.rects[0]);
        let area = self.area;
        let vp = self.viewport;
        let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
        let page_left = (view.content_width().max(area.width) - view.doc.page_size(page).0 * view.zoom) / 2.0;
        let (x0, y0) = (page_left + rect.x0 as f32 * view.zoom, view.tops[page] + rect.y0 as f32 * view.zoom);
        let (x1, y1) = (page_left + rect.x1 as f32 * view.zoom, view.tops[page] + rect.y1 as f32 * view.zoom);
        // Leave the view alone when the match is already comfortably visible.
        if y0 >= vp.y + 40.0 && y1 <= vp.y + area.height - 40.0 && x0 >= vp.x && x1 <= vp.x + area.width {
            return Task::none();
        }
        view.current_page = page;
        let y = (y0 - area.height * 0.35).max(0.0);
        let x = if x0 < vp.x || x1 > vp.x + area.width { ((x0 + x1) / 2.0 - area.width / 2.0).max(0.0) } else { vp.x };
        self.viewport.y = y;
        self.viewport.x = x;
        Task::batch([operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x, y }), self.request_pages()])
    }

    /// Asks the render threads for the visible pages (and one either side) at
    /// the current zoom, and frees pages far from view.
    fn request_pages(&mut self) -> Task<Message> {
        let scale_factor = self.scale_factor;
        let generation = self.generation;
        let vp = self.viewport;
        let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
        let visible = view.visible(&vp);
        let n = view.doc.page_count();
        let lo = visible.start().saturating_sub(1);
        let hi = (*visible.end() + 1).min(n - 1);
        // Quantise so tiny zoom changes reuse what is already rendered.
        let wanted = ((view.zoom * scale_factor * 16.0).round() / 16.0).max(0.05);
        let night = self.night;

        let mut tasks = Vec::new();
        // Visible pages first, then the neighbours.
        let order = visible.clone().chain([lo, hi].into_iter().filter(|i| !visible.contains(i)));
        for i in order {
            let slot = &mut view.slots[i];
            let good = slot.shown.as_ref().is_some_and(|(_, s)| (s / wanted - 1.0).abs() < 0.02);
            if good || slot.pending == Some(wanted) {
                continue;
            }
            slot.pending = Some(wanted);
            let doc = view.doc.clone();
            tasks.push(Task::perform(
                async move { doc.render_toned(i, wanted, night).await.map_err(err_string) },
                move |r| Message::PageRendered(generation, i, wanted, night, r),
            ));
        }
        // Text for selection, for the pages in view.
        for i in visible.clone() {
            let slot = &mut view.slots[i];
            if slot.text.is_some() || slot.text_pending {
                continue;
            }
            slot.text_pending = true;
            let doc = view.doc.clone();
            tasks.push(Task::perform(async move { doc.text(i).await }, move |t| Message::PageText(generation, i, t)));
        }
        // Bound memory: drop bitmaps of pages well out of view.
        for (i, slot) in view.slots.iter_mut().enumerate() {
            if (i + 6 < lo || i > hi + 6) && slot.shown.is_some() {
                slot.shown = None;
            }
        }
        Task::batch(tasks)
    }
}

// ---------------------------------------------------------------- view

fn icon_button<'a>(
    handle: svg::Handle,
    label: impl text::IntoFragment<'a>,
    on_press: Option<Message>,
    active: bool,
) -> Element<'a, Message> {
    let enabled = on_press.is_some();
    let look = match (enabled, active) {
        (true, true) => style::icon_accent,
        (true, false) => style::icon,
        _ => style::icon_muted,
    };
    let glyph = svg(handle).width(18).height(18).style(look);
    let b = button(center(glyph).width(32).height(32))
        .padding(0)
        .style(if active { style::tool_active } else { style::tool })
        .on_press_maybe(on_press);
    tooltip(b, container(text(label).size(12)).padding([4, 8]).style(style::pill), tooltip::Position::Bottom)
        .gap(6)
        .into()
}

/// `content` with a tooltip below it.
fn tip<'a>(content: Element<'a, Message>, label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    tooltip(content, container(text(label).size(12)).padding([4, 8]).style(style::pill), tooltip::Position::Bottom)
        .gap(6)
        .into()
}

/// A small text button that is on or off, with a tooltip (the search bar's options).
fn search_toggle<'a>(label: &'a str, tip_text: String, on: bool, message: Message) -> Element<'a, Message> {
    let look = if on { style::tool_active } else { style::tool };
    let b = button(center(text(label).size(12)).width(32).height(28)).padding(0).style(look).on_press(message);
    tip(b.into(), tip_text)
}

/// A picture's thumbnail: the desktop's cached one when there is a good one, else made now (and
/// cached for the next time, and for the file manager).
fn cached_thumbnail(cache: Option<&Path>, path: &Path) -> Option<raster::Picture> {
    if let Some(found) = cache.and_then(|base| crate::thumbcache::read(base, path)) {
        return Some(found);
    }
    let made = raster::thumbnail(path).ok()?;
    if let Some(base) = cache {
        crate::thumbcache::write(base, path, &made);
    }
    Some(made)
}

/// A folder as shown in the recent list: the home folder is written `~`.
fn shorten_home(dir: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
}

fn separator<'a>() -> Element<'a, Message> {
    container(space().width(1).height(22))
        .style(|t: &Theme| container::Style {
            background: Some(t.extended_palette().background.strong.color.into()),
            ..Default::default()
        })
        .into()
}

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let content = sensor(container(self.content()).width(Length::Fill).height(Length::Fill).style(style::canvas))
            .on_show(Message::AreaResized)
            .on_resize(Message::AreaResized);
        let mut view_area = stack![content];
        if let Some(controls) = self.floating_controls() {
            view_area = view_area.push(controls);
        }
        // Tools float on the view as islands at its top, not as bars across the window.
        let island: Option<Element<'_, Message>> = if self.organizer.is_some() {
            Some(self.pages_bar())
        } else if self.edit_open && matches!(self.doc, Doc::Image(_)) {
            Some(self.edit_bar())
        } else if self.markup_open && matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) {
            Some(self.markup_bar())
        } else {
            None
        };
        if let Some(island) = island.filter(|_| !self.fullscreen) {
            view_area =
                view_area.push(container(island).width(Length::Fill).align_x(Alignment::Center).padding([10, 16]));
        }
        let content = mouse_area(view_area).on_move(Message::Pointer).on_exit(Message::PointerLeft);

        let mut body = row![];
        if !self.fullscreen
            && self.show_sidebar
            && let Some(tab) = self.sidebar_shows()
        {
            body = body.push(self.sidebar(tab));
        }
        // Always a stack, so the view's shape (and the scroll position inside it) does not
        // change when a floating box comes and goes.
        let mut floating = stack![content];
        if let (Some(search), Doc::Pdf(_)) = (&self.search, &self.doc) {
            floating = floating
                .push(container(self.search_bar(search)).width(Length::Fill).align_x(Alignment::End).padding([10, 18]));
        }
        if let (Some(typed), Doc::Pdf(v)) = (&self.goto, &self.doc) {
            floating = floating.push(
                container(self.goto_bar(typed, v.doc.page_count()))
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .padding([10, 18]),
            );
        }
        let content: Element<'_, Message> = floating.into();
        body = body.push(content);
        let mut layout = column![];
        if !self.fullscreen {
            layout = layout.push(self.toolbar());
        }
        layout = layout.push(body.height(Length::Fill));

        let mut layers = stack![layout.width(Length::Fill).height(Length::Fill)];
        if self.drop_hover {
            layers = layers.push(self.drop_overlay());
        }
        if self.edit_open && matches!(self.doc, Doc::Image(_)) && !self.fullscreen {
            if self.adjust_open {
                layers = layers.push(self.adjust_panel());
            }
            if self.resize_form.is_some() {
                layers = layers.push(self.resize_panel());
            }
        }
        if self.sign_menu {
            layers = layers.push(self.sign_menu_view());
        }
        if let Some(palette) = self.palette_view() {
            layers = layers.push(palette);
        }
        if let Some(menu) = self.menu_view() {
            layers = layers.push(menu);
        }
        if let Some(dialog) = self.dialog_view() {
            layers = layers.push(dialog);
        }
        if let Some(chooser) = self.chooser_view() {
            layers = layers.push(chooser);
        }
        if let Some(printing) = self.print_dialog_view() {
            layers = layers.push(printing);
        }
        if self.pad.is_some() {
            layers = layers.push(self.pad_view());
        }
        if self.confirm_quit {
            layers = layers.push(self.quit_dialog());
        }
        if let Some((id, msg, error)) = &self.toast {
            let look = if *error { style::toast_error } else { style::panel };
            let mut line = row![text(msg.as_str()).size(13)].spacing(12).align_y(Alignment::Center);
            if self.trash_undo.as_ref().is_some_and(|(t, ..)| t == id) {
                line = line.push(
                    button(text(tr("Undo")).size(13))
                        .padding([3, 10])
                        .style(style::secondary)
                        .on_press(Message::UndoTrash),
                );
            }
            if self.pages_undo == Some(*id) {
                line = line.push(
                    button(text(tr("Undo")).size(13))
                        .padding([3, 10])
                        .style(style::secondary)
                        .on_press(Message::Pages(PagesMsg::Undo)),
                );
            }
            layers = layers.push(
                container(container(line).padding([9, 14]).style(look))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::End)
                    .padding(Padding { bottom: 24.0, ..Padding::ZERO }),
            );
        }
        layers.into()
    }

    /// The header, as in the GTK Lumen: the sidebar and open buttons, the title, and on the
    /// right the editing tools, the main menu and close.
    fn toolbar(&self) -> Element<'_, Message> {
        let has_doc = matches!(self.doc, Doc::Image(_) | Doc::Pdf(_));
        let is_image = matches!(self.doc, Doc::Image(_));
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let multi = self.files.len() > 1;

        let mut left = row![].spacing(2).align_y(Alignment::Center);
        if self.sidebar_shows().is_some() {
            // Usually on, so it stays quiet rather than lit.
            left = left.push(icon_button(icons::strip(), tr("Sidebar (T)"), Some(Message::ToggleSidebar), false));
        }
        let open = button(
            row![
                svg(icons::folder()).width(20).height(20).style(style::icon),
                svg(icons::down()).width(14).height(14).style(style::icon),
            ]
            .spacing(2)
            .align_y(Alignment::Center),
        )
        .padding([8, 8])
        .style(if self.menu == Some(Menu::Open) { style::tool_active } else { style::tool })
        .on_press(Message::Menu(Some(Menu::Open)));
        left = left.push(tip(open.into(), tr("Open")));

        let (title, subtitle) = match &self.doc {
            Doc::Empty => ("Lumen".to_string(), String::new()),
            Doc::Loading(p) => (file_name(p), tr("Loading…")),
            Doc::Failed { path, .. } => (file_name(path), tr("Could not open")),
            Doc::Locked { path, .. } => (file_name(path), tr("Password-protected")),
            Doc::Image(v) => (file_name(&v.path), String::new()),
            Doc::Pdf(v) => (file_name(v.doc.path()), trf!("Page {} of {}", v.current_page + 1, v.doc.page_count())),
        };
        let position = if multi { trf!("{} of {}", self.current + 1, self.files.len()) } else { String::new() };
        let subtitle = match (subtitle.is_empty(), position.is_empty()) {
            (true, _) => position,
            (false, true) => subtitle,
            (false, false) => format!("{subtitle}  ·  {position}"),
        };
        let mut titles = column![
            text(title)
                .size(15)
                .font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT })
                .wrapping(text::Wrapping::None),
        ]
        .align_x(Alignment::Center)
        .spacing(1);
        if !subtitle.is_empty() {
            let label = text(subtitle).size(12).style(muted);
            titles = titles.push(if is_pdf {
                tip(
                    button(label).padding([0, 6]).style(style::tool).on_press(Message::GoToOpen).into(),
                    tr("Go to page (Ctrl+G)"),
                )
            } else {
                label.into()
            });
        }

        let mut right = row![].spacing(2).align_y(Alignment::Center);
        if is_pdf && self.has_unsaved_changes() && !self.markup_open {
            let save = button(text(tr("Save")).size(13))
                .padding([6, 14])
                .style(style::primary)
                .on_press_maybe(self.markup_busy.is_none().then_some(Message::Markup(MarkupMsg::Save)));
            right = right.push(tip(save.into(), tr("Save into the PDF (Ctrl+S)"))).push(space().width(6));
        }
        if is_pdf {
            right = right.push(icon_button(
                icons::search(),
                tr("Find (Ctrl+F)"),
                Some(Message::SearchOpen),
                self.search.is_some(),
            ));
        }
        right = right.push(icon_button(
            icons::markup(),
            tr("Markup: draw, highlight, add text (M)"),
            has_doc.then_some(Message::Markup(MarkupMsg::Toggle)),
            self.markup_open,
        ));
        if is_image {
            right = right.push(icon_button(
                icons::adjust(),
                tr("Edit: crop, turn, resize, light and colour (E)"),
                Some(Message::Edit(EditMsg::Toggle)),
                self.edit_open,
            ));
        }
        if is_pdf {
            right = right.push(icon_button(
                icons::pages(),
                tr("Organise pages (G)"),
                self.markup_busy.is_none().then_some(Message::Pages(PagesMsg::Open)),
                self.organizer.is_some(),
            ));
        }
        right = right
            .push(icon_button(
                icons::menu(),
                tr("Menu"),
                Some(Message::Menu(Some(Menu::Main))),
                self.menu == Some(Menu::Main),
            ))
            .push(icon_button(icons::close(), tr("Close (Ctrl+Q)"), Some(Message::Quit), false));

        // A picture's size shows when pointing at its name.
        let titles: Element<'_, Message> = match &self.doc {
            Doc::Image(v) => {
                let (w, h) = v.shown.picture.original;
                tip(titles.into(), format!("{w} × {h} pixels"))
            }
            _ => titles.into(),
        };
        container(
            row![left, container(titles).width(Length::Fill).align_x(Alignment::Center).clip(true), right]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .height(TOOLBAR_H)
        .padding([0, 8])
        .align_y(Alignment::Center)
        .style(style::bar)
        .into()
    }

    fn content(&self) -> Element<'_, Message> {
        match &self.doc {
            Doc::Empty => self.empty_state(),
            Doc::Loading(_) => center(text(tr("Loading…")).size(15).style(muted)).into(),
            Doc::Locked { path, wrong } => self.locked_view(path, *wrong),
            Doc::Failed { path, error } => center(
                column![
                    svg(icons::broken()).width(56).height(56).style(style::icon_muted),
                    text(trf!("Could not open “{}”", file_name(path))).size(18),
                    text(error.as_str()).size(13).style(muted),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .into(),
            Doc::Image(view) => self.image_view(view),
            Doc::Pdf(_) if self.organizer.is_some() => self.pages_view(),
            Doc::Pdf(view) => self.pdf_view(view),
        }
    }

    fn pannable<'a>(&self, content: Element<'a, Message>, can_pan: bool) -> Element<'a, Message> {
        if !can_pan {
            return content;
        }
        let mut area = mouse_area(content)
            .on_press(Message::PanStart)
            .on_release(Message::PanEnd)
            .on_middle_press(Message::PanStart)
            .on_middle_release(Message::PanEnd)
            .interaction(if self.pan.is_some() { mouse::Interaction::Grabbing } else { mouse::Interaction::Grab });
        if self.pan.is_some() {
            area = area.on_move(Message::PanMove).on_exit(Message::PanEnd);
        }
        area.into()
    }

    fn image_view<'a>(&'a self, view: &'a ImageView) -> Element<'a, Message> {
        let fit = view.zoom.is_none();
        let zoom = view.zoom.unwrap_or_else(|| self.image_fit_zoom(view));
        let (w, h) = (view.shown.width as f32 * zoom, view.shown.height as f32 * zoom);
        // Nearest-neighbour once pixels are big, so pixel art stays crisp.
        let filter =
            if zoom * self.scale_factor >= 3.0 { image::FilterMethod::Nearest } else { image::FilterMethod::Linear };
        let handle = self.anim_frame().unwrap_or(&view.shown.handle).clone();
        let picture = image(handle).filter_method(filter).content_fit(ContentFit::Fill).width(w).height(h);
        let mut layers = stack![picture].width(w).height(h);
        if let Some(crop) = self.crop_layer(zoom, w, h) {
            layers = layers.push(crop);
        } else {
            for layer in self.markup_layers(0, zoom, w, h) {
                layers = layers.push(layer);
            }
        }
        if fit {
            return container(layers).padding(12).center(Length::Fill).into();
        }
        let canvas = container(layers)
            .width(w.max(self.area.width))
            .height(h.max(self.area.height))
            .center_x(w.max(self.area.width))
            .center_y(h.max(self.area.height));
        let scroller = scrollable(canvas)
            .id(scroll_id())
            .direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::new().width(8).scroller_width(6),
                horizontal: scrollable::Scrollbar::new().width(8).scroller_width(6),
            })
            .on_scroll(on_scroll)
            .style(style::scrollbar)
            .width(Length::Fill)
            .height(Length::Fill);
        let can_pan = w > self.area.width || h > self.area.height;
        self.pannable(scroller.into(), can_pan)
    }

    fn pdf_view<'a>(&'a self, view: &'a PdfView) -> Element<'a, Message> {
        let n = view.doc.page_count();
        let visible = view.visible(&self.viewport);
        let lo = visible.start().saturating_sub(2);
        let hi = (*visible.end() + 2).min(n - 1);
        let total_w = view.content_width().max(self.area.width);

        // Only the pages near the view are real widgets; the rest are spacers.
        let mut pages = column![].width(total_w).align_x(Alignment::Center);
        pages = pages.push(space().height(view.tops[lo]));
        for i in lo..=hi {
            let (pw, ph) = view.doc.page_size(i);
            let (w, h) = (pw * view.zoom, ph * view.zoom);
            let inner: Element<'_, Message> = match &view.slots[i].shown {
                Some((handle, _)) => image(handle.clone())
                    .width(w)
                    .height(h)
                    .content_fit(ContentFit::Fill)
                    .filter_method(image::FilterMethod::Linear)
                    .into(),
                None => space().width(w).height(h).into(),
            };
            let mut layers = stack![inner].width(w).height(h);
            if let Some(search) = &self.search {
                let at = search.hits.partition_point(|hit| hit.page < i);
                for (k, hit) in search.hits[at..].iter().enumerate().take_while(|(_, hit)| hit.page == i) {
                    let current = search.current == Some(at + k);
                    for r in &hit.rects {
                        let r = r.inflate(1.0, 1.0);
                        let mark = container(space())
                            .width(r.width() as f32 * view.zoom)
                            .height(r.height() as f32 * view.zoom)
                            .style(style::highlight(current));
                        layers = layers.push(placed(&r, view.zoom, mark.into()));
                    }
                }
            }
            if let (Some(sel), Some(text)) = (&self.selection, &view.slots[i].text)
                && let Some(range) = sel.on_page(i, text.len())
            {
                for r in text.boxes(range) {
                    let r = r.inflate(0.5, 0.5);
                    let mark = container(space())
                        .width(r.width() as f32 * view.zoom)
                        .height(r.height() as f32 * view.zoom)
                        .style(style::selection);
                    layers = layers.push(placed(&r, view.zoom, mark.into()));
                }
            }
            for link in view.doc.links(i) {
                let area = mouse_area(
                    space().width(link.rect.width() as f32 * view.zoom).height(link.rect.height() as f32 * view.zoom),
                )
                .on_press(Message::GoTo(link.target.clone()))
                .interaction(mouse::Interaction::Pointer);
                let area: Element<'_, Message> = match &link.target {
                    Target::Uri(uri) => tooltip(
                        area,
                        container(text(uri.as_str()).size(12)).padding([4, 8]).style(style::pill),
                        tooltip::Position::FollowCursor,
                    )
                    .into(),
                    Target::Page { page, .. } => tooltip(
                        area,
                        container(text(trf!("Go to page {}", page + 1)).size(12)).padding([4, 8]).style(style::pill),
                        tooltip::Position::FollowCursor,
                    )
                    .into(),
                };
                layers = layers.push(placed(&link.rect, view.zoom, area));
            }
            for layer in self.form_layers(i, view.zoom) {
                layers = layers.push(layer);
            }
            for layer in self.markup_layers(i, view.zoom, w, h) {
                layers = layers.push(layer);
            }
            let hovered = self.hover.is_some_and(|(p, _)| p == i);
            let interaction = if self.pan.is_some() {
                mouse::Interaction::Grabbing
            } else if self.selecting || (hovered && self.hover_text) {
                mouse::Interaction::Text
            } else {
                mouse::Interaction::Grab
            };
            let page = mouse_area(container(layers).width(w).height(h).style(style::page))
                .on_move(move |p| Message::PageHover(i, p))
                .on_press(Message::PagePress(i))
                .on_double_click(Message::PageDoubleClick(i))
                .on_middle_press(Message::PanStart)
                .interaction(interaction);
            pages = pages.push(page);
            if i < hi {
                pages = pages.push(space().height(PAGE_GAP));
            }
        }
        let rest = view.tops[n] - (view.tops[hi] + view.doc.page_size(hi).1 * view.zoom);
        pages = pages.push(space().height(rest.max(0.0)));

        let wide = view.content_width() > self.area.width + 1.0;
        let direction = if wide {
            scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::new().width(8).scroller_width(6),
                horizontal: scrollable::Scrollbar::new().width(8).scroller_width(6),
            }
        } else {
            scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(8).scroller_width(6))
        };
        let scroller = scrollable(pages)
            .id(scroll_id())
            .direction(direction)
            .on_scroll(on_scroll)
            .style(style::scrollbar)
            .width(Length::Fill)
            .height(Length::Fill);
        self.pannable(scroller.into(), true)
    }

    /// The file list on the left. Only the rows on screen (plus a margin) become widgets,
    /// so a folder of ten thousand files costs about as much as one of ten.
    fn sidebar(&self, tab: SidebarTab) -> Element<'_, Message> {
        let list = match tab {
            SidebarTab::Files => self.file_list(),
            SidebarTab::Contents => self.contents_list(),
            SidebarTab::Pages => self.page_list(),
        };
        let mut col = column![];
        let tabs = self.sidebar_tabs();
        if tabs.len() > 1 {
            let segment = |label: String, this: SidebarTab| {
                button(text(label).size(12).width(Length::Fill).align_x(Alignment::Center))
                    .padding([4, 8])
                    .width(Length::Fill)
                    .style(style::segment(tab == this))
                    .on_press(Message::SidebarTab(this))
            };
            let segments = tabs.iter().map(|&t| {
                let label = match t {
                    SidebarTab::Files => "Files",
                    SidebarTab::Contents => "Contents",
                    SidebarTab::Pages => "Pages",
                };
                segment(tr(label), t).into()
            });
            col = col.push(container(row(segments).spacing(2)).padding([6, 10]));
        }
        container(col.push(list)).width(SIDEBAR_W).height(Length::Fill).style(style::bar).into()
    }

    /// The table of contents, virtualised like the file list (rows have a fixed height).
    fn contents_list(&self) -> Element<'_, Message> {
        let Doc::Pdf(view) = &self.doc else { return space().into() };
        let outline = view.doc.outline();
        let (offset, height) = self.contents_scroll;
        let height = if height > 0.0 { height } else { self.area.height };
        let first = ((offset / TOC_ROW_H).floor().max(0.0) as usize).saturating_sub(10).min(outline.len());
        let last = (((offset + height) / TOC_ROW_H).ceil() as usize + 10).min(outline.len());
        let section = self.current_section();
        let mut list = column![space().height(first as f32 * TOC_ROW_H)];
        for (i, item) in outline.iter().enumerate().take(last).skip(first) {
            let page = match &item.target {
                Some(Target::Page { page, .. }) => format!("{}", page + 1),
                _ => String::new(),
            };
            let strong = item.depth == 0;
            let label = row![
                space().width(item.depth.min(6) as f32 * 14.0),
                container(text(item.title.as_str()).size(13).wrapping(text::Wrapping::None).font(Font {
                    weight: if strong { iced::font::Weight::Semibold } else { iced::font::Weight::Normal },
                    ..crate::style::FONT
                }))
                .width(Length::Fill)
                .clip(true),
                text(page).size(11).style(muted),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            let entry = button(label)
                .padding([0, 8])
                .width(Length::Fill)
                .height(Length::Fill)
                .style(style::thumb(section == Some(i)))
                .on_press_maybe(item.target.clone().map(Message::GoTo));
            let entry: Element<'_, Message> = if item.title.chars().count() > 26 {
                tooltip(
                    entry,
                    container(text(item.title.as_str()).size(12)).padding([4, 8]).style(style::pill).max_width(420),
                    tooltip::Position::Right,
                )
                .into()
            } else {
                entry.into()
            };
            list = list.push(container(entry).padding([1, 6]).height(TOC_ROW_H));
        }
        list = list.push(space().height((outline.len() - last) as f32 * TOC_ROW_H));
        scrollable(list.width(Length::Fill))
            .id(contents_id())
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4)))
            .style(style::scrollbar)
            .on_scroll(|vp| Message::ContentsScrolled(vp.absolute_offset().y, vp.bounds().height))
            .height(Length::Fill)
            .into()
    }

    fn search_bar<'a>(&'a self, search: &'a Search) -> Element<'a, Message> {
        let find_hint = tr("Find in document");
        let status = if search.query.trim().is_empty() {
            String::new()
        } else if search.hits.is_empty() {
            if search.done { tr("No matches") } else { tr("Searching…") }
        } else {
            let n = search.hits.len();
            let more = if search.done { "" } else { "+" };
            match search.current {
                Some(c) => format!("{} of {n}{more}", c + 1),
                None => format!("{n}{more} matches"),
            }
        };
        let has_hits = !search.hits.is_empty();
        container(
            row![
                svg(icons::search()).width(16).height(16).style(style::icon_muted),
                text_input(&find_hint, &search.query)
                    .id(search_input_id())
                    .on_input(Message::SearchInput)
                    .on_submit(Message::SearchSubmit)
                    .size(14)
                    .padding([6, 8])
                    .width(210),
                search_toggle("Aa", tr("Match case"), search.options.match_case, Message::SearchMatchCase),
                search_toggle("[ab]", tr("Whole words"), search.options.whole_words, Message::SearchWholeWords),
                text(status).size(12).style(muted).width(78).align_x(Alignment::Center),
                icon_button(
                    icons::up(),
                    tr("Previous match (Shift+Enter)"),
                    has_hits.then_some(Message::SearchPrev),
                    false
                ),
                icon_button(icons::down(), tr("Next match (Enter)"), has_hits.then_some(Message::SearchNext), false),
                icon_button(icons::close(), tr("Close (Esc)"), Some(Message::SearchClose), false),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([6, 10])
        .style(style::panel)
        .into()
    }

    fn file_list(&self) -> Element<'_, Message> {
        let rows = self.sidebar_rows();
        let first = rows.start.saturating_sub(6);
        let last = (rows.end + 6).min(self.files.len());
        let mut list = column![space().height(first as f32 * ROW_H)];
        for (i, path) in self.files.iter().enumerate().take(last).skip(first) {
            let thumb: Element<'_, Message> = match self.thumbs.get(path) {
                Some(Some(handle)) => image(handle.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .border_radius(4)
                    .into(),
                _ if self.locked_thumbs.contains(path) => {
                    container(svg(icons::lock()).width(22).height(22).style(style::icon_muted))
                        .center(Length::Fill)
                        .style(style::thumb_placeholder)
                        .into()
                }
                _ => container(space()).width(Length::Fill).height(Length::Fill).style(style::thumb_placeholder).into(),
            };
            // A big picture with the file name underneath, as in the GTK Lumen. The file on
            // screen gets an accent ring and a stronger name; nothing is filled in.
            let current = i == self.current;
            let name = text(file_name(path))
                .size(11)
                .font(style::font(if current { iced::font::Weight::Medium } else { iced::font::Weight::Normal }))
                .wrapping(text::Wrapping::None)
                .align_x(Alignment::Center)
                .width(Length::Fill)
                .style(move |t: &Theme| text::Style {
                    color: Some(t.extended_palette().background.base.text.scale_alpha(if current {
                        1.0
                    } else {
                        0.55
                    })),
                });
            let framed = container(container(thumb).width(Length::Fill).height(Length::Fill))
                .padding(3)
                .width(Length::Fill)
                .height(ROW_THUMB)
                .style(style::thumb_frame(current));
            let content = column![framed, container(name).width(Length::Fill).clip(true)].spacing(5);
            list = list.push(
                container(
                    button(content)
                        .padding(4)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(style::thumb(false))
                        .on_press(Message::Select(i)),
                )
                .padding([2, 8])
                .height(ROW_H),
            );
        }
        list = list.push(space().height((self.files.len() - last) as f32 * ROW_H));
        scrollable(list.width(Length::Fill))
            .id(sidebar_id())
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4)))
            .style(style::scrollbar)
            .on_scroll(|vp| Message::SidebarScrolled(vp.absolute_offset().y, vp.bounds().height))
            .height(Length::Fill)
            .into()
    }

    fn empty_state(&self) -> Element<'_, Message> {
        let open = button(
            row![
                svg(icons::open()).width(16).height(16).style(style::icon_on_primary),
                text(tr("Open Files…")).size(14)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([9, 16])
        .style(style::primary)
        .on_press(Message::Open { folder: false });
        let folder = button(text(tr("Open a Folder")).size(14))
            .padding([9, 14])
            .style(style::tool)
            .on_press(Message::Open { folder: true });
        let key = |k: &'static str| {
            container(text(k).size(11).font(style::font(iced::font::Weight::Medium))).padding([2, 6]).style(
                |t: &Theme| container::Style {
                    border: iced::Border {
                        radius: 4.0.into(),
                        width: 1.0,
                        color: t.extended_palette().background.base.text.scale_alpha(0.18),
                    },
                    ..Default::default()
                },
            )
        };
        center(
            column![
                svg(icons::app()).width(84).height(84),
                space().height(6),
                text(tr("Lumen")).size(24).font(style::font(iced::font::Weight::Semibold)),
                text(tr("Pictures and PDFs, fast and simple")).size(14).style(muted),
                space().height(14),
                row![open, folder].spacing(8),
                space().height(14),
                row![
                    key("Ctrl O"),
                    text(tr("open")).size(12).style(muted),
                    space().width(8),
                    text(tr("or drop files and folders here")).size(12).style(muted),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                self.recent_list(),
            ]
            .spacing(6)
            .align_x(Alignment::Center),
        )
        .into()
    }

    /// The files opened lately, under the buttons of the empty window.
    fn recent_list(&self) -> Element<'_, Message> {
        let recent: Vec<_> = self.state.recent.iter().take(RECENT_SHOWN).collect();
        if recent.is_empty() {
            return space().into();
        }
        let mut list = column![
            space().height(18),
            row![
                text(tr("Recent")).size(12).font(style::font(iced::font::Weight::Medium)).style(muted),
                space().width(Length::Fill),
                button(text(tr("Clear")).size(12).style(muted))
                    .padding([0, 4])
                    .style(style::tool)
                    .on_press(Message::ClearRecent),
            ]
            .width(360)
            .align_y(Alignment::Center),
        ]
        .spacing(2)
        .align_x(Alignment::Center);
        for r in recent {
            let folder = r.path.parent().map(shorten_home).unwrap_or_default();
            let entry = button(
                row![
                    text(file_name(&r.path)).size(13).wrapping(text::Wrapping::None),
                    container(text(folder).size(11).style(muted).wrapping(text::Wrapping::None))
                        .width(Length::Fill)
                        .align_x(Alignment::End)
                        .clip(true),
                ]
                .spacing(12)
                .width(Length::Fill)
                .align_y(Alignment::Center),
            )
            .padding([6, 10])
            .width(360)
            .style(style::tool)
            .on_press(Message::OpenRecent(r.path.clone()));
            list = list.push(entry);
        }
        list.into()
    }

    /// Asks for the password of a protected PDF.
    fn locked_view<'a>(&'a self, path: &'a Path, wrong: bool) -> Element<'a, Message> {
        let password_hint = tr("Password");
        let input = text_input(&password_hint, &self.password_input)
            .id(password_input_id())
            .secure(true)
            .on_input(Message::PasswordInput)
            .on_submit(Message::Unlock)
            .size(15)
            .padding([8, 10])
            .width(280);
        let unlock = button(text(tr("Unlock")).size(15))
            .padding([8, 18])
            .style(style::primary)
            .on_press_maybe((!self.password_input.is_empty()).then_some(Message::Unlock));
        let hint: Element<'_, Message> = if wrong {
            text(tr("That password is not right. Try again."))
                .size(13)
                .style(|t: &Theme| text::Style { color: Some(t.extended_palette().danger.base.color) })
                .into()
        } else {
            text(tr("Enter the password to open it.")).size(13).style(muted).into()
        };
        center(
            column![
                svg(icons::lock()).width(56).height(56).style(style::icon_muted),
                text(format!("“{}” is password-protected", file_name(path))).size(18),
                hint,
                space().height(4),
                row![input, unlock].spacing(8).align_y(Alignment::Center),
            ]
            .spacing(10)
            .align_x(Alignment::Center),
        )
        .into()
    }

    fn drop_overlay(&self) -> Element<'_, Message> {
        container(
            center(
                column![
                    svg(icons::drop_files()).width(48).height(48).style(style::icon),
                    text(tr("Drop to open")).size(20),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .style(|t: &Theme| {
                let p = t.extended_palette();
                container::Style {
                    background: Some(p.background.base.color.scale_alpha(0.85).into()),
                    border: iced::Border { radius: 16.0.into(), width: 2.0, color: p.primary.base.color },
                    ..Default::default()
                }
            }),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

impl App {
    /// The zoom pill at the bottom and the previous/next arrows at the sides, while they are showing.
    fn floating_controls<'a>(&'a self) -> Option<Element<'a, Message>> {
        if !self.prefs.floating_controls
            || !matches!(self.doc, Doc::Image(_) | Doc::Pdf(_))
            || self.organizer.is_some()
            || self.crop.is_some()
        {
            return None;
        }
        let alpha = self.controls.interpolate(0.0_f32, 1.0, self.now);
        if alpha <= 0.01 {
            return None;
        }
        let round = |icon: svg::Handle, msg: Option<Message>, size: f32, label: String| -> Element<'a, Message> {
            let enabled = msg.is_some();
            let glyph = svg(icon).width(20).height(20).style(move |t: &Theme, _| svg::Style {
                color: Some(
                    t.extended_palette().background.base.text.scale_alpha(alpha * if enabled { 1.0 } else { 0.35 }),
                ),
            });
            let button = button(center(glyph))
                .width(size)
                .height(size)
                .padding(0)
                .style(move |t, status| style::floating(t, status, alpha, size / 2.0))
                .on_press_maybe(msg);
            tip(button.into(), label)
        };
        let label = self.zoom_percent().map(|z| format!("{z:.0}%")).unwrap_or_default();
        let percent = button(
            text(label)
                .size(15)
                .font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT })
                .width(70)
                .align_x(Alignment::Center)
                .style(move |t: &Theme| text::Style {
                    color: Some(t.extended_palette().background.base.text.scale_alpha(alpha)),
                }),
        )
        .padding([6, 4])
        .style(move |t, status| style::floating(t, status, 0.0, 8.0))
        .on_press(Message::ZoomActual);
        let pill = container(
            row![
                round(icons::zoom_out(), Some(Message::ZoomOut), 34.0, tr("Zoom out (−)")),
                tip(percent.into(), tr("Actual size")),
                round(icons::zoom_in(), Some(Message::ZoomIn), 34.0, tr("Zoom in (+)"))
            ]
            .spacing(2)
            .align_y(Alignment::Center),
        )
        .padding(5)
        .style(move |t: &Theme| style::floating_panel(t, alpha));
        let hold = |e: Element<'a, Message>| -> Element<'a, Message> {
            mouse_area(e).on_enter(Message::OverControls(true)).on_exit(Message::OverControls(false)).into()
        };
        let bottom = container(hold(pill.into()))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::End)
            .padding(Padding { bottom: 22.0, ..Padding::ZERO });
        let multi = self.files.len() > 1;
        let wrap = self.prefs.wrap_around;
        let prev = (multi && (wrap || self.current > 0)).then_some(Message::Prev);
        let next = (multi && (wrap || self.current + 1 < self.files.len())).then_some(Message::Next);
        let side = |msg: Option<Message>, icon: svg::Handle, label: String| -> Element<'a, Message> {
            if msg.is_none() {
                return space().width(44).into();
            }
            let glyph = svg(icon).width(22).height(22).style(move |t: &Theme, _| svg::Style {
                color: Some(t.extended_palette().background.base.text.scale_alpha(alpha)),
            });
            let disc = button(center(glyph))
                .width(44)
                .height(44)
                .padding(0)
                .style(move |t, status| style::floating_disc(t, status, alpha))
                .on_press_maybe(msg);
            hold(tip(disc.into(), label))
        };
        let arrows = container(row![
            side(prev, icons::prev(), tr("Previous file (←)")),
            space().width(Length::Fill),
            side(next, icons::next(), tr("Next file (→)"))
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(Alignment::Center)
        .padding([0, 18]);
        Some(stack![arrows, bottom].into())
    }
}

/// Places `inner` at `rect` (page points) on a page drawn at `zoom`; used as a layer of the page's stack.
fn placed<'a>(rect: &Rect, zoom: f32, inner: Element<'a, Message>) -> Element<'a, Message> {
    container(inner)
        .padding(Padding { top: rect.y0.max(0.0) as f32 * zoom, left: rect.x0.max(0.0) as f32 * zoom, ..Padding::ZERO })
        .into()
}

fn on_scroll(vp: scrollable::Viewport) -> Message {
    let offset = vp.absolute_offset();
    let bounds = vp.bounds();
    Message::Scrolled(Viewport { x: offset.x, y: offset.y, height: bounds.height })
}

fn muted(theme: &Theme) -> text::Style {
    text::Style { color: Some(theme.extended_palette().background.base.text.scale_alpha(0.6)) }
}
