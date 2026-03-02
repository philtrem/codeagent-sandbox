// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! Cross-platform `renameat2` with flag support.

use std::os::unix::io::RawFd;

/// Linux FUSE rename flags, defined as cross-platform constants.
/// These match the Linux kernel values used in the FUSE wire protocol.
#[cfg(target_os = "linux")]
pub use libc::{RENAME_EXCHANGE, RENAME_NOREPLACE, RENAME_WHITEOUT};

#[cfg(not(target_os = "linux"))]
pub const RENAME_EXCHANGE: libc::c_uint = 2;
#[cfg(not(target_os = "linux"))]
pub const RENAME_NOREPLACE: libc::c_uint = 1;
#[cfg(not(target_os = "linux"))]
pub const RENAME_WHITEOUT: libc::c_uint = 4;

/// Rename with flags (RENAME_NOREPLACE, RENAME_EXCHANGE, RENAME_WHITEOUT).
///
/// Linux: `syscall(SYS_renameat2, olddirfd, oldpath, newdirfd, newpath, flags)`
/// macOS: `renameatx_np(olddirfd, oldpath, newdirfd, newpath, translated_flags)`
///
/// Returns the raw syscall return value (0 on success, negative on error).
/// Caller must check `io::Error::last_os_error()` on failure.
#[cfg(target_os = "linux")]
pub unsafe fn safe_renameat2(
    olddirfd: RawFd,
    oldpath: *const libc::c_char,
    newdirfd: RawFd,
    newpath: *const libc::c_char,
    flags: u32,
) -> libc::c_long {
    libc::syscall(
        libc::SYS_renameat2,
        olddirfd,
        oldpath,
        newdirfd,
        newpath,
        flags,
    )
}

#[cfg(target_os = "macos")]
pub unsafe fn safe_renameat2(
    olddirfd: RawFd,
    oldpath: *const libc::c_char,
    newdirfd: RawFd,
    newpath: *const libc::c_char,
    flags: u32,
) -> libc::c_long {
    if flags == 0 {
        // Plain rename — use standard renameat
        return libc::renameat(olddirfd, oldpath, newdirfd, newpath) as libc::c_long;
    }

    // macOS has renameatx_np with its own flag constants
    let mut macos_flags: libc::c_uint = 0;

    // RENAME_EXCHANGE (Linux 0x2) → RENAME_SWAP (macOS 0x2)
    if flags & RENAME_EXCHANGE != 0 {
        macos_flags |= libc::RENAME_SWAP;
    }

    // RENAME_NOREPLACE (Linux 0x1) → RENAME_EXCL (macOS 0x4)
    #[allow(clippy::unnecessary_cast)]
    if flags & RENAME_NOREPLACE != 0 {
        macos_flags |= libc::RENAME_EXCL as libc::c_uint;
    }

    // RENAME_WHITEOUT has no macOS equivalent — return ENOTSUP
    if flags & RENAME_WHITEOUT != 0 {
        *libc::__error() = libc::ENOTSUP;
        return -1;
    }

    libc::renameatx_np(olddirfd, oldpath, newdirfd, newpath, macos_flags) as libc::c_long
}
