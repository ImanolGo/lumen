//! Image documents: decoded pixels plus where they came from.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use relm4::gtk::gdk;

#[derive(Debug, Clone)]
pub struct ImageDocument {
    pub path: PathBuf,
    pub texture: gdk::Texture,
}

impl ImageDocument {
    pub fn load(path: &Path) -> Result<Self> {
        let texture = gdk::Texture::from_filename(path)
            .with_context(|| format!("failed to decode {}", path.display()))?;
        Ok(Self {
            path: path.to_owned(),
            texture,
        })
    }
}
