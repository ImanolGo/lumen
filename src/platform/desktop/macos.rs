//! What only macOS does.

use std::path::PathBuf;

pub use crate::platform::unix_fs as fs;

pub const CAPS: crate::platform::Caps = crate::platform::Caps {
    sandbox: false,
    trash_undo: false,
    open_with: false,
    direct_print: true,
    shared_thumbnails: false,
    desktop_dialogs_optional: false,
    primary_selection: false,
    own_title_bar: false,
};

pub fn roots() -> Vec<(String, PathBuf)> {
    vec![(crate::i18n::tr("Computer"), "/".into())]
}

pub mod shell {
    use std::path::PathBuf;

    use anyhow::{Context, Result, bail};

    async fn open(args: &[&std::ffi::OsStr]) -> Result<()> {
        let status = tokio::process::Command::new("/usr/bin/open").args(args).status().await;
        if !status.context("the system could not be asked to open it")?.success() {
            bail!("the system could not open it");
        }
        Ok(())
    }

    /// Callers only pass addresses already checked to be web or mail links.
    pub async fn open_uri(uri: String) -> Result<()> {
        open(&[uri.as_ref()]).await
    }

    pub async fn show_in_folder(path: PathBuf) -> Result<()> {
        open(&["-R".as_ref(), path.as_os_str()]).await
    }

    pub async fn open_with(_path: PathBuf) -> Result<()> {
        bail!("not available on this system")
    }
}

pub mod trash {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};
    use trash::TrashContext;
    use trash::macos::{DeleteMethod, TrashContextExtMacos};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Trashed {
        pub original: PathBuf,
    }

    pub fn trash(path: &Path) -> Result<Trashed> {
        let original = std::path::absolute(path)?;
        let mut context = TrashContext::new();
        // Not through Finder, which would ask for permission to be controlled.
        context.set_delete_method(DeleteMethod::NsFileManager);
        context.delete(&original).with_context(|| format!("cannot move {} to the trash", path.display()))?;
        Ok(Trashed { original })
    }

    pub fn restore(_item: &Trashed) -> Result<()> {
        bail!("not available on this system")
    }

    #[cfg(test)]
    impl Trashed {
        pub fn for_test(original: PathBuf) -> Self {
            Self { original }
        }
    }

    /// It leaves a file in the trash: run by CI with `--ignored`.
    #[test]
    #[ignore]
    fn a_file_goes_to_the_trash() {
        let file = std::env::temp_dir().join(format!("lumen-trash-{}.txt", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        trash(&file).unwrap();
        assert!(!file.exists());
    }
}

pub mod print {
    use std::path::PathBuf;

    use anyhow::Result;

    use crate::print::Outcome;

    pub use crate::platform::unix_print::connect;

    /// Where CUPS listens.
    pub const SOCKETS: [&str; 2] = ["/private/var/run/cupsd", "/var/run/cupsd"];

    /// Lumen's own dialog, which sends to CUPS.
    pub async fn print(_path: PathBuf) -> Result<Outcome> {
        Ok(Outcome::NoPortal)
    }
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

    pub fn prepare(_command: &mut std::process::Command) {}

    pub fn escape(_test: &str) -> bool {
        false
    }
}
