// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! Cross-platform type aliases for 64-bit filesystem types.
//!
//! On Linux, the `*64` variants (`stat64`, `off64_t`, `ino64_t`) are explicit 64-bit types.
//! On macOS, the base types (`stat`, `off_t`, `ino_t`) are already 64-bit, so these aliases
//! map directly to them.

/// 64-bit stat struct.
#[cfg(target_os = "linux")]
pub type stat64 = libc::stat64;
#[cfg(not(target_os = "linux"))]
pub type stat64 = libc::stat;

/// 64-bit file offset type.
#[cfg(target_os = "linux")]
pub type off64_t = libc::off64_t;
#[cfg(not(target_os = "linux"))]
pub type off64_t = libc::off_t;

/// 64-bit inode number type.
#[cfg(target_os = "linux")]
pub type ino64_t = libc::ino64_t;
#[cfg(not(target_os = "linux"))]
pub type ino64_t = libc::ino_t;

/// Cross-platform `lseek64`. On macOS, `lseek` is already 64-bit.
pub fn lseek64(fd: libc::c_int, offset: off64_t, whence: libc::c_int) -> off64_t {
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::lseek64(fd, offset, whence) }
    }
    #[cfg(not(target_os = "linux"))]
    {
        unsafe { libc::lseek(fd, offset, whence) }
    }
}

/// Cross-platform `fstatat64`. On macOS, `fstatat` returns 64-bit stat.
pub fn fstatat64(
    dirfd: libc::c_int,
    pathname: *const libc::c_char,
    buf: *mut stat64,
    flags: libc::c_int,
) -> libc::c_int {
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::fstatat64(dirfd, pathname, buf, flags) }
    }
    #[cfg(not(target_os = "linux"))]
    {
        unsafe { libc::fstatat(dirfd, pathname, buf, flags) }
    }
}

/// Cross-platform `fallocate64`. macOS has no fallocate; use F_PREALLOCATE + ftruncate.
#[cfg(target_os = "linux")]
pub fn fallocate64(
    fd: libc::c_int,
    mode: libc::c_int,
    offset: off64_t,
    len: off64_t,
) -> libc::c_int {
    unsafe { libc::fallocate64(fd, mode, offset, len) }
}

#[cfg(not(target_os = "linux"))]
pub fn fallocate64(
    fd: libc::c_int,
    mode: libc::c_int,
    offset: off64_t,
    len: off64_t,
) -> libc::c_int {
    // macOS: FALLOC_FL_KEEP_SIZE / FALLOC_FL_PUNCH_HOLE not supported.
    // For simple preallocation (mode==0), extend via ftruncate.
    // For other modes, return ENOSYS.
    if mode != 0 {
        unsafe { *libc::__error() = libc::ENOSYS };
        return -1;
    }

    let new_size = offset + len;
    let ret = unsafe { libc::ftruncate(fd, new_size) };
    if ret < 0 {
        return ret;
    }
    0
}
