//! Files and folders with permissions of their own (Linux and macOS).

use std::fs::{File, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// A file that did not exist, with `mode` (as far as the umask lets it) from the start.
pub fn new_file(path: &Path, mode: Option<&Permissions>) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    if let Some(mode) = mode {
        options.mode(mode.mode() & 0o777);
    }
    options.open(path)
}

/// A file that did not exist, for this user only.
pub fn new_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

/// A folder that did not exist, for this user only.
pub fn new_private_dir(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

/// Makes `path` and its parents where they are missing, for this user only.
pub fn private_dirs(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
}

/// Where this user's temporary files go.
pub fn private_temp() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir());
    runtime.unwrap_or_else(std::env::temp_dir)
}

/// Makes a rename in `dir` last through a power cut.
pub fn sync_dir(dir: &Path) {
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
}

/// `path` as the bytes it is.
#[cfg(target_os = "linux")]
pub(super) fn bytes(path: &Path) -> &[u8] {
    std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str())
}

/// Whether a file could not be replaced because another program has it open: never here.
pub fn in_use(_e: &io::Error) -> bool {
    false
}
