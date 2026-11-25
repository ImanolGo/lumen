//! What Lumen remembers between runs besides the preferences: the size of the window and the
//! files opened recently, with the page each PDF was left at. Kept in
//! `~/.local/state/lumen-rs/state.toml` (or under `$XDG_STATE_HOME`). It is only ever a
//! convenience: a missing or damaged file is the same as an empty one.

use std::collections::HashSet;
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
    /// When it was last opened (milliseconds since 1970), to tell whose news is newer.
    #[serde(default)]
    pub at: u64,
    /// It is not there now (a disk that is not mounted, say): kept, but not listed.
    #[serde(skip)]
    pub missing: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The window's size in logical pixels when Lumen last closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<(u32, u32)>,
    /// Most recent first.
    #[serde(default)]
    pub recent: Vec<Recent>,
    /// What this run did to the list, for merging it with what other windows wrote.
    #[serde(skip)]
    forgotten: HashSet<PathBuf>,
    #[serde(skip)]
    touched: HashSet<PathBuf>,
    /// The list was cleared at this time.
    #[serde(skip)]
    cleared: u64,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
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

    /// Reads the file, noting which recent files are not there.
    pub fn load() -> Self {
        let Some(text) = Self::path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Self::default() };
        let mut state = Self::parse(&text);
        for r in &mut state.recent {
            r.missing = !r.path.exists();
        }
        state
    }

    /// The recent files that are there.
    pub fn listed(&self) -> impl Iterator<Item = &Recent> {
        self.recent.iter().filter(|r| !r.missing)
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
        // TOML is text: a path that is not cannot be written, and must not stop the rest.
        let recent = self.recent.iter().filter(|r| r.path.to_str().is_some()).cloned().collect();
        let writable = Self { window: self.window, recent, ..Self::default() };
        toml::to_string_pretty(&writable).unwrap_or_default()
    }

    /// This state with what other windows have written to the file since it was read.
    fn merged(&self, disk: Self) -> Self {
        let mut recent: Vec<Recent> = Vec::new();
        for r in &self.recent {
            // What this run only read, and is not in the file any more, another window took out.
            if self.touched.contains(&r.path) || disk.recent.iter().any(|d| d.path == r.path) {
                recent.push(r.clone());
            }
        }
        for d in disk.recent {
            let gone = self.forgotten.contains(&d.path) || d.at < self.cleared;
            match recent.iter_mut().find(|r| r.path == d.path) {
                Some(r) if d.at > r.at => (r.page, r.at) = (d.page, d.at),
                Some(_) => {}
                None if gone => {}
                None => recent.push(d),
            }
        }
        // Stable: those that were never dated keep their order, at the end.
        recent.sort_by_key(|r| std::cmp::Reverse(r.at));
        recent.truncate(MAX_RECENT);
        Self { window: self.window.or(disk.window), recent, ..Self::default() }
    }

    /// Writes the file all at once (tests never write the real one).
    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
        let Some(path) = Self::path() else { return };
        let disk = std::fs::read_to_string(&path).map(|text| Self::parse(&text));
        let merged = disk.map_or_else(|_| self.clone(), |disk| self.merged(disk));
        if let Err(e) = crate::fsio::write_quickly(&path, merged.to_toml().as_bytes()) {
            tracing::warn!("could not save the state to {}: {e:#}", path.display());
        }
    }

    /// Notes that `path` was opened: it moves to the front, keeping the page it had.
    pub fn touch(&mut self, path: &Path) {
        let page = match self.recent.iter().position(|r| r.path == path) {
            Some(i) => self.recent.remove(i).page,
            None => None,
        };
        self.recent.insert(0, Recent { path: path.to_path_buf(), page, at: now(), missing: false });
        self.forgotten.remove(path);
        self.touched.insert(path.to_path_buf());
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
        self.forgotten.insert(path.to_path_buf());
    }

    pub fn clear_recent(&mut self) {
        self.recent.clear();
        self.cleared = now();
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
    fn a_path_that_is_not_text_does_not_empty_the_file() {
        use std::os::unix::ffi::OsStrExt;
        let mut s = State { window: Some((1000, 700)), ..State::default() };
        s.touch(Path::new("/x/y.pdf"));
        s.touch(Path::new(std::ffi::OsStr::from_bytes(b"/x/\xe9.pdf")));
        let back = State::parse(&s.to_toml());
        assert_eq!(back.window, Some((1000, 700)));
        assert_eq!(back.recent.len(), 1);
    }

    #[test]
    fn files_that_are_not_there_are_kept_but_not_listed() {
        let mut s = State::parse("[[recent]]\npath = \"/not/mounted.pdf\"\npage = 3\n");
        s.recent[0].missing = true;
        assert_eq!(s.listed().count(), 0);
        assert!(s.to_toml().contains("/not/mounted.pdf"));
        s.touch(Path::new("/not/mounted.pdf"));
        assert_eq!((s.listed().count(), s.recent[0].page), (1, Some(3)));
    }

    #[test]
    fn two_windows_do_not_undo_each_other() {
        let old = "[[recent]]\npath = \"/a\"\n[[recent]]\npath = \"/b\"\n[[recent]]\npath = \"/c\"\n";
        let paths = |s: &State| s.recent.iter().map(|r| r.path.to_str().unwrap().to_owned()).collect::<Vec<_>>();
        let (mut one, mut two) = (State::parse(old), State::parse(old));
        one.touch(Path::new("/one"));
        one.forget(Path::new("/b"));
        let disk = one.merged(State::parse(old));
        assert_eq!(paths(&disk), ["/one", "/a", "/c"]);
        std::thread::sleep(std::time::Duration::from_millis(3));
        two.touch(Path::new("/two"));
        let disk = two.merged(State::parse(&disk.to_toml()));
        assert_eq!(paths(&disk), ["/two", "/one", "/a", "/c"], "what one forgot stays forgotten");
        // Clearing the list takes everything that was there, but not what is opened later.
        std::thread::sleep(std::time::Duration::from_millis(3));
        one.clear_recent();
        assert!(one.merged(State::parse(&disk.to_toml())).recent.is_empty());
        std::thread::sleep(std::time::Duration::from_millis(3));
        two.touch(Path::new("/later"));
        assert_eq!(paths(&one.merged(two.clone())), ["/later"]);
    }

    #[test]
    fn it_round_trips_and_survives_nonsense() {
        let mut s = State { window: Some((1000, 700)), ..State::default() };
        s.touch(Path::new("/x/y.pdf"));
        s.set_page(Path::new("/x/y.pdf"), 7);
        let back = State::parse(&s.to_toml());
        assert_eq!((back.window, back.recent), (s.window, s.recent.clone()));
        assert_eq!(State::parse("this is [not toml"), State::default());
        assert_eq!(State::parse("recent = 5"), State::default());
        // A silly window size is brought into range.
        assert_eq!(State::parse("window = [1, 2000000]").window, Some(MIN_WINDOW.0).map(|w| (w, MAX_WINDOW.1)));
    }
}
