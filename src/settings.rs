//! Preferences, kept in `~/.config/lumen-rs/settings.toml` on Linux.
//!
//! The file is read leniently: a missing file, a missing key, an unknown key or a value of
//! the wrong kind never stops Lumen from starting; each such setting just keeps its default.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the desktop's light or dark setting.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PdfFit {
    /// As wide as the window (never past a comfortable size).
    #[default]
    Width,
    /// A whole page at a time.
    Page,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    pub theme: Theme,
    /// Seconds each picture or page stays up in a slideshow.
    pub slideshow_seconds: u32,
    /// How a PDF is fitted when it opens.
    pub pdf_fit: PdfFit,
    /// Whether the sidebar is shown when Lumen starts.
    pub sidebar_open: bool,
    /// The zoom control and previous/next arrows that appear over the picture.
    pub floating_controls: bool,
    /// Use the desktop's file and print dialogs when there is a portal (else Lumen's own).
    pub portals: bool,
    /// PDF night mode.
    pub night_mode: bool,
    /// Decode pictures in a sandboxed child process.
    pub sandbox: bool,
    /// ← on the first file goes to the last, and → on the last goes to the first.
    pub wrap_around: bool,
    /// Minimise and maximise buttons in the header, where the window has no frame.
    pub window_buttons: bool,
    /// Dragging over a picture selects a part of it (else it moves the picture).
    pub select_on_drag: bool,
    /// The interface's language ("en", "es"); empty follows the system.
    pub language: String,
    /// Use the desktop's thumbnail cache (`~/.cache/thumbnails`) and add to it.
    pub thumbnail_cache: bool,
    /// Megabytes of decoded pictures kept for flipping back and forth.
    pub cache_mb: u32,
    /// Neighbouring pictures up to this many megapixels are decoded ahead (0 turns it off).
    pub preload_megapixels: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            slideshow_seconds: 5,
            pdf_fit: PdfFit::Width,
            sidebar_open: true,
            floating_controls: true,
            portals: true,
            night_mode: false,
            sandbox: true,
            wrap_around: false,
            window_buttons: false,
            select_on_drag: true,
            language: String::new(),
            thumbnail_cache: true,
            cache_mb: 64,
            preload_megapixels: 4,
        }
    }
}

/// What the preferences dialog offers.
pub const SLIDESHOW_SECONDS: [u32; 6] = [2, 3, 5, 8, 12, 20];
pub const CACHE_MB: [u32; 4] = [32, 64, 128, 256];
pub const PRELOAD_MEGAPIXELS: [u32; 4] = [0, 4, 12, 24];

const HEADER: &str = "# Lumen preferences. Lumen rewrites this file when you change them in the app.\n";

/// The settings as they were when Lumen started.
static LOADED: OnceLock<toml::Table> = OnceLock::new();

impl Settings {
    /// `~/.config/lumen-rs/settings.toml`.
    pub fn path() -> Option<PathBuf> {
        Some(crate::platform::dirs::config()?.join("settings.toml"))
    }

    pub fn load() -> Self {
        let Some(text) = Self::path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Self::default() };
        let settings = Self::parse(&text);
        let _ = LOADED.set(settings.table());
        settings
    }

    /// Reads settings from TOML, setting by setting: whatever cannot be read keeps its default.
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        let table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("the settings file could not be read ({e}); using the defaults");
                return out;
            }
        };
        fn get<T: serde::de::DeserializeOwned>(table: &toml::Table, key: &str, into: &mut T) {
            if let Some(value) = table.get(key) {
                match value.clone().try_into() {
                    Ok(v) => *into = v,
                    Err(e) => tracing::warn!("setting `{key}` ignored: {e}"),
                }
            }
        }
        get(&table, "theme", &mut out.theme);
        get(&table, "slideshow_seconds", &mut out.slideshow_seconds);
        get(&table, "pdf_fit", &mut out.pdf_fit);
        get(&table, "sidebar_open", &mut out.sidebar_open);
        get(&table, "floating_controls", &mut out.floating_controls);
        get(&table, "portals", &mut out.portals);
        get(&table, "night_mode", &mut out.night_mode);
        get(&table, "sandbox", &mut out.sandbox);
        get(&table, "wrap_around", &mut out.wrap_around);
        get(&table, "window_buttons", &mut out.window_buttons);
        get(&table, "select_on_drag", &mut out.select_on_drag);
        get(&table, "language", &mut out.language);
        get(&table, "thumbnail_cache", &mut out.thumbnail_cache);
        get(&table, "cache_mb", &mut out.cache_mb);
        get(&table, "preload_megapixels", &mut out.preload_megapixels);
        out.clamp();
        out
    }

    /// Keeps numbers in a sensible range, whatever the file says.
    fn clamp(&mut self) {
        self.slideshow_seconds = self.slideshow_seconds.clamp(1, 600);
        self.cache_mb = self.cache_mb.clamp(8, 4096);
        self.preload_megapixels = self.preload_megapixels.min(200);
    }

    #[cfg(test)]
    pub fn to_toml(&self) -> String {
        format!("{HEADER}{}", toml::to_string_pretty(self).unwrap_or_default())
    }

    /// Writes the file (all at once, so a crash never leaves half of it). Tests of the app
    /// never write the user's real file.
    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
        static KNOWN: Mutex<Option<toml::Table>> = Mutex::new(None);
        if let Some(path) = Self::path() {
            let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
            // Before the first save: what this run started from.
            let before = known.take().unwrap_or_else(|| LOADED.get().cloned().unwrap_or_default());
            self.save_to(&path, &before);
            *known = Some(self.table());
        }
    }

    fn table(&self) -> toml::Table {
        toml::Table::try_from(self).unwrap_or_default()
    }

    /// Writes the file at `path`, changing only what differs from `before`: another window may have
    /// changed the rest.
    pub fn save_to(&self, path: &std::path::Path, before: &toml::Table) {
        let disk = std::fs::read_to_string(path).ok().and_then(|text| text.parse::<toml::Table>().ok());
        let mut table = disk.unwrap_or_default();
        for (key, value) in self.table() {
            if before.get(&key) != Some(&value) || !table.contains_key(&key) {
                table.insert(key, value);
            }
        }
        let text = format!("{HEADER}{}", toml::to_string_pretty(&table).unwrap_or_default());
        if let Err(e) = crate::fsio::write_quickly(path, text.as_bytes()) {
            tracing::warn!("could not save the settings to {}: {e:#}", path.display());
        }
    }

    /// Bytes of decoded pictures to keep.
    pub fn cache_bytes(&self) -> usize {
        self.cache_mb as usize * (1 << 20)
    }

    /// Pixels up to which neighbours are decoded ahead.
    pub fn preload_pixels(&self) -> u64 {
        u64::from(self.preload_megapixels) * 1_000_000
    }
}

