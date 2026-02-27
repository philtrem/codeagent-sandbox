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
    interceptor.rollback(1).unwrap();

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

    interceptor.rollback(1).unwrap();

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

    interceptor.rollback(1).unwrap();

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

    interceptor.rollback(1).unwrap();

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

    interceptor.rollback(1).unwrap();

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

    interceptor.rollback(1).unwrap();

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

    interceptor.rollback(1).unwrap();

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

    interceptor.rollback(1).unwrap();

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
