//! The control interface, tried against the real app on a private session bus.
//!
//! These open real windows on your desktop, so they only run when asked:
//! `LUMEN_CONTROL_TESTS=1 cargo test --test control`. (CI has no display to open them on.)

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const APP: &str = env!("CARGO_BIN_EXE_lumen-rs");
const NAME: &str = "io.github.imanolgo.LumenRs";
const PATH: &str = "/io/github/imanolgo/LumenRs/Control";
const IFACE: &str = "io.github.imanolgo.LumenRs.Control1";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// A private bus, and the app on it.
struct Lumen {
    children: Vec<Child>,
    bus: zbus::Connection,
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

impl Lumen {
    async fn start(test: &str, files: &[PathBuf]) -> Self {
        let dir = std::env::temp_dir().join(format!("lumen-rs-control-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon is needed for these tests");
        let mut address = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(daemon.stdout.take().unwrap()), &mut address).unwrap();
        let address = address.trim().to_owned();

        let log = std::fs::File::create(dir.join("lumen.log")).unwrap();
        let app = Command::new(APP)
            .args(files)
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .env("LUMEN_CONTROL", "1")
            .env("LUMEN_NO_PORTAL", "1")
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_DATA_HOME", dir.join("data"))
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let bus = zbus::connection::Builder::address(address.as_str()).build().await.unwrap();
        let lumen = Self { children: vec![daemon, app], bus, dir };

        // Wait until the app is on the bus and has opened what it was given.
        let deadline = Instant::now() + Duration::from_secs(30);
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

    async fn try_call<B: serde::Serialize + zbus::zvariant::DynamicType>(
        &self,
        method: &str,
        body: &B,
    ) -> Result<Value, String> {
        let reply =
            self.bus.call_method(Some(NAME), PATH, Some(IFACE), method, body).await.map_err(|e| e.to_string())?;
        let json: String = reply.body().deserialize().map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }

    async fn call<B: serde::Serialize + zbus::zvariant::DynamicType>(&self, method: &str, body: &B) -> Value {
        self.try_call(method, body).await.unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    /// Calls `method`, then waits until nothing is going on any more.
    async fn settled<B: serde::Serialize + zbus::zvariant::DynamicType>(&self, method: &str, body: &B) -> Value {
        self.call(method, body).await;
        self.wait(|s| s["busy"] == false).await
    }

    /// Polls the state until `done` says so.
    async fn wait(&self, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
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
