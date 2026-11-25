//! The control interface, tried against the real app: over standard input and output everywhere,
//! and on a private session bus on Linux with `LUMEN_CONTROL_TESTS=bus`.
//!
//! These open real windows on your desktop, so they only run when asked:
//! `LUMEN_CONTROL_TESTS=1 cargo test --test control`. (CI has no display to open them on.)

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

const APP: &str = env!("CARGO_BIN_EXE_lumen-rs");
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// What a call takes: a tuple.
#[cfg(target_os = "linux")]
trait Body: serde::Serialize + zbus::zvariant::DynamicType {}
#[cfg(target_os = "linux")]
impl<T: serde::Serialize + zbus::zvariant::DynamicType> Body for T {}
#[cfg(not(target_os = "linux"))]
trait Body: serde::Serialize {}
#[cfg(not(target_os = "linux"))]
impl<T: serde::Serialize> Body for T {}

/// How the app is reached.
enum Transport {
    Stdio(Mutex<(ChildStdin, BufReader<ChildStdout>)>),
    #[cfg(target_os = "linux")]
    Bus(zbus::Connection),
}

impl Transport {
    async fn call<B: Body>(&self, method: &str, body: &B) -> Result<Value, String> {
        match self {
            Self::Stdio(pipes) => {
                let mut pipes = pipes.lock().unwrap();
                let call = serde_json::json!({ "method": method, "args": body });
                writeln!(pipes.0, "{call}").and_then(|()| pipes.0.flush()).map_err(|e| e.to_string())?;
                let mut line = String::new();
                pipes.1.read_line(&mut line).map_err(|e| e.to_string())?;
                let answer: Value = serde_json::from_str(&line).map_err(|e| format!("{e}: {line:?}"))?;
                match answer.get("error") {
                    Some(e) => Err(e.as_str().unwrap_or_default().to_owned()),
                    None => Ok(answer["ok"].clone()),
                }
            }
            #[cfg(target_os = "linux")]
            Self::Bus(bus) => {
                let reply =
                    bus.call_method(Some(NAME), PATH, Some(IFACE), method, body).await.map_err(|e| e.to_string())?;
                let json: String = reply.body().deserialize().map_err(|e| e.to_string())?;
                serde_json::from_str(&json).map_err(|e| e.to_string())
            }
        }
    }
}

#[cfg(target_os = "linux")]
const NAME: &str = "io.github.imanolgo.LumenRs";
#[cfg(target_os = "linux")]
const PATH: &str = "/io/github/imanolgo/LumenRs/Control";
#[cfg(target_os = "linux")]
const IFACE: &str = "io.github.imanolgo.LumenRs.Control1";

/// The app, and what it needs to run.
struct Lumen {
    children: Vec<Child>,
    transport: Transport,
    dir: PathBuf,
}

