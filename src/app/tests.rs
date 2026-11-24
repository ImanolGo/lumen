//! The app's message flows, driven without a window: `App::new`, then `App::update` with the
//! messages a person's clicks and keys would send. Work that needs the iced runtime (tasks)
//! is built but not run; what the app decides is read back from its state.

use std::time::{Duration, Instant};

use super::prefs::PrefMsg;
use super::*;
use crate::annotate::editor::Tool;
use crate::control::Command;
use crate::settings::Theme as ThemePref;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Toasts and timers need a Tokio context to be built in; nothing is ever run on it.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap())
}

fn app() -> App {
    let _inside = runtime().enter();
    let (app, _startup) = App::new(Vec::new());
    app
}

/// Lets an easing zoom finish, as the frames after it would.
fn settle(app: &mut App) {
    let _ = app.update(Message::Frame(Instant::now() + Duration::from_secs(1)));
}

/// An app showing the five-page `pages.pdf`.
fn pdf_app() -> App {
    let mut app = app();
    let path = fixture("pages.pdf");
    let doc = pdf::Document::open(&path).unwrap();
    app.files = vec![path.clone()];
    app.current = 0;
    let _ = app.open_pdf_view(path, doc);
    app
}

fn page(app: &App) -> usize {
    match &app.doc {
        Doc::Pdf(v) => v.current_page,
        _ => panic!("no PDF is open"),
    }
}

fn state(app: &App, key: &str) -> serde_json::Value {
    app.control_state()[key].clone()
}

#[test]
fn a_new_app_shows_nothing_and_offers_nothing() {
    let _inside = runtime().enter();
    let app = app();
    assert!(matches!(app.doc, Doc::Empty));
    assert_eq!(app.title(), "Lumen");
    assert_eq!(state(&app, "kind"), "none");
    assert!(app.sidebar_tabs().is_empty());
    assert!(!app.can_slideshow());
    assert!(app.current_file().is_none());
}

#[test]
fn an_open_pdf_reports_its_pages_and_sidebar() {
    let _inside = runtime().enter();
    let app = pdf_app();
    assert_eq!((state(&app, "kind"), state(&app, "page"), state(&app, "pages")), ("pdf".into(), 1.into(), 5.into()));
    assert!(app.title().starts_with("pages.pdf"));
    // No outline and one file: only the page thumbnails have something to show.
    assert_eq!(app.sidebar_tabs(), vec![SidebarTab::Pages]);
    assert_eq!(app.sidebar_shows(), Some(SidebarTab::Pages));
    assert!(app.can_slideshow());
}

#[test]
fn the_go_to_page_box_jumps_clamps_and_ignores_nonsense() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let _ = app.update(Message::GoToOpen);
    assert_eq!(app.goto.as_deref(), Some(""));
    let _ = app.update(Message::GoToInput("3".into()));
    let _ = app.update(Message::GoToSubmit);
    assert_eq!((page(&app), app.goto.clone()), (2, None));

    // Past the end lands on the last page.
    let _ = app.update(Message::GoToOpen);
    let _ = app.update(Message::GoToInput("99".into()));
    let _ = app.update(Message::GoToSubmit);
    assert_eq!(page(&app), 4);

    // Nothing typed, or no number: stay where you are, box closed.
    for typed in ["", "abc"] {
        let _ = app.update(Message::GoToOpen);
        let _ = app.update(Message::GoToInput(typed.into()));
        let _ = app.update(Message::GoToSubmit);
        assert_eq!((page(&app), app.goto.clone()), (4, None), "typed {typed:?}");
    }
    // Closing it with the button, too.
    let _ = app.update(Message::GoToOpen);
    let _ = app.update(Message::GoToClose);
    assert!(app.goto.is_none());
}

#[test]
fn escape_closes_things_one_at_a_time() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let _ = app.update(Message::GoToOpen);
    let _ = app.update(Message::ToggleFullscreen);
    assert!(app.fullscreen && app.goto.is_some());
    let _ = app.update(Message::Escape);
    assert!(app.goto.is_none() && app.fullscreen, "the box goes first");
    let _ = app.update(Message::Escape);
    assert!(!app.fullscreen);
}

