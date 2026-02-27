#![allow(dead_code)]

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_interceptor::write_interceptor::WriteInterceptor;
use codeagent_test_support::snapshot::SnapshotCompareOptions;

/// Mirrors the behaviour of a filesystem backend: calls the interceptor hook
/// then performs the real `std::fs` operation.
pub struct OperationApplier<'a> {
    interceptor: &'a UndoInterceptor,
}

impl<'a> OperationApplier<'a> {
    pub fn new(interceptor: &'a UndoInterceptor) -> Self {
        Self { interceptor }
    }

    /// Write (or overwrite) a file.
    pub fn write_file(&self, path: &Path, contents: &[u8]) {
        if path.exists() {
            self.interceptor.pre_write(path).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    /// Create a brand-new file (that didn't exist before) and write contents.
    pub fn create_file(&self, path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).unwrap();
            }
        }
        fs::write(path, contents).unwrap();
        self.interceptor.post_create(path).unwrap();
    }

    /// Create a directory.
    pub fn mkdir(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        self.interceptor.post_mkdir(path).unwrap();
    }

    /// Delete a single file.
    pub fn delete_file(&self, path: &Path) {
        self.interceptor.pre_unlink(path, false).unwrap();
        fs::remove_file(path).unwrap();
    }

    /// Recursively delete a directory tree.
    pub fn delete_tree(&self, path: &Path) {
        self.interceptor.pre_unlink(path, true).unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    /// Rename a file or directory.
    pub fn rename(&self, from: &Path, to: &Path) {
        let is_dir = from.is_dir();
        self.interceptor.pre_rename(from, to).unwrap();
        fs::rename(from, to).unwrap();

        // Record destination entries as created (they are new at those paths).
        if is_dir {
            self.record_tree_creation(to);
        } else {
            self.interceptor.post_create(to).unwrap();
        }
    }

    /// Open an existing file with O_TRUNC (truncates to zero length).
    pub fn open_trunc(&self, path: &Path) {
        self.interceptor.pre_open_trunc(path).unwrap();
        File::create(path).unwrap();
    }

    /// Truncate an existing file to a shorter length via setattr.
    pub fn setattr_truncate(&self, path: &Path, new_len: u64) {
        self.interceptor.pre_setattr(path).unwrap();
        let file = File::options().write(true).open(path).unwrap();
        file.set_len(new_len).unwrap();
    }

    /// Change mode bits on a file (Unix only).
    #[cfg(unix)]
    pub fn chmod(&self, path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        self.interceptor.pre_setattr(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Extend a file to a larger size via fallocate.
    pub fn fallocate(&self, path: &Path, new_len: u64) {
        self.interceptor.pre_fallocate(path).unwrap();
        let file = File::options().write(true).open(path).unwrap();
        file.set_len(new_len).unwrap();
    }

    /// Simulate copy_file_range by reading source contents and writing to destination.
    pub fn copy_file_range(&self, src: &Path, dst: &Path) {
        let src_contents = fs::read(src).unwrap();
        self.interceptor.pre_copy_file_range(dst).unwrap();
        let mut file = File::options().write(true).open(dst).unwrap();
        file.write_all(&src_contents).unwrap();
        file.set_len(src_contents.len() as u64).unwrap();
    }

    /// Set an extended attribute on a file (Linux only).
    #[cfg(target_os = "linux")]
    pub fn set_xattr(&self, path: &Path, key: &str, value: &[u8]) {
        self.interceptor.pre_xattr(path).unwrap();
        xattr::set(path, key, value).unwrap();
    }

    /// Remove an extended attribute from a file (Linux only).
    #[cfg(target_os = "linux")]
    pub fn remove_xattr(&self, path: &Path, key: &str) {
        self.interceptor.pre_xattr(path).unwrap();
        xattr::remove(path, key).unwrap();
    }

    /// Recursively record all entries under a directory as newly created.
    pub fn record_tree_creation(&self, dir: &Path) {
        self.interceptor.post_mkdir(dir).unwrap();
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                self.record_tree_creation(&path);
            } else {
                self.interceptor.post_create(&path).unwrap();
            }
        }
    }
}

/// Snapshot comparison options that ignore mtime (filesystem operations alter mtimes
/// and we only care about content + structure after rollback).
pub fn compare_opts() -> SnapshotCompareOptions {
    SnapshotCompareOptions {
        mtime_tolerance_ns: i128::MAX,
        ..SnapshotCompareOptions::default()
    }
}
