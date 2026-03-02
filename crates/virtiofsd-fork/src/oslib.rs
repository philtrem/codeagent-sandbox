// SPDX-License-Identifier: BSD-3-Clause

use crate::soft_idmap::{HostGid, HostUid, Id};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Error, Result};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::prelude::FromRawFd;

// Re-export compat types that replace Linux-specific ones
pub use crate::compat::io_ops::{ReadvFlags, WritevFlags};
pub use crate::compat::os_facts::OsFacts;

// A helper function that check the return value of a C function call
// and wraps it in a `Result` type, returning the `errno` code as `Err`.
fn check_retval<T: From<i8> + PartialEq>(t: T) -> Result<T> {
    if t == T::from(-1_i8) {
        Err(Error::last_os_error())
    } else {
        Ok(t)
    }
}

// ======================== Linux-only mount operations ========================
#[cfg(target_os = "linux")]
pub fn mount(source: Option<&str>, target: &str, fstype: Option<&str>, flags: u64) -> Result<()> {
    let source = CString::new(source.unwrap_or("")).unwrap();
    let source = source.as_ptr();

    let target = CString::new(target).unwrap();
    let target = target.as_ptr();

    let fstype = CString::new(fstype.unwrap_or("")).unwrap();
    let fstype = fstype.as_ptr();

    check_retval(unsafe { libc::mount(source, target, fstype, flags, std::ptr::null()) })?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn umount2(target: &str, flags: i32) -> Result<()> {
    let target = CString::new(target).unwrap();
    let target = target.as_ptr();

    check_retval(unsafe { libc::umount2(target, flags) })?;
    Ok(())
}

// ======================== POSIX-portable functions ========================

/// Safe wrapper for `fchdir(2)`
pub fn fchdir(fd: RawFd) -> Result<()> {
    check_retval(unsafe { libc::fchdir(fd) })?;
    Ok(())
}

/// Safe wrapper for `fchmod(2)`
pub fn fchmod(fd: RawFd, mode: libc::mode_t) -> Result<()> {
    check_retval(unsafe { libc::fchmod(fd, mode) })?;
    Ok(())
}

/// Safe wrapper for `fchmodat(2)`
pub fn fchmodat(dirfd: RawFd, pathname: String, mode: libc::mode_t, flags: i32) -> Result<()> {
    let pathname =
        CString::new(pathname).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let pathname = pathname.as_ptr();

    check_retval(unsafe { libc::fchmodat(dirfd, pathname, mode, flags) })?;
    Ok(())
}

/// Safe wrapper for `umask(2)`
pub fn umask(mask: u32) -> u32 {
    unsafe { libc::umask(mask) }
}

pub struct ScopedUmask {
    umask: libc::mode_t,
}

impl ScopedUmask {
    pub fn new(new_umask: u32) -> Self {
        Self {
            umask: umask(new_umask),
        }
    }
}

impl Drop for ScopedUmask {
    fn drop(&mut self) {
        umask(self.umask);
    }
}

/// Safe wrapper around `openat(2)`.
pub fn openat(dir: &impl AsRawFd, pathname: &CStr, flags: i32, mode: Option<u32>) -> Result<RawFd> {
    let mode = u64::from(mode.unwrap_or(0));

    check_retval(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            pathname.as_ptr(),
            flags as libc::c_int,
            mode,
        )
    })
}

// ======================== openat2 (Linux 5.6+) ========================
#[cfg(target_os = "linux")]
pub fn do_open_relative_to(
    dir: &impl AsRawFd,
    pathname: &CStr,
    flags: i32,
    mode: Option<u32>,
) -> Result<RawFd> {
    let mode = u64::from(mode.unwrap_or(0)) & 0o7777;

    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.resolve = libc::RESOLVE_IN_ROOT | libc::RESOLVE_NO_MAGICLINKS;
    how.flags = flags as u64;
    how.mode = mode;

    check_retval(unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dir.as_raw_fd(),
            pathname.as_ptr(),
            std::ptr::addr_of!(how),
            std::mem::size_of::<libc::open_how>(),
        )
    } as RawFd)
}

// macOS fallback: no openat2, use regular openat
#[cfg(target_os = "macos")]
pub fn do_open_relative_to(
    dir: &impl AsRawFd,
    pathname: &CStr,
    flags: i32,
    mode: Option<u32>,
) -> Result<RawFd> {
    // On macOS, fall back to openat without RESOLVE_IN_ROOT safety.
    // Path containment is enforced at a higher level.
    openat(dir, pathname, flags, mode)
}

// ======================== File handles (Linux-only) ========================
#[cfg(target_os = "linux")]
mod filehandle {
    use crate::passthrough::file_handle::SerializableFileHandle;
    use crate::util::other_io_error;
    use std::convert::{TryFrom, TryInto};
    use std::io;

    const MAX_HANDLE_SZ: usize = 128;

    #[derive(Clone, PartialOrd, Ord, PartialEq, Eq)]
    #[repr(C)]
    pub struct CFileHandle {
        handle_bytes: libc::c_uint,
        handle_type: libc::c_int,
        f_handle: [u8; MAX_HANDLE_SZ],
    }

    impl Default for CFileHandle {
        fn default() -> Self {
            CFileHandle {
                handle_bytes: MAX_HANDLE_SZ as libc::c_uint,
                handle_type: 0,
                f_handle: [0; MAX_HANDLE_SZ],
            }
        }
    }

    impl CFileHandle {
        pub fn as_bytes(&self) -> &[u8] {
            &self.f_handle[..(self.handle_bytes as usize)]
        }

