//! Working out which files to browse: ordering them, and listing folders that are opened or dropped.

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

/// The files to show for `paths`: files as given (only pictures and PDFs if
/// `only_supported`), and for folders the pictures and PDFs directly inside them,
/// naturally sorted. Blocking: call it off the main thread.
pub fn expand(paths: &[PathBuf], only_supported: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut inside = list_dir(path).unwrap_or_default();
            inside.sort_by(|a, b| natural_cmp(a, b));
            files.extend(inside);
        } else if !only_supported || is_supported(path) {
            files.push(path.clone());
        }
    }
    files
}

/// Whether `path` looks like a picture or a PDF, judging by its name.
pub fn is_supported(path: &Path) -> bool {
    let (content_type, _) = gio::content_type_guess(Some(path), None);
    gio::content_type_get_mime_type(&content_type)
        .is_some_and(|mime| DocumentKind::from_mime(&mime).is_some())
}

/// The pictures and PDFs in the folder `dir`, unsorted.
fn list_dir(dir: &Path) -> Option<Vec<PathBuf>> {
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

    #[test]
    fn folders_expand_to_their_pictures_and_pdfs() {
        let dir = std::env::temp_dir().join(format!("lumen-expand-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for name in ["b10.png", "b2.jpg", "notes.txt", ".hidden.png", "doc.pdf"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let loose = PathBuf::from("/elsewhere/x.txt");

        let files = expand(&[dir.clone(), loose.clone()], false);
        let names: Vec<_> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_str().unwrap())
            .collect();
        // Natural order inside the folder; the loose file is kept as given.
        assert_eq!(names, ["b2.jpg", "b10.png", "doc.pdf", "x.txt"]);
        // Dropped files that are not pictures or PDFs are left out.
        assert_eq!(expand(&[loose], true), Vec::<PathBuf>::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
