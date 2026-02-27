use std::path::Path;

use codeagent_common::{Result, StepId};

/// Shared write interception logic called by both filesystem backends.
///
/// On the first mutating touch of a path within a step, the interceptor
/// captures the full preimage (file contents + metadata). Subsequent
/// touches to the same path within the same step are no-ops for capture.
pub trait WriteInterceptor: Send + Sync {
    /// Called before a file is written or truncated.
    fn pre_write(&self, path: &Path) -> Result<()>;

    /// Called before a file or directory is deleted.
    fn pre_unlink(&self, path: &Path, is_dir: bool) -> Result<()>;

    /// Called before a rename. Records state of both source and destination.
    fn pre_rename(&self, from: &Path, to: &Path) -> Result<()>;

    /// Called after a file is created (genuinely new inode).
    fn post_create(&self, path: &Path) -> Result<()>;

    /// Called after a directory is created.
    fn post_mkdir(&self, path: &Path) -> Result<()>;

    /// Called before attributes are changed (chmod, chown, truncate, utimes).
    fn pre_setattr(&self, path: &Path) -> Result<()>;

    /// Called before a hard link is created.
    fn pre_link(&self, target: &Path, link_path: &Path) -> Result<()>;

    /// Called after a symlink is created.
    fn post_symlink(&self, target: &Path, link_path: &Path) -> Result<()>;

    /// Called before extended attributes are set or removed.
    fn pre_xattr(&self, path: &Path) -> Result<()>;

    /// Called before an open with O_TRUNC on an existing file.
    fn pre_open_trunc(&self, path: &Path) -> Result<()>;

    /// Called before fallocate (including hole-punch and size changes).
    fn pre_fallocate(&self, path: &Path) -> Result<()>;

    /// Called before copy_file_range (destination path mutates).
    fn pre_copy_file_range(&self, dst_path: &Path) -> Result<()>;

    /// Query the current active step.
    fn current_step(&self) -> Option<StepId>;
}
