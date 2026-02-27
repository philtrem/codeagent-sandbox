#![allow(dead_code)]

use std::fs;
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
