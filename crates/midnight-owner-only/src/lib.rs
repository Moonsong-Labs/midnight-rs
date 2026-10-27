//! Owner-only directories and durable files for the on-disk stores of the SDK.
//!
//! On Unix, a directory gets mode 0700 and a file gets mode 0600. A call also
//! narrows a path that already exists to that mode. On other platforms, a path
//! keeps the default permissions.
//!
//! To replace a file durably, write a temporary file with [`write()`], rename it
//! into place, and then call [`sync_dir`] on its directory. After a crash or a
//! power loss, the path holds the old contents or the new contents.
//!
//! A file system that cannot sync a file or a directory refuses the sync. The
//! calls then skip that sync, and the file system alone decides what persists
//! after a power loss.

use std::fs;
use std::io;
use std::path::Path;

/// Creates `dir` and its missing ancestors, readable only by the owner.
///
/// On Unix, each directory that the call creates gets mode 0700, and `dir`
/// gets mode 0700 also when it exists. An existing ancestor keeps its mode.
/// On other platforms, this is [`std::fs::create_dir_all`].
///
/// # Errors
///
/// Returns the error of the create or of the mode change.
pub fn create_dir_all(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)
}

/// Writes `contents` to `path`, in a file readable only by the owner, and
/// syncs the file to disk.
///
/// On Unix, the file gets mode 0600, also when it exists. The call replaces
/// the contents of an existing file. When the call returns, the contents are
/// on disk, so a rename of `path` cannot persist before them. On a file system
/// that refuses the sync, the call skips it (see the module docs).
///
/// # Errors
///
/// Returns the error of the open, of the mode change, of the write or of the
/// sync.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        // `mode` applies only when the open creates the file. Narrow an
        // existing file before the contents land, or they are briefly readable.
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        file
    };
    #[cfg(not(unix))]
    let mut file = fs::File::create(path)?;
    file.write_all(contents)?;
    sync(&file)
}

/// Syncs the entries of `dir` to disk, so a rename or a create in `dir`
/// persists.
///
/// On a file system that refuses the sync of a directory, the call skips it
/// (see the module docs). On platforms other than Unix, the call does nothing,
/// because [`fs::File::open`] fails on a directory on Windows.
///
/// # Errors
///
/// Returns the error of the open or of the sync.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    sync(&fs::File::open(dir)?)?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// [`fs::File::sync_all`], or `Ok` when the file system refuses the sync.
fn sync(file: &fs::File) -> io::Result<()> {
    match file.sync_all() {
        Err(err) if sync_refused(&err) => Ok(()),
        result => result,
    }
}

/// Whether `err` says that the file system cannot sync the file, rather than
/// that a sync it supports failed.
///
/// Some file systems refuse the sync of a directory with `EINVAL`, or with
/// `EBADF` for a descriptor that is open only for reading. On Apple targets,
/// `F_FULLFSYNC` fails with `ENOTSUP` on a file system without it, such as an
/// SMB share, and `std` leaves that code uncategorized.
fn sync_refused(err: &io::Error) -> bool {
    #[cfg(unix)]
    let refused_code = matches!(err.raw_os_error(), Some(libc::EBADF | libc::ENOTSUP));
    #[cfg(not(unix))]
    let refused_code = false;
    refused_code
        || matches!(
            err.kind(),
            io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
        )
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
        super::write(&file, b"new").unwrap();

        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&file), 0o600);
    }
}