#[test]
fn night_mode_toggles_and_is_remembered() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    assert!(!app.night);
    let _ = app.update(Message::ToggleNight);
    assert!(app.night && app.prefs.night_mode);
    let _ = app.update(Message::ToggleNight);
    assert!(!app.night && !app.prefs.night_mode);
}

#[test]
fn rendered_pages_arrive_only_when_they_are_still_wanted() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let picture = || Picture { width: 2, height: 2, original: (2, 2), rgba: vec![255; 16].into() };
    let shown = |app: &App| match &app.doc {
        Doc::Pdf(v) => v.slots[0].shown.is_some(),
        _ => false,
    };
    // From a document that is gone.
    let _ = app.update(Message::PageRendered(app.generation + 7, 0, 1.0, false, Ok(picture())));
    assert!(!shown(&app));
    // Drawn for the other mode (night was switched meanwhile).
    let _ = app.update(Message::ToggleNight);
    let _ = app.update(Message::PageRendered(app.generation, 0, 1.0, false, Ok(picture())));
    assert!(!shown(&app));
    // The right one.
    let _ = app.update(Message::PageRendered(app.generation, 0, 1.0, true, Ok(picture())));
    assert!(shown(&app));
    // A failed render leaves the page blank but does not break anything.
    let mut app = pdf_app();
    let _ = app.update(Message::PageRendered(app.generation, 1, 1.0, false, Err("nope".into())));
    assert!(matches!(&app.doc, Doc::Pdf(v) if v.slots[1].shown.is_none()));
}

#[test]
fn zoom_messages_change_the_pdf_zoom() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let zoom = |app: &App| match &app.doc {
        Doc::Pdf(v) => v.zoom,
        _ => 0.0,
    };
    let start = zoom(&app);
    let _ = app.update(Message::ZoomIn);
    settle(&mut app);
    assert!(zoom(&app) > start, "{} -> {}", start, zoom(&app));
    let _ = app.update(Message::ZoomOut);
    settle(&mut app);
    let _ = app.update(Message::ZoomOut);
    settle(&mut app);
    assert!(zoom(&app) < start);
    let _ = app.update(Message::ZoomFit);
    settle(&mut app);
    assert!(matches!(&app.doc, Doc::Pdf(v) if v.fit == PdfFit::Page));
    let _ = app.update(Message::ZoomFitWidth);
    settle(&mut app);
    assert!(matches!(&app.doc, Doc::Pdf(v) if v.fit == PdfFit::Width));
}

#[test]
fn browsing_files_stays_inside_the_list() {
    let _inside = runtime().enter();
    let mut app = app();
    let pictures = fixture("pictures");
    let files: Vec<PathBuf> = ["g.png", "g.jpg", "g.gif"].iter().map(|n| pictures.join(n)).collect();
    let _ = app.update(Message::Expanded { files: files.clone(), start: 1, append: false });
    assert_eq!((app.current, app.files.len()), (1, 3));
    assert!(matches!(&app.doc, Doc::Loading(p) if *p == files[1]));
    let _ = app.update(Message::Next);
    let _ = app.update(Message::Next);
    assert_eq!(app.current, 2, "the end of the list is the end");
    let _ = app.update(Message::First);
    assert_eq!(app.current, 0);
    let _ = app.update(Message::Prev);
    assert_eq!(app.current, 0);
    let _ = app.update(Message::Last);
    assert_eq!(app.current, 2);
    assert_eq!(state(&app, "index"), 3);
    // Nothing to show is said out loud, and changes nothing.
    let _ = app.update(Message::Expanded { files: Vec::new(), start: 0, append: false });
    assert!(app.toast.as_ref().is_some_and(|(_, _, error)| *error));
    assert_eq!(app.files.len(), 3);
}

