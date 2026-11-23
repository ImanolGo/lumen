//! The app's message flows, driven without a window: `App::new`, then `App::update` with the
//! messages a person's clicks and keys would send. Work that needs the iced runtime (tasks)
//! is built but not run; what the app decides is read back from its state.

use std::time::{Duration, Instant};

use super::prefs::PrefMsg;
use super::*;
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