/// Set from the preferences; read wherever the portals might be used.
static PORTALS: AtomicBool = AtomicBool::new(true);

pub fn set_portals(on: bool) {
    PORTALS.store(on, Ordering::Relaxed);
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code, reason = "until docs/improvement-plan.md package 7"))]
pub fn portals_allowed() -> bool {
    PORTALS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_empty_file_gives_the_defaults() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("# nothing here\n"), Settings::default());
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let s = Settings {
            theme: Theme::Dark,
            slideshow_seconds: 12,
            pdf_fit: PdfFit::Page,
            sidebar_open: false,
            floating_controls: false,
            portals: false,
            night_mode: true,
            sandbox: false,
            wrap_around: true,
            window_buttons: true,
            select_on_drag: false,
            language: "es".into(),
            thumbnail_cache: false,
            cache_mb: 128,
            preload_megapixels: 0,
        };
        let text = s.to_toml();
        assert!(text.contains("theme = \"dark\"") && text.contains("pdf_fit = \"page\""), "{text}");
        assert_eq!(Settings::parse(&text), s);
    }

    #[test]
    fn bad_values_and_unknown_keys_only_affect_themselves() {
        let s = Settings::parse(
            "theme = \"purple\"\nslideshow_seconds = 9\nfuture_option = true\nsidebar_open = \"yes\"\npdf_fit = \"page\"\n",
        );
        assert_eq!(s.theme, Theme::System, "an unknown theme keeps the default");
        assert_eq!(s.slideshow_seconds, 9);
        assert!(s.sidebar_open, "a value of the wrong kind keeps the default");
        assert_eq!(s.pdf_fit, PdfFit::Page);
    }

    #[test]
    fn saving_to_a_path_makes_the_folder_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-settings-{}", std::process::id()));
        let path = dir.join("nested/lumen-rs/settings.toml");
        let settings = Settings { slideshow_seconds: 9, theme: Theme::Light, ..Settings::default() };
        settings.save_to(&path, &toml::Table::new());
        assert_eq!(Settings::parse(&std::fs::read_to_string(&path).unwrap()), settings);
        assert!(!path.with_extension("toml.tmp").exists());
        // Saving again replaces it.
        let before = settings.table();
        Settings { slideshow_seconds: 3, ..settings }.save_to(&path, &before);
        assert_eq!(Settings::parse(&std::fs::read_to_string(&path).unwrap()).slideshow_seconds, 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_toml_gives_the_defaults() {
        assert_eq!(Settings::parse("this is = = not toml ["), Settings::default());
    }

    #[test]
    fn numbers_are_kept_in_range() {
        let s = Settings::parse("slideshow_seconds = 0\ncache_mb = 999999\npreload_megapixels = 5000\n");
        assert_eq!((s.slideshow_seconds, s.cache_mb, s.preload_megapixels), (1, 4096, 200));
        let s = Settings::parse("slideshow_seconds = -3\ncache_mb = 1\n");
        assert_eq!(s.slideshow_seconds, 5, "a negative number is not a u32: default");
        assert_eq!(s.cache_mb, 8);
        assert_eq!(Settings { cache_mb: 64, preload_megapixels: 4, ..Settings::default() }.preload_pixels(), 4_000_000);
    }
}