#[test]
fn wrap_around_joins_the_ends_of_the_list_when_asked() {
    let _inside = runtime().enter();
    let mut app = app();
    let pictures = fixture("pictures");
    let files: Vec<PathBuf> = ["g.png", "g.jpg", "g.gif"].iter().map(|n| pictures.join(n)).collect();
    let _ = app.update(Message::Expanded { files, start: 2, append: false });
    let _ = app.update(Message::Next);
    assert_eq!(app.current, 2, "off by default: the end is the end");
    let _ = app.update(Message::Pref(PrefMsg::WrapAround(true)));
    let _ = app.update(Message::Next);
    assert_eq!(app.current, 0, "next after the last is the first");
    let _ = app.update(Message::Prev);
    assert_eq!(app.current, 2, "previous before the first is the last");
}

fn key(app: &mut App, key: iced::keyboard::Key, modifiers: iced::keyboard::Modifiers) {
    let _ = app.update(Message::Key(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced::keyboard::key::Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
        location: iced::keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    }));
}

#[test]
fn alt_enter_opens_properties_and_letters_pick_markup_tools() {
    use iced::keyboard::{Key, Modifiers, key::Named};
    let _inside = runtime().enter();
    let mut app = pdf_app();
    key(&mut app, Key::Named(Named::Enter), Modifiers::ALT);
    assert_eq!(app.dialog, Some(Dialog::Properties));
    app.dialog = None;
    // Without the markup bar a letter keeps its usual job (R turns the page)...
    key(&mut app, Key::Character("r".into()), Modifiers::empty());
    assert_ne!(app.editor.tool, Tool::Rectangle);
    // ...and with it open, letters pick the tools.
    key(&mut app, Key::Character("m".into()), Modifiers::empty());
    assert!(app.markup_open);
    for (letter, tool) in [
        ("p", Tool::Pen),
        ("r", Tool::Rectangle),
        ("o", Tool::Ellipse),
        ("a", Tool::Arrow),
        ("l", Tool::Line),
        ("t", Tool::Text),
        ("h", Tool::Highlight),
        ("v", Tool::Select),
    ] {
        key(&mut app, Key::Character(letter.into()), Modifiers::empty());
        assert_eq!(app.editor.tool, tool, "key {letter}");
    }
}

/// What the scroll area reports after a jump (there is no scroll area to do it here).
fn scrolled_to_page(app: &mut App, page: usize) {
    let y = match &app.doc {
        Doc::Pdf(v) => v.tops[page],
        _ => panic!("no PDF is open"),
    };
    let _ = app.update(Message::Scrolled(Viewport { x: 0.0, y, height: 600.0 }));
}

#[test]
fn back_and_forward_retrace_followed_links() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    scrolled_to_page(&mut app, 0);
    let _ = app.update(Message::GoTo(Target::Page { page: 3, top: None }));
    scrolled_to_page(&mut app, 3);
    let _ = app.update(Message::GoTo(Target::Page { page: 1, top: None }));
    scrolled_to_page(&mut app, 1);
    let _ = app.update(Message::LinkBack);
    assert_eq!(page(&app), 3, "back goes to where the second link was followed from");
    scrolled_to_page(&mut app, 3);
    let _ = app.update(Message::LinkBack);
    assert_eq!(page(&app), 0);
    scrolled_to_page(&mut app, 0);
    let _ = app.update(Message::LinkBack);
    assert_eq!(page(&app), 0, "nothing further back");
    let _ = app.update(Message::LinkForward);
    assert_eq!(page(&app), 3);
    scrolled_to_page(&mut app, 3);
    let _ = app.update(Message::LinkForward);
    assert_eq!(page(&app), 1);
    // Following a new link forgets what was ahead.
    scrolled_to_page(&mut app, 1);
    let _ = app.update(Message::LinkBack);
    scrolled_to_page(&mut app, 3);
    let _ = app.update(Message::GoTo(Target::Page { page: 2, top: None }));
    scrolled_to_page(&mut app, 2);
    let _ = app.update(Message::LinkForward);
    assert_eq!(page(&app), 2, "forward has nothing after a new jump");
}

