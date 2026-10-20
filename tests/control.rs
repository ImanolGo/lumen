//! End-to-end tests through the control interface (`io.github.imanolgo.Lumen.Control1`).
//!
//! Each test starts its own D-Bus session bus and Broadway display, runs the real `lumen`
//! binary in them with throwaway settings, and drives it only over D-Bus. A test is skipped
//! (with a note) where `dbus-daemon` or `gtk4-broadwayd` is not installed.

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use relm4::gtk::{gio, glib, prelude::*};
use serde_json::Value;

const NAME: &str = "io.github.imanolgo.Lumen";
const PATH: &str = "/io/github/imanolgo/Lumen/Control";
const INTERFACE: &str = "io.github.imanolgo.Lumen.Control1";

/// A display number per test, so tests can run in parallel.
static NEXT_DISPLAY: AtomicU32 = AtomicU32::new(0);

struct Lumen {
    children: Vec<Child>,
    bus: gio::DBusConnection,
    dir: PathBuf,
}

impl Drop for Lumen {
    fn drop(&mut self) {
        for child in self.children.iter_mut().rev() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn installed(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

impl Lumen {
    /// Start Lumen with `files`, or `None` if the tools to run it headless are missing.
    fn start(test: &str, files: &[PathBuf]) -> Option<Self> {
        if !installed("dbus-daemon") || !installed("gtk4-broadwayd") {
            eprintln!("skipping {test}: dbus-daemon or gtk4-broadwayd is not installed");
            return None;
        }
        let dir = std::env::temp_dir().join(format!("lumen-control-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut children = Vec::new();

        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        children.push(daemon);

        // Broadway's socket lives in the runtime directory, which containers often lack.
        let runtime = dir.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let display = format!(
            ":{}",
            40 + (std::process::id() % 400) + 10 * NEXT_DISPLAY.fetch_add(1, Ordering::SeqCst)
        );
        children.push(
            Command::new("gtk4-broadwayd")
                .arg(&display)
                .env("XDG_RUNTIME_DIR", &runtime)
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(dir.join("broadway.log")).unwrap())
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(Duration::from_millis(300));

        let log = std::fs::File::create(dir.join("lumen.log")).unwrap();
        children.push(
            Command::new(env!("CARGO_BIN_EXE_lumen"))
                .args(files)
                .env("DBUS_SESSION_BUS_ADDRESS", &address)
                .env("GDK_BACKEND", "broadway")
                .env("BROADWAY_DISPLAY", &display)
                .env("XDG_CONFIG_HOME", dir.join("config"))
                .env("XDG_DATA_HOME", dir.join("data"))
                .env("XDG_CACHE_HOME", dir.join("cache"))
                .env("GSETTINGS_BACKEND", "memory")
                .env("XDG_RUNTIME_DIR", &runtime)
                .stdout(Stdio::null())
                .stderr(log)
                .spawn()
                .unwrap(),
        );

        let bus = gio::DBusConnection::for_address_sync(
            &address,
            gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
                | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
            None,
            gio::Cancellable::NONE,
        )
        .unwrap();
        let lumen = Self { children, bus, dir };
        // Wait until Lumen is on the bus and has opened what it was given.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(state) = lumen.try_call("GetState", ().to_variant())
                && (files.is_empty() || !state["file"].is_null())
                && state["busy"] == false
            {
                break;
            }
            if Instant::now() >= deadline {
                let read =
                    |name: &str| std::fs::read_to_string(lumen.dir.join(name)).unwrap_or_default();
                panic!(
                    "Lumen did not start\n--- lumen.log ---\n{}\n--- broadway.log ---\n{}\n--- last answer ---\n{:?}",
                    read("lumen.log"),
                    read("broadway.log"),
                    lumen.try_call("GetState", ().to_variant()),
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Some(lumen)
    }

    fn try_call(&self, method: &str, args: glib::Variant) -> Result<Value, String> {
        let reply = self
            .bus
            .call_sync(
                Some(NAME),
                PATH,
                INTERFACE,
                method,
                Some(&args),
                Some(glib::VariantTy::new("(s)").unwrap()),
                gio::DBusCallFlags::NONE,
                60_000,
                gio::Cancellable::NONE,
            )
            .map_err(|e| e.to_string())?;
        let (text,) = reply.get::<(String,)>().unwrap();
        Ok(serde_json::from_str(&text).unwrap())
    }

    fn call(&self, method: &str, args: glib::Variant) -> Value {
        self.try_call(method, args)
            .unwrap_or_else(|e| panic!("{method} failed: {e}"))
    }

    fn state(&self) -> Value {
        self.call("GetState", ().to_variant())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// An `a{sv}` from pairs.
fn options(pairs: &[(&str, glib::Variant)]) -> glib::Variant {
    let dict = glib::VariantDict::new(None);
    for (key, value) in pairs {
        dict.insert_value(key, value);
    }
    dict.end()
}

fn no_options() -> glib::Variant {
    options(&[])
}

/// The arguments of a call, keeping each one's type (`a{sv}` stays a dictionary).
fn args(items: &[glib::Variant]) -> glib::Variant {
    glib::Variant::tuple_from_iter(items.iter().cloned())
}

/// A `v` argument holding `value`.
fn boxed(value: glib::Variant) -> glib::Variant {
    glib::Variant::from_variant(&value)
}

fn paths(list: &[&Path]) -> glib::Variant {
    list.iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .to_variant()
}

#[test]
fn opens_reports_and_navigates_a_pdf() {
    let Some(lumen) = Lumen::start("navigate", &[fixture("pages.pdf")]) else {
        return;
    };
    let state = lumen.state();
    assert_eq!(state["kind"], "pdf");
    assert_eq!(state["page_count"], 5);
    assert_eq!(state["dirty"], false);

    let pages = lumen.call("ListPages", ().to_variant());
    assert_eq!(pages.as_array().unwrap().len(), 5);
    assert_eq!(pages[0]["width"], 300.0);

    assert_eq!(lumen.call("GotoPage", (3u32,).to_variant())["page"], 3);
    assert!(lumen.try_call("GotoPage", (9u32,).to_variant()).is_err());

    // Opening another file replaces it.
    let state = lumen.call(
        "Open",
        args(&[paths(&[&fixture("links.pdf")]), no_options()]),
    );
    assert_eq!(state["page_count"], 2);
    assert!(state["file"].as_str().unwrap().ends_with("links.pdf"));
}

#[test]
fn fills_in_a_form_and_saves_it() {
    let Some(lumen) = Lumen::start("forms", &[fixture("form.pdf")]) else {
        return;
    };
    let fields = lumen.call("ListFields", ().to_variant());
    let id_of = |kind: &str| {
        fields
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} field"))["id"]
            .as_i64()
            .unwrap() as i32
    };
    let (text, check, choice) = (id_of("text"), id_of("check"), id_of("choice"));

    let state = lumen.call(
        "FillField",
        args(&[text.to_variant(), boxed("Ada Lovelace".to_variant())]),
    );
    assert_eq!(state["dirty"], true);
    lumen.call(
        "FillField",
        args(&[check.to_variant(), boxed(true.to_variant())]),
    );
    lumen.call(
        "FillField",
        args(&[choice.to_variant(), boxed(2u32.to_variant())]),
    );

    // Saving over the original needs to be asked for.
    let refused = lumen.try_call("Save", args(&["".to_variant(), no_options()]));
    assert!(
        refused.unwrap_err().contains("overwrite"),
        "saving in place needs overwrite"
    );

    let copy = lumen.path("filled.pdf");
    let state = lumen.call(
        "Save",
        args(&[copy.display().to_string().to_variant(), no_options()]),
    );
    assert!(
        state["file"].as_str().unwrap().ends_with("filled.pdf"),
        "{state}"
    );
    assert_eq!(state["dirty"], false);

    // The saved file holds the answers.
    let fields = lumen.call("ListFields", ().to_variant());
    let value = |id: i32| {
        fields
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id)
            .unwrap()["value"]
            .clone()
    };
    assert_eq!(value(text), "Ada Lovelace");
    assert_eq!(value(check), true);
    assert_eq!(value(choice)["selected"], 2);
}

#[test]
fn annotates_undoes_and_keeps_annotations_in_the_file() {
    let Some(lumen) = Lumen::start("annotate", &[fixture("pages.pdf")]) else {
        return;
    };
    let rect = r##"{"type": "rect", "x": 20, "y": 30, "w": 100, "h": 50, "color": "#e01b24"}"##;
    let added = lumen.call("Annotate", (1u32, rect).to_variant());
    let ink = r#"{"type": "ink", "strokes": [[[10, 10], [40, 30]], [[15, 25], [35, 22]]]}"#;
    lumen.call("Annotate", (1u32, ink).to_variant());
    assert!(
        lumen
            .try_call("Annotate", (1u32, r#"{"type": "star"}"#).to_variant())
            .is_err()
    );

    let listed = lumen.call("ListAnnotations", (1u32,).to_variant());
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["type"], "rect");
    assert_eq!(listed[0]["color"], "#e01b24");
    assert_eq!(listed[1]["strokes"].as_array().unwrap().len(), 2);

    lumen.call(
        "DeleteAnnotation",
        (1u32, added["id"].as_u64().unwrap()).to_variant(),
    );
    assert_eq!(
        lumen
            .call("ListAnnotations", (1u32,).to_variant())
            .as_array()
            .unwrap()
            .len(),
        1
    );
    lumen.call("Undo", ().to_variant());
    assert_eq!(
        lumen
            .call("ListAnnotations", (1u32,).to_variant())
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let copy = lumen.path("annotated.pdf");
    lumen.call(
        "Save",
        args(&[copy.display().to_string().to_variant(), no_options()]),
    );
    let listed = lumen.call("ListAnnotations", (1u32,).to_variant());
    assert_eq!(
        listed.as_array().unwrap().len(),
        2,
        "annotations survive saving and reopening"
    );
    assert_eq!(listed[1]["type"], "ink", "a signature stays one annotation");
}

#[test]
fn changes_pages_and_undoes_it() {
    let Some(lumen) = Lumen::start("pages", &[fixture("pages.pdf")]) else {
        return;
    };
    let state = lumen.call(
        "ChangePages",
        args(&[
            "delete".to_variant(),
            vec![0u32, 1].to_variant(),
            no_options(),
        ]),
    );
    assert_eq!(state["page_count"], 3);
    let state = lumen.call(
        "ChangePages",
        args(&[
            "rotate".to_variant(),
            vec![0u32].to_variant(),
            options(&[("degrees", 90i32.to_variant())]),
        ]),
    );
    assert_eq!(state["page_count"], 3);
    let pages = lumen.call("ListPages", ().to_variant());
    assert_eq!(
        (pages[0]["width"].as_f64(), pages[0]["height"].as_f64()),
        (Some(200.0), Some(300.0))
    );
    assert!(
        lumen
            .try_call(
                "ChangePages",
                args(&[
                    "delete".to_variant(),
                    vec![0u32, 1, 2].to_variant(),
                    no_options()
                ])
            )
            .is_err(),
        "a PDF keeps at least one page"
    );

    lumen.call("Undo", ().to_variant());
    let state = lumen.call("Undo", ().to_variant());
    assert_eq!(state["page_count"], 5);
    assert_eq!(state["can_undo"], false);
}

#[test]
fn edits_a_picture_and_saves_a_copy() {
    let Some(lumen) = Lumen::start("picture", &[]) else {
        return;
    };
    let picture = lumen.path("picture.png");
    image::RgbImage::from_pixel(300, 200, image::Rgb([200, 120, 40]))
        .save(&picture)
        .unwrap();
    let state = lumen.call("Open", args(&[paths(&[&picture]), no_options()]));
    assert_eq!(state["kind"], "image");
    assert_eq!(state["size"], serde_json::json!([300, 200]));

    let state = lumen.call(
        "EditImage",
        args(&["rotate-right".to_variant(), no_options()]),
    );
    assert_eq!(state["size"], serde_json::json!([200, 300]));
    let crop = options(&[
        ("x", 0u32.to_variant()),
        ("y", 0u32.to_variant()),
        ("w", 100u32.to_variant()),
        ("h", 150u32.to_variant()),
    ]);
    let state = lumen.call("EditImage", args(&["crop".to_variant(), crop]));
    assert_eq!(state["dirty"], true);

    // Opening something else would lose the edits: refused unless asked for.
    let refused = lumen.try_call(
        "Open",
        args(&[paths(&[&fixture("pages.pdf")]), no_options()]),
    );
    assert!(refused.unwrap_err().contains("discard"));

    let copy = lumen.path("edited.png");
    let state = lumen.call(
        "Save",
        args(&[copy.display().to_string().to_variant(), no_options()]),
    );
    assert_eq!(state["dirty"], false);
    let saved = image::open(&copy).unwrap();
    assert_eq!((saved.width(), saved.height()), (100, 150));
}

#[test]
fn protected_pdfs_open_with_their_password_and_never_ask() {
    let Some(lumen) = Lumen::start("password", &[]) else {
        return;
    };
    let path = paths(&[&fixture("encrypted.pdf")]);
    let refused = lumen.try_call("Open", args(&[path.clone(), no_options()]));
    assert!(refused.unwrap_err().contains("password"));
    let wrong = options(&[("password", "nope".to_variant())]);
    assert!(
        lumen
            .try_call("Open", args(&[path.clone(), wrong]))
            .is_err()
    );
    let right = options(&[("password", "secret".to_variant())]);
    let state = lumen.call("Open", args(&[path, right]));
    assert_eq!(state["kind"], "pdf");
}

#[test]
fn searches_and_exports() {
    let Some(lumen) = Lumen::start("search", &[fixture("selection.pdf")]) else {
        return;
    };
    let found = lumen.call("Search", ("line",).to_variant());
    assert_eq!(found["count"], 2, "{found}");
    assert_eq!(found["hits"][0]["page"], 0);

    let out = lumen.path("page.pdf");
    let pages = options(&[("pages", vec![1u32].to_variant())]);
    lumen.call(
        "Export",
        args(&[
            "pages".to_variant(),
            out.display().to_string().to_variant(),
            pages,
        ]),
    );
    let second = lumen.call("Open", args(&[paths(&[&out]), no_options()]));
    assert_eq!(second["page_count"], 1);
    // Writing over a file needs to be asked for.
    let again = options(&[("pages", vec![0u32].to_variant())]);
    assert!(
        lumen
            .try_call(
                "Export",
                args(&[
                    "pages".to_variant(),
                    out.display().to_string().to_variant(),
                    again
                ])
            )
            .is_err()
    );
}
