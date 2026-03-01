//! L3 filesystem backend integration tests (FB-01..FB-16).
//!
//! These tests verify that POSIX syscalls arriving via the FUSE protocol
//! trigger the correct WriteInterceptor method calls through InterceptedFs.
//!
//! All tests require a running FUSE/vhost-user setup (QEMU or libfuse)
//! and are `#[ignore]` by default.
//!
//! Platform: Linux only (`#[cfg(target_os = "linux")]`).

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use codeagent_common::{Result, StepId};
use codeagent_control::InFlightTracker;
use codeagent_interceptor::write_interceptor::WriteInterceptor;

/// Records all WriteInterceptor method calls for assertion.
#[derive(Default)]
struct MockInterceptor {
    calls: Mutex<Vec<InterceptorCall>>,
    step: Mutex<Option<StepId>>,
}

#[derive(Debug, Clone, PartialEq)]
enum InterceptorCall {
    PreWrite { path: PathBuf },
    PreUnlink { path: PathBuf, is_dir: bool },
    PreRename { from: PathBuf, to: PathBuf },
    PostCreate { path: PathBuf },
    PostMkdir { path: PathBuf },
    PreSetattr { path: PathBuf },
    PreLink { target: PathBuf, link_path: PathBuf },
    PostSymlink { target: PathBuf, link_path: PathBuf },
    PreXattr { path: PathBuf },
    PreOpenTrunc { path: PathBuf },
    PreFallocate { path: PathBuf },
    PreCopyFileRange { dst_path: PathBuf },
}

impl MockInterceptor {
    fn calls(&self) -> Vec<InterceptorCall> {
        self.calls.lock().unwrap().clone()
    }

    fn set_step(&self, step: Option<StepId>) {
        *self.step.lock().unwrap() = step;
    }
}

impl WriteInterceptor for MockInterceptor {
    fn pre_write(&self, path: &Path) -> Result<()> {
        self.calls.lock().unwrap().push(InterceptorCall::PreWrite {
            path: path.to_path_buf(),
        });
        Ok(())
    }

    fn pre_unlink(&self, path: &Path, is_dir: bool) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PreUnlink {
                path: path.to_path_buf(),
                is_dir,
            });
        Ok(())
    }

    fn pre_rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PreRename {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            });
        Ok(())
    }

    fn post_create(&self, path: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PostCreate {
                path: path.to_path_buf(),
            });
        Ok(())
    }

    fn post_mkdir(&self, path: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PostMkdir {
                path: path.to_path_buf(),
            });
        Ok(())
    }

    fn pre_setattr(&self, path: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PreSetattr {
                path: path.to_path_buf(),
            });
        Ok(())
    }

    fn pre_link(&self, target: &Path, link_path: &Path) -> Result<()> {
        self.calls.lock().unwrap().push(InterceptorCall::PreLink {
            target: target.to_path_buf(),
            link_path: link_path.to_path_buf(),
        });
        Ok(())
    }

    fn post_symlink(&self, target: &Path, link_path: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PostSymlink {
                target: target.to_path_buf(),
                link_path: link_path.to_path_buf(),
            });
        Ok(())
    }

    fn pre_xattr(&self, path: &Path) -> Result<()> {
        self.calls.lock().unwrap().push(InterceptorCall::PreXattr {
            path: path.to_path_buf(),
        });
        Ok(())
    }

    fn pre_open_trunc(&self, path: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PreOpenTrunc {
                path: path.to_path_buf(),
            });
        Ok(())
    }

    fn pre_fallocate(&self, path: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PreFallocate {
                path: path.to_path_buf(),
            });
        Ok(())
    }

    fn pre_copy_file_range(&self, dst_path: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(InterceptorCall::PreCopyFileRange {
                dst_path: dst_path.to_path_buf(),
            });
        Ok(())
    }

    fn current_step(&self) -> Option<StepId> {
        *self.step.lock().unwrap()
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Start an InterceptedVirtioFsBackend with a MockInterceptor, mount it via
/// QEMU or libfuse, and return the mount point + mock interceptor for assertion.
///
/// This setup is complex and requires:
/// - A running QEMU instance, OR
/// - A FUSE mount using virtiofsd's library API (no QEMU needed)
///
/// For now, these tests sketch the expected assertions. The actual setup
/// will be implemented when QEMU/KVM infrastructure is available in CI.
fn _setup_intercepted_mount() -> (PathBuf, Arc<MockInterceptor>, InFlightTracker) {
    let interceptor = Arc::new(MockInterceptor::default());
    interceptor.set_step(Some(1));
    let in_flight = InFlightTracker::new();
    // TODO: Start InterceptedVirtioFsBackend, mount via QEMU/libfuse
    let mount_point = PathBuf::from("/mnt/test");
    (mount_point, interceptor, in_flight)
}

// ---------------------------------------------------------------------------
// L3 integration tests — POSIX syscalls → WriteInterceptor method calls
// ---------------------------------------------------------------------------

/// FB-01: write(2) triggers pre_write with the correct path.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_01_write_triggers_pre_write() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    // std::fs::write(mount.join("file.txt"), b"hello");
    let calls = interceptor.calls();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, InterceptorCall::PreWrite { path } if path.ends_with("file.txt")))
    );
}

