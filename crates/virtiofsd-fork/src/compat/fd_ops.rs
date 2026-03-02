// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! File descriptor path resolution and reopening.

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};

/// Platform-appropriate flag for opening an fd for metadata/traversal only.
///
/// Linux: `O_PATH` (metadata-only fd that cannot be used for I/O).
/// macOS: `O_RDONLY` (closest equivalent; macOS has no `O_PATH`).
#[cfg(target_os = "linux")]
pub const O_PATH_OR_RDONLY: i32 = libc::O_PATH;
#[cfg(not(target_os = "linux"))]
pub const O_PATH_OR_RDONLY: i32 = libc::O_RDONLY;

/// Unbuffered I/O flag. macOS has no `O_DIRECT`; define as 0 so flag
/// checks are always false (harmless no-op).
#[cfg(target_os = "linux")]
pub const O_DIRECT: libc::c_int = libc::O_DIRECT;
#[cfg(not(target_os = "linux"))]
pub const O_DIRECT: libc::c_int = 0;

/// Resolve an open file descriptor to its filesystem path.
///
/// Linux: `readlink("/proc/self/fd/{fd}")`
/// macOS: `fcntl(fd, F_GETPATH)`
#[cfg(target_os = "linux")]
pub fn fd_to_path(fd: RawFd) -> io::Result<CString> {
    let link = format!("/proc/self/fd/{fd}");
    let link_cstr = CString::new(link).unwrap();

    let mut buf = vec![0u8; libc::PATH_MAX as usize + 1];
    let ret = unsafe {
        libc::readlink(
            link_cstr.as_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len() - 1,
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(ret as usize);
    buf.push(0);
    CString::from_vec_with_nul(buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(target_os = "macos")]
pub fn fd_to_path(fd: RawFd) -> io::Result<CString> {
    let mut buf = vec![0u8; libc::PATH_MAX as usize + 1];
    let ret = unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    // F_GETPATH writes a NUL-terminated string
    let nul_pos = buf.iter().position(|&b| b == 0).unwrap_or(buf.len() - 1);
    buf.truncate(nul_pos + 1);
    CString::from_vec_with_nul(buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Open `/proc/self/fd` as a directory.
///
/// Linux: opens the procfs directory for fd path resolution.
/// macOS: returns `None` (no procfs — use `fd_to_path` + `fcntl` instead).
#[cfg(target_os = "linux")]
pub fn open_proc_self_fd() -> Option<File> {
    let path = CString::new("/proc/self/fd").unwrap();
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        None
    } else {
        Some(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(target_os = "macos")]
pub fn open_proc_self_fd() -> Option<File> {
    None
}

/// Reopen a file descriptor with new flags, bypassing the original open mode.
///
/// Linux: opens `/proc/self/fd/{fd}` to get a new fd with the requested flags.
/// macOS: uses `fcntl(F_GETPATH)` to resolve the path, then opens with new flags.
///
/// `proc_self_fd` is an open fd to `/proc/self/fd` (Linux only). If `None` on
/// Linux, falls back to constructing the path directly.
#[cfg(target_os = "linux")]
pub fn reopen_fd(
    fd: &impl AsRawFd,
    flags: libc::c_int,
    proc_self_fd: Option<&File>,
) -> io::Result<File> {
    let path = format!("{}\0", fd.as_raw_fd());
    let path_cstr = std::ffi::CStr::from_bytes_with_nul(path.as_bytes()).unwrap();
    let clean_flags = flags & !libc::O_NOFOLLOW;

    let dir_fd = if let Some(psfd) = proc_self_fd {
        psfd.as_raw_fd()
    } else {
        // Fallback: open /proc/self/fd inline
        let proc_path = CString::new("/proc/self/fd").unwrap();
        let pfd = unsafe {
            libc::open(
                proc_path.as_ptr(),
                libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if pfd < 0 {
            return Err(io::Error::last_os_error());
        }
        // We'll need to close this after openat
        let result_fd = unsafe {
            libc::openat(pfd, path_cstr.as_ptr(), clean_flags)
        };
        unsafe { libc::close(pfd) };
        if result_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(unsafe { File::from_raw_fd(result_fd) });
    };

    let result_fd = unsafe {
        libc::openat(dir_fd, path_cstr.as_ptr(), clean_flags)
    };
    if result_fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(result_fd) })
    }
}

#[cfg(target_os = "macos")]
pub fn reopen_fd(
    fd: &impl AsRawFd,
    flags: libc::c_int,
    _proc_self_fd: Option<&File>,
) -> io::Result<File> {
    let path = fd_to_path(fd.as_raw_fd())?;
    let clean_flags = flags & !libc::O_NOFOLLOW;
    let result_fd = unsafe { libc::open(path.as_ptr(), clean_flags | libc::O_CLOEXEC) };
    if result_fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(result_fd) })
    }
}

/// Open a file descriptor for path-based operations (metadata only).
///
/// Linux: `openat(dirfd, name, O_PATH | O_NOFOLLOW | O_CLOEXEC | extra_flags)`
/// macOS: `openat(dirfd, name, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | extra_flags)`
///        (macOS has no O_PATH; O_RDONLY is the closest equivalent)
#[cfg(target_os = "linux")]
pub fn open_path_fd(
    dir_fd: RawFd,
    name: &std::ffi::CStr,
    extra_flags: i32,
) -> io::Result<RawFd> {
    let fd = unsafe {
        libc::openat(
            dir_fd,
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC | extra_flags,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

#[cfg(target_os = "macos")]
pub fn open_path_fd(
    dir_fd: RawFd,
    name: &std::ffi::CStr,
    extra_flags: i32,
) -> io::Result<RawFd> {
    let fd = unsafe {
        libc::openat(
            dir_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | extra_flags,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}
