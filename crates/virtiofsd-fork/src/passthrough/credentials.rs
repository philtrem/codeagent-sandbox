// SPDX-License-Identifier: BSD-3-Clause

use crate::oslib;
use crate::soft_idmap::{HostGid, HostUid, Id};
use std::io;

// Re-export ScopedCaps from compat layer
pub use crate::compat::credentials::{drop_effective_cap, ScopedCaps};

pub struct UnixCredentials {
    uid: HostUid,
    gid: HostGid,
    sup_gid: Option<HostGid>,
    keep_capability: bool,
}

impl UnixCredentials {
    pub fn new(uid: HostUid, gid: HostGid) -> Self {
        UnixCredentials {
            uid,
            gid,
            sup_gid: None,
            keep_capability: false,
        }
    }

    /// Set a supplementary group. Set `supported_extension` to `false` to signal that a
    /// supplementary group maybe required, but the guest was not able to tell us which,
    /// so we have to rely on keeping the DAC_OVERRIDE capability.
    pub fn supplementary_gid(self, supported_extension: bool, sup_gid: Option<HostGid>) -> Self {
        UnixCredentials {
            uid: self.uid,
            gid: self.gid,
            sup_gid,
            keep_capability: !supported_extension,
        }
    }

    /// Changes the effective uid/gid of the current thread to `val`.  Changes
    /// the thread's credentials back to root when the returned struct is dropped.
    pub fn set(self) -> io::Result<Option<UnixCredentialsGuard>> {
        // Safe: Always succesful
        let current_uid = HostUid::from(unsafe { libc::geteuid() });
        let current_gid = HostGid::from(unsafe { libc::getegid() });

        // Not to change UID/GID when they're 0 (root) is legacy behavior that we're afraid to
        // change
        let change_uid = !self.uid.is_root() && self.uid != current_uid;
        let change_gid = !self.gid.is_root() && self.gid != current_gid;

        // We have to change the gid before we change the uid because if we
        // change the uid first then we lose the capability to change the gid.
        // However changing back can happen in any order.
        if let Some(sup_gid) = self.sup_gid {
            oslib::setsupgroup(sup_gid)?;
        }

        if change_gid {
            oslib::seteffgid(self.gid)?;
        }

        if change_uid {
            oslib::seteffuid(self.uid)?;
        }

        if change_uid && self.keep_capability {
            if let Err(e) = crate::compat::credentials::add_cap_to_eff("DAC_OVERRIDE") {
                warn!("failed to add 'DAC_OVERRIDE' to the effective set of capabilities: {e}");
            }
        }

        if !change_uid && !change_gid {
            return Ok(None);
        }

        Ok(Some(UnixCredentialsGuard {
            reset_uid: change_uid.then_some(current_uid),
            reset_gid: change_gid.then_some(current_gid),
            drop_sup_gid: self.sup_gid.is_some(),
        }))
    }
}

pub struct UnixCredentialsGuard {
    reset_uid: Option<HostUid>,
    reset_gid: Option<HostGid>,
    drop_sup_gid: bool,
}

impl Drop for UnixCredentialsGuard {
    fn drop(&mut self) {
        if let Some(uid) = self.reset_uid {
            oslib::seteffuid(uid).unwrap_or_else(|e| {
                error!("failed to change uid back to {uid}: {e}");
            });
        }

        if let Some(gid) = self.reset_gid {
            oslib::seteffgid(gid).unwrap_or_else(|e| {
                error!("failed to change gid back to {gid}: {e}");
            });
        }

        if self.drop_sup_gid {
            oslib::dropsupgroups().unwrap_or_else(|e| {
                error!("failed to drop supplementary groups: {e}");
            });
        }
    }
}