/// FB-02: creat(2) triggers post_create for a genuinely new file.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_02_creat_triggers_post_create() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, InterceptorCall::PostCreate { path } if path.ends_with("new_file.txt")))
    );
}

/// FB-03: open(O_TRUNC) triggers pre_open_trunc for an existing file.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_03_open_trunc_triggers_pre_open_trunc() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, InterceptorCall::PreOpenTrunc { path } if path.ends_with("existing.txt")))
    );
}

/// FB-04: unlink(2) triggers pre_unlink(path, false).
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_04_unlink_triggers_pre_unlink_file() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls.iter().any(
        |c| matches!(c, InterceptorCall::PreUnlink { path, is_dir } if path.ends_with("file.txt") && !is_dir)
    ));
}

/// FB-05: rmdir(2) triggers pre_unlink(path, true).
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_05_rmdir_triggers_pre_unlink_dir() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls.iter().any(
        |c| matches!(c, InterceptorCall::PreUnlink { path, is_dir } if path.ends_with("subdir") && *is_dir)
    ));
}

/// FB-06: rename(2) triggers pre_rename(old, new).
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_06_rename_triggers_pre_rename() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls.iter().any(
        |c| matches!(c, InterceptorCall::PreRename { from, to } if from.ends_with("old.txt") && to.ends_with("new.txt"))
    ));
}

/// FB-07: mkdir(2) triggers post_mkdir.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_07_mkdir_triggers_post_mkdir() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, InterceptorCall::PostMkdir { path } if path.ends_with("newdir")))
    );
}

/// FB-08: symlink(2) triggers post_symlink.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_08_symlink_triggers_post_symlink() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls
        .iter()
        .any(|c| matches!(c, InterceptorCall::PostSymlink { .. })));
}

/// FB-09: link(2) triggers pre_link.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_09_link_triggers_pre_link() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls
        .iter()
        .any(|c| matches!(c, InterceptorCall::PreLink { .. })));
}

/// FB-10: truncate(2) triggers pre_setattr.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_10_truncate_triggers_pre_setattr() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, InterceptorCall::PreSetattr { path } if path.ends_with("file.txt")))
    );
}

/// FB-11: chmod(2) triggers pre_setattr.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_11_chmod_triggers_pre_setattr() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls
        .iter()
        .any(|c| matches!(c, InterceptorCall::PreSetattr { .. })));
}

/// FB-12: setxattr(2) triggers pre_xattr.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_12_setxattr_triggers_pre_xattr() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls
        .iter()
        .any(|c| matches!(c, InterceptorCall::PreXattr { .. })));
}

/// FB-13: removexattr(2) triggers pre_xattr.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_13_removexattr_triggers_pre_xattr() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls
        .iter()
        .any(|c| matches!(c, InterceptorCall::PreXattr { .. })));
}

/// FB-14: fallocate(2) triggers pre_fallocate.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_14_fallocate_triggers_pre_fallocate() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls
        .iter()
        .any(|c| matches!(c, InterceptorCall::PreFallocate { .. })));
}

/// FB-15: copy_file_range(2) triggers pre_copy_file_range.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_15_copy_file_range_triggers_pre_copy_file_range() {
    let (_mount, interceptor, _tracker) = _setup_intercepted_mount();
    let calls = interceptor.calls();
    assert!(calls
        .iter()
        .any(|c| matches!(c, InterceptorCall::PreCopyFileRange { .. })));
}

/// FB-16: InFlightTracker count returns to 0 after all operations complete.
#[test]
#[ignore = "requires FUSE/vhost-user setup"]
fn fb_16_in_flight_tracker_drains() {
    let (_mount, _interceptor, tracker) = _setup_intercepted_mount();
    // After all filesystem operations complete, the tracker should be drained.
    assert_eq!(tracker.count(), 0);
}
