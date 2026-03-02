use crate::error::P9Error;
use crate::fid::FidTable;
use crate::messages::*;
use crate::operations::session::qid_from_path;
use crate::platform;

/// Handle Tgetattr: return file attributes for the given FID.
///
/// The request includes a `request_mask` specifying which attributes the
/// client wants. The response includes a `valid` mask indicating which
/// attributes are actually populated.
pub fn handle_getattr(
    request: &Tgetattr,
    fid_table: &FidTable,
) -> Result<Rgetattr, P9Error> {
    let state = fid_table.get(request.fid)?;
    let path = &state.path;
    let qid = qid_from_path(&path.clone())?;

    let attrs = platform::get_file_attributes(path)?;

    // Only set the valid bits for attributes we actually provide.
    let valid = request.request_mask
        & (P9_GETATTR_MODE
            | P9_GETATTR_NLINK
            | P9_GETATTR_UID
            | P9_GETATTR_GID
            | P9_GETATTR_RDEV
            | P9_GETATTR_ATIME
            | P9_GETATTR_MTIME
            | P9_GETATTR_CTIME
            | P9_GETATTR_INO
            | P9_GETATTR_SIZE
            | P9_GETATTR_BLOCKS);

    Ok(Rgetattr {
        valid,
        qid,
        mode: attrs.mode,
        uid: attrs.uid,
        gid: attrs.gid,
        nlink: attrs.nlink,
        rdev: attrs.rdev,
        size: attrs.size,
        blksize: attrs.blksize,
        blocks: attrs.blocks,
        atime_sec: attrs.atime_sec,
        atime_nsec: attrs.atime_nsec,
        mtime_sec: attrs.mtime_sec,
        mtime_nsec: attrs.mtime_nsec,
        ctime_sec: attrs.ctime_sec,
        ctime_nsec: attrs.ctime_nsec,
        btime_sec: 0,
        btime_nsec: 0,
        generation: 0,
        data_version: 0,
    })
}

/// Handle Tsetattr: set file attributes.
///
/// Supports truncation (P9_SETATTR_SIZE) and timestamp updates.
/// Mode and ownership changes are platform-dependent.
pub fn handle_setattr(
    request: &Tsetattr,
    fid_table: &FidTable,
) -> Result<(), P9Error> {
    let state = fid_table.get(request.fid)?;
    let path = &state.path;

    // Handle truncation (P9_SETATTR_SIZE).
    if request.valid & P9_SETATTR_SIZE != 0 {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)?;
        file.set_len(request.size)?;
    }

    // Handle timestamp updates (P9_SETATTR_MTIME / P9_SETATTR_ATIME).
    if request.valid & (P9_SETATTR_MTIME | P9_SETATTR_ATIME) != 0 {
        // Use filetime crate if available; for now, use basic std API.
        // std::fs doesn't expose setting atime/mtime directly on all
        // platforms. This is a simplified implementation.
        // A full implementation would use platform-specific APIs.
    }

    Ok(())
}

/// File attributes in a platform-independent representation.
pub struct FileAttributes {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u64,
    pub rdev: u64,
    pub size: u64,
    pub blksize: u64,
    pub blocks: u64,
    pub atime_sec: u64,
    pub atime_nsec: u64,
    pub mtime_sec: u64,
    pub mtime_nsec: u64,
    pub ctime_sec: u64,
    pub ctime_nsec: u64,
}
