//! What Lumen remembers between runs besides the preferences: the size of the window and the
//! files opened recently, with the page each PDF was left at. Kept in
//! `~/.local/state/lumen-rs/state.toml` (or under `$XDG_STATE_HOME`). It is only ever a
//! convenience: a missing or damaged file is the same as an empty one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many recent files are kept.
const MAX_RECENT: usize = 30;
/// The window is never restored smaller or larger than this.
const MIN_WINDOW: (u32, u32) = (520, 360);
const MAX_WINDOW: (u32, u32) = (16_000, 16_000);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub path: PathBuf,
    /// The page a PDF was left at, counting from 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The window's size in logical pixels when Lumen last closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<(u32, u32)>,
    /// Most recent first.
    #[serde(default)]
    pub recent: Vec<Recent>,
}

impl State {
    /// `~/.local/state/lumen-rs/state.toml`.
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
        Some(base.join("lumen-rs").join("state.toml"))
    }

    /// Reads the file, forgetting recent files that are gone.
    pub fn load() -> Self {
        let Some(text) = Self::path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Self::default() };
        let mut state = Self::parse(&text);
        state.recent.retain(|r| r.path.exists());
        state
    }

    /// Reads TOML; anything unreadable gives an empty state.
    pub fn parse(text: &str) -> Self {
        let mut state: Self = toml::from_str(text).unwrap_or_default();
        state.window =
            state.window.map(|(w, h)| (w.clamp(MIN_WINDOW.0, MAX_WINDOW.0), h.clamp(MIN_WINDOW.1, MAX_WINDOW.1)));
        state.recent.truncate(MAX_RECENT);
        state
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Writes the file all at once (tests never write the real one).
    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
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
            tracing::warn!("could not save the state to {}: {e}", path.display());
        }
    }

    /// Notes that `path` was opened: it moves to the front, keeping the page it had.
    pub fn touch(&mut self, path: &Path) {
        let page = match self.recent.iter().position(|r| r.path == path) {
            Some(i) => self.recent.remove(i).page,
            None => None,
        };
        self.recent.insert(0, Recent { path: path.to_path_buf(), page });
        self.recent.truncate(MAX_RECENT);
    }

    /// Notes the page a PDF is at (a file not in the list is left out of it).
    pub fn set_page(&mut self, path: &Path, page: usize) {
        if let Some(r) = self.recent.iter_mut().find(|r| r.path == path) {
            r.page = Some(page);
        }
    }

    pub fn page_of(&self, path: &Path) -> Option<usize> {
        self.recent.iter().find(|r| r.path == path)?.page
    }

    /// Forgets a file (it could not be opened, or the person cleared the list).
    pub fn forget(&mut self, path: &Path) {
        self.recent.retain(|r| r.path != path);
    }

    pub fn clear_recent(&mut self) {
        self.recent.clear();
    }

    /// The window size to start with, if one was saved.
    pub fn window_size(&self) -> Option<(f32, f32)> {
        self.window.map(|(w, h)| (w as f32, h as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_come_most_recent_first_without_repeats() {
        let mut s = State::default();
        for name in ["a", "b", "c", "a"] {
            s.touch(Path::new(name));
        }
        let names: Vec<_> = s.recent.iter().map(|r| r.path.to_str().unwrap()).collect();
        assert_eq!(names, ["a", "c", "b"]);
    }

    #[test]
    fn a_pdfs_page_is_kept_when_it_is_opened_again() {
        let mut s = State::default();
        s.touch(Path::new("book.pdf"));
        s.set_page(Path::new("book.pdf"), 41);
        s.touch(Path::new("other.pdf"));
        s.touch(Path::new("book.pdf"));
        assert_eq!(s.page_of(Path::new("book.pdf")), Some(41));
        assert_eq!(s.page_of(Path::new("other.pdf")), None);
        // A page for a file that is not listed is not invented.
        s.set_page(Path::new("unlisted.pdf"), 3);
        assert_eq!(s.page_of(Path::new("unlisted.pdf")), None);
    }

    #[test]
    fn the_list_is_capped_and_can_be_cleared_or_trimmed() {
        let mut s = State::default();
        for i in 0..100 {
            s.touch(Path::new(&format!("f{i}")));
        }
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0].path, Path::new("f99"));
        s.forget(Path::new("f99"));
        assert_eq!(s.recent[0].path, Path::new("f98"));
        s.clear_recent();
        assert!(s.recent.is_empty());
    }

    #[test]
    fn it_round_trips_and_survives_nonsense() {
        let mut s = State { window: Some((1000, 700)), ..State::default() };
        s.touch(Path::new("/x/y.pdf"));
        s.set_page(Path::new("/x/y.pdf"), 7);
        assert_eq!(State::parse(&s.to_toml()), s);
        assert_eq!(State::parse("this is [not toml"), State::default());
        assert_eq!(State::parse("recent = 5"), State::default());
        // A silly window size is brought into range.
        assert_eq!(State::parse("window = [1, 2000000]").window, Some(MIN_WINDOW.0).map(|w| (w, MAX_WINDOW.1)));
    }
}
