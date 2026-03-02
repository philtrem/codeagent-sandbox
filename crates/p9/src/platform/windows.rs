use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::error::P9Error;
use crate::operations::attr::FileAttributes;

/// Reserved device names on Windows (case-insensitive, with or without extension).
///
/// These names cannot be used as filenames or directory names on Windows NTFS/FAT.
/// The list follows the Win32 API documentation for reserved names.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9",
];

/// Check whether a filename is a Windows reserved device name.
///
/// Reserved names are checked case-insensitively and both with and without
/// extensions (e.g., both "CON" and "CON.txt" are reserved).
pub fn is_reserved_name(name: &str) -> bool {
    // Extract the stem (before the first dot).
    let stem = name.split('.').next().unwrap_or(name);
    let upper = stem.to_uppercase();
    RESERVED_NAMES.contains(&upper.as_str())
}

/// Check for case collisions in a directory.
///
/// Returns `Some(existing_name)` if an entry with the same case-insensitive
/// name already exists in the directory but with different casing.
pub fn check_case_collision(parent: &Path, name: &str) -> Result<Option<String>, P9Error> {
    let name_lower = name.to_lowercase();

    let entries = std::fs::read_dir(parent)?;
    for entry in entries {
        let entry = entry?;
        if let Some(existing) = entry.file_name().to_str() {
            if existing.to_lowercase() == name_lower && existing != name {
                return Ok(Some(existing.to_string()));
            }
        }
    }

    Ok(None)
}

/// Synthesize POSIX-compatible file attributes from Windows metadata.
///
/// Windows lacks native POSIX mode bits, uid/gid, and nlink.
/// We synthesize reasonable values:
/// - Directories get mode 0o755, regular files get 0o644.
/// - Executable extensions (.exe, .bat, .cmd, .sh, .py) get 0o755.
/// - uid/gid are set to 0 (root).
/// - nlink is always 1.
pub fn get_file_attributes(path: &Path) -> Result<FileAttributes, P9Error> {
    let meta = std::fs::metadata(path)?;

    let is_dir = meta.is_dir();
    let mode = if is_dir {
        // S_IFDIR | rwxr-xr-x
        0o40755
    } else {
        let base_mode = if is_executable_extension(path) {
            0o755
        } else {
            0o644
        };
        // S_IFREG | mode
        0o100000 | base_mode
    };

    let size = meta.len();
    let blocks = size.div_ceil(512);

    let (atime_sec, atime_nsec) = system_time_to_unix(meta.accessed().ok());
    let (mtime_sec, mtime_nsec) = system_time_to_unix(meta.modified().ok());
    let (ctime_sec, ctime_nsec) = system_time_to_unix(meta.created().ok());

    Ok(FileAttributes {
        mode,
        uid: 0,
        gid: 0,
        nlink: 1,
        rdev: 0,
        size,
        blksize: 4096,
        blocks,
        atime_sec,
        atime_nsec,
        mtime_sec,
        mtime_nsec,
        ctime_sec,
        ctime_nsec,
    })
}

/// Check if a file has an "executable" extension.
fn is_executable_extension(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    matches!(
        ext.as_str(),
        "exe" | "bat" | "cmd" | "com" | "sh" | "py" | "rb" | "pl" | "ps1"
    )
}

/// Convert a SystemTime to (seconds, nanoseconds) since Unix epoch.
fn system_time_to_unix(time: Option<std::time::SystemTime>) -> (u64, u64) {
    match time {
        Some(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => (d.as_secs(), d.subsec_nanos() as u64),
            Err(_) => (0, 0),
        },
        None => (0, 0),
    }
}
