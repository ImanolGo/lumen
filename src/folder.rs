//! Listing the browsable files next to an opened one.

use std::path::{Path, PathBuf};

use relm4::gtk::{gio, prelude::*};

use crate::document::DocumentKind;

/// Natural ordering by file name: `photo2` < `photo10`.
pub fn natural_cmp(a: &Path, b: &Path) -> std::cmp::Ordering {
    let name = |p: &Path| {
        p.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    natord::compare_ignore_case(&name(a), &name(b))
}

/// All images and PDFs in the directory of `path`, naturally sorted.
/// Always contains `path` itself. Blocking: call it off the main thread.
pub fn siblings(path: &Path) -> Vec<PathBuf> {
    let mut files = list_dir(path).unwrap_or_default();
    if !files.iter().any(|f| f == path) {
        files.push(path.to_owned());
    }
    files.sort_by(|a, b| natural_cmp(a, b));
    files
}

fn list_dir(path: &Path) -> Option<Vec<PathBuf>> {
    let dir = path.parent()?;
    let enumerator = gio::File::for_path(dir)
        .enumerate_children(
            "standard::name,standard::type,standard::fast-content-type",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()?;

    let files = enumerator
        .flatten()
        .filter(|info| info.file_type() != gio::FileType::Directory)
        .filter(|info| !info.name().to_string_lossy().starts_with('.'))
        .filter(|info| {
            info.attribute_string("standard::fast-content-type")
                .and_then(|c| gio::content_type_get_mime_type(&c))
                .is_some_and(|mime| DocumentKind::from_mime(&mime).is_some())
        })
        .map(|info| dir.join(info.name()))
        .collect();
    Some(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut files = [
            PathBuf::from("/a/photo10.jpg"),
            PathBuf::from("/a/photo2.jpg"),
            PathBuf::from("/a/Photo1.jpg"),
        ];
        files.sort_by(|a, b| natural_cmp(a, b));
        let names: Vec<_> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["Photo1.jpg", "photo2.jpg", "photo10.jpg"]);
    }
}
