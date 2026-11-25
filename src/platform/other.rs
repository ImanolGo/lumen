//! Systems that have no services of their own here yet: everything says so.

#![allow(dead_code)]

pub const CAPS: super::Caps = super::Caps {
    sandbox: false,
    trash_undo: false,
    open_with: false,
    direct_print: false,
    shared_thumbnails: false,
    primary_selection: false,
    own_title_bar: false,
};

const NOT_YET: &str = "not available on this system yet";

pub mod dirs {
    use std::path::PathBuf;

    pub fn home() -> Option<PathBuf> {
        std::env::home_dir()
    }

    fn own(name: &str) -> Option<PathBuf> {
        Some(home()?.join(".lumen-rs").join(name))
    }

    pub fn config() -> Option<PathBuf> {
        own("config")
    }

    pub fn state() -> Option<PathBuf> {
        own("state")
    }

    pub fn data() -> Option<PathBuf> {
        own("data")
    }

    pub fn places() -> Vec<(String, PathBuf)> {
        home().map(|h| (crate::i18n::tr("Home"), h)).into_iter().collect()
    }
}

pub mod dialogs {
    use std::path::PathBuf;

    use anyhow::Result;

    use crate::platform::NoDialog;

    pub async fn choose(_folder: bool) -> Result<Vec<PathBuf>> {
        Err(NoDialog.into())
    }

    pub async fn save_as(_suggested: PathBuf) -> Result<Option<PathBuf>> {
        Err(NoDialog.into())
    }
}

pub mod shell {
    use std::path::PathBuf;

    use anyhow::{Result, bail};

    pub async fn open_uri(_uri: String) -> Result<()> {
        bail!(super::NOT_YET)
    }

    pub async fn show_in_folder(_path: PathBuf) -> Result<()> {
        bail!(super::NOT_YET)
    }

    pub async fn open_with(_path: PathBuf) -> Result<()> {
        bail!(super::NOT_YET)
    }
}

pub mod clipboard {
    pub fn copy_png(_png: Vec<u8>) -> Result<(), String> {
        Err(super::NOT_YET.into())
    }
}

pub mod watch {
    use std::path::PathBuf;

    use futures::SinkExt;
    use futures::channel::mpsc;

    /// No watching here: the caller looks now and then instead.
    pub fn changes(_path: PathBuf) -> impl futures::Stream<Item = Option<()>> {
        iced::stream::channel(1, async move |mut out: mpsc::Sender<Option<()>>| {
            let _ = out.send(None).await;
            std::future::pending::<()>().await;
        })
    }
}

pub mod trash {
    use std::path::{Path, PathBuf};

    use anyhow::{Result, bail};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Trashed {
        pub original: PathBuf,
    }

    #[cfg(test)]
    impl Trashed {
        pub fn for_test(original: PathBuf) -> Self {
            Self { original }
        }
    }

    pub fn trash(_path: &Path) -> Result<Trashed> {
        bail!(super::NOT_YET)
    }

    pub fn restore(_item: &Trashed) -> Result<()> {
        bail!(super::NOT_YET)
    }
}

pub mod print {
    use std::path::{Path, PathBuf};

    use anyhow::{Result, bail};

    use crate::print::{Connection, Outcome};

    pub async fn print(_path: PathBuf) -> Result<Outcome> {
        bail!(super::NOT_YET)
    }

    pub fn connect(_socket: &Path) -> Result<Box<dyn Connection>> {
        bail!(super::NOT_YET)
    }

    pub const SOCKETS: [&str; 0] = [];
}

pub mod confine {
    use std::path::PathBuf;

    use anyhow::Result;

    /// Nothing here shuts the decoder in yet: it is a process of its own, and no more.
    pub fn enter(_max_memory: u64) -> Result<()> {
        Ok(())
    }

    pub fn explain(_status: &std::process::ExitStatus) -> Option<&'static str> {
        None
    }

    pub fn worker_exe() -> std::io::Result<PathBuf> {
        std::env::current_exe()
    }

    pub fn escape(_test: &str) -> bool {
        false
    }
}

pub mod startup {
    pub fn before_window() {}

    pub fn window() -> iced::window::settings::PlatformSpecific {
        Default::default()
    }
}

pub mod thumbs {
    use std::path::{Path, PathBuf};

    pub fn shared_cache() -> Option<PathBuf> {
        None
    }

    pub fn file_uri(_path: &Path) -> Option<String> {
        None
    }
}

pub mod control {
    use crate::control::Shared;

    pub fn requests() -> impl futures::Stream<Item = Shared> {
        futures::stream::pending()
    }
}

#[cfg(unix)]
pub use super::unix_fs as fs;

#[cfg(not(unix))]
pub mod fs {
    use std::fs::{File, OpenOptions, Permissions};
    use std::io;
    use std::path::{Path, PathBuf};

    pub fn new_file(path: &Path, _mode: Option<&Permissions>) -> io::Result<File> {
        OpenOptions::new().write(true).create_new(true).open(path)
    }

    pub fn new_private_file(path: &Path) -> io::Result<File> {
        new_file(path, None)
    }

    pub fn new_private_dir(path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    pub fn private_dirs(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    pub fn private_temp() -> PathBuf {
        std::env::temp_dir()
    }

    pub fn sync_dir(_dir: &Path) {}
}
