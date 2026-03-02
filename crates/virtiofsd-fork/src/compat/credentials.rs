// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

//! Cross-platform credentials and capabilities.
//!
//! Linux: Uses per-thread credential syscalls (SYS_setresuid/SYS_setresgid)
//! and POSIX capabilities via capng.
//!
//! macOS: Uses process-wide seteuid/setegid (no per-thread credential support)
//! and no-op capability functions (macOS has no POSIX capabilities).

use std::io;

// ======================== Per-thread credential switching ========================

/// Set effective user ID (per-thread on Linux, process-wide on macOS).
#[cfg(target_os = "linux")]
pub fn seteffuid(uid: u32) -> io::Result<()> {
    let ret = unsafe { libc::syscall(libc::SYS_setresuid, -1i32, uid, -1i32) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn seteffuid(uid: u32) -> io::Result<()> {
    let ret = unsafe { libc::seteuid(uid) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Set effective group ID (per-thread on Linux, process-wide on macOS).
#[cfg(target_os = "linux")]
pub fn seteffgid(gid: u32) -> io::Result<()> {
    let ret = unsafe { libc::syscall(libc::SYS_setresgid, -1i32, gid, -1i32) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn seteffgid(gid: u32) -> io::Result<()> {
    let ret = unsafe { libc::setegid(gid) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Set supplementary group.
#[cfg(target_os = "linux")]
pub fn setsupgroup(gid: u32) -> io::Result<()> {
    let ret = unsafe { libc::syscall(libc::SYS_setgroups, 1, &gid) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn setsupgroup(gid: u32) -> io::Result<()> {
    let gid_val = gid as libc::gid_t;
    let ret = unsafe { libc::setgroups(1, &gid_val) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Drop all supplementary groups.
#[cfg(target_os = "linux")]
pub fn dropsupgroups() -> io::Result<()> {
    let ret = unsafe {
        libc::syscall(libc::SYS_setgroups, 0, std::ptr::null::<libc::gid_t>())
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn dropsupgroups() -> io::Result<()> {
    let ret = unsafe { libc::setgroups(0, std::ptr::null()) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// ======================== POSIX capabilities ========================

/// Add a capability to the effective set.
///
/// Linux: Uses `capng` crate.
/// macOS: No-op (macOS has no POSIX capabilities).
#[cfg(target_os = "linux")]
pub fn add_cap_to_eff(cap_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    use capng::{Action, CUpdate, Set, Type};
    let cap = capng::name_to_capability(cap_name)?;
    capng::get_caps_process()?;
    let req = vec![CUpdate {
        action: Action::ADD,
        cap_type: Type::EFFECTIVE,
        capability: cap,
    }];
    capng::update(req)?;
    capng::apply(Set::CAPS)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn add_cap_to_eff(_cap_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

/// Check whether the process has a specific effective capability.
///
/// Linux: Uses `capng::have_capability`.
/// macOS: Always returns `false`.
#[cfg(target_os = "linux")]
pub fn have_capability(cap_name: &str) -> bool {
    if let Ok(cap) = capng::name_to_capability(cap_name) {
        capng::have_capability(capng::Type::EFFECTIVE, cap)
    } else {
        false
    }
}

#[cfg(target_os = "macos")]
pub fn have_capability(_cap_name: &str) -> bool {
    false
}

/// Scoped capability drop and restore.
///
/// Linux: Drops the named capability from the effective set and restores it on Drop.
/// macOS: No-op (returns `None`).
pub struct ScopedCaps {
    #[cfg(target_os = "linux")]
    cap: capng::Capability,
    #[cfg(target_os = "macos")]
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(target_os = "linux")]
impl ScopedCaps {
    pub fn new(cap_name: &str) -> io::Result<Option<Self>> {
        use capng::{Action, CUpdate, Set, Type};

        let cap = capng::name_to_capability(cap_name).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown capability: {cap_name}"),
            )
        })?;

        if capng::have_capability(Type::EFFECTIVE, cap) {
            let req = vec![CUpdate {
                action: Action::DROP,
                cap_type: Type::EFFECTIVE,
                capability: cap,
            }];
            capng::update(req).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("couldn't drop {cap} capability: {e:?}"),
                )
            })?;
            capng::apply(Set::CAPS).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("couldn't apply capabilities: {e:?}"),
                )
            })?;
            Ok(Some(Self { cap }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for ScopedCaps {
    fn drop(&mut self) {
        use capng::{Action, CUpdate, Set, Type};

        let req = vec![CUpdate {
            action: Action::ADD,
            cap_type: Type::EFFECTIVE,
            capability: self.cap,
        }];

        if let Err(e) = capng::update(req) {
            panic!("couldn't restore {} capability: {:?}", self.cap, e);
        }
        if let Err(e) = capng::apply(Set::CAPS) {
            panic!(
                "couldn't apply capabilities after restoring {}: {:?}",
                self.cap, e
            );
        }
    }
}

#[cfg(target_os = "macos")]
impl ScopedCaps {
    pub fn new(_cap_name: &str) -> io::Result<Option<Self>> {
        Ok(None)
    }
}

/// Drop a capability from the effective set for the duration of a scope.
pub fn drop_effective_cap(cap_name: &str) -> io::Result<Option<ScopedCaps>> {
    ScopedCaps::new(cap_name)
}

// ======================== Process isolation ========================

/// Clear all capabilities and apply.
///
/// Linux: Uses `capng::clear` + `capng::apply`.
/// macOS: No-op.
#[cfg(target_os = "linux")]
pub fn clear_all_caps() -> io::Result<()> {
    capng::clear(capng::Set::BOTH);
    capng::apply(capng::Set::BOTH).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("can't apply capabilities: {e}"),
        )
    })
}

#[cfg(target_os = "macos")]
pub fn clear_all_caps() -> io::Result<()> {
    Ok(())
}
