// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! Cross-platform vectored I/O with optional per-call flags.

use std::io;
use std::os::unix::io::{AsRawFd, BorrowedFd};

use bitflags::bitflags;

// ======================== Linux: pwritev2/preadv2 ========================
#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    bitflags! {
        pub struct WritevFlags: i32 {
            const RWF_HIPRI = libc::RWF_HIPRI;
            const RWF_DSYNC = libc::RWF_DSYNC;
            const RWF_SYNC = libc::RWF_SYNC;
            const RWF_APPEND = libc::RWF_APPEND;
            const RWF_NOAPPEND = libc::RWF_NOAPPEND;
            const RWF_ATOMIC = libc::RWF_ATOMIC;
            const RWF_DONTCACHE = libc::RWF_DONTCACHE;
        }
    }

    bitflags! {
        pub struct ReadvFlags: i32 {
            const RWF_HIPRI = libc::RWF_HIPRI;
            const RWF_NOWAIT = libc::RWF_NOWAIT;
            const RWF_DONTCACHE = libc::RWF_DONTCACHE;
        }
    }

    fn check_retval(ret: isize) -> io::Result<usize> {
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }

    pub unsafe fn writev_at(
        fd: BorrowedFd,
        iovecs: &[libc::iovec],
        offset: i64,
        flags: Option<WritevFlags>,
    ) -> io::Result<usize> {
        let flags = flags.unwrap_or(WritevFlags::empty());
        check_retval(unsafe {
            libc::pwritev2(
                fd.as_raw_fd(),
                iovecs.as_ptr(),
                iovecs.len() as libc::c_int,
                offset,
                flags.bits(),
            )
        } as isize)
    }

    pub unsafe fn readv_at(
        fd: BorrowedFd,
        iovecs: &[libc::iovec],
        offset: i64,
        flags: Option<ReadvFlags>,
    ) -> io::Result<usize> {
        let flags = flags.unwrap_or(ReadvFlags::empty());
        check_retval(unsafe {
            libc::preadv2(
                fd.as_raw_fd(),
                iovecs.as_ptr(),
                iovecs.len() as libc::c_int,
                offset,
                flags.bits(),
            )
        } as isize)
    }
}

// ======================== macOS: pwritev/preadv (no flags) ========================
#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    bitflags! {
        /// Write flags — defined for API compatibility but ignored on macOS.
        pub struct WritevFlags: i32 {
            const RWF_HIPRI = 0x01;
            const RWF_DSYNC = 0x02;
            const RWF_SYNC = 0x04;
            const RWF_APPEND = 0x10;
            const RWF_NOAPPEND = 0x20;
            const RWF_ATOMIC = 0x40;
            const RWF_DONTCACHE = 0x80;
        }
    }

    bitflags! {
        /// Read flags — defined for API compatibility but ignored on macOS.
        pub struct ReadvFlags: i32 {
            const RWF_HIPRI = 0x01;
            const RWF_NOWAIT = 0x08;
            const RWF_DONTCACHE = 0x80;
        }
    }

    fn check_retval(ret: isize) -> io::Result<usize> {
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }

    pub unsafe fn writev_at(
        fd: BorrowedFd,
        iovecs: &[libc::iovec],
        offset: i64,
        _flags: Option<WritevFlags>,
    ) -> io::Result<usize> {
        // macOS pwritev has no flags parameter; flags are silently ignored
        check_retval(unsafe {
            libc::pwritev(
                fd.as_raw_fd(),
                iovecs.as_ptr(),
                iovecs.len() as libc::c_int,
                offset,
            )
        } as isize)
    }

    pub unsafe fn readv_at(
        fd: BorrowedFd,
        iovecs: &[libc::iovec],
        offset: i64,
        _flags: Option<ReadvFlags>,
    ) -> io::Result<usize> {
        check_retval(unsafe {
            libc::preadv(
                fd.as_raw_fd(),
                iovecs.as_ptr(),
                iovecs.len() as libc::c_int,
                offset,
            )
        } as isize)
    }
}

pub use platform::*;