#[test]
fn a_file_changed_by_another_program_is_reloaded_where_it_was() {
    use std::time::{Duration, SystemTime};
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let path = app.current_path().unwrap().to_path_buf();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    scrolled_to_page(&mut app, 3);
    let generation = app.generation;
    // The first look only notes how the file is.
    let _ = app.update(Message::FileStat(path.clone(), Some((t0, 100))));
    assert_eq!(app.generation, generation);
    // Same again, a vanished file and some other file's news change nothing.
    let _ = app.update(Message::FileStat(path.clone(), Some((t0, 100))));
    let _ = app.update(Message::FileStat(path.clone(), None));
    let _ = app.update(Message::FileStat(PathBuf::from("/elsewhere.pdf"), Some((t0, 5))));
    assert_eq!(app.generation, generation);
    // A different size or time is a change: load it again, and remember the place.
    let _ = app.update(Message::FileStat(path.clone(), Some((t0, 120))));
    assert_eq!(app.generation, generation + 1);
    assert!(matches!(&app.doc, Doc::Loading(p) if *p == path));
    assert_eq!(app.restore.map(|p| p.page), Some(3));
    // The same news again is not a second change.
    let _ = app.update(Message::FileStat(path.clone(), Some((t0, 120))));
    assert_eq!(app.generation, generation + 1);
}

#[test]
fn opened_files_are_remembered_and_a_pdf_comes_back_at_its_page() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let pdf = fixture("pages.pdf");
    // Reading page 3, then moving on to a picture, notes the page...
    let _ = app.update(Message::Expanded { files: vec![pdf.clone()], start: 0, append: false });
    assert_eq!(app.state.recent[0].path, pdf);
    app.doc = Doc::Empty;
    let doc = pdf::Document::open(&pdf).unwrap();
    let _ = app.open_pdf_view(pdf.clone(), doc);
    scrolled_to_page(&mut app, 3);
    let picture = fixture("pictures").join("g.png");
    let _ = app.update(Message::Expanded { files: vec![picture.clone()], start: 0, append: false });
    assert_eq!(app.state.recent[0].path, picture, "the newest file is first");
    assert_eq!(app.state.page_of(&pdf), Some(3));
    // ...and opening the PDF again starts there.
    let doc = pdf::Document::open(&pdf).unwrap();
    let _ = app.open_pdf_view(pdf.clone(), doc);
    assert_eq!(page(&app), 3);
    // A reload keeps its own place rather than the remembered one.
    app.restore = Some(Place { page: 1, down: 0.0, across: 0.0 });
    let doc = pdf::Document::open(&pdf).unwrap();
    let _ = app.open_pdf_view(pdf.clone(), doc);
    assert_eq!(page(&app), 1);
}

#[test]
fn the_recent_list_can_be_opened_from_and_cleared() {
    let _inside = runtime().enter();
    let mut app = app();
    let picture = fixture("pictures").join("g.png");
    let _ = app.update(Message::Expanded { files: vec![picture.clone()], start: 0, append: false });
    app.doc = Doc::Empty;
    app.files.clear();
    assert_eq!(app.state.recent.len(), 1);
    let _ = app.update(Message::ClearRecent);
    assert!(app.state.recent.is_empty());
    // A file that cannot be opened drops out of the list.
    let missing = fixture("pictures").join("not-there.png");
    app.state.touch(&missing);
    let _ = app.update(Message::PictureLoaded(app.generation, missing.clone(), Err("gone".into())));
    assert!(app.state.recent.is_empty());
}

