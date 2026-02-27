use std::fs;

use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_test_support::fixtures;
use codeagent_test_support::snapshot::assert_tree_eq;
use codeagent_test_support::workspace::TempWorkspace;

mod common;
use common::{OperationApplier, compare_opts};

// ---------------------------------------------------------------------------
// UI-01: Write same file 3x in one step
// ---------------------------------------------------------------------------
#[test]
fn ui_01_write_same_file_3x_in_one_step() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    ops.write_file(&target, b"version 1");
    ops.write_file(&target, b"version 2");
    ops.write_file(&target, b"version 3");
    interceptor.close_step(1).unwrap();

    // File should now contain "version 3"
    assert_eq!(fs::read_to_string(&target).unwrap(), "version 3");

    // Rollback should restore original
    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-02: Create new file + write
// ---------------------------------------------------------------------------
#[test]
fn ui_02_create_new_file_and_write() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let new_file = ws.working_dir.join("brand_new.txt");
    ops.create_file(&new_file, b"new content");
    ops.write_file(&new_file, b"updated content");
    interceptor.close_step(1).unwrap();

    assert!(new_file.exists());
    assert_eq!(fs::read_to_string(&new_file).unwrap(), "updated content");

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert!(!new_file.exists());
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-03: Create nested dirs
// ---------------------------------------------------------------------------
#[test]
fn ui_03_create_nested_dirs() {
    let ws = TempWorkspace::new();
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let dir1 = ws.working_dir.join("a");
    let dir2 = dir1.join("b");
    let dir3 = dir2.join("c");
    ops.mkdir(&dir1);
    ops.mkdir(&dir2);
    ops.mkdir(&dir3);
    let file = dir3.join("deep.txt");
    ops.create_file(&file, b"deep content");
    interceptor.close_step(1).unwrap();

    assert!(file.exists());

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert!(!dir1.exists());
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-04: Delete file
// ---------------------------------------------------------------------------
#[test]
fn ui_04_delete_file() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    ops.delete_file(&target);
    interceptor.close_step(1).unwrap();

    assert!(!target.exists());

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert!(target.exists());
    assert_eq!(fs::read_to_string(&target).unwrap(), "hello world");
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-05: Delete tree (rm -rf)
// ---------------------------------------------------------------------------
#[test]
fn ui_05_delete_tree() {
    let ws = TempWorkspace::with_fixture(fixtures::deep_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let tree_root = ws.working_dir.join("level0");
    ops.delete_tree(&tree_root);
    interceptor.close_step(1).unwrap();

    assert!(!tree_root.exists());

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert!(tree_root.is_dir());
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-06: Rename A→B (B absent)
// ---------------------------------------------------------------------------
#[test]
fn ui_06_rename_file_dest_absent() {
    let ws = TempWorkspace::with_fixture(fixtures::rename_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    // Remove b.txt so destination is absent
    fs::remove_file(ws.working_dir.join("b.txt")).unwrap();
    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let from = ws.working_dir.join("a.txt");
    let to = ws.working_dir.join("c.txt");
    ops.rename(&from, &to);
    interceptor.close_step(1).unwrap();

    assert!(!from.exists());
    assert!(to.exists());
    assert_eq!(fs::read_to_string(&to).unwrap(), "content of a");

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert!(from.exists());
    assert!(!to.exists());
    assert_eq!(fs::read_to_string(&from).unwrap(), "content of a");
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-07: Rename A→B (B exists)
// ---------------------------------------------------------------------------
#[test]
fn ui_07_rename_file_dest_exists() {
    let ws = TempWorkspace::with_fixture(fixtures::rename_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let from = ws.working_dir.join("a.txt");
    let to = ws.working_dir.join("b.txt");
    ops.rename(&from, &to);
    interceptor.close_step(1).unwrap();

    assert!(!from.exists());
    assert_eq!(fs::read_to_string(&to).unwrap(), "content of a");

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert!(from.exists());
    assert_eq!(fs::read_to_string(&from).unwrap(), "content of a");
    assert_eq!(
        fs::read_to_string(ws.working_dir.join("b.txt")).unwrap(),
        "content of b"
    );
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-08: Rename dir with nested files
// ---------------------------------------------------------------------------
#[test]
fn ui_08_rename_dir_with_nested_files() {
    let ws = TempWorkspace::new();
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    // Set up a directory tree to rename
    let src_dir = ws.working_dir.join("src_dir");
    fs::create_dir_all(src_dir.join("sub")).unwrap();
    fs::write(src_dir.join("top.txt"), "top content").unwrap();
    fs::write(src_dir.join("sub/nested.txt"), "nested content").unwrap();

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let dst_dir = ws.working_dir.join("dst_dir");
    ops.rename(&src_dir, &dst_dir);
    interceptor.close_step(1).unwrap();

    assert!(!src_dir.exists());
    assert!(dst_dir.is_dir());
    assert_eq!(
        fs::read_to_string(dst_dir.join("top.txt")).unwrap(),
        "top content"
    );
    assert_eq!(
        fs::read_to_string(dst_dir.join("sub/nested.txt")).unwrap(),
        "nested content"
    );

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert!(src_dir.is_dir());
    assert!(!dst_dir.exists());
    assert_eq!(
        fs::read_to_string(src_dir.join("top.txt")).unwrap(),
        "top content"
    );
    assert_eq!(
        fs::read_to_string(src_dir.join("sub/nested.txt")).unwrap(),
        "nested content"
    );
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-09: Open existing file with O_TRUNC
// ---------------------------------------------------------------------------
#[test]
fn ui_09_truncate_open() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    ops.open_trunc(&target);
    interceptor.close_step(1).unwrap();

    assert_eq!(fs::read_to_string(&target).unwrap(), "");

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert_eq!(fs::read_to_string(&target).unwrap(), "hello world");
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-10: Truncate via setattr to shorter length
// ---------------------------------------------------------------------------
#[test]
fn ui_10_truncate_setattr() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("medium.txt");
    let original_len = fs::metadata(&target).unwrap().len();
    assert_eq!(original_len, 4096);
    ops.setattr_truncate(&target, 10);
    interceptor.close_step(1).unwrap();

    assert_eq!(fs::metadata(&target).unwrap().len(), 10);

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert_eq!(fs::metadata(&target).unwrap().len(), 4096);
    assert_eq!(fs::read_to_string(&target).unwrap(), "x".repeat(4096));
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-11: Chmod — flip executable bit (Unix only)
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[test]
fn ui_11_chmod() {
    use std::os::unix::fs::PermissionsExt;

    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let target = ws.working_dir.join("run.sh");
    let original_mode = fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
    assert_eq!(original_mode, 0o755);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    ops.chmod(&target, 0o644);
    interceptor.close_step(1).unwrap();

    let changed_mode = fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
    assert_eq!(changed_mode, 0o644);

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    let restored_mode = fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
    assert_eq!(restored_mode, 0o755);
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-12: Set xattr on file (Linux only)
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
#[test]
fn ui_12_xattr_set() {
    let ws = TempWorkspace::new();
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let target = ws.working_dir.join("test.txt");
    fs::write(&target, "xattr test content").unwrap();

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    ops.set_xattr(&target, "user.test", b"test_value");
    interceptor.close_step(1).unwrap();

    let xattr_value = xattr::get(&target, "user.test").unwrap();
    assert_eq!(xattr_value, Some(b"test_value".to_vec()));

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    let xattr_after = xattr::get(&target, "user.test").unwrap();
    assert_eq!(xattr_after, None);
    assert_tree_eq(
        &before,
        &after,
        &codeagent_test_support::snapshot::SnapshotCompareOptions {
            mtime_tolerance_ns: i128::MAX,
            check_xattrs: true,
            ..Default::default()
        },
    );
}

// ---------------------------------------------------------------------------
// UI-13: Remove existing xattr (Linux only)
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
#[test]
fn ui_13_xattr_remove() {
    let ws = TempWorkspace::new();
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let target = ws.working_dir.join("test.txt");
    fs::write(&target, "xattr test content").unwrap();
    xattr::set(&target, "user.existing", b"original_value").unwrap();

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    ops.remove_xattr(&target, "user.existing");
    interceptor.close_step(1).unwrap();

    let xattr_value = xattr::get(&target, "user.existing").unwrap();
    assert_eq!(xattr_value, None);

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    let xattr_restored = xattr::get(&target, "user.existing").unwrap();
    assert_eq!(xattr_restored, Some(b"original_value".to_vec()));
    assert_tree_eq(
        &before,
        &after,
        &codeagent_test_support::snapshot::SnapshotCompareOptions {
            mtime_tolerance_ns: i128::MAX,
            check_xattrs: true,
            ..Default::default()
        },
    );
}

// ---------------------------------------------------------------------------
// UI-14: Fallocate — extend file size
// ---------------------------------------------------------------------------
#[test]
fn ui_14_fallocate() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    let original_len = fs::metadata(&target).unwrap().len();
    assert_eq!(original_len, 11); // "hello world"
    ops.fallocate(&target, 1_000_000);
    interceptor.close_step(1).unwrap();

    assert_eq!(fs::metadata(&target).unwrap().len(), 1_000_000);

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert_eq!(fs::metadata(&target).unwrap().len(), 11);
    assert_eq!(fs::read_to_string(&target).unwrap(), "hello world");
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-15: Copy-file-range into existing file
// ---------------------------------------------------------------------------
#[test]
fn ui_15_copy_file_range() {
    let ws = TempWorkspace::new();
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let src = ws.working_dir.join("source.txt");
    let dst = ws.working_dir.join("destination.txt");
    fs::write(&src, "source data to copy").unwrap();
    fs::write(&dst, "original destination content").unwrap();

    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    ops.copy_file_range(&src, &dst);
    interceptor.close_step(1).unwrap();

    assert_eq!(fs::read_to_string(&dst).unwrap(), "source data to copy");

    interceptor.rollback(1, false).unwrap();

    let after = ws.snapshot();
    assert_eq!(
        fs::read_to_string(&dst).unwrap(),
        "original destination content"
    );
    assert_tree_eq(&before, &after, &compare_opts());
}
