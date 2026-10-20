//! Toolkit-light document loading.
//!
//! Detects what a file is and decodes it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use relm4::gtk::{gio, prelude::*};

use crate::{
    image::ImageDocument,
    pdf::{self, PdfError, PdfInfo, PdfSource},
};

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
        None => bail!(crate::i18n::tr_fmt(
            "Unsupported file type: {type}",
            &[("type", &mime)]
        )),
    }
}

/// A decoded file, ready to show.
#[derive(Debug)]
pub struct Opened {
    pub info: DocumentInfo,
    pub image: Option<ImageDocument>,
    pub pdf: Option<PdfInfo>,
}

/// Why a file could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// An encrypted PDF; `retry` is set when a password was given and was wrong.
    PasswordRequired {
        retry: bool,
    },
    Other(String),
}

impl OpenError {
    fn other(err: anyhow::Error) -> Self {
        Self::Other(format!("{err:#}"))
    }

    /// Text for the error page.
    pub fn message(&self) -> String {
        match self {
            Self::PasswordRequired { retry: true } => crate::i18n::tr("The password is incorrect"),
            Self::PasswordRequired { retry: false } => {
                crate::i18n::tr("This PDF is password protected")
            }
            Self::Other(message) => message.clone(),
        }
    }
}

/// Detect, decode and summarize `path`. Blocking: call it off the main thread.
pub fn open(path: &Path, password: Option<&str>) -> Result<Opened, OpenError> {
    let kind = detect_kind(path).map_err(OpenError::other)?;
    let (summary, image, pdf) = match kind {
        DocumentKind::Image => {
            let image = ImageDocument::load(path).map_err(OpenError::other)?;
            let (w, h) = image.display_size();
            (
                crate::i18n::dimensions(w as u32, h as u32),
                Some(image),
                None,
            )
        }
        DocumentKind::Pdf => {
            let info =
                pdf::load_info(&PdfSource::new(path, password)).map_err(|err| match err {
                    PdfError::PasswordRequired => OpenError::PasswordRequired {
                        retry: password.is_some(),
                    },
                    PdfError::Other(err) => OpenError::other(err),
                })?;
            let summary = crate::i18n::ntr("{n} page", "{n} pages", info.n_pages as u32);
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

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// `tests/fixtures/encrypted.pdf` has the user password "secret".
    #[test]
    fn encrypted_pdf_asks_for_the_password() {
        let path = fixture("encrypted.pdf");
        assert!(matches!(
            open(&path, None),
            Err(OpenError::PasswordRequired { retry: false })
        ));
        assert!(matches!(
            open(&path, Some("wrong")),
            Err(OpenError::PasswordRequired { retry: true })
        ));
        let opened = open(&path, Some("secret")).expect("the right password opens it");
        let pdf = opened.pdf.expect("is a PDF");
        assert_eq!(pdf.n_pages, 2);
        assert_eq!(pdf.password.as_deref(), Some("secret"));
    }

    #[test]
    fn plain_pdf_needs_no_password() {
        let opened = open(&fixture("selection.pdf"), None).expect("opens");
        assert_eq!(opened.pdf.expect("is a PDF").password, None);
    }
}
