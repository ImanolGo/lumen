//! Preferences, kept in `~/.config/lumen-rs/settings.toml` (or under `$XDG_CONFIG_HOME`).
//!
//! The file is read leniently: a missing file, a missing key, an unknown key or a value of
//! the wrong kind never stops Lumen from starting; each such setting just keeps its default.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

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
            cache_mb: 64,
            preload_megapixels: 4,
        }
    }
}

/// What the preferences dialog offers.
pub const SLIDESHOW_SECONDS: [u32; 6] = [2, 3, 5, 8, 12, 20];
pub const CACHE_MB: [u32; 4] = [32, 64, 128, 256];
pub const PRELOAD_MEGAPIXELS: [u32; 4] = [0, 4, 12, 24];

impl Settings {
    /// `~/.config/lumen-rs/settings.toml`.
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("lumen-rs").join("settings.toml"))
    }

    pub fn load() -> Self {
        let Some(text) = Self::path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Self::default() };
        Self::parse(&text)
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

    pub fn to_toml(&self) -> String {
        let mut text = String::from("# Lumen preferences. Lumen rewrites this file when you change them in the app.\n");
        text.push_str(&toml::to_string_pretty(self).unwrap_or_default());
        text
    }

    /// Writes the file (all at once, so a crash never leaves half of it).
    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let temp = path.with_extension("toml.tmp");
            std::fs::write(&temp, self.to_toml())?;
            std::fs::rename(&temp, &path)
        };
        if let Err(e) = write() {
            tracing::warn!("could not save the settings to {}: {e}", path.display());
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
