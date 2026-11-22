//! The application: state, messages, update and view (Elm architecture, via iced).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
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

use crate::library::{self, Kind};
use crate::pdf;
use crate::print::{self, Outcome};
use crate::raster::{self, Picture};
use crate::structure::Target;
use crate::{icons, style};
use hayro::kurbo::Rect;

// ---------------------------------------------------------------- constants

const TOOLBAR_H: f32 = 52.0;
const SIDEBAR_W: f32 = 236.0;
/// Height of one sidebar row; rows are fixed-height so the list can be virtualised.
const ROW_H: f32 = 64.0;
const ROW_THUMB: f32 = 52.0;
/// Height of one table-of-contents row.
const TOC_ROW_H: f32 = 30.0;
/// How many thumbnails are decoded at once.
const THUMB_PARALLEL: usize = 4;
/// How far either side of the current file thumbnails are prepared.
const THUMB_RADIUS: usize = 40;
/// Decoded pictures kept around for instant back/forward.
const PICTURE_CACHE: usize = 5;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Contents,
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
    /// Aborts the running scan when replaced or dropped.
    _scan: Option<iced::task::Handle>,
}

// ---------------------------------------------------------------- state

pub struct App {
    mode: Mode,
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
    search: Option<Search>,
    search_seq: u64,
    /// When the file now loading was asked for, for the `opened` timing log.
    load_started: Option<Instant>,
    first_view_logged: bool,
    /// Size of the area pictures and pages are drawn in (logical px).
    area: Size,
    viewport: Viewport,
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
    thumbs_running: usize,

    pending_drops: Vec<PathBuf>,
    drop_hover: bool,
    toast: Option<(u64, String)>,
    toast_seq: u64,
    printing: bool,
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
    original: (u32, u32),
    picture: Picture,
}

impl From<Picture> for Loaded {
    fn from(p: Picture) -> Self {
        let handle = image::Handle::from_rgba(p.width, p.height, p.rgba.clone());
        Self { handle, width: p.width, height: p.height, original: p.original, picture: p }
    }
}

enum Doc {
    Empty,
    Loading(PathBuf),
    Failed { path: PathBuf, error: String },
    Image(ImageView),
    Pdf(PdfView),
}

struct ImageView {
    path: PathBuf,
    shown: Loaded,
    /// `None` fits the picture to the window (never enlarging it).
    zoom: Option<f32>,
    /// Quarter turns clockwise, view only.
    turns: u8,
    rotating: bool,
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
    WindowEvent(window::Id, window::Event),
    ScaleFactor(f32),
    Key(keyboard::Event),
    Wheel(ScrollDelta),
    AreaResized(Size),
    Scrolled(Viewport),

    Open { folder: bool },
    Chosen(Result<Vec<PathBuf>, String>),
    Expanded { files: Vec<PathBuf>, start: usize, append: bool },
    FlushDrops,
    Select(usize),
    Next,
    Prev,
    First,
    Last,

    PictureLoaded(u64, PathBuf, Result<Picture, String>),
    Preloaded(PathBuf, Result<Picture, String>),
    PdfOpened(u64, PathBuf, Result<pdf::Document, String>),
    PageRendered(u64, usize, f32, Result<Picture, String>),
    Thumb(PathBuf, Option<Picture>),
    SidebarScrolled(f32, f32),
    SidebarTab(SidebarTab),
    ContentsScrolled(f32, f32),
    GoTo(Target),
    LinkOpened(Result<(), String>),

    SearchOpen,
    SearchClose,
    SearchInput(String),
    SearchSubmit,
    SearchNext,
    SearchPrev,
    SearchHits(u64, usize, Vec<Vec<Rect>>),
    SearchDone(u64),
    Escape,

    ZoomIn,
    ZoomOut,
    ZoomFit,
    ZoomFitWidth,
    ZoomActual,
    Rotate(i8),
    Rotated(u64, u8, Option<Picture>),
    PageNext,
    PagePrev,

    PanStart,
    PanMove(Point),
    PanEnd,

