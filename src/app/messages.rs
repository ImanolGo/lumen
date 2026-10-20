//! Everything that can happen to the main window, as messages for `App::update`.

use super::*;

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
    /// Move the file on screen to the trash (Ctrl+Delete).
    Trash,
    /// Show the file on screen in the file manager.
    ShowInFolder,
    /// Open the file on screen with another program (the system's chooser).
    OpenWith,
    /// A file was moved to the trash from list position `index`.
    Trashed {
        path: PathBuf,
        index: usize,
        result: Result<(), String>,
    },
    /// Bring a trashed file back (the toast's Undo).
    Untrash(PathBuf, usize),
    Untrashed {
        path: PathBuf,
        index: usize,
        result: Result<(), String>,
    },
    /// The open file changed on disk (change `ticket`, after a pause).
    FileChanged {
        ticket: u64,
    },
    /// Something happened to the open file; wait a moment for it to settle.
    FileTouched,
    SetPdfFit(settings::PdfFit),
    SetNightMode(bool),
    /// Ctrl+Shift+P: the command palette.
    Palette,
    /// Alt+Left or the toast's button: back to where a link or the outline was followed from.
    JumpBack,
    /// Alt+Right: forward again.
    JumpForward,
    /// The remembered state is due to be written (see `save_state_soon`).
    SaveState,
    /// A tool's letter key: choose the tool while the markup toolbar is open.
    ToolKey(Tool),
    /// Space (1) or Shift+Space (-1): scroll a PDF a screen, or move through the files.
    Screen(isize),
    /// Home (false) or End (true): the first or last page of a PDF, or file.
    Edge(bool),
    /// The pictures next to a picture that was opened alone were listed.
    Siblings {
        listing: u64,
        picture: PathBuf,
        siblings: Vec<PathBuf>,
    },
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
    /// A matching option was switched in the search bar.
    SearchOptions(pdf::SearchOptions),
    /// The search text has stayed the same since change `ticket`: search for it.
    SearchTyped {
        ticket: u64,
    },
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
    /// The pointer entered (true) or left the view.
    Hover(bool),
    /// The pointer entered (true) or left one of the floating controls.
    ControlsHover(bool),
    /// Nothing happened for a while since wake-up `ticket`.
    ControlsIdle(u64),
}
