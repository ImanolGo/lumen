//! The XDG base folders.

use std::path::PathBuf;

use crate::i18n::tr;

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn xdg(variable: &str, default: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        // The standard says to ignore relative ones.
        .filter(|p| p.is_absolute())
        .or_else(|| Some(home()?.join(default)))
}

/// Lumen's preferences: `~/.config/lumen-rs`.
pub fn config() -> Option<PathBuf> {
    Some(xdg("XDG_CONFIG_HOME", ".config")?.join("lumen-rs"))
}

/// What Lumen remembers between runs: `~/.local/state/lumen-rs`.
pub fn state() -> Option<PathBuf> {
    Some(xdg("XDG_STATE_HOME", ".local/state")?.join("lumen-rs"))
}

/// Signatures: `~/.local/share/lumen`, which the GTK Lumen shares.
pub fn data() -> Option<PathBuf> {
    Some(xdg("XDG_DATA_HOME", ".local/share")?.join("lumen"))
}

/// Everybody's data, which holds the trash: `~/.local/share`.
pub(super) fn shared_data() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share")
}

/// Everybody's cache: `~/.cache`.
pub(super) fn shared_cache() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// The folders the file chooser offers: home, the usual user folders, the whole computer.
pub fn places() -> Vec<(String, PathBuf)> {
    let Some(home) = home() else { return vec![(tr("Computer"), "/".into())] };
    let mut out = vec![(tr("Home"), home.clone())];
    // ~/.config/user-dirs.dirs knows the (possibly translated) names.
    let config = xdg("XDG_CONFIG_HOME", ".config").unwrap_or_default();
    let dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    for (key, default) in
        [("DESKTOP", "Desktop"), ("DOCUMENTS", "Documents"), ("DOWNLOAD", "Downloads"), ("PICTURES", "Pictures")]
    {
        let path = dirs
            .lines()
            .find_map(|l| l.strip_prefix(&format!("XDG_{key}_DIR=")))
            .map(|v| PathBuf::from(v.trim_matches('"').replace("$HOME", &home.to_string_lossy())))
            .unwrap_or_else(|| home.join(default));
        if path.is_dir() && path != home && !out.iter().any(|(_, p)| *p == path) {
            out.push((path.file_name().unwrap_or_default().to_string_lossy().into_owned(), path));
        }
    }
    out.push((tr("Computer"), "/".into()));
    out
}
