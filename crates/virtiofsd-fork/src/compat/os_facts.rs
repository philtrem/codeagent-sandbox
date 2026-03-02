// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! OS feature detection.

use std::ffi::CString;

/// Facts about available syscalls on the current OS.
pub struct OsFacts {
    pub has_openat2: bool,
}

#[allow(clippy::new_without_default)]
impl OsFacts {
    #[must_use]
    #[cfg(target_os = "linux")]
    pub fn new() -> Self {
        // Probe for openat2() (Linux 5.6+)
        // SAFETY: all-zero byte-pattern is a valid libc::open_how
        let how: libc::open_how = unsafe { std::mem::zeroed() };
        let cwd = CString::new(".").unwrap();
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                libc::AT_FDCWD,
                cwd.as_ptr(),
                std::ptr::addr_of!(how),
                std::mem::size_of::<libc::open_how>(),
            )
        };

        let has_openat2 = fd >= 0;
        if has_openat2 {
            unsafe { libc::close(fd as libc::c_int) };
        }

        Self { has_openat2 }
    }

    #[must_use]
    #[cfg(target_os = "macos")]
    pub fn new() -> Self {
        // openat2 is Linux-only; macOS always uses the openat fallback
        let _ = CString::new("."); // suppress unused import
        Self { has_openat2: false }
    }
}