#[test]
fn the_command_palette_finds_and_runs_commands() {
    use commands::PaletteMsg;
    use iced::keyboard::{Key, Modifiers, key::Named};
    let _inside = runtime().enter();
    // With nothing open, only what makes sense is offered.
    let mut empty = app();
    key(&mut empty, Key::Character("p".into()), Modifiers::CTRL | Modifiers::SHIFT);
    assert!(empty.palette.is_some());
    let names: Vec<String> = empty.palette_matches().into_iter().map(|c| c.0).collect();
    assert!(names.iter().any(|n| n.starts_with("Open Files")) && !names.iter().any(|n| n.starts_with("Print")));
    key(&mut empty, Key::Named(Named::Escape), Modifiers::empty());
    let _ = empty.update(Message::Escape);
    assert!(empty.palette.is_none());

    let mut app = pdf_app();
    let _ = app.update(Message::Palette(PaletteMsg::Open));
    let _ = app.update(Message::Palette(PaletteMsg::Input("organise".into())));
    assert_eq!(app.palette_matches().len(), 1);
    let _ = app.update(Message::Palette(PaletteMsg::Run));
    assert!(app.palette.is_none(), "running a command closes the palette");
    assert!(app.organizer.is_some());
    // The highlight wraps round the matches.
    let _ = app.update(Message::Palette(PaletteMsg::Open));
    let _ = app.update(Message::Palette(PaletteMsg::Input("page".into())));
    let n = app.palette_matches().len().min(8);
    assert!(n > 1);
    let _ = app.update(Message::Palette(PaletteMsg::Move(-1)));
    assert_eq!(app.palette.as_ref().unwrap().selected, n - 1);
    let _ = app.update(Message::Palette(PaletteMsg::Move(1)));
    assert_eq!(app.palette.as_ref().unwrap().selected, 0);
    // Nothing matching runs nothing.
    let _ = app.update(Message::Palette(PaletteMsg::Input("qqqq".into())));
    let _ = app.update(Message::Palette(PaletteMsg::Run));
    assert!(app.palette.is_none());
}

#[test]
fn search_options_survive_a_new_query_and_are_remembered_by_the_bar() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let _ = app.update(Message::SearchOpen);
    let _ = app.update(Message::SearchMatchCase);
    let _ = app.update(Message::SearchWholeWords);
    let _ = app.update(Message::SearchInput("page".into()));
    let search = app.search.as_ref().unwrap();
    assert!(search.options.match_case && search.options.whole_words);
    let _ = app.update(Message::SearchMatchCase);
    let search = app.search.as_ref().unwrap();
    assert!(!search.options.match_case && search.options.whole_words);
    assert_eq!(search.query, "page", "the query is kept when an option changes");
}

#[test]
fn dropping_more_files_adds_them_without_losing_the_place() {
    let _inside = runtime().enter();
    let mut app = app();
    let pictures = fixture("pictures");
    let (a, b, c) = (pictures.join("g.png"), pictures.join("g.jpg"), pictures.join("g.gif"));
    let _ = app.update(Message::Expanded { files: vec![a.clone(), b.clone()], start: 0, append: false });
    // One already there and one new: no duplicates, and the first dropped file is shown.
    let _ = app.update(Message::Expanded { files: vec![b.clone(), c.clone()], start: 0, append: true });
    assert_eq!(app.files, vec![a, b.clone(), c]);
    assert_eq!(app.files[app.current], b);
}

#[test]
fn preferences_apply_at_once() {
    let _inside = runtime().enter();
    let mut app = app();
    let _ = app.update(Message::Pref(PrefMsg::Theme(ThemePref::Dark)));
    assert_eq!(app.mode, Mode::Dark);
    let _ = app.update(Message::Pref(PrefMsg::Theme(ThemePref::Light)));
    assert_eq!(app.mode, Mode::Light);
    // "System" follows what the desktop says, live.
    let _ = app.update(Message::Pref(PrefMsg::Theme(ThemePref::System)));
    let _ = app.update(Message::SystemTheme(Mode::Dark));
    assert_eq!(app.mode, Mode::Dark);
    let _ = app.update(Message::SystemTheme(Mode::Light));
    assert_eq!(app.mode, Mode::Light);

    let _ = app.update(Message::Pref(PrefMsg::SlideshowSeconds(12)));
    let _ = app.update(Message::Pref(PrefMsg::CacheMb(128)));
    let _ = app.update(Message::Pref(PrefMsg::PreloadMegapixels(0)));
    let _ = app.update(Message::Pref(PrefMsg::FloatingControls(false)));
    assert_eq!((app.prefs.slideshow_seconds, app.prefs.cache_mb, app.prefs.preload_megapixels), (12, 128, 0));
    assert_eq!(app.prefs.preload_pixels(), 0);
    assert!(!app.prefs.floating_controls);

    let _ = app.update(Message::Pref(PrefMsg::Reset));
    assert_eq!(app.prefs, crate::settings::Settings::default());
}

