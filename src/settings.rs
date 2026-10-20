//! Preferences and remembered state.
//!
//! Plain TOML files in the user's config and data folders. A file that is
//! missing, unreadable or damaged just means defaults: preferences must never
//! stop the app from starting.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// How a new image is first shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultZoom {
    #[default]
    FitToWindow,
    ActualSize,
}

/// What lies behind an image or page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Background {
    /// The window's own colour, which follows the system theme.
    #[default]
    System,
    Dark,
    Light,
    /// A grey chequerboard behind transparent images.
    Checkerboard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub default_zoom: DefaultZoom,
    pub background: Background,
    /// Animate zoom changes instead of jumping.
    pub smooth_zoom: bool,
    /// Seconds each picture (or page) stays up in a slideshow.
    pub slideshow_seconds: u32,
    /// Remember the window size, the sidebar and the page a PDF was left on.
    pub remember_state: bool,
    /// Opening a single picture also lets you browse the other pictures in its folder.
    pub browse_folder: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_zoom: DefaultZoom::FitToWindow,
            background: Background::System,
            smooth_zoom: true,
            slideshow_seconds: 5,
            remember_state: true,
            browse_folder: true,
        }
    }
}

/// How many PDFs the last page is remembered for.
const REMEMBERED_FILES: usize = 200;

/// What is remembered between runs (as opposed to chosen in preferences).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    pub window_width: i32,
    pub window_height: i32,
    pub maximized: bool,
    pub sidebar_open: bool,
    /// The page each PDF was left on, most recently used first.
    last_pages: Vec<(String, usize)>,
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            window_width: 1000,
            window_height: 720,
            maximized: false,
            sidebar_open: true,
            last_pages: Vec::new(),
        }
    }
}

impl SavedState {
    pub fn last_page(&self, path: &Path) -> Option<usize> {
        let key = path.to_string_lossy();
        self.last_pages
            .iter()
            .find(|(p, _)| *p == key)
            .map(|(_, page)| *page)
    }

    /// Remember that `path` was left on `page`. Page 0 is the default, so it is not kept.
    pub fn set_last_page(&mut self, path: &Path, page: usize) {
        let key = path.to_string_lossy().into_owned();
        self.last_pages.retain(|(p, _)| *p != key);
        if page > 0 {
            self.last_pages.insert(0, (key, page));
            self.last_pages.truncate(REMEMBERED_FILES);
        }
    }
}

/// Read a TOML file, falling back to the default for anything wrong with it.
pub(crate) fn load<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    let Ok(text) = fs::read_to_string(path) else {
        return T::default();
    };
    toml::from_str(&text).unwrap_or_else(|err| {
        tracing::warn!(path = %path.display(), "ignoring unreadable settings: {err}");
        T::default()
    })
}

/// Write a TOML file atomically (a crash mid-write must not leave half a file).
pub(crate) fn save<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(value).map_err(std::io::Error::other)?;
    let temp = path.with_extension("toml.tmp");
    fs::write(&temp, text)?;
    fs::rename(&temp, path)
}

/// Where the files live.
pub struct Paths {
    pub settings: PathBuf,
    pub state: PathBuf,
}

impl Paths {
    pub fn user() -> Self {
        Self {
            settings: relm4::gtk::glib::user_config_dir()
                .join("lumen")
                .join("settings.toml"),
            state: relm4::gtk::glib::user_data_dir()
                .join("lumen")
                .join("state.toml"),
        }
    }
}

pub struct Store {
    paths: Paths,
    pub settings: Settings,
    pub state: SavedState,
}

impl Store {
    pub fn load(paths: Paths) -> Self {
        Self {
            settings: load(&paths.settings),
            state: load(&paths.state),
            paths,
        }
    }

    pub fn save_settings(&self) {
        if let Err(err) = save(&self.paths.settings, &self.settings) {
            tracing::warn!("cannot save preferences: {err}");
        }
    }

    /// Save what is remembered, unless the user turned remembering off.
    pub fn save_state(&self) {
        if !self.settings.remember_state {
            return;
        }
        if let Err(err) = save(&self.paths.state, &self.state) {
            tracing::warn!("cannot save the window state: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lumen-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn paths(dir: &Path) -> Paths {
        Paths {
            settings: dir.join("config/settings.toml"),
            state: dir.join("data/state.toml"),
        }
    }

    #[test]
    fn missing_files_mean_defaults() {
        let dir = scratch("missing");
        let store = Store::load(paths(&dir));
        assert_eq!(store.settings, Settings::default());
        assert_eq!(store.state, SavedState::default());
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let dir = scratch("roundtrip");
        let mut store = Store::load(paths(&dir));
        store.settings.default_zoom = DefaultZoom::ActualSize;
        store.settings.background = Background::Checkerboard;
        store.settings.smooth_zoom = false;
        store.settings.slideshow_seconds = 12;
        store.state.window_width = 1280;
        store.state.maximized = true;
        store.state.set_last_page(Path::new("/a/book.pdf"), 41);
        store.save_settings();
        store.save_state();

        let again = Store::load(paths(&dir));
        assert_eq!(again.settings, store.settings);
        assert_eq!(again.state, store.state);
        assert_eq!(again.state.last_page(Path::new("/a/book.pdf")), Some(41));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn damaged_or_partial_files_never_break_startup() {
        let dir = scratch("damaged");
        fs::create_dir_all(dir.join("config")).unwrap();
        fs::write(dir.join("config/settings.toml"), "this is { not toml").unwrap();
        assert_eq!(Store::load(paths(&dir)).settings, Settings::default());

        // A file with only some keys keeps the defaults for the rest, and unknown keys are ignored.
        fs::write(
            dir.join("config/settings.toml"),
            "slideshow_seconds = 9\nfuture_option = true\n",
        )
        .unwrap();
        let store = Store::load(paths(&dir));
        assert_eq!(store.settings.slideshow_seconds, 9);
        assert!(store.settings.smooth_zoom);

        // A wrong value for a known key falls back to defaults rather than panicking.
        fs::write(dir.join("config/settings.toml"), "background = \"neon\"\n").unwrap();
        assert_eq!(Store::load(paths(&dir)).settings, Settings::default());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remembering_can_be_switched_off() {
        let dir = scratch("off");
        let mut store = Store::load(paths(&dir));
        store.settings.remember_state = false;
        store.state.window_width = 555;
        store.save_state();
        assert!(!dir.join("data/state.toml").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn last_pages_are_recent_first_and_bounded() {
        let mut state = SavedState::default();
        state.set_last_page(Path::new("/a.pdf"), 3);
        state.set_last_page(Path::new("/b.pdf"), 7);
        state.set_last_page(Path::new("/a.pdf"), 4);
        assert_eq!(
            state.last_pages[0],
            ("/a.pdf".to_owned(), 4),
            "updated entries move to the front"
        );
        assert_eq!(state.last_pages.len(), 2);

        // The first page is the default and is not worth a slot.
        state.set_last_page(Path::new("/a.pdf"), 0);
        assert_eq!(state.last_page(Path::new("/a.pdf")), None);

        for i in 0..(REMEMBERED_FILES + 50) {
            state.set_last_page(Path::new(&format!("/book{i}.pdf")), 1);
        }
        assert_eq!(state.last_pages.len(), REMEMBERED_FILES);
        assert_eq!(
            state.last_page(Path::new("/book0.pdf")),
            None,
            "the oldest were dropped"
        );
        assert_eq!(
            state.last_page(Path::new(&format!("/book{}.pdf", REMEMBERED_FILES + 49))),
            Some(1)
        );
    }
}
