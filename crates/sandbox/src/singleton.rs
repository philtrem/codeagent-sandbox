use std::fs::File;
use std::path::PathBuf;

/// Guard that holds the instance lock. The lock is released when dropped.
pub struct InstanceLock {
    _file: File,
    _path: PathBuf,
}

/// Try to acquire the singleton instance lock.
///
/// Returns `Ok(InstanceLock)` if this is the only running instance.
/// Returns `Err(message)` if another instance holds the lock.
pub fn try_acquire_instance_lock() -> Result<InstanceLock, String> {
    let lock_path = lock_file_path()
        .ok_or_else(|| "Could not determine config directory for lock file".to_string())?;

    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create lock file directory: {e}"))?;
    }

    let file = File::create(&lock_path)
        .map_err(|e| format!("Failed to create lock file: {e}"))?;

    try_lock(&file).map_err(|_| {
        "Another sandbox instance is already running. Only one sandbox process can run at a time."
            .to_string()
    })?;

    Ok(InstanceLock {
        _file: file,
        _path: lock_path,
    })
}

fn lock_file_path() -> Option<PathBuf> {
    crate::config::default_config_dir().map(|d| d.join("sandbox.lock"))
}

#[cfg(windows)]
fn try_lock(file: &File) -> Result<(), ()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};

    let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };

    let result = unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };

    if result != 0 { Ok(()) } else { Err(()) }
}

#[cfg(unix)]
fn try_lock(file: &File) -> Result<(), ()> {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

    if result == 0 { Ok(()) } else { Err(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_file_path_is_in_config_dir() {
        let path = lock_file_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("CodeAgent"));
        assert!(path.to_string_lossy().ends_with("sandbox.lock"));
    }

    #[test]
    fn acquire_and_release_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("sandbox.lock");
        let file = File::create(&lock_path).unwrap();
        assert!(try_lock(&file).is_ok());
    }

    #[test]
    fn second_lock_fails() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("sandbox.lock");
        let file1 = File::create(&lock_path).unwrap();
        assert!(try_lock(&file1).is_ok());

        let file2 = File::open(&lock_path).unwrap();
        assert!(try_lock(&file2).is_err());
    }
}
