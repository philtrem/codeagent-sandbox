/// QID type bits for filesystem object classification.
pub mod qid_type {
    /// Regular file.
    pub const QTFILE: u8 = 0x00;
    /// Symlink.
    pub const QTSYMLINK: u8 = 0x02;
    /// Exclusive-use file.
    pub const QTEXCL: u8 = 0x20;
    /// Append-only file.
    pub const QTAPPEND: u8 = 0x40;
    /// Directory.
    pub const QTDIR: u8 = 0x80;
}

/// Unique identifier for a filesystem object in the 9P protocol.
///
/// A QID is a 13-byte value that uniquely identifies a file or directory.
/// It consists of a type byte, a version counter (incremented on modification),
/// and a unique path identifier (analogous to an inode number).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Qid {
    /// Type of the filesystem object (see `qid_type` constants).
    pub ty: u8,

    /// Version counter, incremented on each modification.
    /// Can be derived from mtime or a monotonic counter.
    pub version: u32,

    /// Unique identifier for this object (inode number or synthetic).
    pub path: u64,
}

impl Qid {
    /// Size of a QID on the wire: 1 + 4 + 8 = 13 bytes.
    pub const WIRE_SIZE: usize = 13;

    pub fn new(ty: u8, version: u32, path: u64) -> Self {
        Self { ty, version, path }
    }

    /// Create a QID for a directory.
    pub fn directory(version: u32, path: u64) -> Self {
        Self::new(qid_type::QTDIR, version, path)
    }

    /// Create a QID for a regular file.
    pub fn file(version: u32, path: u64) -> Self {
        Self::new(qid_type::QTFILE, version, path)
    }

    /// Create a QID for a symlink.
    pub fn symlink(version: u32, path: u64) -> Self {
        Self::new(qid_type::QTSYMLINK, version, path)
    }

    /// Returns true if this QID represents a directory.
    pub fn is_dir(&self) -> bool {
        self.ty & qid_type::QTDIR != 0
    }

    /// Returns true if this QID represents a symlink.
    pub fn is_symlink(&self) -> bool {
        self.ty & qid_type::QTSYMLINK != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qid_directory() {
        let qid = Qid::directory(1, 100);
        assert!(qid.is_dir());
        assert!(!qid.is_symlink());
        assert_eq!(qid.ty, qid_type::QTDIR);
    }

    #[test]
    fn qid_file() {
        let qid = Qid::file(5, 200);
        assert!(!qid.is_dir());
        assert!(!qid.is_symlink());
        assert_eq!(qid.ty, qid_type::QTFILE);
    }

    #[test]
    fn qid_symlink() {
        let qid = Qid::symlink(3, 300);
        assert!(qid.is_symlink());
        assert!(!qid.is_dir());
        assert_eq!(qid.ty, qid_type::QTSYMLINK);
    }

    #[test]
    fn qid_wire_size() {
        assert_eq!(Qid::WIRE_SIZE, 13);
    }
}