        pub fn handle_type(&self) -> libc::c_int {
            self.handle_type
        }
    }

    impl TryFrom<&SerializableFileHandle> for CFileHandle {
        type Error = io::Error;

        fn try_from(sfh: &SerializableFileHandle) -> io::Result<Self> {
            let sfh_bytes = sfh.as_bytes();
            if sfh_bytes.len() > MAX_HANDLE_SZ {
                return Err(other_io_error("File handle too long"));
            }
            let mut f_handle = [0u8; MAX_HANDLE_SZ];
            f_handle[..sfh_bytes.len()].copy_from_slice(sfh_bytes);

            Ok(CFileHandle {
                handle_bytes: sfh_bytes.len().try_into().map_err(|err| {
                    other_io_error(format!(
                        "Handle size ({} bytes) too big: {err}",
                        sfh_bytes.len(),
                    ))
                })?,
                #[allow(clippy::useless_conversion)]
                handle_type: sfh.handle_type().try_into().map_err(|err| {
                    other_io_error(format!(
                        "Handle type (0x{:x}) too large: {err}",
                        sfh.handle_type(),
                    ))
                })?,
                f_handle,
            })
        }
    }

    extern "C" {
        pub fn name_to_handle_at(
            dirfd: libc::c_int,
            pathname: *const libc::c_char,
            file_handle: *mut CFileHandle,
            mount_id: *mut libc::c_int,
            flags: libc::c_int,
        ) -> libc::c_int;

        pub fn open_by_handle_at(
            mount_fd: libc::c_int,
            file_handle: *const CFileHandle,
            flags: libc::c_int,
        ) -> libc::c_int;
    }
}

// macOS stub: file handles are not available
#[cfg(target_os = "macos")]
mod filehandle {
    #[derive(Clone, Default, PartialOrd, Ord, PartialEq, Eq)]
    pub struct CFileHandle {
        _private: (),
    }

    impl CFileHandle {
        pub fn as_bytes(&self) -> &[u8] {
            &[]
        }

        pub fn handle_type(&self) -> libc::c_int {
            0
        }
    }
}

pub use filehandle::CFileHandle;

#[cfg(target_os = "linux")]
pub fn name_to_handle_at(
    dirfd: &impl AsRawFd,
    pathname: &CStr,
    file_handle: &mut CFileHandle,
    mount_id: &mut libc::c_int,
    flags: libc::c_int,
) -> Result<()> {
    check_retval(unsafe {
        filehandle::name_to_handle_at(
            dirfd.as_raw_fd(),
            pathname.as_ptr(),
            file_handle,
            mount_id,
            flags,
        )
    })?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn name_to_handle_at(
    _dirfd: &impl AsRawFd,
    _pathname: &CStr,
    _file_handle: &mut CFileHandle,
    _mount_id: &mut libc::c_int,
    _flags: libc::c_int,
) -> Result<()> {
    Err(io::Error::from_raw_os_error(libc::ENOTSUP))
}

#[cfg(target_os = "linux")]
pub fn open_by_handle_at(
    mount_fd: &impl AsRawFd,
    file_handle: &CFileHandle,
    flags: libc::c_int,
) -> Result<File> {
    let fd = check_retval(unsafe {
        filehandle::open_by_handle_at(mount_fd.as_raw_fd(), file_handle, flags)
    })?;

    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(target_os = "macos")]
pub fn open_by_handle_at(
    _mount_fd: &impl AsRawFd,
    _file_handle: &CFileHandle,
    _flags: libc::c_int,
) -> Result<File> {
    Err(io::Error::from_raw_os_error(libc::ENOTSUP))
}

// ======================== Vectored I/O (delegated to compat) ========================
pub use crate::compat::io_ops::{readv_at, writev_at};

// ======================== Pipe ========================

pub struct PipeReader(File);

impl io::Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

pub struct PipeWriter(File);

impl io::Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(target_os = "linux")]
pub fn pipe() -> io::Result<(PipeReader, PipeWriter)> {
    let mut fds: [RawFd; 2] = [-1, -1];
    let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if ret == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok((
            PipeReader(unsafe { File::from_raw_fd(fds[0]) }),
            PipeWriter(unsafe { File::from_raw_fd(fds[1]) }),
        ))
    }
}

#[cfg(target_os = "macos")]
pub fn pipe() -> io::Result<(PipeReader, PipeWriter)> {
    let mut fds: [RawFd; 2] = [-1, -1];
    let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if ret == -1 {
        return Err(io::Error::last_os_error());
    }
    // Set O_CLOEXEC on both ends
    for &fd in &fds {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok((
        PipeReader(unsafe { File::from_raw_fd(fds[0]) }),
        PipeWriter(unsafe { File::from_raw_fd(fds[1]) }),
    ))
}

// ======================== Credential switching (delegated to compat) ========================

/// Set effective user ID
pub fn seteffuid(uid: HostUid) -> io::Result<()> {
    crate::compat::credentials::seteffuid(uid.into_inner())
}

/// Set effective group ID
pub fn seteffgid(gid: HostGid) -> io::Result<()> {
    crate::compat::credentials::seteffgid(gid.into_inner())
}

/// Set supplementary group
pub fn setsupgroup(gid: HostGid) -> io::Result<()> {
    crate::compat::credentials::setsupgroup(gid.into_inner())
}

/// Drop all supplementary groups
pub fn dropsupgroups() -> io::Result<()> {
    crate::compat::credentials::dropsupgroups()
}
