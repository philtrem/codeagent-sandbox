use std::path::PathBuf;

use crate::error::{errno, P9Error};
use crate::fid::{FidState, FidTable};
use crate::messages::*;
use crate::qid::Qid;

/// The 9P2000.L version string used during protocol negotiation.
pub const P9_VERSION_STRING: &str = "9P2000.L";

/// Handle Tversion: negotiate protocol version and maximum message size.
///
/// The server responds with the minimum of the client's msize and the server's
/// configured maximum, along with the protocol version string. If the version
/// doesn't match 9P2000.L, we respond with "unknown" per the spec.
pub fn handle_version(request: &Tversion, server_msize: u32) -> Rversion {
    let negotiated_msize = request.msize.min(server_msize);
    let version = if request.version == P9_VERSION_STRING {
        P9_VERSION_STRING.to_string()
    } else {
        "unknown".to_string()
    };
    Rversion {
        msize: negotiated_msize,
        version,
    }
}

/// Handle Tauth: authentication is not supported, return EOPNOTSUPP.
///
/// The 9P2000.L protocol allows servers to reject auth by returning Rlerror
/// with EOPNOTSUPP, which tells the client to proceed without authentication.
pub fn handle_auth() -> Rlerror {
    Rlerror {
        ecode: errno::EOPNOTSUPP,
    }
}

/// Handle Tattach: bind a FID to the root of the shared directory.
///
/// Creates the root FID entry in the FID table and returns its QID.
/// The root QID uses a synthetic inode number of 1.
pub fn handle_attach(
    request: &Tattach,
    fid_table: &mut FidTable,
) -> Result<Rattach, P9Error> {
    let root_path = fid_table.root_path().to_path_buf();
    let root_qid = qid_from_path(&root_path)?;

    let state = FidState::new(root_path, root_qid);
    fid_table.insert(request.fid, state)?;

    Ok(Rattach { qid: root_qid })
}

/// Handle Tclunk: release a FID.
///
/// Removes the FID from the table. Any associated open file handle is dropped
/// (closed) when the FidState is dropped.
pub fn handle_clunk(request: &Tclunk, fid_table: &mut FidTable) -> Result<(), P9Error> {
    fid_table.remove(request.fid)?;
    Ok(())
}

/// Handle Tremove: delete the file or directory referenced by the FID, then clunk it.
///
/// Per the 9P spec, Tremove both removes the filesystem object and releases the
/// FID, regardless of whether the remove succeeds. The FID is always clunked.
pub fn handle_remove(request: &Tremove, fid_table: &mut FidTable) -> Result<(), P9Error> {
    let state = fid_table.get(request.fid)?;
    let path = state.path.clone();

    let result = if path.is_dir() {
        std::fs::remove_dir(&path)
    } else {
        std::fs::remove_file(&path)
    };

    // Always clunk the FID, even if the remove failed.
    let _ = fid_table.remove(request.fid);

    result.map_err(|e| P9Error::Io { source: e })
}

/// Handle Tflush: cancel an in-progress request.
///
/// For simplicity, we handle all requests synchronously in the dispatch loop,
/// so there is nothing to cancel. Return an empty Rflush.
pub fn handle_flush() {}

/// Derive a QID from a host filesystem path by reading its metadata.
///
/// Maps the file type and metadata into a 9P QID. Uses the file's metadata
/// length as a simple version proxy (changes on write).
pub fn qid_from_path(path: &PathBuf) -> Result<Qid, P9Error> {
    let metadata = std::fs::metadata(path)?;

    let ty = if metadata.is_dir() {
        crate::qid::qid_type::QTDIR
    } else if metadata.is_symlink() {
        crate::qid::qid_type::QTSYMLINK
    } else {
        crate::qid::qid_type::QTFILE
    };

    // Use file length as a simple version counter (changes on modification).
    // A production server would use inode + mtime, but this suffices for
    // correctness since QID versions are advisory.
    let version = (metadata.len() & 0xFFFF_FFFF) as u32;

    // Synthesize a path identifier from the canonicalized path hash.
    // On Windows we don't have stable inode numbers, so we hash the path.
    let canonical = std::fs::canonicalize(path)?;
    let path_id = path_to_qid_path(&canonical);

    Ok(Qid::new(ty, version, path_id))
}

/// Convert a canonical path to a QID path identifier.
///
/// Uses a simple hash of the path bytes to produce a u64 identifier.
/// This is deterministic for the same path but not guaranteed collision-free.
fn path_to_qid_path(path: &std::path::Path) -> u64 {
    let bytes = path.to_string_lossy();
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for byte in bytes.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_negotiation_picks_smaller_msize() {
        let request = Tversion {
            msize: 8192,
            version: P9_VERSION_STRING.to_string(),
        };
        let response = handle_version(&request, 65536);
        assert_eq!(response.msize, 8192);
        assert_eq!(response.version, P9_VERSION_STRING);
    }

    #[test]
    fn version_negotiation_server_msize_smaller() {
        let request = Tversion {
            msize: 1_048_576,
            version: P9_VERSION_STRING.to_string(),
        };
        let response = handle_version(&request, 65536);
        assert_eq!(response.msize, 65536);
        assert_eq!(response.version, P9_VERSION_STRING);
    }

    #[test]
    fn version_unknown_protocol_returns_unknown() {
        let request = Tversion {
            msize: 8192,
            version: "9P2000.u".to_string(),
        };
        let response = handle_version(&request, 65536);
        assert_eq!(response.version, "unknown");
    }

    #[test]
    fn auth_returns_eopnotsupp() {
        let response = handle_auth();
        assert_eq!(response.ecode, errno::EOPNOTSUPP);
    }

    #[test]
    fn path_to_qid_path_is_deterministic() {
        let path = std::path::Path::new("/tmp/test");
        let a = path_to_qid_path(path);
        let b = path_to_qid_path(path);
        assert_eq!(a, b);
    }

    #[test]
    fn path_to_qid_path_differs_for_different_paths() {
        let a = path_to_qid_path(std::path::Path::new("/tmp/a"));
        let b = path_to_qid_path(std::path::Path::new("/tmp/b"));
        assert_ne!(a, b);
    }
}