#[test]
fn a_slideshow_needs_something_to_show_then_steps_pauses_and_ends() {
    let _inside = runtime().enter();
    let mut app = app();
    let _ = app.update(Message::Slideshow);
    assert!(app.slideshow.is_none(), "nothing to step through");
    assert!(app.toast.is_some());

    let mut app = pdf_app();
    let start = Instant::now();
    let _ = app.update(Message::Slideshow);
    assert!(app.slideshow.is_some() && app.fullscreen);
    // The first look only starts the count; the page changes after the time is up.
    let _ = app.update(Message::SlideshowTick(start));
    assert_eq!(page(&app), 0);
    let _ = app.update(Message::SlideshowTick(start + Duration::from_secs(2)));
    assert_eq!(page(&app), 0);
    let _ = app.update(Message::SlideshowTick(start + Duration::from_secs(6)));
    assert_eq!(page(&app), 1);

    // Paused: time passes and nothing happens.
    let _ = app.update(Message::SlideshowPause);
    let _ = app.update(Message::SlideshowTick(start + Duration::from_secs(20)));
    let _ = app.update(Message::SlideshowTick(start + Duration::from_secs(40)));
    assert_eq!(page(&app), 1);
    let _ = app.update(Message::SlideshowPause);

    // On to the end: four more pages, then it stops and the window goes back as it was.
    let mut now = start + Duration::from_secs(100);
    for _ in 0..12 {
        let _ = app.update(Message::SlideshowTick(now));
        now += Duration::from_secs(6);
        if app.slideshow.is_none() {
            break;
        }
    }
    assert_eq!(page(&app), 4);
    assert!(app.slideshow.is_none() && !app.fullscreen);
}

#[test]
fn leaving_a_slideshow_with_escape_restores_the_window() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let _ = app.update(Message::Slideshow);
    assert!(app.fullscreen);
    let _ = app.update(Message::Escape);
    assert!(app.slideshow.is_none() && !app.fullscreen);
    // Started from fullscreen, it stays fullscreen afterwards.
    let _ = app.update(Message::ToggleFullscreen);
    let _ = app.update(Message::Slideshow);
    let _ = app.update(Message::Escape);
    assert!(app.slideshow.is_none() && app.fullscreen);
}

#[test]
fn control_commands_answer_with_the_state_or_a_reason() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let ok = |app: &mut App, command: Command| -> serde_json::Value {
        serde_json::from_str(&app.control_run(&command).0.unwrap()).unwrap()
    };
    let err = |app: &mut App, command: Command| app.control_run(&command).0.unwrap_err();
    assert_eq!(ok(&mut app, Command::GotoPage(4))["page"], 4);
    assert_eq!(page(&app), 3);
    assert!(err(&mut app, Command::GotoPage(0)).contains("pages 1 to 5"));
    assert!(err(&mut app, Command::GotoPage(6)).contains("pages 1 to 5"));
    assert!(err(&mut app, Command::Zoom("sideways".into())).contains("unknown zoom"));
    assert!(err(&mut app, Command::Open { paths: vec![], password: None }).contains("nothing to open"));
    assert!(
        err(&mut app, Command::Open { paths: vec!["/no/such/file".into()], password: None }).contains("does not exist")
    );
    // Opening marks the app busy until the files are found.
    let opened = ok(&mut app, Command::Open { paths: vec![fixture("form.pdf")], password: Some("pw".into()) });
    assert_eq!(opened["busy"], true);
    assert_eq!(app.passwords.get(&fixture("form.pdf")).map(String::as_str), Some("pw"));
    let _ = app.update(Message::Expanded { files: vec![fixture("form.pdf")], start: 0, append: false });
    assert!(!app.control_opening);

    let mut empty = super::tests::app();
    assert!(err(&mut empty, Command::Search("x".into())).contains("no PDF"));
    assert!(err(&mut empty, Command::GotoPage(1)).contains("no PDF"));
}