impl Drop for Lumen {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn enabled(test: &str) -> bool {
    if std::env::var_os("LUMEN_CONTROL_TESTS").is_none() {
        eprintln!("skipping {test}: set LUMEN_CONTROL_TESTS=1 to run it (it opens a window)");
        return false;
    }
    true
}

fn on_stdio_started(mut app: Command, dir: PathBuf) -> Lumen {
    let mut app = app.env("LUMEN_CONTROL", "stdio").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let pipes = (app.stdin.take().unwrap(), BufReader::new(app.stdout.take().unwrap()));
    Lumen { children: vec![app], transport: Transport::Stdio(Mutex::new(pipes)), dir }
}

#[cfg(not(target_os = "linux"))]
async fn on_bus_started(_app: Command, _dir: PathBuf) -> Lumen {
    panic!("there is no session bus here")
}

#[cfg(target_os = "linux")]
async fn on_bus_started(mut app: Command, dir: PathBuf) -> Lumen {
    let mut daemon = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("dbus-daemon is needed for these tests");
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let address = address.trim().to_owned();
    let app = app.env("DBUS_SESSION_BUS_ADDRESS", &address).env("LUMEN_CONTROL", "1").stdout(Stdio::null()).spawn();
    let bus = zbus::connection::Builder::address(address.as_str()).build().await.unwrap();
    Lumen { children: vec![daemon, app.unwrap()], transport: Transport::Bus(bus), dir }
}

impl Lumen {
    async fn start(test: &str, files: &[PathBuf]) -> Self {
        let dir = std::env::temp_dir().join(format!("lumen-rs-control-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let log = std::fs::File::create(dir.join("lumen.log")).unwrap();
        let mut app = Command::new(APP);
        app.args(files)
            .env("LUMEN_NO_PORTAL", "1")
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_CACHE_HOME", dir.join("cache"))
            .stderr(log);
        let on_bus = std::env::var_os("LUMEN_CONTROL_TESTS").is_some_and(|v| v == "bus");
        let lumen = if on_bus { on_bus_started(app, dir).await } else { on_stdio_started(app, dir) };

        // Wait until the app is on the bus and has opened what it was given.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(state) = lumen.try_call("GetState", &()).await
                && (files.is_empty() || !state["file"].is_null())
                && state["busy"] == false
            {
                return lumen;
            }
            assert!(
                Instant::now() < deadline,
                "Lumen did not start\n{}",
                std::fs::read_to_string(lumen.dir.join("lumen.log")).unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn try_call<B: Body>(&self, method: &str, body: &B) -> Result<Value, String> {
        self.transport.call(method, body).await
    }

    async fn call<B: Body>(&self, method: &str, body: &B) -> Value {
        self.try_call(method, body).await.unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    /// Calls `method`, then waits until nothing is going on any more.
    async fn settled<B: Body>(&self, method: &str, body: &B) -> Value {
        self.call(method, body).await;
        self.wait(|s| s["busy"] == false).await
    }

    /// Polls the state until `done` says so.
    async fn wait(&self, done: impl Fn(&Value) -> bool) -> Value {
        // Software rendering in a debug build can take most of this.
        let deadline = Instant::now() + Duration::from_secs(150);
        loop {
            let state = self.call("GetState", &()).await;
            if done(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "timed out; last state {state}");
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    }
}

fn paths(list: &[PathBuf]) -> Vec<String> {
    list.iter().map(|p| p.display().to_string()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn opens_reports_and_navigates_a_pdf() {
    if !enabled("opens_reports_and_navigates_a_pdf") {
        return;
    }
    let lumen = Lumen::start("pdf", &[fixture("pages.pdf")]).await;
    let state = lumen.wait(|s| s["kind"] == "pdf").await;
    assert_eq!((state["page"].clone(), state["pages"].clone()), (1.into(), 5.into()));
    // Opening one file brings in the supported files next to it, starting at that one.
    assert!(state["file"].as_str().unwrap().ends_with("pages.pdf"));
    assert!(state["count"].as_u64().unwrap() > 1);

    let state = lumen.call("GotoPage", &(3u32,)).await;
    assert_eq!(state["page"], 3);
    let state = lumen.wait(|s| s["page"] == 3).await;
    assert_eq!(state["kind"], "pdf");

    // Pages outside the document, and calls that make no sense, are refused with a reason.
    for page in [0u32, 6] {
        let e = lumen.try_call("GotoPage", &(page,)).await.unwrap_err();
        assert!(e.contains("pages 1 to 5"), "{e}");
    }
    let e = lumen.try_call("Zoom", &("sideways",)).await.unwrap_err();
    assert!(e.contains("unknown zoom"), "{e}");

    // Zooming changes the number the state reports.
    let before = lumen.call("GetState", &()).await["zoom"].as_f64().unwrap();
    lumen.call("Zoom", &("in",)).await;
    let after =
        lumen.wait(|s| s["zoom"].as_f64().is_some_and(|z| (z - before).abs() > 0.01)).await["zoom"].as_f64().unwrap();
    assert!(after > before, "{before} -> {after}");
}

#[tokio::test(flavor = "multi_thread")]
async fn searches_the_text_of_a_pdf() {
    if !enabled("searches_the_text_of_a_pdf") {
        return;
    }
    let lumen = Lumen::start("search", &[fixture("pages.pdf")]).await;
    lumen.wait(|s| s["kind"] == "pdf").await;
    lumen.call("Search", &("needle",)).await;
    let state = lumen.wait(|s| s["search"]["done"] == true).await;
    assert_eq!(state["search"]["query"], "needle");
    assert_eq!(state["search"]["hits"], 1, "{state}");
    // The search jumps to the page where the word is.
    let state = lumen.wait(|s| s["page"] == 3).await;
    assert_eq!(state["search"]["current"], 1);
    lumen.call("Search", &("nothing like this",)).await;
    let state = lumen.wait(|s| s["search"]["done"] == true && s["search"]["query"] == "nothing like this").await;
    assert_eq!(state["search"]["hits"], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_one_picture_lets_you_browse_its_folder() {
    if !enabled("opening_one_picture_lets_you_browse_its_folder") {
        return;
    }
    // A folder of three pictures; opening the middle one brings in its neighbours.
    let lumen = Lumen::start("folder", &[]).await;
    let folder = lumen.dir.join("pics");
    std::fs::create_dir_all(&folder).unwrap();
    for (from, to) in [("g.png", "a.png"), ("gi.png", "b.png"), ("g.jpg", "c.jpg")] {
        std::fs::copy(fixture("pictures").join(from), folder.join(to)).unwrap();
    }
    let state = lumen.settled("Open", &(paths(&[folder.join("b.png")]), "")).await;
    assert_eq!(state["kind"], "picture");
    assert_eq!((state["index"].clone(), state["count"].clone()), (2.into(), 3.into()));
    assert!(state["file"].as_str().unwrap().ends_with("b.png"));

    let state = lumen.call("Next", &()).await;
    assert_eq!(state["index"], 3);
    let state =
        lumen.wait(|s| s["kind"] == "picture" && s["file"].as_str().is_some_and(|f| f.ends_with("c.jpg"))).await;
    assert_eq!(state["index"], 3);
    // Past the end nothing changes.
    assert_eq!(lumen.call("Next", &()).await["index"], 3);
    lumen.call("Previous", &()).await;
    lumen.call("First", &()).await;
    assert_eq!(lumen.wait(|s| s["index"] == 1).await["kind"], "picture");
    lumen.call("Last", &()).await;
    lumen.wait(|s| s["index"] == 3).await;

    // A picture has no pages.
    let e = lumen.try_call("GotoPage", &(1u32,)).await.unwrap_err();
    assert!(e.contains("no PDF"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn protected_pdfs_open_with_their_password_and_never_ask_again() {
    if !enabled("protected_pdfs_open_with_their_password_and_never_ask_again") {
        return;
    }
    let lumen = Lumen::start("locked", &[]).await;
    let secret = fixture("encrypted.pdf");
    // Without the password the file waits for one.
    let state = lumen.settled("Open", &(paths(std::slice::from_ref(&secret)), "")).await;
    assert_eq!(state["kind"], "locked", "{state}");
    // With it, it opens.
    let state = lumen.settled("Open", &(paths(std::slice::from_ref(&secret)), "secret")).await;
    assert_eq!(state["kind"], "pdf", "{state}");
    // A wrong one does not.
    let state = lumen.settled("Open", &(paths(&[fixture("pages.pdf")]), "")).await;
    assert_eq!(state["kind"], "pdf");
    let state = lumen.settled("Open", &(paths(&[secret]), "wrong")).await;
    assert_eq!(state["kind"], "locked", "{state}");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_get_reasons_not_silence() {
    if !enabled("bad_requests_get_reasons_not_silence") {
        return;
    }
    let lumen = Lumen::start("errors", &[]).await;
    let state = lumen.call("GetState", &()).await;
    assert_eq!(state["kind"], "none");
    assert!(state["file"].is_null());
    let e = lumen.try_call("Open", &(Vec::<String>::new(), "")).await.unwrap_err();
    assert!(e.contains("nothing to open"), "{e}");
    let e = lumen.try_call("Open", &(vec!["/no/such/file.pdf".to_string()], "")).await.unwrap_err();
    assert!(e.contains("does not exist"), "{e}");
    let e = lumen.try_call("Search", &("x",)).await.unwrap_err();
    assert!(e.contains("no PDF"), "{e}");
    let e = lumen.try_call("GotoPage", &(1u32,)).await.unwrap_err();
    assert!(e.contains("no PDF"), "{e}");
}

/// Copies a fixture into the test's folder, so the test may change it.
fn work_copy(lumen: &Lumen, fixture_path: PathBuf, name: &str) -> PathBuf {
    let to = lumen.dir.join(name);
    std::fs::copy(fixture_path, &to).unwrap();
    to
}

/// Waits until the window has said something with `text` in it (it may already have).
async fn noticed(lumen: &Lumen, text: &str) -> Value {
    lumen.wait(|s| s["busy"] == false && s["notice"]["text"].as_str().is_some_and(|t| t.contains(text))).await
}

fn field<'a>(form: &'a Value, name: &str) -> &'a Value {
    form["fields"].as_array().unwrap().iter().find(|f| f["name"] == name).unwrap_or_else(|| panic!("no field {name}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn fills_in_a_form_and_saves_the_answers() {
    if !enabled("fills_in_a_form_and_saves_the_answers") {
        return;
    }
    let lumen = Lumen::start("form", &[]).await;
    let file = work_copy(&lumen, fixture("form.pdf"), "form.pdf");
    let state = lumen.settled("Open", &(paths(std::slice::from_ref(&file)), "")).await;
    assert_eq!(state["kind"], "pdf");
    assert_eq!(state["form"]["dirty"], false);

    let form = lumen.call("GetForm", &()).await;
    assert_eq!(field(&form, "plan")["kind"], "choice");
    assert_eq!(field(&form, "experience")["options"], serde_json::json!(["new", "hobby", "pro"]));
    assert_eq!(field(&form, "newsletter")["value"], false);

    for (name, value) in [
        ("name", "Ada Lovelace"),
        ("email", "ada@example.org"),
        ("plan", "Premium"),
        ("experience", "pro"),
        ("newsletter", "on"),
        ("comments", "Line one\nLine two"),
    ] {
        lumen.call("SetField", &(name, value)).await;
    }
    let state = lumen.call("GetState", &()).await;
    assert_eq!((state["form"]["dirty"].clone(), state["unsaved"].clone()), (true.into(), true.into()));
    let form = lumen.call("GetForm", &()).await;
    assert_eq!(field(&form, "name")["value"], "Ada Lovelace");
    assert_eq!(field(&form, "newsletter")["value"], true);
    assert_eq!(field(&form, "experience")["value"], "pro");

    // Nonsense is refused with a reason, and changes nothing.
    for (name, value, why) in [
        ("nope", "x", "no field"),
        ("experience", "expert", "choose one of: new, hobby, pro"),
        ("plan", "Gold", "choose one of: Basic, Standard, Premium"),
        ("newsletter", "maybe", "not on or off"),
    ] {
        let e = lumen.try_call("SetField", &(name, value)).await.unwrap_err();
        assert!(e.contains(why), "{e}");
    }

    // Saving writes the answers into the file where any reader sees them…
    lumen.call("Save", &()).await;
    let state = lumen.wait(|s| s["busy"] == false && s["unsaved"] == false).await;
    assert!(state["notice"]["text"].as_str().unwrap().contains("Saved"), "{state}");
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.windows(12).any(|w| w == b"Ada Lovelace"), "the name is in the file");
    // …and opening the file afresh finds them there.
    lumen.settled("Open", &(paths(&[file]), "")).await;
    let form = lumen.call("GetForm", &()).await;
    assert_eq!(field(&form, "email")["value"], "ada@example.org");
    assert_eq!(field(&form, "plan")["value"], "Premium");
    assert_eq!(field(&form, "experience")["value"], "pro");
    assert_eq!(field(&form, "newsletter")["value"], true);
    assert_eq!(field(&form, "comments")["value"], "Line one\nLine two");
    assert_eq!(lumen.call("GetState", &()).await["form"]["dirty"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn draws_annotations_on_a_pdf_and_saves_them_as_real_ones() {
    if !enabled("draws_annotations_on_a_pdf_and_saves_them_as_real_ones") {
        return;
    }
    let lumen = Lumen::start("annotate", &[]).await;
    let file = work_copy(&lumen, fixture("pages.pdf"), "pages.pdf");
    lumen.settled("Open", &(paths(std::slice::from_ref(&file)), "")).await;

    // Drawing needs markup on, and says so.
    let e = lumen.try_call("Annotate", &("rectangle", 1u32, 10.0, 10.0, 50.0, 50.0, "", "")).await.unwrap_err();
    assert!(e.contains("turn markup on"), "{e}");
    lumen.call("SetMarkup", &(true,)).await;
    lumen.wait(|s| s["markup_open"] == true && s["markup"].is_object() && s["busy"] == false).await;

    let shapes: [(&str, u32, [f64; 4], &str, &str); 7] = [
        ("rectangle", 1, [72.0, 72.0, 200.0, 140.0], "", "#d62828"),
        ("ellipse", 1, [220.0, 72.0, 320.0, 140.0], "", ""),
        ("line", 1, [72.0, 170.0, 320.0, 170.0], "", ""),
        ("arrow", 1, [72.0, 200.0, 320.0, 260.0], "", ""),
        ("pen", 1, [72.0, 280.0, 320.0, 340.0], "", ""),
        ("highlight", 1, [72.0, 360.0, 320.0, 380.0], "", ""),
        ("text", 2, [72.0, 100.0, 260.0, 140.0], "Hello from the control interface", ""),
    ];
    for (kind, page, [x0, y0, x1, y1], text, color) in shapes {
        lumen.call("Annotate", &(kind, page, x0, y0, x1, y1, text, color)).await;
    }
    let listed = lumen.call("GetAnnotations", &()).await;
    let items = listed["annotations"].as_array().unwrap();
    assert_eq!(items.len(), 7, "{listed}");
    let kinds: Vec<&str> = items.iter().map(|a| a["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["rectangle", "ellipse", "line", "arrow", "pen", "highlight", "text"]);
    assert_eq!(items[0]["color"], "#d62828");
    assert_eq!(items[6]["page"], 2);
    assert_eq!(items[6]["text"], "Hello from the control interface");
    let state = lumen.call("GetState", &()).await;
    assert_eq!((state["markup"]["annotations"].clone(), state["markup"]["dirty"].clone()), (7.into(), true.into()));

    for (kind, page, color, why) in [
        ("rectangle", 9u32, "", "pages 1 to 5"),
        ("rectangle", 0, "", "counted from 1"),
        ("star", 1, "", "unknown kind"),
        ("rectangle", 1, "red", "not a colour"),
    ] {
        let e = lumen.try_call("Annotate", &(kind, page, 0.0, 0.0, 9.0, 9.0, "", color)).await.unwrap_err();
        assert!(e.contains(why), "{e}");
    }

    // Saved, they are annotations any reader knows…
    lumen.call("Save", &()).await;
    lumen.wait(|s| s["busy"] == false && s["unsaved"] == false).await;
    let doc = lopdf::Document::load(&file).unwrap();
    let pages = doc.get_pages();
    let subtypes = |page: u32| -> Vec<String> {
        doc.get_page_annotations(pages[&page])
            .unwrap()
            .iter()
            .map(|a| String::from_utf8_lossy(a.get(b"Subtype").unwrap().as_name().unwrap()).into_owned())
            .collect()
    };
    let first = subtypes(1);
    for want in ["Square", "Circle", "Line", "Ink", "Highlight"] {
        assert!(first.iter().any(|s| s == want), "{want} missing from {first:?}");
    }
    assert_eq!(subtypes(2), ["FreeText"]);
    // …and Lumen finds them again when it opens the file.
    lumen.settled("Open", &(paths(&[file]), "")).await;
    lumen.call("SetMarkup", &(true,)).await;
    let state = lumen.wait(|s| s["markup"].is_object() && s["busy"] == false).await;
    assert_eq!(state["markup"]["annotations"], 7, "{state}");
    assert_eq!(state["markup"]["dirty"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_a_picture_and_saves_it() {
    if !enabled("edits_a_picture_and_saves_it") {
        return;
    }
    let lumen = Lumen::start("edit", &[]).await;
    let file = work_copy(&lumen, fixture("pictures").join("g.png"), "p.png");
    let (w, h) = image::image_dimensions(&file).unwrap();
    let state = lumen.settled("Open", &(paths(std::slice::from_ref(&file)), "")).await;
    assert_eq!(state["edit"]["original"], serde_json::json!([w, h]));
    assert_eq!(state["edit"]["dirty"], false);

    let size = |s: &Value| (s["edit"]["width"].as_u64().unwrap() as u32, s["edit"]["height"].as_u64().unwrap() as u32);
    let step = |op: &'static str, arg: &'static str| {
        let lumen = &lumen;
        async move {
            lumen.call("Edit", &(op, arg)).await;
            lumen.wait(|s| s["busy"] == false).await
        }
    };
    // A quarter turn swaps the sides; undo and redo go back and forth; reset forgets it all.
    assert_eq!(size(&step("rotate-right", "").await), (h, w));
    assert_eq!(size(&step("undo", "").await), (w, h));
    assert_eq!(size(&step("redo", "").await), (h, w));
    let state = step("reset", "").await;
    assert_eq!((size(&state), state["edit"]["dirty"].clone()), ((w, h), false.into()));

    // Crop to a rectangle, then resize, then change the colours.
    assert_eq!(size(&step("crop", "2,2,20,12").await), (20, 12));
    assert_eq!(size(&step("resize", "10x6").await), (10, 6));
    assert_eq!(size(&step("resize", "200%").await), (20, 12));
    assert_eq!(size(&step("resize", "10x6").await), (10, 6));
    let state = step("adjust", "exposure=0.5,contrast=0.2").await;
    assert_eq!(
        (size(&state), state["edit"]["dirty"].clone(), state["unsaved"].clone()),
        ((10, 6), true.into(), true.into())
    );

    for (op, arg, why) in [
        ("crop", "1,2,3", "x,y,w,h"),
        ("crop", "9999,0,5,5", "must lie inside"),
        ("resize", "big", "WxH"),
        ("adjust", "sparkle=1", "unknown adjustment"),
        ("tilt", "", "unknown edit"),
    ] {
        let e = lumen.try_call("Edit", &(op, arg)).await.unwrap_err();
        assert!(e.contains(why), "{e}");
    }

    // Saving in place changes the file itself, and the picture is shown again from it…
    lumen.call("Save", &()).await;
    let state = lumen.wait(|s| s["busy"] == false && s["unsaved"] == false).await;
    assert_eq!(image::image_dimensions(&file).unwrap(), (10, 6));
    assert_eq!(state["edit"]["original"], serde_json::json!([10, 6]), "{state}");
    let e = lumen.try_call("Save", &()).await.unwrap_err();
    assert!(e.contains("nothing to save"), "{e}");
    // …and a copy keeps the file as it is, taking the new edit with it.
    assert_eq!(size(&step("rotate-right", "").await), (6, 10));
    let copy = lumen.dir.join("copy.png");
    lumen.call("SaveCopy", &(copy.display().to_string(),)).await;
    noticed(&lumen, "Saved a copy").await;
    assert_eq!(image::image_dimensions(&copy).unwrap(), (6, 10));
    assert_eq!(image::image_dimensions(&file).unwrap(), (10, 6), "the file is untouched");
}

#[tokio::test(flavor = "multi_thread")]
async fn exports_pages_and_pictures() {
    if !enabled("exports_pages_and_pictures") {
        return;
    }
    let lumen = Lumen::start("export", &[]).await;
    let pdf = work_copy(&lumen, fixture("pages.pdf"), "pages.pdf");
    let picture = work_copy(&lumen, fixture("pictures").join("g.png"), "g.png");
    lumen.settled("Open", &(paths(std::slice::from_ref(&pdf)), "")).await;
    lumen.call("GotoPage", &(2u32,)).await;
    lumen.wait(|s| s["page"] == 2).await;

    // The page in view as a PNG, at two resolutions.
    let low = lumen.dir.join("low.png");
    let high = lumen.dir.join("high.png");
    lumen.call("Export", &("png", low.display().to_string(), 72u32)).await;
    lumen.wait(|s| s["busy"] == false).await;
    lumen.call("Export", &("png", high.display().to_string(), 150u32)).await;
    lumen.wait(|s| s["busy"] == false).await;
    let (lw, lh) = image::image_dimensions(&low).unwrap();
    let (hw, hh) = image::image_dimensions(&high).unwrap();
    assert!(lw > 100 && lh > lw, "a portrait page: {lw}×{lh}");
    assert!((f64::from(hw) / f64::from(lw) - 150.0 / 72.0).abs() < 0.02, "{lw}→{hw}");
    assert!((f64::from(hh) / f64::from(lh) - 150.0 / 72.0).abs() < 0.02, "{lh}→{hh}");

    // The page as a one-page PDF.
    let one = lumen.dir.join("one.pdf");
    lumen.call("Export", &("pdf", one.display().to_string(), 0u32)).await;
    lumen.wait(|s| s["busy"] == false).await;
    assert_eq!(lopdf::Document::load(&one).unwrap().get_pages().len(), 1);

    // A picture as a PDF.
    lumen.settled("Open", &(paths(&[picture]), "")).await;
    let made = lumen.dir.join("picture.pdf");
    lumen.call("Export", &("picture-pdf", made.display().to_string(), 0u32)).await;
    lumen.wait(|s| s["busy"] == false).await;
    assert_eq!(lopdf::Document::load(&made).unwrap().get_pages().len(), 1);

    for (kind, dpi, why) in [("png", 100u32, "dpi must be one of"), ("docx", 72, "unknown export")] {
        let e = lumen.try_call("Export", &(kind, "/tmp/never.out", dpi)).await.unwrap_err();
        assert!(e.contains(why), "{e}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reloads_a_file_that_another_program_changes() {
    if !enabled("reloads_a_file_that_another_program_changes") {
        return;
    }
    let lumen = Lumen::start("reload", &[]).await;
    let file = work_copy(&lumen, fixture("pages.pdf"), "book.pdf");
    let state = lumen.settled("Open", &(paths(std::slice::from_ref(&file)), "")).await;
    assert_eq!((state["kind"].clone(), state["pages"].clone()), ("pdf".into(), 5.into()));
    lumen.call("GotoPage", &(3u32,)).await;
    lumen.wait(|s| s["page"] == 3).await;

    // Written over in place, the way `cp` does it…
    std::fs::copy(fixture("form.pdf"), &file).unwrap();
    let state = lumen.wait(|s| s["pages"] == 1 && s["busy"] == false).await;
    assert!(state["notice"]["text"].as_str().unwrap().contains("Reloaded"), "{state}");
    // …and replaced by a rename, the way most editors save.
    let again = lumen.dir.join("book.tmp");
    std::fs::copy(fixture("pages.pdf"), &again).unwrap();
    std::fs::rename(&again, &file).unwrap();
    let state = lumen.wait(|s| s["pages"] == 5 && s["busy"] == false).await;
    assert_eq!(state["kind"], "pdf");
    assert_eq!(state["unsaved"], false);

    // With something unsaved, the change is reported and the work is kept.
    lumen.settled("Open", &(paths(std::slice::from_ref(&file)), "")).await;
    lumen.call("SetMarkup", &(true,)).await;
    lumen.wait(|s| s["markup"].is_object() && s["busy"] == false).await;
    lumen.call("Annotate", &("rectangle", 1u32, 10.0, 10.0, 90.0, 60.0, "", "")).await;
    std::fs::copy(fixture("form.pdf"), &file).unwrap();
    let state = lumen.wait(|s| s["notice"]["text"].as_str().is_some_and(|t| t.contains("changed on disk"))).await;
    assert_eq!(state["pages"], 5, "still the document that was being marked up");
    assert_eq!(state["markup"]["annotations"], 1);
}
