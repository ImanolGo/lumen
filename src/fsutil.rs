//! Replacing files without damaging what is around them.

use std::{fs, io, os::unix::fs::MetadataExt, path::Path};

/// Move the finished file `temp` into place as `dest`, keeping what was there: the
/// permissions, a symlink (the file it points to is replaced) and hard links (the shared
/// inode is rewritten). `temp` is gone afterwards, also on failure.
pub fn replace_file(temp: &Path, dest: &Path) -> io::Result<()> {
    let result = replace(temp, dest);
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn replace(temp: &Path, dest: &Path) -> io::Result<()> {
    fs::File::open(temp)?.sync_all()?;
    // Through symlinks, to the file that really holds the data.
    let target = fs::canonicalize(dest).unwrap_or_else(|_| dest.to_owned());
    let shared = match fs::metadata(&target) {
        Ok(existing) => {
            fs::set_permissions(temp, existing.permissions())?;
            existing.nlink() > 1
        }
        Err(_) => false,
    };
    // `fs::copy` writes into the existing inode, so hard links keep sharing it. It is also the
    // way across file systems, where a rename is refused.
    if shared || fs::rename(temp, &target).is_err() {
        fs::copy(temp, &target)?;
        fs::remove_file(temp)?;
    }
    if let Some(dir) = target.parent() {
        // Make the rename durable; not every file system lets a directory be synced.
        let _ = fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-fsutil-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn keeps_the_permissions_of_the_old_file() {
        let dir = scratch("mode");
        let (dest, temp) = (dir.join("a.png"), dir.join(".a.tmp"));
        fs::write(&dest, "old").unwrap();
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&temp, "new").unwrap();
        replace_file(&temp, &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
        assert_eq!(fs::metadata(&dest).unwrap().mode() & 0o777, 0o600);
        assert!(!temp.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_symlink_stays_a_symlink() {
        let dir = scratch("link");
        let (real, link, temp) = (dir.join("real.png"), dir.join("link.png"), dir.join(".t"));
        fs::write(&real, "old").unwrap();
        symlink(&real, &link).unwrap();
        fs::write(&temp, "new").unwrap();
        replace_file(&temp, &link).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hard_links_stay_shared() {
        let dir = scratch("hard");
        let (a, b, temp) = (dir.join("a"), dir.join("b"), dir.join(".t"));
        fs::write(&a, "old").unwrap();
        fs::hard_link(&a, &b).unwrap();
        fs::write(&temp, "new").unwrap();
        replace_file(&temp, &a).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "new");
        assert_eq!(
            fs::metadata(&a).unwrap().ino(),
            fs::metadata(&b).unwrap().ino()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_file_is_simply_moved_into_place() {
        let dir = scratch("new");
        let (dest, temp) = (dir.join("a"), dir.join(".t"));
        fs::write(&temp, "new").unwrap();
        replace_file(&temp, &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
        fs::remove_dir_all(&dir).unwrap();
    }
}
