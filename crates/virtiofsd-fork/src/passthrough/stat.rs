// Copyright 2021 Red Hat, Inc. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::CStr;
use std::io;
use std::os::unix::io::AsRawFd;

// Re-export StatExt from the compat layer (includes stat64 + mnt_id)
pub use crate::compat::stat_ops::StatExt;

// Keep file_status module on Linux for any code that references it directly
#[cfg(target_os = "linux")]
mod file_status;

pub type MountId = u64;

const EMPTY_CSTR: &[u8] = b"\0";

// On Linux, try name_to_handle_at as fallback for mount ID
#[cfg(target_os = "linux")]
fn get_mount_id(dir: &impl AsRawFd, path: &CStr) -> Option<MountId> {
    use crate::oslib;

    let mut mount_id: libc::c_int = 0;
    let mut c_fh = oslib::CFileHandle::default();

    oslib::name_to_handle_at(dir, path, &mut c_fh, &mut mount_id, libc::AT_EMPTY_PATH)
        .ok()
        .and(Some(mount_id as MountId))
}

#[cfg(target_os = "macos")]
fn get_mount_id(_dir: &impl AsRawFd, _path: &CStr) -> Option<MountId> {
    None
}

/// Extended stat, delegating to the compat layer for platform-specific implementation.
pub fn statx(dir: &impl AsRawFd, path: Option<&CStr>) -> io::Result<StatExt> {
    let result = crate::compat::stat_ops::statx(dir, path)?;

    // If the compat layer didn't provide a mount ID, try name_to_handle_at as fallback
    if result.mnt_id == 0 {
        let path = path.unwrap_or_else(|| unsafe {
            CStr::from_bytes_with_nul_unchecked(EMPTY_CSTR)
        });
        if let Some(mnt_id) = get_mount_id(dir, path) {
            return Ok(StatExt {
                st: result.st,
                mnt_id,
            });
        }
    }

    Ok(result)
}
