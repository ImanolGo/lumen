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

/// Lumen's preferences: `~/.config/lumen`.
pub fn config() -> Option<PathBuf> {
    Some(moved(xdg("XDG_CONFIG_HOME", ".config")?, "lumen"))
}

/// What Lumen remembers between runs: `~/.local/state/lumen`.
pub fn state() -> Option<PathBuf> {
    Some(moved(xdg("XDG_STATE_HOME", ".local/state")?, "lumen"))
}

/// `base/name`, where versions called `lumen-rs` kept what is now kept there: moved over once.
fn moved(base: PathBuf, name: &str) -> PathBuf {
    let (old, new) = (base.join("lumen-rs"), base.join(name));
    if old.is_dir()
        && !new.exists()
        && let Err(e) = std::fs::rename(&old, &new)
    {
        tracing::warn!("could not move {} to {}: {e}", old.display(), new.display());
        return old;
    }
    new
}

/// Signatures: `~/.local/share/lumen`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_kept_as_lumen_rs_moves_over_once() {
        let base = std::env::temp_dir().join(format!("lumen-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("lumen-rs")).unwrap();
        std::fs::write(base.join("lumen-rs/settings.toml"), "theme = \"dark\"").unwrap();
        assert_eq!(moved(base.clone(), "lumen"), base.join("lumen"));
        assert!(base.join("lumen/settings.toml").is_file() && !base.join("lumen-rs").exists());
        // Never over what is there already.
        std::fs::create_dir_all(base.join("lumen-rs")).unwrap();
        assert_eq!(moved(base.clone(), "lumen"), base.join("lumen"));
        assert!(base.join("lumen-rs").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
