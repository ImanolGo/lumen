//! Toolkit-light document loading.
//!
//! Detects what a file is and decodes it. Phase 2 adds a real `PdfDocument`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use relm4::gtk::{gio, prelude::*};

use crate::{image::ImageDocument, pdf::{self, PdfInfo}};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Image,
    Pdf,
}

impl DocumentKind {
    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime {
            "application/pdf" => Some(Self::Pdf),
            m if m.starts_with("image/") => Some(Self::Image),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocumentInfo {
    pub path: PathBuf,
    pub kind: DocumentKind,
    /// Human readable, e.g. "4032 × 3024 px" or "12 pages".
    pub summary: String,
}

impl DocumentInfo {
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

pub fn detect_kind(path: &Path) -> Result<DocumentKind> {
    let file = gio::File::for_path(path);
    let info = file
        .query_info(
            gio::FILE_ATTRIBUTE_STANDARD_CONTENT_TYPE,
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .with_context(|| format!("cannot read {}", path.display()))?;
    let content_type = info.content_type().context("unknown content type")?;
    let mime = gio::content_type_get_mime_type(&content_type)
        .map(|m| m.to_string())
        .unwrap_or_else(|| content_type.to_string());

    match DocumentKind::from_mime(&mime) {
        Some(kind) => Ok(kind),
        None => bail!("unsupported file type: {mime}"),
    }
}

/// A decoded file, ready to show.
#[derive(Debug)]
pub struct Opened {
    pub info: DocumentInfo,
    pub image: Option<ImageDocument>,
    pub pdf: Option<PdfInfo>,
}

/// Detect, decode and summarize `path`. Blocking: call it off the main thread.
pub fn open(path: &Path) -> Result<Opened> {
    let kind = detect_kind(path)?;
    let (summary, image, pdf) = match kind {
        DocumentKind::Image => {
            let image = ImageDocument::load(path)?;
            let (w, h) = image.display_size();
            (format!("{w} × {h} px"), Some(image), None)
        }
        DocumentKind::Pdf => {
            let info = pdf::load_info(path)?;
            let summary = match info.n_pages {
                1 => "1 page".to_owned(),
                n => format!("{n} pages"),
            };
            (summary, None, Some(info))
        }
    };

    Ok(Opened {
        info: DocumentInfo {
            path: path.to_owned(),
            kind,
            summary,
        },
        image,
        pdf,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_mapping() {
        assert_eq!(
            DocumentKind::from_mime("application/pdf"),
            Some(DocumentKind::Pdf)
        );
        assert_eq!(
            DocumentKind::from_mime("image/png"),
            Some(DocumentKind::Image)
        );
        assert_eq!(
            DocumentKind::from_mime("image/svg+xml"),
            Some(DocumentKind::Image)
        );
        assert_eq!(DocumentKind::from_mime("text/plain"), None);
    }
}
