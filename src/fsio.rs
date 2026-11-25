//! Writing files so that a crash, a power cut or a second writer never leaves half of one.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

/// Replaces `path` with `bytes` all at once: whatever happens, the old file or the new one is
/// there, whole. A link is followed, and the file keeps its permissions.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_with(path, |file| Ok(file.write_all(bytes)?))
}

/// Like [`write_atomic`], with the bytes written by `fill`.
pub fn write_atomic_with(path: &Path, fill: impl FnOnce(&mut BufWriter<File>) -> Result<()>) -> Result<()> {
    replace(path, true, fill).with_context(|| format!("cannot save {}", path.display()))
}

/// Like [`write_atomic`] for what is only a convenience (the window's size, say): it does not wait
/// for the disk, so it is quick enough for the interface's thread.
pub fn write_quickly(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    replace(path, false, |file| Ok(file.write_all(bytes)?))
}

fn replace(path: &Path, durable: bool, fill: impl FnOnce(&mut BufWriter<File>) -> Result<()>) -> Result<()> {
    let target = behind_links(path);
    let mode = std::fs::metadata(&target).ok().map(|m| m.permissions());
    let (temp, file) = create_beside(&target, mode.as_ref())?;
    let result = (|| -> Result<()> {
        let mut out = BufWriter::new(file);
        fill(&mut out)?;
        let file = out.into_inner().map_err(std::io::IntoInnerError::into_error)?;
        // What the mode at creation could not say (the umask takes bits away).
        if let Some(mode) = mode {
            let _ = file.set_permissions(mode);
        }
        if durable {
            file.sync_all()?;
        }
        drop(file);
        std::fs::rename(&temp, &target).map_err(|e| {
            if crate::platform::fs::in_use(&e) {
                anyhow::Error::new(e).context("another program has it open: close it there")
            } else {
                e.into()
            }
        })?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            // The rename itself is on the disk only once the folder is.
            if durable && let Some(dir) = folder_of(&target) {
                crate::platform::fs::sync_dir(dir);
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

fn folder_of(path: &Path) -> Option<&Path> {
    path.parent().map(|p| if p.as_os_str().is_empty() { Path::new(".") } else { p })
}

/// The file a chain of symbolic links leads to (it may not exist yet).
fn behind_links(path: &Path) -> PathBuf {
    let mut at = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::read_link(&at) {
            Ok(next) => at = folder_of(&at).map_or(next.clone(), |dir| dir.join(&next)),
            Err(_) => break,
        }
    }
    at
}

/// A new file next to `target`, under a name nobody else has. It is made, never opened, so a file
/// or link planted under that name is not written through.
fn create_beside(target: &Path, mode: Option<&std::fs::Permissions>) -> Result<(PathBuf, File)> {
    for _ in 0..100 {
        let temp = target.with_file_name(format!(".lumen-{}.tmp", unique()));
        match crate::platform::fs::new_file(&temp, mode) {
            Ok(file) => return Ok((temp, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).context("cannot write there"),
        }
    }
    anyhow::bail!("cannot write there")
}

/// Text that is different every time, also between processes.
pub fn unique() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    // The standard library seeds this at random.
    let mut random = std::collections::hash_map::RandomState::new().build_hasher();
    random.write_u64(COUNT.fetch_add(1, Ordering::Relaxed));
    random.write_u32(std::process::id());
    format!("{:016x}", random.finish())
}

/// A folder only this user can look into, made new.
pub fn private_dir(name: &str) -> std::io::Result<PathBuf> {
    let dir = crate::platform::fs::private_temp().join(format!("{name}-{}", unique()));
    crate::platform::fs::new_private_dir(&dir)?;
    Ok(dir)
}

/// Writes a file that must not exist yet, for this user only.
pub fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = crate::platform::fs::new_private_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-rs-fsio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> =
            std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    }

    #[test]
    #[cfg(unix)]
    fn a_linked_file_is_replaced_behind_its_link_and_keeps_its_permissions() {
        let dir = temp("link");
        std::fs::create_dir(dir.join("real")).unwrap();
        let file = dir.join("real/a.txt");
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
        symlink("real/a.txt", dir.join("one")).unwrap();
        symlink(dir.join("one"), dir.join("two")).unwrap();
        write_atomic(&dir.join("two"), b"new").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert!(dir.join("two").is_symlink() && dir.join("one").is_symlink());
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o666);
        // A link to a file that is not there yet.
        symlink("real/b.txt", dir.join("three")).unwrap();
        write_atomic(&dir.join("three"), b"made").unwrap();
        assert_eq!(std::fs::read(dir.join("real/b.txt")).unwrap(), b"made");
        assert_eq!(names(&dir.join("real")), ["a.txt", "b.txt"], "nothing is left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_saves_at_once_do_not_mix() {
        let dir = temp("race");
        let path = dir.join("f");
        std::thread::scope(|s| {
            for b in *b"abcd" {
                let path = &path;
                s.spawn(move || (0..20).for_each(|_| write_atomic(path, &vec![b; 100_000]).unwrap()));
            }
        });
        let out = std::fs::read(&path).unwrap();
        assert!(out.len() == 100_000 && out.iter().all(|b| *b == out[0]));
        assert_eq!(names(&dir), ["f"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_save_leaves_the_old_file() {
        let dir = temp("fail");
        let path = dir.join("f");
        std::fs::write(&path, "old").unwrap();
        let failed = write_atomic_with(&path, |file| {
            file.write_all(b"half")?;
            anyhow::bail!("no")
        });
        assert!(failed.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(names(&dir), ["f"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn a_private_folder_is_private() {
        let dir = private_dir("lumen-rs-test").unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir(&dir).unwrap();
    }
}
