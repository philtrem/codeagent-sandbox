/// Errors that can occur in the 9P2000.L server.
#[derive(Debug, thiserror::Error)]
pub enum P9Error {
    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("malformed message: {reason}")]
    MalformedMessage { reason: String },

    #[error("oversized message: {size} bytes (max {max_size})")]
    OversizedMessage { size: u32, max_size: u32 },

    #[error("unknown message type: {msg_type}")]
    UnknownMessageType { msg_type: u8 },

    #[error("unknown fid: {fid}")]
    UnknownFid { fid: u32 },

    #[error("fid already in use: {fid}")]
    FidInUse { fid: u32 },

    #[error("fid not open: {fid}")]
    FidNotOpen { fid: u32 },

    #[error("protocol error: {reason}")]
    ProtocolError { reason: String },

    #[error("write interceptor denied operation: {reason}")]
    InterceptorDenied { reason: String },

    #[error("reserved filename: {name}")]
    ReservedName { name: String },

    #[error("case collision: existing \"{existing}\" vs attempted \"{attempted}\"")]
    CaseCollision { existing: String, attempted: String },

    #[error("path escapes root: {path}")]
    PathOutsideRoot { path: String },
}

/// Linux errno values used in `Rlerror` responses.
///
/// These are Linux-specific (the guest always runs Linux regardless of host OS).
pub mod errno {
    pub const EPERM: u32 = 1;
    pub const ENOENT: u32 = 2;
    pub const EIO: u32 = 5;
    pub const EBADF: u32 = 9;
    pub const EACCES: u32 = 13;
    pub const EEXIST: u32 = 17;
    pub const ENOTDIR: u32 = 20;
    pub const EISDIR: u32 = 21;
    pub const EINVAL: u32 = 22;
    pub const ENOSPC: u32 = 28;
    pub const ENAMETOOLONG: u32 = 36;
    pub const ENOTEMPTY: u32 = 39;
    pub const ENODATA: u32 = 61;
    pub const EOVERFLOW: u32 = 75;
    pub const EOPNOTSUPP: u32 = 95;
}

/// Convert an `std::io::ErrorKind` to a Linux errno value.
pub fn io_error_to_errno(error: &std::io::Error) -> u32 {
    match error.kind() {
        std::io::ErrorKind::NotFound => errno::ENOENT,
        std::io::ErrorKind::PermissionDenied => errno::EACCES,
        std::io::ErrorKind::AlreadyExists => errno::EEXIST,
        std::io::ErrorKind::InvalidInput => errno::EINVAL,
        std::io::ErrorKind::DirectoryNotEmpty => errno::ENOTEMPTY,
        std::io::ErrorKind::IsADirectory => errno::EISDIR,
        std::io::ErrorKind::NotADirectory => errno::ENOTDIR,
        std::io::ErrorKind::StorageFull => errno::ENOSPC,
        std::io::ErrorKind::InvalidFilename => errno::EINVAL,
        _ => {
            // Check raw OS error codes for cases not covered by ErrorKind.
            if let Some(raw) = error.raw_os_error() {
                #[cfg(windows)]
                {
                    // Windows ERROR_DIR_NOT_EMPTY = 145
                    if raw == 145 {
                        return errno::ENOTEMPTY;
                    }
                }
                #[cfg(unix)]
                {
                    // On Unix, raw OS errors map directly to errno values.
                    // Translate common ones that ErrorKind doesn't cover.
                    match raw as u32 {
                        v if v == errno::ENOTEMPTY => return errno::ENOTEMPTY,
                        v if v == errno::EISDIR => return errno::EISDIR,
                        v if v == errno::ENOTDIR => return errno::ENOTDIR,
                        v if v == errno::ENOSPC => return errno::ENOSPC,
                        v if v == errno::ENAMETOOLONG => return errno::ENAMETOOLONG,
                        _ => {}
                    }
                }
                let _ = raw; // Suppress unused variable warning.
            }
            errno::EIO
        }
    }
}

/// Convert a `P9Error` to a Linux errno for `Rlerror`.
pub fn p9_error_to_errno(error: &P9Error) -> u32 {
    match error {
        P9Error::Io { source } => io_error_to_errno(source),
        P9Error::UnknownFid { .. } | P9Error::FidNotOpen { .. } => errno::EBADF,
        P9Error::FidInUse { .. } => errno::EBADF,
        P9Error::InterceptorDenied { .. } => errno::EACCES,
        P9Error::ReservedName { .. } => errno::EINVAL,
        P9Error::CaseCollision { .. } => errno::EEXIST,
        P9Error::PathOutsideRoot { .. } => errno::EACCES,
        P9Error::MalformedMessage { .. }
        | P9Error::OversizedMessage { .. }
        | P9Error::UnknownMessageType { .. }
        | P9Error::ProtocolError { .. } => errno::EIO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_not_found_maps_to_enoent() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        assert_eq!(io_error_to_errno(&error), errno::ENOENT);
    }

    #[test]
    fn io_error_permission_maps_to_eacces() {
        let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        assert_eq!(io_error_to_errno(&error), errno::EACCES);
    }

    #[test]
    fn p9_error_unknown_fid_maps_to_ebadf() {
        let error = P9Error::UnknownFid { fid: 42 };
        assert_eq!(p9_error_to_errno(&error), errno::EBADF);
    }

    #[test]
    fn p9_error_interceptor_denied_maps_to_eacces() {
        let error = P9Error::InterceptorDenied {
            reason: "safeguard".to_string(),
        };
        assert_eq!(p9_error_to_errno(&error), errno::EACCES);
    }

    #[test]
    fn p9_error_reserved_name_maps_to_einval() {
        let error = P9Error::ReservedName {
            name: "CON".to_string(),
        };
        assert_eq!(p9_error_to_errno(&error), errno::EINVAL);
    }
}
