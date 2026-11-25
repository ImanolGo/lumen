//! Moving files to the trash and back, following the freedesktop.org Trash spec: the file
//! goes to `files/` and a `.trashinfo` note with its old place and the date goes to `info/`,
//! so file managers can restore it too. Files on the home disk use `~/.local/share/Trash`;
//! files on another disk use `.Trash-<uid>` at that disk's top. A file that cannot be
//! renamed into a trash (other disk, no write access there) is an error, never copied
//! and deleted. Blocking.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// A file that was moved to the trash, with what is needed to bring it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trashed {
    pub original: PathBuf,
    stored: PathBuf,
    info: PathBuf,
}

#[cfg(test)]
impl Trashed {
    /// A file that was never really moved.
    pub fn for_test(original: PathBuf) -> Self {
        Self { original, stored: PathBuf::new(), info: PathBuf::new() }
    }
}

/// Moves `path` to the trash that belongs to its disk.
pub fn trash(path: &Path) -> Result<Trashed> {
    // A link is moved itself, not what it leads to.
    let path = std::path::absolute(path).with_context(|| format!("cannot find {}", path.display()))?;
    let dev = std::fs::symlink_metadata(&path)?.dev();
    let home = home_trash().context("there is no home folder to keep a trash in")?;
    std::fs::create_dir_all(home.parent().unwrap_or(&home))?;
    if std::fs::metadata(home.parent().unwrap_or(&home)).is_ok_and(|m| m.dev() == dev) {
        return trash_into(&path, &home, None);
    }
    // Another disk: its own trash at its top.
    let top = mount_top(&path, dev);
    let uid = std::fs::metadata("/proc/self")?.uid();
    trash_into(&path, &disk_trash(&top, uid), Some(&top))
}

/// The trash of the disk whose top folder is `top`: the one its administrator made for everybody (if it
/// is safe to use: a real folder with the sticky bit), else the user's own.
fn disk_trash(top: &Path, uid: u32) -> PathBuf {
    let shared = top.join(".Trash");
    if std::fs::symlink_metadata(&shared).is_ok_and(|m| m.is_dir() && m.mode() & 0o1000 != 0) {
        let own = shared.join(uid.to_string());
        if std::fs::create_dir_all(&own).is_ok() {
            return own;
        }
    }
    top.join(format!(".Trash-{uid}"))
}

/// `~/.local/share/Trash` (or under `$XDG_DATA_HOME`).
fn home_trash() -> Option<PathBuf> {
    Some(super::dirs::shared_data()?.join("Trash"))
}

/// The highest folder on the same disk as `path`.
fn mount_top(path: &Path, dev: u64) -> PathBuf {
    let mut top = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    while let Some(parent) = top.parent() {
        if std::fs::metadata(parent).map(|m| m.dev()).ok() != Some(dev) {
            break;
        }
        top = parent.to_path_buf();
    }
    top
}

/// Moves `path` into `trash_dir`; `top` is the disk's top folder when the trash is on another
/// disk (the note then records the place relative to it).
fn trash_into(path: &Path, trash_dir: &Path, top: Option<&Path>) -> Result<Trashed> {
    let (files, infos) = (trash_dir.join("files"), trash_dir.join("info"));
    for dir in [&files, &infos] {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot use the trash at {}", trash_dir.display()))?;
    }
    let _ = std::fs::set_permissions(trash_dir, std::fs::Permissions::from_mode(0o700));

    let recorded = match top {
        Some(top) => path.strip_prefix(top).unwrap_or(path),
        None => path,
    };
    let note = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", encode(recorded.as_os_str().as_bytes()), now());
    let name = path.file_name().context("that has no name")?.to_os_string();

    // The note is created first, exclusively, which claims the name.
    for n in 1..10_000u32 {
        let mut candidate = name.clone();
        if n > 1 {
            candidate.push(format!(".{n}"));
        }
        let mut info_name = candidate.clone();
        info_name.push(".trashinfo");
        let (info, stored) = (infos.join(&info_name), files.join(&candidate));
        if stored.symlink_metadata().is_ok() {
            continue;
        }
        let created = std::fs::OpenOptions::new().write(true).create_new(true).open(&info);
        let mut file = match created {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).context("cannot write to the trash"),
        };
        use std::io::Write as _;
        if let Err(e) = file.write_all(note.as_bytes()).and_then(|()| std::fs::rename(path, &stored)) {
            let _ = std::fs::remove_file(&info);
            return Err(e).with_context(|| format!("cannot move {} to the trash", path.display()));
        }
        return Ok(Trashed { original: path.to_path_buf(), stored, info });
    }
    bail!("the trash has too many files with this name")
}

/// Puts a trashed file back where it was.
pub fn restore(item: &Trashed) -> Result<()> {
    if let Some(dir) = item.original.parent() {
        std::fs::create_dir_all(dir)?;
    }
    rename_new(&item.stored, &item.original).with_context(|| format!("cannot restore {}", item.original.display()))?;
    let _ = std::fs::remove_file(&item.info);
    Ok(())
}

