//! Owner-only directories and durable files for the on-disk stores of the SDK.
//!
//! On Unix, a directory gets mode 0700 and a file gets mode 0600. A call also
//! narrows a path that already exists to that mode. On other platforms, a path
//! keeps the default permissions.
//!
//! The calls also sync their changes to disk. After a crash or a power loss, a
//! file that [`replace`] wrote holds the old contents or the new contents, and
//! a directory that [`create_dir_all`] created exists.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use atomic_write_file::AtomicWriteFile;

/// Creates `dir` and its missing ancestors, readable only by the owner.
///
/// On Unix, each directory that the call creates gets mode 0700, and `dir`
/// gets mode 0700 also when it exists. An existing ancestor keeps its mode.
/// The call syncs the parent of each directory that it creates, so the new
/// directories persist after a power loss. On other platforms, this is
/// [`std::fs::create_dir_all`].
///
/// # Errors
///
/// Returns the error of the create, of the mode change or of a sync.
pub fn create_dir_all(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let missing: Vec<&Path> = dir
            .ancestors()
            .take_while(|ancestor| !ancestor.as_os_str().is_empty() && !ancestor.exists())
            .collect();
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        for created in missing {
            let parent = created
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            sync_dir(parent)?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)
}

/// Replaces the contents of `path` with `contents`, in a file readable only by
/// the owner.
///
/// The call writes `contents` to a hidden temporary file in the directory of
/// `path`, named `.<file name>.<random suffix>`. Then
/// [`AtomicWriteFile::commit`] syncs that file, renames it to `path` and syncs
/// the directory. After a crash or a power loss, `path` holds the old contents
/// or the new contents. A crash, or a commit that fails before the rename, can
/// leave the temporary file. On Unix, the file gets mode 0600, also when
/// `path` exists with another mode.
///
/// # Errors
///
/// Returns the error of the open, of the write or of the commit. A file system
/// that cannot sync a file or a directory fails the commit. The directory sync
/// comes after the rename, so `path` can hold the new contents when the commit
/// fails.
pub fn replace(path: &Path, contents: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    let mut file = {
        use atomic_write_file::unix::OpenOptionsExt as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        AtomicWriteFile::options()
            .mode(0o600)
            .preserve_mode(false)
            .open(path)?
    };
    #[cfg(not(unix))]
    let mut file = AtomicWriteFile::open(path)?;
    file.write_all(contents)?;
    file.commit()
}

/// Syncs the entries of `dir` to disk, so a create, a rename or a removal in
/// `dir` persists after a power loss.
///
/// On platforms other than Unix, the call does nothing, because
/// [`fs::File::open`] fails on a directory on Windows.
///
/// # Errors
///
/// Returns the error of the open or of the sync.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn existing_paths_are_narrowed() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("dir");
        let file = dir.join("file");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&file, b"old").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

        super::create_dir_all(&dir).unwrap();
        super::replace(&file, b"new").unwrap();

        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&file), 0o600);
    }
}
