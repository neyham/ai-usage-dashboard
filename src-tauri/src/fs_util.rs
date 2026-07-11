use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replace a file without exposing a partially-written destination.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
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
