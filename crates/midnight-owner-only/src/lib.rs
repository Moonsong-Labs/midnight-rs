//! Owner-only directories and durable files for the on-disk stores of the SDK.
//!
//! On Unix, a directory gets mode 0700 and a file gets mode 0600. A call also
//! narrows a path that already exists to that mode. On other platforms, a path
//! keeps the default permissions.
//!
//! [`replace`] writes a file through a temporary sibling, which it syncs to
//! disk and renames into place before it syncs the directory. After a crash or
//! a power loss, the path holds the old contents or the new contents.
//!
//! A file system that cannot sync a file or a directory refuses the sync. The
//! calls then skip that sync, and the file system alone decides what persists
//! after a power loss.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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

/// Replaces the contents of `path` with `contents`, in a file readable only by
/// the owner.
///
/// The call writes `contents` to a temporary sibling, the name of `path` with
/// `.tmp` appended, and syncs it to disk. It then renames the sibling to `path`
/// and syncs the directory. After a crash or a power loss, `path` holds the
/// old contents or the new contents. On Unix, `path` gets mode 0600. On a file
/// system that refuses a sync, the call skips it (see the module docs).
///
/// # Errors
///
/// Returns the error of the first step that fails, with the step and the path
/// in its message. The directory sync comes after the rename, so `path` can
/// hold the new contents when that sync fails.
pub fn replace(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = tmp_path(path);
    write(&tmp, contents).map_err(|err| at_step(err, "write", &tmp))?;
    fs::rename(&tmp, path).map_err(|err| at_step(err, "rename into", path))?;
    let dir = parent_dir(path);
    sync_dir(dir).map_err(|err| at_step(err, "sync", dir))
}

/// The temporary sibling that [`replace`] writes before its rename.
fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    tmp.into()
}

/// The directory that holds `path`, or `.` for a bare file name.
fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

/// `err` with the failed `step` and its `path` in the message.
fn at_step(err: io::Error, step: &str, path: &Path) -> io::Error {
    io::Error::new(err.kind(), format!("{step} {}: {err}", path.display()))
}

/// Writes `contents` to `path` with mode 0600 on Unix, also when the file
/// exists, and syncs the file to disk.
fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
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
        let stale_tmp = super::tmp_path(&file);
        fs::write(&stale_tmp, b"old").unwrap();
        fs::set_permissions(&stale_tmp, fs::Permissions::from_mode(0o644)).unwrap();

        super::create_dir_all(&dir).unwrap();
        super::replace(&file, b"new").unwrap();

        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&file), 0o600);
    }
}
