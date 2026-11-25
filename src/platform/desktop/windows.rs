//! What only Windows does.

use std::path::PathBuf;

pub const CAPS: crate::platform::Caps = crate::platform::Caps {
    sandbox: false,
    trash_undo: true,
    open_with: true,
    direct_print: false,
    shared_thumbnails: false,
    desktop_dialogs_optional: false,
    primary_selection: false,
    own_title_bar: false,
};

/// No console window for the programs Lumen starts.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The drives.
pub fn roots() -> Vec<(String, PathBuf)> {
    ('A'..='Z')
        .map(|d| format!("{d}:\\"))
        .filter(|d| std::path::Path::new(d).is_dir())
        .map(|d| (d.clone(), d.into()))
        .collect()
}

/// A program of the system's, by its full path: never one that lies in the current folder.
fn system_program(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join(name)
}

pub mod shell {
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context, Result, bail};

    use super::{CREATE_NO_WINDOW, system_program};

    fn rundll32(arguments: String) -> Result<()> {
        // These take the rest of the line as it is; no shell reads it.
        Command::new(system_program(r"System32\rundll32.exe"))
            .raw_arg(arguments)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .context("the system could not be asked to open it")?;
        Ok(())
    }

    fn plain(path: &Path) -> Result<&str> {
        path.to_str().filter(|p| !p.contains('"')).context("that name cannot be passed on")
    }

    /// Callers only pass addresses already checked to be web or mail links.
    pub async fn open_uri(uri: String) -> Result<()> {
        if uri.contains(['"', ' ']) {
            bail!("not a valid address");
        }
        rundll32(format!("url.dll,FileProtocolHandler {uri}"))
    }

    pub async fn show_in_folder(path: PathBuf) -> Result<()> {
        // Explorer's exit status says nothing.
        Command::new(system_program("explorer.exe"))
            .raw_arg(format!("/select,\"{}\"", plain(&path)?))
            .spawn()
            .context("the file manager did not start")?;
        Ok(())
    }

    pub async fn open_with(path: PathBuf) -> Result<()> {
        rundll32(format!("shell32.dll,OpenAs_RunDLL {}", plain(&path)?))
    }
}

pub mod trash {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Trashed {
        pub original: PathBuf,
    }

    pub fn trash(path: &Path) -> Result<Trashed> {
        let original = std::path::absolute(path)?;
        trash::delete(&original).with_context(|| format!("cannot move {} to the trash", path.display()))?;
        Ok(Trashed { original })
    }

    pub fn restore(item: &Trashed) -> Result<()> {
        if item.original.symlink_metadata().is_ok() {
            bail!("there is already something at {}", item.original.display());
        }
        let same = |t: &trash::TrashItem| t.original_path() == item.original;
        let newest = trash::os_limited::list()?.into_iter().filter(same).max_by_key(|t| t.time_deleted);
        let newest = newest.context("it is not in the recycle bin any more")?;
        trash::os_limited::restore_all([newest]).with_context(|| format!("cannot restore {}", item.original.display()))
    }

    #[cfg(test)]
    impl Trashed {
        pub fn for_test(original: PathBuf) -> Self {
            Self { original }
        }
    }

    /// Run by CI with `--ignored`: it goes through the real recycle bin.
    #[test]
    #[ignore]
    fn a_file_goes_to_the_recycle_bin_and_back() {
        let file = std::env::temp_dir().join(format!("lumen-trash-{}.txt", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        // The short form of the folder's name (`RUNNER~1`) is not what the bin records.
        let file = PathBuf::from(std::fs::canonicalize(&file).unwrap().to_str().unwrap().trim_start_matches(r"\\?\"));
        let item = trash(&file).unwrap();
        assert!(!file.exists());
        restore(&item).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"x");
        assert!(restore(&item).is_err(), "there is something there");
        std::fs::remove_file(&file).unwrap();
    }
}

pub mod print {
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};

    use super::{CREATE_NO_WINDOW, system_program};
    use crate::library::Kind;
    use crate::print::{Connection, Job, MM_TO_PT, Outcome, Paper, picture_to_pdf};

    pub const SOCKETS: [&str; 0] = [];

    pub fn connect(_socket: &Path) -> Result<Box<dyn Connection>> {
        bail!("not available on this system")
    }

    /// Windows has no print dialog that safe Rust can reach: the PDF is handed to whichever program
    /// prints PDFs here, or else opened in the one that shows them.
    pub async fn print(path: PathBuf) -> Result<Outcome> {
        tokio::task::spawn_blocking(move || send(&path)).await.context("printing stopped")?
    }

    fn send(path: &Path) -> Result<Outcome> {
        let bytes = if Kind::of(path) == Some(Kind::Pdf) {
            std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?
        } else {
            picture_to_pdf(path, &Paper::sized(210.0 * MM_TO_PT, 297.0 * MM_TO_PT))?
        };
        let copy = Job::copy("print.pdf", |p| Ok(crate::fsio::write_new(p, &bytes)?))?;
        let file = copy.path.clone();
        // The other program reads it in its own time: it is deleted when Lumen ends.
        std::mem::forget(copy);
        if start(&file, "-Verb Print")? {
            return Ok(Outcome::Sent);
        }
        if start(&file, "")? {
            return Ok(Outcome::Opened);
        }
        bail!("no program here prints PDFs")
    }

    fn start(file: &Path, verb: &str) -> Result<bool> {
        // The path is passed in the environment, where nothing reads it as a command.
        let script = format!("$ErrorActionPreference='Stop'; Start-Process -FilePath $env:LUMEN_PRINT_FILE {verb}");
        let status = std::process::Command::new(system_program(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .env("LUMEN_PRINT_FILE", file)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .context("the system could not be asked to print")?;
        Ok(status.success())
    }
}

pub mod confine {
    use std::os::windows::process::CommandExt;
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

    /// No console flashes up for each picture.
    pub fn prepare(command: &mut std::process::Command) {
        command.creation_flags(super::CREATE_NO_WINDOW);
    }

    pub fn escape(_test: &str) -> bool {
        false
    }
}

pub mod fs {
    use std::fs::{File, OpenOptions, Permissions};
    use std::io;
    use std::path::{Path, PathBuf};

    // A user's folders, the temporary one too, are closed to other users here.

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

    /// Whether a file could not be replaced because another program has it open.
    pub fn in_use(e: &io::Error) -> bool {
        // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        matches!(e.raw_os_error(), Some(5 | 32 | 33))
    }
}