    Print,
    Printed(Result<Outcome, String>),
    ToggleFullscreen,
    ToggleSidebar,
    DismissToast(u64),
    Quit,
}

fn err_string(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Runs CPU-bound or blocking work off the UI thread. A panic in there (say, a
/// decoder choking on a hostile file) becomes an error instead of a crash.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|_| "the file could not be decoded".to_string())
}

// ---------------------------------------------------------------- boot

impl App {
    pub fn new(paths: Vec<PathBuf>) -> (Self, Task<Message>) {
        let app = Self {
            mode: Mode::Light,
            theme: style::theme(Mode::Light),
            window: None,
            scale_factor: 1.0,
            fullscreen: false,
            show_sidebar: true,
            sidebar_scroll: (0.0, 0.0),
            sidebar_tab: None,
            contents_scroll: (0.0, 0.0),
            search: None,
            search_seq: 0,
            load_started: None,
            first_view_logged: false,
            area: Size::new(1100.0, 700.0),
            viewport: Viewport::default(),
            modifiers: Modifiers::default(),
            pan: None,
            files: Vec::new(),
            current: 0,
            generation: 0,
            doc: Doc::Empty,
            pictures: VecDeque::new(),
            preloading: HashSet::new(),
            thumbs: HashMap::new(),
            thumbs_running: 0,
            pending_drops: Vec::new(),
            drop_hover: false,
            toast: None,
            toast_seq: 0,
            printing: false,
        };
        let mut tasks = vec![system::theme().map(Message::SystemTheme)];
        if !paths.is_empty() {
            tracing::info!(ms = crate::since_start(), "open requested");
            tasks.push(expand(paths, false));
        }
        (app, Task::batch(tasks))
    }

    pub fn title(&self) -> String {
        match self.current_path() {
            Some(p) => format!("{} — Lumen", file_name(p)),
            None => "Lumen".into(),
        }
    }

    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            system::theme_changes().map(Message::SystemTheme),
            window::events().map(|(id, e)| Message::WindowEvent(id, e)),
            keyboard::listen().map(Message::Key),
            event::listen_with(|event, _status, _id| match event {
                Event::Mouse(mouse::Event::WheelScrolled { delta }) => Some(Message::Wheel(delta)),
                Event::Keyboard(keyboard::Event::KeyPressed { key: Key::Named(Named::Escape), .. }) => {
                    Some(Message::Escape)
                }
                _ => None,
            }),
        ])
    }

    fn current_path(&self) -> Option<&Path> {
        self.files.get(self.current).map(PathBuf::as_path)
    }

    fn toast(&mut self, msg: impl Into<String>) -> Task<Message> {
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.toast = Some((id, msg.into()));
        Task::perform(tokio::time::sleep(Duration::from_secs(6)), move |_| Message::DismissToast(id))
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
                self.mode = mode;
                self.theme = style::theme(mode);
                Task::none()
            }
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
                if y > 0.0 {
                    self.zoom_by(ZOOM_STEP.powf(y.min(3.0)))
                } else if y < 0.0 {
                    self.zoom_by(1.0 / ZOOM_STEP.powf((-y).min(3.0)))
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
                let follow = page_changed && self.sidebar_shows() == Some(SidebarTab::Contents);
                Task::batch([self.request_pages(), if follow { self.reveal_in_contents() } else { Task::none() }])
            }

            Message::Open { folder } => {
                Task::perform(async move { crate::portal::choose(folder).await.map_err(err_string) }, Message::Chosen)
            }
            Message::Chosen(Ok(paths)) if paths.is_empty() => Task::none(),
            Message::Chosen(Ok(paths)) => expand(paths, false),
            Message::Chosen(Err(e)) => self.toast(e),
            Message::Expanded { files, start, append } => self.set_files(files, start, append),
            Message::FlushDrops => {
                let paths = std::mem::take(&mut self.pending_drops);
                if paths.is_empty() { Task::none() } else { expand(paths, true) }
            }
            Message::Select(i) => self.show(i),
            Message::Next => self.show(self.current + 1),
            Message::Prev => match self.current.checked_sub(1) {
                Some(i) => self.show(i),
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
                        self.doc = Doc::Image(ImageView { path, shown: loaded, zoom: None, turns: 0, rotating: false });
                        self.preload_neighbours()
                    }
                    Err(error) => {
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
                    Ok(doc) => {
                        self.log_opened(&path, "pdf");
                        let slots = (0..doc.page_count()).map(|_| Slot::default()).collect();
                        let mut view =
                            PdfView { doc, zoom: 1.0, fit: PdfFit::Width, tops: Vec::new(), slots, current_page: 0 };
                        view.zoom = self.pdf_fit_zoom(&view, PdfFit::Width).min(1.5 * PT_TO_PX);
                        view.layout();
                        self.doc = Doc::Pdf(view);
                        self.viewport.y = 0.0;
                        self.viewport.x = 0.0;
                        self.contents_scroll = (0.0, 0.0);
                        Task::batch([
                            operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }),
                            operation::scroll_to(contents_id(), scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }),
                            self.request_pages(),
                            self.start_search(),
                        ])
                    }
                    Err(error) => {
                        self.doc = Doc::Failed { path, error };
                        Task::none()
                    }
                }
            }
            Message::PageRendered(generation, page, scale, result) => {
                if generation != self.generation {
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
            Message::Thumb(path, picture) => {
                self.thumbs_running = self.thumbs_running.saturating_sub(1);
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
                }
            }
            Message::ContentsScrolled(offset, height) => {
                self.contents_scroll = (offset, height);
                Task::none()
            }
            Message::GoTo(Target::Page { page, top }) => self.go_to_point(page, top.map(|t| (0.0, t))),
            Message::GoTo(Target::Uri(uri)) => Task::perform(
                async move { crate::portal::open_uri(uri).await.map_err(err_string) },
                Message::LinkOpened,
            ),
            Message::LinkOpened(Ok(())) => Task::none(),
            Message::LinkOpened(Err(e)) => self.toast(format!("Could not open the link: {e}")),

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
                if self.search.is_some() {
                    self.search = None;
                    Task::none()
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
                Doc::Image(_) => self.set_image_zoom(Some(1.0 / self.scale_factor)),
                Doc::Pdf(_) => self.set_pdf_zoom(PT_TO_PX, PdfFit::Manual),
                _ => Task::none(),
            },
            Message::Rotate(dir) => {
                let Doc::Image(view) = &mut self.doc else { return Task::none() };
                if view.rotating {
                    return Task::none();
                }
                view.rotating = true;
                let turns = ((view.turns as i8 + dir).rem_euclid(4)) as u8;
                let pic = view.shown.picture.clone();
                let generation = self.generation;
                let clockwise = dir > 0;
                Task::perform(blocking(move || rotate(pic, clockwise)), move |p| {
                    Message::Rotated(generation, turns, p.ok())
                })
            }
            Message::Rotated(generation, turns, picture) => {
                if generation == self.generation
                    && let Doc::Image(view) = &mut self.doc
                {
                    view.rotating = false;
                    if let Some(picture) = picture.filter(|p| p.width > 0) {
                        view.shown = picture.into();
                        view.turns = turns;
                    }
                }
                Task::none()
            }
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
                Task::perform(async move { print::print(path).await.map_err(err_string) }, Message::Printed)
            }
            Message::Printed(result) => {
                self.printing = false;
                match result {
                    Ok(Outcome::Sent) => self.toast("Sent to the printer"),
                    Ok(Outcome::Cancelled) => Task::none(),
                    Err(e) => self.toast(format!("Could not print: {e}")),
                }
            }
            Message::ToggleFullscreen => self.set_fullscreen(!self.fullscreen),
            Message::ToggleSidebar => {
                self.show_sidebar = !self.show_sidebar;
                if self.show_sidebar { self.reveal_in_sidebar() } else { Task::none() }
            }
            Message::DismissToast(id) => {
                if self.toast.as_ref().is_some_and(|(t, _)| *t == id) {
                    self.toast = None;
                }
                Task::none()
            }
            Message::Quit => iced::exit(),
        }
    }

    fn window_event(&mut self, id: window::Id, event: window::Event) -> Task<Message> {
        match event {
            window::Event::Opened { .. } => {
                tracing::info!(ms = crate::since_start(), "window built");
                self.window = Some(id);
                window::scale_factor(id).map(Message::ScaleFactor)
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
        let ctrl = modifiers.command();
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let msg = match key.as_ref() {
            Key::Character(c) => match (c.to_lowercase().as_str(), ctrl, modifiers.shift()) {
                ("o", true, false) => Message::Open { folder: false },
                ("o", true, true) => Message::Open { folder: true },
                ("p", true, _) => Message::Print,
                ("f", true, _) if is_pdf => Message::SearchOpen,
                ("g", true, false) if is_pdf => Message::SearchNext,
                ("g", true, true) if is_pdf => Message::SearchPrev,
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
                _ => return Task::none(),
            },
            Key::Named(named) => match named {
                Named::ArrowRight if !ctrl => Message::Next,
                Named::ArrowLeft if !ctrl => Message::Prev,
                Named::Space if !is_pdf => Message::Next,
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
            return if append { Task::none() } else { self.toast("Nothing to show: no pictures or PDFs there") };
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
        self.current = index;
        self.generation += 1;
        self.load_started = Some(Instant::now());
        if let Some(search) = &mut self.search {
            // Results belong to the old file; the scan restarts once the new one is open.
            *search = Search { query: std::mem::take(&mut search.query), ..Search::default() };
        }
        self.pan = None;
        let generation = self.generation;
        let path = self.files[index].clone();

        let load = match Kind::of(&path) {
            Some(Kind::Pdf) => {
                self.doc = Doc::Loading(path.clone());
                Task::perform(
                    {
                        let path = path.clone();
                        blocking(move || pdf::Document::open(&path).map_err(err_string))
                    },
                    move |r| Message::PdfOpened(generation, path.clone(), r.and_then(|r| r)),
                )
            }
            _ => {
                if let Some(loaded) = self.cached(&path) {
                    self.log_opened(&path, "cached");
                    self.doc = Doc::Image(ImageView { path, shown: loaded, zoom: None, turns: 0, rotating: false });
                    self.preload_neighbours()
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
        Task::batch([load, reset, self.request_thumbs(), self.reveal_in_sidebar()])
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
        while self.pictures.len() > PICTURE_CACHE {
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
            tasks.push(Task::perform(
                {
                    let path = path.clone();
                    blocking(move || raster::load(&path).map_err(err_string))
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
            tasks.push(Task::perform(
                {
                    let path = path.clone();
                    blocking(move || match Kind::of(&path) {
                        Some(Kind::Pdf) => pdf::thumbnail(&path, raster::THUMB_SIDE).ok(),
                        _ => raster::thumbnail(&path).ok(),
                    })
                },
                move |p| Message::Thumb(path.clone(), p.ok().flatten()),
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
        let files = self.files.len() > 1;
        let contents = self.has_contents();
        match self.sidebar_tab {
            Some(SidebarTab::Contents) | None if contents => Some(SidebarTab::Contents),
            _ if files => Some(SidebarTab::Files),
            _ if contents => Some(SidebarTab::Contents),
            _ => None,
        }
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

    fn zoom_by(&mut self, factor: f32) -> Task<Message> {
        match &self.doc {
            Doc::Image(v) => {
                let z = v.zoom.unwrap_or_else(|| self.image_fit_zoom(v)) * factor;
                self.set_image_zoom(Some(z))
            }
            Doc::Pdf(v) => self.set_pdf_zoom(v.zoom * factor, PdfFit::Manual),
            _ => Task::none(),
        }
    }

    fn zoom_fit(&mut self, width: bool) -> Task<Message> {
        match &self.doc {
            Doc::Image(_) => self.set_image_zoom(None),
            Doc::Pdf(v) => {
                let fit = if width { PdfFit::Width } else { PdfFit::Page };
                let z = self.pdf_fit_zoom(v, fit);
                self.set_pdf_zoom(z, fit)
            }
            _ => Task::none(),
        }
    }

    fn set_image_zoom(&mut self, zoom: Option<f32>) -> Task<Message> {
        let vp = self.viewport;
        let area = self.area;
        let Doc::Image(view) = &mut self.doc else { return Task::none() };
        let old = view.zoom;
        let zoom = zoom.map(|z| z.clamp(MIN_ZOOM, MAX_ZOOM));
        view.zoom = zoom;
        let (Some(new), w, h) = (zoom, view.shown.width as f32, view.shown.height as f32) else {
            return Task::none();
        };
        // Keep the point at the centre of the view where it is.
        let old_z = old.unwrap_or(new);
        let ratio = if old.is_some() { new / old_z } else { 1.0 };
        let (cx, cy) = if old.is_some() {
            ((vp.x + area.width / 2.0) * ratio, (vp.y + area.height / 2.0) * ratio)
        } else {
            (w * new / 2.0, h * new / 2.0)
        };
        operation::scroll_to(
            scroll_id(),
            scrollable::AbsoluteOffset { x: (cx - area.width / 2.0).max(0.0), y: (cy - area.height / 2.0).max(0.0) },
        )
    }

    fn set_pdf_zoom(&mut self, zoom: f32, fit: PdfFit) -> Task<Message> {
        let vp = self.viewport;
        let area = self.area;
        let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        // Anchor: the page and spot inside it at the middle of the view.
        let mid = vp.y + area.height / 2.0;
        let page = view.page_at(mid);
        let frac = (mid - view.tops[page]) / view.zoom;
        let old_cx = (vp.x + area.width / 2.0) / view.content_width().max(1.0);

        view.zoom = zoom;
        view.fit = fit;
        view.layout();
        let y = (view.tops[page] + frac * zoom - area.height / 2.0).max(0.0);
        let x = (old_cx * view.content_width() - area.width / 2.0).max(0.0);
        self.viewport.y = y;
        self.viewport.x = x;
        Task::batch([operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x, y }), self.request_pages()])
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
        ])
    }

    /// Scrolls so that `point` (page points from the page's top-left) is near the top-left of the view.
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
        Task::batch([operation::scroll_to(scroll_id(), scrollable::AbsoluteOffset { x, y }), self.request_pages()])
    }

    /// (Re)starts searching the open PDF for the current query, from the page in view onwards.
    fn start_search(&mut self) -> Task<Message> {
        self.search_seq += 1;
        let seq = self.search_seq;
        let Some(search) = &mut self.search else { return Task::none() };
        *search = Search { query: std::mem::take(&mut search.query), seq, ..Search::default() };
        let Doc::Pdf(view) = &self.doc else { return Task::none() };
        let query = search.query.trim().to_string();
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
                let found = doc.text(page).await.find(&query);
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
            tasks.push(Task::perform(async move { doc.render(i, wanted).await.map_err(err_string) }, move |r| {
                Message::PageRendered(generation, i, wanted, r)
            }));
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

fn rotate(p: Picture, clockwise: bool) -> Picture {
    let rgba = Vec::from(p.rgba);
    let Some(img) = ::image::RgbaImage::from_raw(p.width, p.height, rgba) else {
        return Picture { width: 0, height: 0, original: p.original, rgba: Default::default() };
    };
    let out = if clockwise { ::image::imageops::rotate90(&img) } else { ::image::imageops::rotate270(&img) };
    let (width, height) = out.dimensions();
    Picture { width, height, original: (p.original.1, p.original.0), rgba: out.into_raw().into() }
}

// ---------------------------------------------------------------- view

fn icon_button<'a>(
    handle: svg::Handle,
    label: &'a str,
    on_press: Option<Message>,
    active: bool,
) -> Element<'a, Message> {
    let enabled = on_press.is_some();
    let glyph = svg(handle).width(20).height(20).style(if enabled { style::icon } else { style::icon_muted });
    let b = button(center(glyph).width(36).height(36))
        .padding(0)
        .style(if active { style::tool_active } else { style::tool })
        .on_press_maybe(on_press);
    tooltip(b, container(text(label).size(12)).padding([4, 8]).style(style::pill), tooltip::Position::Bottom)
        .gap(6)
        .into()
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

        let mut body = row![];
        if !self.fullscreen
            && self.show_sidebar
            && let Some(tab) = self.sidebar_shows()
        {
            body = body.push(self.sidebar(tab));
        }
        let content: Element<'_, Message> = match &self.search {
            Some(search) if matches!(self.doc, Doc::Pdf(_)) => stack![
                content,
                container(self.search_bar(search)).width(Length::Fill).align_x(Alignment::End).padding([10, 18])
            ]
            .into(),
            _ => content.into(),
        };
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
        if let Some((_, msg)) = &self.toast {
            layers = layers.push(
                container(container(text(msg.as_str()).size(14)).padding([10, 16]).style(style::toast_error))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::End)
                    .padding(Padding { bottom: 24.0, ..Padding::ZERO }),
            );
        }
        layers.into()
    }

    fn toolbar(&self) -> Element<'_, Message> {
        let has_doc = matches!(self.doc, Doc::Image(_) | Doc::Pdf(_));
        let is_image = matches!(self.doc, Doc::Image(_));
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let multi = self.files.len() > 1;

        let left = row![
            icon_button(icons::open(), "Open files (Ctrl+O)", Some(Message::Open { folder: false }), false),
            icon_button(icons::folder(), "Open folder (Ctrl+Shift+O)", Some(Message::Open { folder: true }), false),
            separator(),
            icon_button(
                icons::prev(),
                "Previous file (←)",
                (multi && self.current > 0).then_some(Message::Prev),
                false
            ),
            icon_button(
                icons::next(),
                "Next file (→)",
                (multi && self.current + 1 < self.files.len()).then_some(Message::Next),
                false
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        let (title, subtitle) = match &self.doc {
            Doc::Empty => ("Lumen".to_string(), String::new()),
            Doc::Loading(p) => (file_name(p), "Loading…".into()),
            Doc::Failed { path, .. } => (file_name(path), "Could not open".into()),
            Doc::Image(v) => {
                let (w, h) = v.shown.original;
                (file_name(&v.path), format!("{w} × {h}"))
            }
            Doc::Pdf(v) => (file_name(v.doc.path()), format!("Page {} of {}", v.current_page + 1, v.doc.page_count())),
        };
        let position =
            if multi { format!("  ·  {} of {}", self.current + 1, self.files.len()) } else { String::new() };
        let titles = column![
            text(title)
                .size(14)
                .font(Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT })
                .wrapping(text::Wrapping::None),
            text(format!("{subtitle}{position}")).size(12).style(|t: &Theme| text::Style {
                color: Some(t.extended_palette().background.base.text.scale_alpha(0.6))
            }),
        ]
        .align_x(Alignment::Center)
        .spacing(1);

        let zoom_label = self.zoom_percent().map(|z| format!("{:.0} %", z)).unwrap_or_default();
        let zoom_pill: Element<'_, Message> = button(text(zoom_label).size(12).width(52).align_x(Alignment::Center))
            .padding([6, 4])
            .style(style::tool)
            .on_press_maybe(has_doc.then_some(Message::ZoomActual))
            .into();

        let mut right = row![].spacing(4).align_y(Alignment::Center);
        if is_pdf {
            right = right
                .push(icon_button(icons::search(), "Find (Ctrl+F)", Some(Message::SearchOpen), self.search.is_some()))
                .push(icon_button(icons::up(), "Previous page (Page Up)", Some(Message::PagePrev), false))
                .push(icon_button(icons::down(), "Next page (Page Down)", Some(Message::PageNext), false))
                .push(separator());
        }
        let fit_active = match &self.doc {
            Doc::Image(v) => v.zoom.is_none(),
            Doc::Pdf(v) => v.fit == PdfFit::Page,
            _ => false,
        };
        right = right
            .push(icon_button(icons::zoom_out(), "Zoom out (−)", has_doc.then_some(Message::ZoomOut), false))
            .push(
                tooltip(
                    zoom_pill,
                    container(text("Actual size (1)").size(12)).padding([4, 8]).style(style::pill),
                    tooltip::Position::Bottom,
                )
                .gap(6),
            )
            .push(icon_button(icons::zoom_in(), "Zoom in (+)", has_doc.then_some(Message::ZoomIn), false));
        if is_pdf {
            let width_active = matches!(&self.doc, Doc::Pdf(v) if v.fit == PdfFit::Width);
            right =
                right.push(icon_button(icons::fit_width(), "Fit width (W)", Some(Message::ZoomFitWidth), width_active));
        }
        right = right.push(icon_button(icons::fit(), "Best fit (0)", has_doc.then_some(Message::ZoomFit), fit_active));
        if is_image {
            right = right
                .push(icon_button(icons::rotate_left(), "Rotate left (Shift+R)", Some(Message::Rotate(-1)), false))
                .push(icon_button(icons::rotate_right(), "Rotate right (R)", Some(Message::Rotate(1)), false));
        }
        right = right.push(separator()).push(icon_button(
            icons::print(),
            "Print (Ctrl+P)",
            (has_doc && !self.printing).then_some(Message::Print),
            self.printing,
        ));
        if self.sidebar_shows().is_some() {
            right =
                right.push(icon_button(icons::strip(), "Sidebar (T)", Some(Message::ToggleSidebar), self.show_sidebar));
        }
        right = right.push(icon_button(icons::fullscreen(), "Fullscreen (F)", Some(Message::ToggleFullscreen), false));

        container(
            row![left, container(titles).width(Length::Fill).align_x(Alignment::Center).clip(true), right,]
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
            Doc::Loading(_) => center(text("Loading…").size(15).style(muted)).into(),
            Doc::Failed { path, error } => center(
                column![
                    svg(icons::broken()).width(56).height(56).style(style::icon_muted),
                    text(format!("Could not open “{}”", file_name(path))).size(18),
                    text(error.as_str()).size(13).style(muted),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .into(),
            Doc::Image(view) => self.image_view(view),
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
        let picture = image(view.shown.handle.clone()).filter_method(image::FilterMethod::Linear);
        let Some(zoom) = view.zoom else {
            return container(picture.content_fit(ContentFit::ScaleDown).width(Length::Fill).height(Length::Fill))
                .padding(12)
                .center(Length::Fill)
                .into();
        };
        let (w, h) = (view.shown.width as f32 * zoom, view.shown.height as f32 * zoom);
        // Nearest-neighbour once pixels are big, so pixel art stays crisp.
        let filter =
            if zoom * self.scale_factor >= 3.0 { image::FilterMethod::Nearest } else { image::FilterMethod::Linear };
        let canvas = container(picture.filter_method(filter).width(w).height(h))
            .width(w.max(self.area.width))
            .height(h.max(self.area.height))
            .center_x(w.max(self.area.width))
            .center_y(h.max(self.area.height));
        let scroller = scrollable(canvas)
            .id(scroll_id())
            .direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::new(),
                horizontal: scrollable::Scrollbar::new(),
            })
            .on_scroll(on_scroll)
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
                        container(text(format!("Go to page {}", page + 1)).size(12)).padding([4, 8]).style(style::pill),
                        tooltip::Position::FollowCursor,
                    )
                    .into(),
                };
                layers = layers.push(placed(&link.rect, view.zoom, area));
            }
            pages = pages.push(container(layers).width(w).height(h).style(style::page));
            if i < hi {
                pages = pages.push(space().height(PAGE_GAP));
            }
        }
        let rest = view.tops[n] - (view.tops[hi] + view.doc.page_size(hi).1 * view.zoom);
        pages = pages.push(space().height(rest.max(0.0)));

        let wide = view.content_width() > self.area.width + 1.0;
        let direction = if wide {
            scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::new(),
                horizontal: scrollable::Scrollbar::new(),
            }
        } else {
            scrollable::Direction::Vertical(scrollable::Scrollbar::new())
        };
        let scroller = scrollable(pages)
            .id(scroll_id())
            .direction(direction)
            .on_scroll(on_scroll)
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
        };
        let mut col = column![];
        if self.files.len() > 1 && self.has_contents() {
            let segment = |label: &'static str, this: SidebarTab| {
                button(text(label).size(13).width(Length::Fill).align_x(Alignment::Center))
                    .padding([5, 8])
                    .width(Length::Fill)
                    .style(style::segment(tab == this))
                    .on_press(Message::SidebarTab(this))
            };
            col = col.push(
                container(
                    row![segment("Files", SidebarTab::Files), segment("Contents", SidebarTab::Contents)].spacing(4),
                )
                .padding([8, 8]),
            );
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
                    ..Font::DEFAULT
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
            .on_scroll(|vp| Message::ContentsScrolled(vp.absolute_offset().y, vp.bounds().height))
            .height(Length::Fill)
            .into()
    }

    fn search_bar<'a>(&'a self, search: &'a Search) -> Element<'a, Message> {
        let status = if search.query.trim().is_empty() {
            String::new()
        } else if search.hits.is_empty() {
            if search.done { "No matches".into() } else { "Searching…".into() }
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
                text_input("Find in document", &search.query)
                    .id(search_input_id())
                    .on_input(Message::SearchInput)
                    .on_submit(Message::SearchSubmit)
                    .size(14)
                    .padding([6, 8])
                    .width(210),
                text(status).size(12).style(muted).width(78).align_x(Alignment::Center),
                icon_button(
                    icons::up(),
                    "Previous match (Shift+Enter)",
                    has_hits.then_some(Message::SearchPrev),
                    false
                ),
                icon_button(icons::down(), "Next match (Enter)", has_hits.then_some(Message::SearchNext), false),
                icon_button(icons::close(), "Close (Esc)", Some(Message::SearchClose), false),
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
                _ => container(space()).width(Length::Fill).height(Length::Fill).style(style::thumb_placeholder).into(),
            };
            let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            let (stem, ext) = match name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), ext.to_ascii_uppercase()),
                _ => (name.to_string(), String::new()),
            };
            let label = column![
                text(stem).size(13).wrapping(text::Wrapping::None),
                text(ext).size(11).style(muted).wrapping(text::Wrapping::None),
            ]
            .spacing(2);
            let content = row![
                container(thumb).width(ROW_THUMB).height(ROW_THUMB),
                container(label).width(Length::Fill).clip(true),
            ]
            .spacing(10)
            .align_y(Alignment::Center);
            list = list.push(
                container(
                    button(content)
                        .padding([4, 6])
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(style::thumb(i == self.current))
                        .on_press(Message::Select(i)),
                )
                .padding([2, 6])
                .height(ROW_H),
            );
        }
        list = list.push(space().height((self.files.len() - last) as f32 * ROW_H));
        scrollable(list.width(Length::Fill))
            .id(sidebar_id())
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4)))
            .on_scroll(|vp| Message::SidebarScrolled(vp.absolute_offset().y, vp.bounds().height))
            .height(Length::Fill)
            .into()
    }

    fn empty_state(&self) -> Element<'_, Message> {
        let open = button(
            row![svg(icons::open()).width(18).height(18).style(style::icon_on_primary), text("Open Files…").size(15)]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .padding([10, 18])
        .style(style::primary)
        .on_press(Message::Open { folder: false });
        let folder = button(
            row![svg(icons::folder()).width(18).height(18).style(style::icon), text("Open Folder…").size(15)]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .padding([10, 18])
        .style(style::secondary)
        .on_press(Message::Open { folder: true });

        center(
            column![
                svg(icons::app()).width(112).height(112),
                text("Lumen").size(30).font(Font { weight: iced::font::Weight::Bold, ..Font::DEFAULT }),
                text("A fast, safe viewer for pictures and PDFs").size(15).style(muted),
                space().height(8),
                row![open, folder].spacing(12),
                space().height(4),
                text("or drop files and folders anywhere").size(13).style(muted),
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
                    text("Drop to open").size(20),
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
