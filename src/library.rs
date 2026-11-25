//! Which files to browse: classifying paths, listing folders and ordering them.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// What kind of document a path holds, judged by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Image,
    Svg,
    Pdf,
}

const RASTER: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "gif", "webp", "bmp", "tif", "tiff", "ico", "qoi", "tga", "pnm", "pbm", "pgm",
    "ppm", "pam", "jxl", "jp2", "j2k", "j2c", "jpx", "exr",
];

impl Kind {
    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "pdf" => Some(Self::Pdf),
            "svg" | "svgz" => Some(Self::Svg),
            e if RASTER.contains(&e) => Some(Self::Image),
            #[cfg(feature = "heif")]
            "heic" | "heif" | "hif" | "avif" => Some(Self::Image),
            _ => None,
        }
    }
}

/// Natural ordering by file name, ignoring case: `photo2` < `photo10`.
pub fn natural_cmp(a: &Path, b: &Path) -> Ordering {
    let name = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    natord::compare_ignore_case(&name(a), &name(b))
}

/// The supported, non-hidden files directly inside `dir`, naturally sorted.
pub fn list_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .filter(|p| Kind::of(p).is_some() && !p.is_dir())
        .collect();
    files.sort_by(|a, b| natural_cmp(a, b));
    files
}

/// Expands what the user opened into the files to browse, plus the index to show first.
///
/// * One file: every supported file in its folder, starting at that file, so the
///   arrow keys flip through its neighbours.
/// * Folders: their supported files, naturally sorted.
/// * Several files: those files, in the given order.
///
/// Blocking; call it off the UI thread.
pub fn expand(paths: &[PathBuf]) -> (Vec<PathBuf>, usize) {
    // As they were given, only made absolute: what a link leads to may have another name and folder.
    let paths: Vec<PathBuf> = paths.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect();

    if let [single] = paths.as_slice()
        && single.is_file()
        && Kind::of(single).is_some()
    {
        if let Some(parent) = single.parent() {
            let siblings = list_dir(parent);
            if let Some(i) = siblings.iter().position(|p| p == single) {
                return (siblings, i);
            }
        }
        return (vec![single.clone()], 0);
    }

    let mut files = Vec::new();
    for path in &paths {
        if path.is_dir() {
            files.extend(list_dir(path));
        } else if Kind::of(path).is_some() && !files.contains(path) {
            files.push(path.clone());
        }
    }
    (files, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn a_link_is_browsed_where_it_is() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("store")).unwrap();
        std::fs::create_dir_all(dir.join("album")).unwrap();
        std::fs::write(dir.join("store/ab12cd"), b"x").unwrap();
        std::fs::write(dir.join("album/z.png"), b"x").unwrap();
        std::os::unix::fs::symlink("../store/ab12cd", dir.join("album/a.png")).unwrap();
        let (files, start) = expand(&[dir.join("album/a.png")]);
        assert_eq!((files, start), (vec![dir.join("album/a.png"), dir.join("album/z.png")], 0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn kinds() {
        assert_eq!(Kind::of(Path::new("a/B.JPG")), Some(Kind::Image));
        assert_eq!(Kind::of(Path::new("doc.pdf")), Some(Kind::Pdf));
        assert_eq!(Kind::of(Path::new("logo.svg")), Some(Kind::Svg));
        assert_eq!(Kind::of(Path::new("notes.txt")), None);
        assert_eq!(Kind::of(Path::new("noext")), None);
    }

    #[test]
    fn natural_order() {
        let mut v = vec![PathBuf::from("p10.png"), PathBuf::from("P2.png"), PathBuf::from("p1.png")];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["p1.png", "P2.png", "p10.png"].map(PathBuf::from));
    }

    #[test]
    fn expand_folder_and_single_file() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["b10.png", "b2.jpg", "a.pdf", ".hidden.png", "skip.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let (files, start) = expand(std::slice::from_ref(&dir));
        let names: Vec<_> = files.iter().map(|p| p.file_name().unwrap().to_owned()).collect();
        assert_eq!(names, ["a.pdf", "b2.jpg", "b10.png"]);
        assert_eq!(start, 0);

        let (files, start) = expand(&[dir.join("b10.png")]);
        assert_eq!(files.len(), 3);
        assert_eq!(start, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
