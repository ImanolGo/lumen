//! Root Relm4 component: the main window.

use std::{cell::RefCell, collections::HashSet, path::PathBuf, rc::Rc};

use relm4::{
    ComponentParts, ComponentSender, RelmWidgetExt, SimpleComponent,
    adw::{self, prelude::*},
    gtk::{self, gdk, gio, glib},
};

use crate::{
    APP_NAME,
    document::{self, DocumentInfo, DocumentKind, Opened},
    folder,
    pdf::{self, PageCache, PageLayout, PageRenderer, RenderJob, RenderedPage},
    thumbnails::{ThumbData, ThumbnailLoader},
    widgets::{CanvasContent, FileStrip, LumenCanvas, PropertiesPanel, ViewInfo},
};

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

/// The PDF being shown as a continuous column of pages.
struct PdfView {
    n_pages: usize,
    layout: Rc<PageLayout>,
    /// Page filling most of the viewport.
    page: usize,
    /// Identifies this document, so late renders of an older one are dropped.
    doc_key: u64,
    visible: (usize, usize),
    cache: PageCache,
    renderer: PageRenderer,
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
        result: Result<Opened, String>,
    },
    /// A neighbour finished decoding in the background.
    Preloaded {
        path: PathBuf,
        result: Result<Opened, String>,
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
        self.sidebar_open && self.list.len() > 1
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
                let result = open_logged(&path);
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
        self.pdf = opened.pdf.map(|pdf_info| {
            self.next_key += 1;
            let doc_key = self.next_key;
            let layout = Rc::new(PageLayout::new(&pdf_info.page_sizes));
            let input = sender.input_sender().clone();
            PdfView {
                n_pages: pdf_info.n_pages,
                layout,
                page: 0,
                doc_key,
                visible: (0, 0),
                cache: PageCache::new(PAGE_CACHE_BYTES),
                renderer: PageRenderer::new(&info.path, move |job, result| {
                    input.emit(AppMsg::PageRendered { doc_key, job, result });
                }),
            }
        });
        self.document = Some(info);
        self.sync_canvas();
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
                let result = open_logged(&path);
                sender.input(AppMsg::Preloaded { path, result });
            });
        }
    }

    fn fail(&mut self, err: String) {
        self.loading = false;
        self.document = None;
        self.image = None;
        self.pdf = None;
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

                #[wrap(Some)]
                set_content = &gtk::Box {
                    append = &gtk::Box {
                        #[watch]
                        set_visible: model.sidebar_visible(),

                        #[local_ref]
                        strip_root -> gtk::ScrolledWindow {},

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
        let strip_root = model.strip.root().clone();
        let properties_root = model.properties.root().clone();
        let widgets = view_output!();

        // Window actions + keyboard shortcuts.
        let open_action = gio::SimpleAction::new("open", None);
        let open_sender = sender.clone();
        open_action.connect_activate(move |_, _| open_sender.input(AppMsg::OpenDialog));
        widgets.main_window.add_action(&open_action);

        let canvas = model_canvas;
        widgets.scroller.set_child(Some(&canvas));
        canvas.connect_view_changed({
            let sender = sender.clone();
            move |info| sender.input(AppMsg::ViewChanged(info))
        });

        let goto_action = gio::SimpleAction::new("goto-page", None);
        let entry = widgets.page_entry.clone();
        goto_action.connect_activate(move |_, _| {
            entry.grab_focus();
            entry.select_region(0, -1);
        });
        widgets.main_window.add_action(&goto_action);
        relm4::main_application().set_accels_for_action("win.goto-page", &["<Control>g"]);

        let nav_actions: [(&str, AppMsg, &[&str]); 8] = [
            ("page-next", AppMsg::PageBy(1), &["Page_Down"]),
            ("page-prev", AppMsg::PageBy(-1), &["Page_Up"]),
            ("page-first", AppMsg::GotoPage(0), &["Home"]),
            ("page-last", AppMsg::GotoPage(usize::MAX), &["End"]),
            ("next", AppMsg::Navigate(1), &["Right", "space"]),
            ("prev", AppMsg::Navigate(-1), &["Left", "BackSpace"]),
            ("toggle-sidebar", AppMsg::ToggleSidebar, &["F9"]),
            ("toggle-properties", AppMsg::ToggleProperties, &["<Alt>Return"]),
        ];
        let app = relm4::main_application();
        for (name, msg, accels) in nav_actions {
            let action = gio::SimpleAction::new(name, None);
            let sender = sender.clone();
            action.connect_activate(move |_, _| {
                sender.input(match msg {
                    AppMsg::Navigate(delta) => AppMsg::Navigate(delta),
                    AppMsg::PageBy(delta) => AppMsg::PageBy(delta),
                    AppMsg::GotoPage(page) => AppMsg::GotoPage(page),
                    AppMsg::ToggleProperties => AppMsg::ToggleProperties,
                    _ => AppMsg::ToggleSidebar,
                })
            });
            widgets.main_window.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }

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
                    Err(err) => self.fail(err),
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
                        self.fail(err);
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
                    pdf.visible = (info.first_visible, info.last_visible);
                    let delay = if zoom_changed { RERENDER_DELAY_MS } else { SCROLL_RENDER_DELAY_MS };
                    self.schedule_render(delay, &sender);
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
            AppMsg::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            AppMsg::ToggleProperties => self.properties_open = !self.properties_open,
            AppMsg::Hover(hover) => self.hover = hover,
        }
    }
}

fn open_logged(path: &std::path::Path) -> Result<Opened, String> {
    document::open(path).map_err(|err| {
        tracing::warn!(path = %path.display(), "open failed: {err:#}");
        format!("{err:#}")
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