#[test]
fn the_export_dialog_counts_the_page_in_view_or_the_selected_pages() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let _ = app.update(Message::GoToPage(2));
    assert_eq!(app.export_selection().map(|(pages, ..)| pages.len()), Some(1));
    assert_eq!(app.export_selection().unwrap().0[0].index, 2);
    let _ = app.update(Message::Export(export::ExportMsg::Open));
    assert!(app.dialog == Some(menu::Dialog::Export));
    // In the page organiser it is what is selected.
    let _ = app.update(Message::Pages(PagesMsg::Open));
    let _ = app.update(Message::Pages(PagesMsg::SelectAll));
    assert_eq!(app.export_selection().map(|(pages, ..)| pages.len()), Some(5));
    // Closing the dialog is just a message away.
    let _ = app.update(Message::Dialog(None));
    assert!(app.dialog.is_none());
}

#[test]
fn the_page_organiser_tracks_changes_and_undoes_them() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let _ = app.update(Message::Pages(PagesMsg::Open));
    assert!(matches!(&app.organizer, Some(o) if !o.is_changed()));
    let _ = app.update(Message::Pages(PagesMsg::Rotate(90)));
    assert!(app.organizer.as_ref().unwrap().is_changed());
    let _ = app.update(Message::Pages(PagesMsg::Undo));
    assert!(!app.organizer.as_ref().unwrap().is_changed());
    let _ = app.update(Message::Pages(PagesMsg::Redo));
    assert!(app.organizer.as_ref().unwrap().is_changed());
    // Cancelling drops the organiser (and with it the unsaved plan).
    let _ = app.update(Message::Pages(PagesMsg::Close));
    assert!(app.organizer.is_none());
}

#[test]
fn deleting_pages_offers_to_undo_it() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let _ = app.update(Message::Pages(PagesMsg::Open));
    let _ = app.update(Message::Pages(PagesMsg::SelectAll));
    // Not all of them: a document keeps at least one page, and says so.
    let _ = app.update(Message::Pages(PagesMsg::Delete));
    assert!(app.pages_undo.is_none() && app.toast.as_ref().is_some_and(|(_, _, error)| *error));
    // Opened afresh, only the page being read is selected.
    let _ = app.update(Message::Pages(PagesMsg::Close));
    let _ = app.update(Message::Pages(PagesMsg::Open));
    let _ = app.update(Message::Pages(PagesMsg::Delete));
    let count = |app: &mut App| {
        let _ = app.update(Message::Pages(PagesMsg::SelectAll));
        app.export_selection().map(|(pages, ..)| pages.len())
    };
    assert_eq!(count(&mut app), Some(4));
    assert_eq!(app.pages_undo, Some(app.toast_seq));
    assert!(app.toast.as_ref().is_some_and(|(_, text, error)| text.contains('1') && !*error));
    // The toast's button undoes it and the offer goes away.
    let _ = app.update(Message::Pages(PagesMsg::Undo));
    assert_eq!(count(&mut app), Some(5));
    assert!(app.pages_undo.is_none() && app.toast.is_none());
}

#[test]
fn the_property_and_shortcut_dialogs_open_and_close() {
    let _inside = runtime().enter();
    let mut app = pdf_app();
    let _ = app.update(Message::ShowProperties);
    assert!(app.dialog == Some(menu::Dialog::Properties) && app.properties.is_none());
    let _ = app.update(Message::Dialog(Some(menu::Dialog::Shortcuts)));
    assert!(app.dialog == Some(menu::Dialog::Shortcuts));
    let _ = app.update(Message::Escape);
    assert!(app.dialog.is_none());
}