/// Renames without ever replacing something.
fn rename_new(from: &Path, to: &Path) -> Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => return Ok(()),
        Err(Errno::EXIST) => bail!("there is already something at {}", to.display()),
        // A file system that cannot do it in one step.
        Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => {}
        Err(e) => return Err(e.into()),
    }
    if to.symlink_metadata().is_ok() {
        bail!("there is already something at {}", to.display());
    }
    Ok(std::fs::rename(from, to)?)
}

/// Percent-encodes a path as the spec asks: everything but unreserved characters and `/`.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The local time as the spec writes it (`2026-11-23T16:20:05`).
fn now() -> String {
    jiff::ZonedDateTime::now().strftime("%Y-%m-%dT%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    /// The reverse of [`encode`].
    pub fn decode(text: &str) -> PathBuf {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%'
                && let Some(b) = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        PathBuf::from(OsString::from_vec(out))
    }

    #[test]
    fn the_date_is_the_local_one() {
        let asked = std::process::Command::new("date").arg("+%Y-%m-%dT%H:%M").output().unwrap();
        let asked = String::from_utf8(asked.stdout).unwrap();
        // The minute may turn in between.
        assert_eq!(now().len(), 19);
        assert_eq!(now()[..13], asked[..13]);
    }

    #[test]
    fn the_shared_trash_of_a_disk_is_used_only_if_it_is_safe() {
        let top = scratch("shared");
        assert_eq!(disk_trash(&top, 7), top.join(".Trash-7"));
        std::fs::create_dir(top.join(".Trash")).unwrap();
        assert_eq!(disk_trash(&top, 7), top.join(".Trash-7"), "no sticky bit");
        std::fs::set_permissions(top.join(".Trash"), std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert_eq!(disk_trash(&top, 7), top.join(".Trash/7"));
        std::fs::rename(top.join(".Trash"), top.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(top.join("elsewhere"), top.join(".Trash")).unwrap();
        assert_eq!(disk_trash(&top, 7), top.join(".Trash-7"), "a link is not trusted");
        std::fs::remove_dir_all(&top).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-rs-trash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn encodes_and_decodes_paths() {
        assert_eq!(encode(b"/home/u/My Pics/caf\xC3\xA9.png"), "/home/u/My%20Pics/caf%C3%A9.png");
        assert_eq!(decode("/home/u/My%20Pics/caf%C3%A9.png"), PathBuf::from("/home/u/My Pics/café.png"));
        assert_eq!(decode("/100%"), PathBuf::from("/100%"));
        let odd = b"/a/\xFF\xFEb %.x";
        assert_eq!(decode(&encode(odd)).as_os_str().as_bytes(), odd);
    }

    #[test]
    fn trashing_writes_the_note_and_restoring_undoes_it() {
        let dir = scratch("roundtrip");
        let file = dir.join("My Photo.jpg");
        std::fs::write(&file, b"pixels").unwrap();
        let bin = dir.join("Trash");
        let item = trash_into(&file, &bin, None).unwrap();
        assert!(!file.exists());
        assert_eq!(std::fs::read(bin.join("files/My Photo.jpg")).unwrap(), b"pixels");
        let note = std::fs::read_to_string(bin.join("info/My Photo.jpg.trashinfo")).unwrap();
        assert!(note.starts_with("[Trash Info]\nPath="), "{note}");
        assert!(note.contains(&format!("Path={}/My%20Photo.jpg\n", encode(dir.as_os_str().as_bytes()))), "{note}");
        let date = note.lines().find_map(|l| l.strip_prefix("DeletionDate=")).unwrap();
        assert_eq!(date.len(), 19, "{date}");
        assert_eq!(date.as_bytes()[10], b'T');

        restore(&item).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"pixels");
        assert!(!bin.join("files/My Photo.jpg").exists());
        assert!(!bin.join("info/My Photo.jpg.trashinfo").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn same_names_do_not_clash_and_restore_never_overwrites() {
        let dir = scratch("clash");
        let bin = dir.join("Trash");
        let file = dir.join("a.png");
        std::fs::write(&file, b"one").unwrap();
        let first = trash_into(&file, &bin, None).unwrap();
        std::fs::write(&file, b"two").unwrap();
        let second = trash_into(&file, &bin, None).unwrap();
        assert_ne!(first.stored, second.stored);
        assert_eq!(std::fs::read(&second.stored).unwrap(), b"two");
        // The place is taken again: restoring must refuse rather than overwrite.
        std::fs::write(&file, b"three").unwrap();
        assert!(restore(&first).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"three");
        assert_eq!(std::fs::read(&first.stored).unwrap(), b"one");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn another_disks_trash_records_a_path_relative_to_its_top() {
        let dir = scratch("top");
        let file = dir.join("sub/x.png");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"x").unwrap();
        let bin = dir.join(".Trash-1000");
        trash_into(&file, &bin, Some(&dir)).unwrap();
        let note = std::fs::read_to_string(bin.join("info/x.png.trashinfo")).unwrap();
        assert!(note.contains("Path=sub/x.png\n"), "{note}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_error_and_leaves_no_note() {
        let dir = scratch("missing");
        let bin = dir.join("Trash");
        assert!(trash_into(&dir.join("nope.png"), &bin, None).is_err());
        assert_eq!(std::fs::read_dir(bin.join("info")).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
