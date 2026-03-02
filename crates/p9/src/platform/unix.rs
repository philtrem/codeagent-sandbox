use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::error::P9Error;
use crate::operations::attr::FileAttributes;

/// Read POSIX file attributes from the filesystem.
pub fn get_file_attributes(path: &Path) -> Result<FileAttributes, P9Error> {
    let meta = std::fs::metadata(path)?;

    Ok(FileAttributes {
        mode: meta.mode(),
        uid: meta.uid(),
        gid: meta.gid(),
        nlink: meta.nlink(),
        rdev: meta.rdev(),
        size: meta.size(),
        blksize: meta.blksize(),
        blocks: meta.blocks(),
        atime_sec: meta.atime() as u64,
        atime_nsec: meta.atime_nsec() as u64,
        mtime_sec: meta.mtime() as u64,
        mtime_nsec: meta.mtime_nsec() as u64,
        ctime_sec: meta.ctime() as u64,
        ctime_nsec: meta.ctime_nsec() as u64,
    })
}
