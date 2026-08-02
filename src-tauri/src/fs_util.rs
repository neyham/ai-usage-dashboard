use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct OsFileLock {
    file: fs::File,
}

impl OsFileLock {
    pub(crate) fn acquire(path: &Path, timeout: Duration) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let started = Instant::now();

        loop {
            if try_lock_file(&file)? {
                return Ok(Self { file });
            }
            if started.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "file lock timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for OsFileLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
}

#[cfg(unix)]
fn try_lock_file(file: &fs::File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }

    if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(err)
    }
}

#[cfg(unix)]
fn unlock_file(file: &fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    const LOCK_UN: i32 = 8;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }

    if unsafe { flock(file.as_raw_fd(), LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock_file(file: &fs::File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows::Win32::System::IO::OVERLAPPED;

    let handle = HANDLE(file.as_raw_handle());
    let mut overlapped = OVERLAPPED::default();
    match unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    } {
        Ok(()) => Ok(true),
        Err(err) => {
            let err = io::Error::from(err);
            if err.raw_os_error() == Some(33) {
                Ok(false)
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(windows)]
fn unlock_file(file: &fs::File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::UnlockFileEx;
    use windows::Win32::System::IO::OVERLAPPED;

    let handle = HANDLE(file.as_raw_handle());
    let mut overlapped = OVERLAPPED::default();
    unsafe { UnlockFileEx(handle, 0, 1, 0, &mut overlapped) }.map_err(io::Error::from)
}

/// Replace a file without exposing a partially-written destination.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_write_with_privacy(path, contents, false)
}

/// Atomically replace a file that may contain credentials. On Unix the new
/// inode is created mode 0600 before it ever becomes visible at `path`.
pub fn atomic_write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_write_with_privacy(path, contents, true)
}

/// Tighten an existing credential-bearing file during upgrades as well as on
/// writes. Windows relies on the per-user profile ACL inherited by the file.
pub fn restrict_file_to_owner(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn atomic_write_with_privacy(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
    #[cfg(not(unix))]
    let _ = private;
    let resolved_path;
    let path = if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        resolved_path = fs::canonicalize(path)?;
        resolved_path.as_path()
    } else {
        path
    };
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;

    let existing_permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    let temp_path = temp_path(path, parent);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;

        #[cfg(unix)]
        if private {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        } else if let Some(permissions) = existing_permissions {
            file.set_permissions(permissions)?;
        }
        #[cfg(not(unix))]
        if let Some(permissions) = existing_permissions {
            file.set_permissions(permissions)?;
        }

        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        replace_temp_file(&temp_path, path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(not(windows))]
fn replace_temp_file(temp_path: &Path, target_path: &Path) -> io::Result<()> {
    fs::rename(temp_path, target_path)
}

#[cfg(windows)]
fn replace_temp_file(temp_path: &Path, target_path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};

    if !target_path.try_exists()? {
        return fs::rename(temp_path, target_path);
    }

    let target_wide: Vec<u16> = target_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let temp_wide: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        ReplaceFileW(
            PCWSTR::from_raw(target_wide.as_ptr()),
            PCWSTR::from_raw(temp_wide.as_ptr()),
            PCWSTR::null(),
            REPLACE_FILE_FLAGS::default(),
            None,
            None,
        )
    }
    .map_err(io::Error::from)
}

fn temp_path(path: &Path, parent: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{name}.tmp-{}-{sequence}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::atomic_write;
    #[cfg(unix)]
    use super::{atomic_write_private, restrict_file_to_owner};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("ai-usage-dashboard-{unique}"))
            .join(name)
    }

    #[test]
    fn atomic_write_creates_and_replaces_file() {
        let path = test_path("state.json");

        atomic_write(&path, b"first").expect("create file");
        #[cfg(windows)]
        let alternate_stream = {
            let stream = PathBuf::from(format!("{}:atomic-write-test", path.display()));
            fs::write(&stream, b"preserved").expect("create alternate data stream");
            stream
        };

        atomic_write(&path, b"second").expect("replace file once");
        atomic_write(&path, b"third").expect("replace file twice");

        assert_eq!(fs::read(&path).expect("read file"), b"third");
        #[cfg(windows)]
        assert_eq!(
            fs::read(alternate_stream).expect("read preserved alternate data stream"),
            b"preserved"
        );
        fs::remove_dir_all(path.parent().expect("test directory")).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn private_atomic_write_restricts_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = test_path("private.json");
        atomic_write_private(&path, b"secret").expect("write private file");

        assert_eq!(
            fs::metadata(&path)
                .expect("private file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_private_file_is_restricted_during_upgrade() {
        use std::os::unix::fs::PermissionsExt;

        let path = test_path("legacy-config.json");
        fs::create_dir_all(path.parent().expect("test directory")).expect("create test directory");
        fs::write(&path, b"legacy secret").expect("create legacy config");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o664))
            .expect("widen legacy permissions");

        restrict_file_to_owner(&path).expect("restrict legacy config");

        assert_eq!(
            fs::metadata(&path)
                .expect("legacy config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_symlink_and_updates_its_target() {
        use std::os::unix::fs::symlink;

        let link = test_path("credentials.json");
        let target = link.parent().expect("test directory").join("actual.json");
        fs::create_dir_all(link.parent().expect("test directory")).expect("create test directory");
        fs::write(&target, b"old").expect("create target");
        symlink(&target, &link).expect("create symlink");

        atomic_write(&link, b"new").expect("replace symlink target");

        assert!(fs::symlink_metadata(&link)
            .expect("read symlink metadata")
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&target).expect("read target"), b"new");
        fs::remove_dir_all(link.parent().expect("test directory")).expect("remove test directory");
    }
}
