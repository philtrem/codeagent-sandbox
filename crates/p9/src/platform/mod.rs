use std::path::Path;

use crate::error::P9Error;
use crate::operations::attr::FileAttributes;

#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;

/// Get file attributes for the given path in a platform-independent way.
///
/// On Unix, reads real POSIX metadata (mode, uid, gid, etc.).
/// On Windows, synthesizes POSIX-compatible attributes.
pub fn get_file_attributes(path: &Path) -> Result<FileAttributes, P9Error> {
    #[cfg(unix)]
    {
        unix::get_file_attributes(path)
    }
    #[cfg(windows)]
    {
        windows::get_file_attributes(path)
    }
}

/// Check whether a filename is a Windows reserved device name.
///
/// On non-Windows platforms, always returns `false` since there are no
/// reserved names. On Windows, checks against CON, NUL, LPT1, COM1, etc.
pub fn is_reserved_name(name: &str) -> bool {
    #[cfg(windows)]
    {
        windows::is_reserved_name(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        false
    }
}

/// Check for case collisions in a directory.
///
/// On case-insensitive filesystems (Windows), returns `Some(existing_name)` if
/// an entry with the same name but different casing exists. On case-sensitive
/// filesystems (most Unix), always returns `None`.
pub fn check_case_collision(parent: &Path, name: &str) -> Result<Option<String>, P9Error> {
    #[cfg(windows)]
    {
        windows::check_case_collision(parent, name)
    }
    #[cfg(not(windows))]
    {
        let _ = (parent, name);
        Ok(None)
    }
}
