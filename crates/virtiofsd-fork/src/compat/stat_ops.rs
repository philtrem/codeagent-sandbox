// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! Cross-platform extended stat (`statx` on Linux, `fstatat` on macOS).

use super::types::stat64;
use std::ffi::CStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::io::AsRawFd;

/// Extended stat result with mount ID.
pub struct StatExt {
    pub st: stat64,
    pub mnt_id: u64,
}

const EMPTY_CSTR: &[u8] = b"\0";

// ======================== Linux implementation ========================
#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    mod file_status {
        #[cfg(target_env = "gnu")]
        pub use libc::statx as statx_st;
        #[cfg(target_env = "gnu")]
        pub use libc::{STATX_BASIC_STATS, STATX_MNT_ID};

        #[cfg(not(target_env = "gnu"))]
        #[repr(C)]
        pub struct statx_st_timestamp {
            pub tv_sec: i64,
            pub tv_nsec: u32,
            pub __statx_timestamp_pad1: [i32; 1],
        }

        #[cfg(not(target_env = "gnu"))]
        #[repr(C)]
        pub struct statx_st {
            pub stx_mask: u32,
            pub stx_blksize: u32,
            pub stx_attributes: u64,
            pub stx_nlink: u32,
            pub stx_uid: u32,
            pub stx_gid: u32,
            pub stx_mode: u16,
            __statx_pad1: [u16; 1],
            pub stx_ino: u64,
            pub stx_size: u64,
            pub stx_blocks: u64,
            pub stx_attributes_mask: u64,
            pub stx_atime: statx_st_timestamp,
            pub stx_btime: statx_st_timestamp,
            pub stx_ctime: statx_st_timestamp,
            pub stx_mtime: statx_st_timestamp,
            pub stx_rdev_major: u32,
            pub stx_rdev_minor: u32,
            pub stx_dev_major: u32,
            pub stx_dev_minor: u32,
            pub stx_mnt_id: u64,
            __statx_pad2: u64,
            __statx_pad3: [u64; 12],
        }

        #[cfg(not(target_env = "gnu"))]
        pub const STATX_BASIC_STATS: libc::c_uint = 0x07ff;

        #[cfg(not(target_env = "gnu"))]
        pub const STATX_MNT_ID: libc::c_uint = 0x1000;
    }

    use file_status::{statx_st, STATX_BASIC_STATS, STATX_MNT_ID};

    trait SafeStatXAccess {
        fn stat64(&self) -> Option<stat64>;
        fn mount_id(&self) -> Option<u64>;
    }

    impl SafeStatXAccess for statx_st {
        fn stat64(&self) -> Option<stat64> {
            fn makedev(maj: libc::c_uint, min: libc::c_uint) -> libc::dev_t {
                libc::makedev(maj, min)
            }

            if self.stx_mask & STATX_BASIC_STATS != 0 {
                let mut st = unsafe { MaybeUninit::<stat64>::zeroed().assume_init() };

                st.st_dev = makedev(self.stx_dev_major, self.stx_dev_minor);
                st.st_ino = self.stx_ino;
                st.st_mode = self.stx_mode as _;
                st.st_nlink = self.stx_nlink as _;
                st.st_uid = self.stx_uid;
                st.st_gid = self.stx_gid;
                st.st_rdev = makedev(self.stx_rdev_major, self.stx_rdev_minor);
                st.st_size = self.stx_size as _;
                st.st_blksize = self.stx_blksize as _;
                st.st_blocks = self.stx_blocks as _;
                st.st_atime = self.stx_atime.tv_sec;
                st.st_atime_nsec = self.stx_atime.tv_nsec as _;
                st.st_mtime = self.stx_mtime.tv_sec;
                st.st_mtime_nsec = self.stx_mtime.tv_nsec as _;
                st.st_ctime = self.stx_ctime.tv_sec;
                st.st_ctime_nsec = self.stx_ctime.tv_nsec as _;

                Some(st)
            } else {
                None
            }
        }

        fn mount_id(&self) -> Option<u64> {
            if self.stx_mask & STATX_MNT_ID != 0 {
                Some(self.stx_mnt_id)
            } else {
                None
            }
        }
    }

    unsafe fn do_statx(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mask: libc::c_uint,
        statxbuf: *mut statx_st,
    ) -> libc::c_int {
        libc::syscall(libc::SYS_statx, dirfd, pathname, flags, mask, statxbuf) as libc::c_int
    }

    pub fn statx(dir: &impl AsRawFd, path: Option<&CStr>) -> io::Result<StatExt> {
        let mut stx_ui = MaybeUninit::<statx_st>::zeroed();

        let path =
            path.unwrap_or_else(|| unsafe { CStr::from_bytes_with_nul_unchecked(EMPTY_CSTR) });

        let res = unsafe {
            do_statx(
                dir.as_raw_fd(),
                path.as_ptr(),
                libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
                STATX_BASIC_STATS | STATX_MNT_ID,
                stx_ui.as_mut_ptr(),
            )
        };
        if res >= 0 {
            let stx = unsafe { stx_ui.assume_init() };

            let mnt_id = stx.mount_id().unwrap_or(0);

            Ok(StatExt {
                st: stx
                    .stat64()
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOSYS))?,
                mnt_id,
            })
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

// ======================== macOS implementation ========================
#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub fn statx(dir: &impl AsRawFd, path: Option<&CStr>) -> io::Result<StatExt> {
        let mut st = MaybeUninit::<stat64>::zeroed();

        let path =
            path.unwrap_or_else(|| unsafe { CStr::from_bytes_with_nul_unchecked(EMPTY_CSTR) });

        let res = unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                path.as_ptr(),
                st.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };

        if res < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(StatExt {
            st: unsafe { st.assume_init() },
            mnt_id: 0, // mount ID not available on macOS
        })
    }
}

pub use platform::statx;
