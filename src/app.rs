//! Root Relm4 component: the main window.

mod chrome;
mod control;
mod edit;
mod files;
mod forms;
mod helpers;
mod markup;
mod menus;
mod messages;
mod pages;
mod prefs;
mod reading;
mod shortcuts;
mod signature;
mod update;

use crate::i18n::{tr, tr_fmt};
use shortcuts::accels;
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
use helpers::{
    ask_password, color_circle, entry_has_focus, is_plain_key, label_icon_buttons, open_logged,
    refresh_recent_menu, refresh_recent_row, restore_from_trash, screenshot, typing,
};
use markup::MarkupState;
pub use messages::AppMsg;
use pages::{PageRequest, WorkDir};

const RECENT_LIMIT: usize = 10;
/// How many neighbours on each side of the current file stay decoded.
const PRELOAD_RADIUS: usize = 1;
/// Wait this long after the zoom stops changing before re-rendering pages.
const RERENDER_DELAY_MS: u64 = 150;
/// Wait this long after the last keystroke before searching.
const SEARCH_DELAY_MS: u64 = 150;
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
    /// The recent files shown in the empty window.
    recent_row: gtk::Box,
    /// What the document menu in the header shows: the picture edits or the page changes.
    edit_menu: gio::Menu,
    pages_menu: gio::Menu,
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
    /// Case-sensitive and whole-word matching, from the toggles in the search bar.
    search_options: pdf::SearchOptions,
    /// Identifies the latest change of the search text, for the typing pause.
    search_ticket: u64,
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
    /// The single picture just opened, whose folder is to be listed for browsing.
    browse_after: Option<PathBuf>,
    /// Watches the open file, so a changed file is shown again (see `watch_file`).
    watcher: Option<gio::FileMonitor>,
    /// Identifies the latest change seen in the open file, for the pause before reloading.
    watch_ticket: u64,
    /// When Lumen last wrote a file itself, so that is not mistaken for someone else's change.
    last_save: Option<std::time::Instant>,
    /// Where the reader was before following links and outline entries (and where to go
    /// forward to after going back), as scroll positions in layout points.
    jump_back: Vec<f64>,
    jump_forward: Vec<f64>,
    /// Where short messages with an action (Undo…) are shown; set once the window exists.
    toasts: Option<adw::ToastOverlay>,
    /// A write of the remembered state is already scheduled.
    state_save_pending: bool,
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

            #[name = "toasts"]
            #[wrap(Some)]
            set_content = &adw::ToastOverlay {
              #[wrap(Some)]
              set_child = &adw::ToolbarView {
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

                    // One menu for what can be done to the document: page changes for a PDF,
                    // picture edits for a picture.
                    pack_end = &gtk::MenuButton {
                        #[watch]
                        set_icon_name: if model.pdf.is_some() {
                            "view-paged-symbolic"
                        } else {
                            "lumen-image-edit-symbolic"
                        },
                        #[watch]
                        set_tooltip_text: Some(&if model.pdf.is_some() {
                            tr("Pages")
                        } else {
                            tr("Edit")
                        }),
                        #[watch]
                        set_menu_model: Some(if model.pdf.is_some() {
                            &model.pages_menu
                        } else {
                            &model.edit_menu
                        }),
                        #[watch]
                        set_visible: model.pdf.is_some() || (model.image.is_some() && model.edit.is_some()),
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

                        gtk::ToggleButton {
                            set_label: "Aa",
                            set_tooltip_text: Some(&tr("Match Case")),
                            connect_toggled[sender, model_options] => move |button| {
                                let mut options = model_options.get();
                                options.case_sensitive = button.is_active();
                                model_options.set(options);
                                sender.input(AppMsg::SearchOptions(options));
                            },
                        },
                        gtk::ToggleButton {
                            set_label: "ab",
                            set_tooltip_text: Some(&tr("Whole Words")),
                            connect_toggled[sender, model_options] => move |button| {
                                let mut options = model_options.get();
                                options.whole_words = button.is_active();
                                model_options.set(options);
                                sender.input(AppMsg::SearchOptions(options));
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
                            set_tooltip_text: Some(&format!("{} ({})", tr("Select"), "V")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Select,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Select)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-rectangle-symbolic",
                            set_tooltip_text: Some(&format!("{} ({})", tr("Rectangle"), "R")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Rectangle,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Rectangle)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-ellipse-symbolic",
                            set_tooltip_text: Some(&format!("{} ({})", tr("Ellipse"), "O")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Ellipse,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Ellipse)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-arrow-symbolic",
                            set_tooltip_text: Some(&format!("{} ({})", tr("Arrow"), "A")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Arrow,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Arrow)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-line-symbolic",
                            set_tooltip_text: Some(&format!("{} ({})", tr("Line"), "L")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Line,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Line)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-pen-symbolic",
                            set_tooltip_text: Some(&format!("{} ({})", tr("Pen"), "P")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Pen,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Pen)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-text-symbolic",
                            set_tooltip_text: Some(&format!("{} ({})", tr("Text"), "T")),
                            #[watch]
                            set_active: model.markup_tool() == Tool::Text,
                            connect_clicked[sender] => move |_| sender.input(AppMsg::SetTool(Tool::Text)),
                        },
                        gtk::ToggleButton {
                            set_icon_name: "lumen-tool-highlight-symbolic",
                            set_tooltip_text: Some(&format!("{} ({})", tr("Highlight"), "H")),
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
                                set_child = &gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 24,
                                    #[watch]
                                    set_visible: model.document.is_none() && !model.loading,

                                    gtk::Button {
                                        set_label: &tr("Open…"),
                                        set_halign: gtk::Align::Center,
                                        set_action_name: Some("win.open"),
                                        add_css_class: "pill",
                                        add_css_class: "suggested-action",
                                    },

                                    #[local_ref]
                                    recent_row -> gtk::Box {
                                        set_halign: gtk::Align::Center,
                                        set_spacing: 6,
                                    },
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
                            set_visible: model.loading && !model.canvas.has_placeholder(),
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
            },
        }
    }

    fn post_view() {
        // The page number follows the view, but never overwrites what is being typed.
        let page_text = model.page_text();
        if widgets.page_entry.text() != page_text && !entry_has_focus(&widgets.page_entry) {
            widgets.page_entry.set_text(&page_text);
        }
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
            ("trash", model.has_document() && !model.is_dirty()),
            ("show-in-folder", model.has_document()),
            ("open-with", model.has_document()),
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

        let menus = menus::build();
        let (edit_menu, pages_menu, main_menu) = (menus.edit, menus.pages, menus.main);

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
        let mut model = App {
            recent_menu: recent_menu.clone(),
            recent_row: gtk::Box::new(gtk::Orientation::Horizontal, 6),
            edit_menu: edit_menu.clone(),
            pages_menu: pages_menu.clone(),
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
            search_options: pdf::SearchOptions::default(),
            search_ticket: 0,
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
            browse_after: None,
            watcher: None,
            watch_ticket: 0,
            last_save: None,
            jump_back: Vec::new(),
            jump_forward: Vec::new(),
            toasts: None,
            state_save_pending: false,
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
        let model_options = Rc::new(Cell::new(pdf::SearchOptions::default()));
        let recent_row = model.recent_row.clone();
        refresh_recent_row(&recent_row, &sender);
        let sidebar_stack = model.sidebar_stack.clone();
        sidebar_stack.add_named(model.strip.root(), Some("files"));
        sidebar_stack.add_named(model.page_strip.root(), Some("pages"));
        sidebar_stack.add_named(model.outline.root(), Some("outline"));
        let properties_root = model.properties.root().clone();
        let widgets = view_output!();
        model.toasts = Some(widgets.toasts.clone());
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
            ("<Shift>Left", || AppMsg::MarkupNudge(-10.0, 0.0)),
            ("<Shift>Right", || AppMsg::MarkupNudge(10.0, 0.0)),
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
        let goto_sender = sender.clone();
        goto_action.connect_activate(move |_, _| {
            // The page controls may be faded out: bring them back before focusing the entry.
            goto_sender.input(AppMsg::Hover(true));
            let entry = entry.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
                entry.grab_focus();
                entry.select_region(0, -1);
            });
        });
        // The controls stay while the entry is in use.
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter({
            let sender = sender.clone();
            move |_| sender.input(AppMsg::ControlsHover(true))
        });
        focus.connect_leave({
            let sender = sender.clone();
            move |_| sender.input(AppMsg::ControlsHover(false))
        });
        widgets.page_entry.add_controller(focus);
        widgets.main_window.add_action(&goto_action);
        relm4::main_application().set_accels_for_action("win.goto-page", accels("goto-page"));

        let nav_actions: &[NavAction] = &[
            ("page-next", || AppMsg::PageBy(1), accels("page-next")),
            ("page-prev", || AppMsg::PageBy(-1), accels("page-prev")),
            ("page-first", || AppMsg::Edge(false), accels("page-first")),
            ("page-last", || AppMsg::Edge(true), accels("page-last")),
            (
                "tool-select",
                || AppMsg::ToolKey(Tool::Select),
                accels("tool-select"),
            ),
            (
                "tool-rectangle",
                || AppMsg::ToolKey(Tool::Rectangle),
                accels("tool-rectangle"),
            ),
            (
                "tool-ellipse",
                || AppMsg::ToolKey(Tool::Ellipse),
                accels("tool-ellipse"),
            ),
            (
                "tool-arrow",
                || AppMsg::ToolKey(Tool::Arrow),
                accels("tool-arrow"),
            ),
            (
                "tool-line",
                || AppMsg::ToolKey(Tool::Line),
                accels("tool-line"),
            ),
            (
                "tool-pen",
                || AppMsg::ToolKey(Tool::Pen),
                accels("tool-pen"),
            ),
            (
                "tool-text",
                || AppMsg::ToolKey(Tool::Text),
                accels("tool-text"),
            ),
            (
                "tool-highlight",
                || AppMsg::ToolKey(Tool::Highlight),
                accels("tool-highlight"),
            ),
            ("trash", || AppMsg::Trash, accels("trash")),
            (
                "show-in-folder",
                || AppMsg::ShowInFolder,
                accels("show-in-folder"),
            ),
            ("open-with", || AppMsg::OpenWith, accels("open-with")),
            ("palette", || AppMsg::Palette, accels("palette")),
            ("jump-back", || AppMsg::JumpBack, accels("jump-back")),
            (
                "jump-forward",
                || AppMsg::JumpForward,
                accels("jump-forward"),
            ),
            ("screen-down", || AppMsg::Screen(1), accels("screen-down")),
            ("screen-up", || AppMsg::Screen(-1), accels("screen-up")),
            ("next", || AppMsg::Navigate(1), accels("next")),
            ("prev", || AppMsg::Navigate(-1), accels("prev")),
            (
                "toggle-sidebar",
                || AppMsg::ToggleSidebar,
                accels("toggle-sidebar"),
            ),
            (
                "toggle-properties",
                || AppMsg::ToggleProperties,
                accels("toggle-properties"),
            ),
            ("find-next", || AppMsg::SearchStep(1), accels("find-next")),
            ("find-prev", || AppMsg::SearchStep(-1), accels("find-prev")),
            (
                "find-close",
                || AppMsg::SearchOpened(false),
                accels("find-close"),
            ),
            ("copy", || AppMsg::Copy, accels("copy")),
            ("print", || AppMsg::Print(None), accels("print")),
            ("undo", || AppMsg::Undo, accels("undo")),
            ("redo", || AppMsg::Redo, accels("redo")),
            (
                "rotate-right",
                || AppMsg::Edit(EditOp::RotateCw),
                accels("rotate-right"),
            ),
            (
                "rotate-left",
                || AppMsg::Edit(EditOp::RotateCcw),
                accels("rotate-left"),
            ),
            ("flip-h", || AppMsg::Edit(EditOp::FlipH), accels("flip-h")),
            ("flip-v", || AppMsg::Edit(EditOp::FlipV), accels("flip-v")),
            ("crop", || AppMsg::EnterMode(EditMode::Crop), accels("crop")),
            ("resize", || AppMsg::ResizeDialog, accels("resize")),
            (
                "adjust",
                || AppMsg::EnterMode(EditMode::Adjust),
                accels("adjust"),
            ),
            ("save", || AppMsg::Save, accels("save")),
            ("save-as", || AppMsg::SaveAs, accels("save-as")),
            ("apply-tool", || AppMsg::Confirm, accels("apply-tool")),
            ("markup", || AppMsg::ToggleMarkup, accels("markup")),
            ("preferences", || AppMsg::Preferences, accels("preferences")),
            (
                "fullscreen",
                || AppMsg::ToggleFullscreen,
                accels("fullscreen"),
            ),
            ("slideshow", || AppMsg::ToggleSlideshow, accels("slideshow")),
            (
                "leave-presentation",
                || AppMsg::LeavePresentation,
                accels("leave-presentation"),
            ),
            ("about", || AppMsg::About, accels("about")),
            ("shortcuts", || AppMsg::Shortcuts, accels("shortcuts")),
            (
                "delete-pages",
                || AppMsg::DeletePages,
                accels("delete-pages"),
            ),
            (
                "insert-pages",
                || AppMsg::InsertDialog,
                accels("insert-pages"),
            ),
            (
                "export-pages",
                || AppMsg::ExportPagesDialog,
                accels("export-pages"),
            ),
            (
                "export-png",
                || AppMsg::ExportPngDialog,
                accels("export-png"),
            ),
            (
                "export-image-pdf",
                || AppMsg::ExportImagePdfDialog,
                accels("export-image-pdf"),
            ),
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

        // How PDFs are fitted, and night mode: both are kept in the preferences.
        let fit_action = gio::SimpleAction::new_stateful(
            "pdf-fit",
            Some(glib::VariantTy::STRING),
            &(if store.borrow().settings.pdf_fit == settings::PdfFit::Page {
                "page"
            } else {
                "width"
            })
            .to_variant(),
        );
        fit_action.connect_activate({
            let sender = sender.clone();
            move |action, choice| {
                if let Some(choice) = choice.and_then(|c| c.str()) {
                    action.set_state(&choice.to_variant());
                    sender.input(AppMsg::SetPdfFit(if choice == "page" {
                        settings::PdfFit::Page
                    } else {
                        settings::PdfFit::Width
                    }));
                }
            }
        });
        widgets.main_window.add_action(&fit_action);
        let night_action = gio::SimpleAction::new_stateful(
            "night-mode",
            None,
            &store.borrow().settings.night_mode.to_variant(),
        );
        night_action.connect_activate({
            let sender = sender.clone();
            move |action, _| {
                let on = !action
                    .state()
                    .and_then(|s| s.get::<bool>())
                    .unwrap_or(false);
                action.set_state(&on.to_variant());
                sender.input(AppMsg::SetNightMode(on));
            }
        });
        widgets.main_window.add_action(&night_action);
        app.set_accels_for_action("win.night-mode", accels("night-mode"));

        app.set_accels_for_action("win.find", accels("find"));
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
            ("zoom-in", LumenCanvas::zoom_in, accels("zoom-in")),
            ("zoom-out", LumenCanvas::zoom_out, accels("zoom-out")),
            ("best-fit", LumenCanvas::best_fit, accels("best-fit")),
            (
                "actual-size",
                LumenCanvas::actual_size,
                accels("actual-size"),
            ),
        ];
        for (name, handler, accels) in view_actions {
            let action = gio::SimpleAction::new(name, None);
            let canvas = canvas.clone();
            action.connect_activate(move |_, _| handler(&canvas));
            widgets.main_window.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }
        app.set_accels_for_action("win.open", accels("open"));
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
