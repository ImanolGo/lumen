//! The desktop's thumbnail cache.

use std::path::{Path, PathBuf};

/// `~/.cache/thumbnails`.
pub fn shared_cache() -> Option<PathBuf> {
    Some(super::dirs::shared_cache()?.join("thumbnails"))
}

/// `file:///home/me/a%20b.png`, written the way GLib writes it (so other programs, which
/// name the thumbnail after the MD5 of this text, find the same file as we do).
pub fn file_uri(path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let mut uri = String::from("file://");
    for &b in super::fs::bytes(path) {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => uri.push(b as char),
            b'/' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':' | b'@' => {
                uri.push(b as char)
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    Some(uri)
}
