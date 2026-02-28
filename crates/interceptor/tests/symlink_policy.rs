use std::fs;
use std::path::Path;

use codeagent_common::SymlinkPolicy;
use codeagent_interceptor::manifest::StepManifest;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_interceptor::write_interceptor::WriteInterceptor;
use codeagent_test_support::workspace::TempWorkspace;

mod common;
use common::OperationApplier;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read the step manifest from the completed steps directory.
fn read_step_manifest(ws: &TempWorkspace, step_id: u64) -> StepManifest {
    let step_dir = ws.undo_dir.join("steps").join(step_id.to_string());
    StepManifest::read_from(&step_dir).unwrap()
}

/// Try to create a symlink. Returns false if the OS doesn't support it
/// (e.g. Windows without Developer Mode).
fn try_create_symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }
}

// ---------------------------------------------------------------------------
// SY-01: Ignore policy — post_symlink is a no-op
// ---------------------------------------------------------------------------
#[test]
fn sy_01_ignore_policy_post_symlink_is_noop() {
    let ws = TempWorkspace::new();
    let target = ws.working_dir.join("target.txt");
    fs::write(&target, "content").unwrap();

    let link = ws.working_dir.join("link.txt");
    if !try_create_symlink(&target, &link) {
        eprintln!("skipping: symlink creation not supported");
        return;
    }

    let interceptor = UndoInterceptor::with_symlink_policy(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        SymlinkPolicy::Ignore,
    );

    interceptor.open_step(1).unwrap();
    // Call post_symlink directly — with Ignore policy it should be a no-op
    interceptor.post_symlink(&target, &link).unwrap();
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        manifest.entries.is_empty(),
        "Ignore policy: post_symlink should not create manifest entries"
    );
}

// ---------------------------------------------------------------------------
// SY-02: Ignore policy — pre_link is a no-op
// ---------------------------------------------------------------------------
#[test]
fn sy_02_ignore_policy_pre_link_is_noop() {
    let ws = TempWorkspace::new();
    let target = ws.working_dir.join("target.txt");
    fs::write(&target, "content").unwrap();

    let interceptor = UndoInterceptor::with_symlink_policy(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        SymlinkPolicy::Ignore,
    );

    interceptor.open_step(1).unwrap();
    // Call pre_link directly — with Ignore policy it should be a no-op
    let link = ws.working_dir.join("hardlink.txt");
    interceptor.pre_link(&target, &link).unwrap();
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        manifest.entries.is_empty(),
        "Ignore policy: pre_link should not capture preimage for link target"
    );
}

// ---------------------------------------------------------------------------
// SY-03: Ignore policy — symlink in tree delete is skipped
// ---------------------------------------------------------------------------
#[test]
fn sy_03_ignore_policy_symlink_skipped_in_tree_delete() {
    let ws = TempWorkspace::new();
    let sub_dir = ws.working_dir.join("mydir");
    fs::create_dir_all(&sub_dir).unwrap();

    let real_file = sub_dir.join("real.txt");
    fs::write(&real_file, "real content").unwrap();

    let link = sub_dir.join("link.txt");
    if !try_create_symlink(&real_file, &link) {
        eprintln!("skipping: symlink creation not supported");
        return;
    }

    let interceptor = UndoInterceptor::with_symlink_policy(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        SymlinkPolicy::Ignore,
    );
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.delete_tree(&sub_dir);
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    // The real file and directory should be captured, but the symlink should not
    assert!(
        !manifest.contains_path("mydir/link.txt"),
        "Ignore policy: symlink should not appear in manifest during tree delete"
    );
    assert!(
        manifest.contains_path("mydir/real.txt"),
        "Real file should still appear in manifest"
    );
}

// ---------------------------------------------------------------------------
// SY-04: ReadOnly policy — symlink preimage captured
// ---------------------------------------------------------------------------
#[test]
fn sy_04_read_only_policy_symlink_preimage_captured() {
    let ws = TempWorkspace::new();
    let target = ws.working_dir.join("target.txt");
    fs::write(&target, "symlink target").unwrap();

    let link = ws.working_dir.join("link.txt");
    if !try_create_symlink(&target, &link) {
        eprintln!("skipping: symlink creation not supported");
        return;
    }

    let interceptor = UndoInterceptor::with_symlink_policy(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        SymlinkPolicy::ReadOnly,
    );
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    // Delete the symlink — should capture preimage because ReadOnly allows reads
    ops.delete_file(&link);
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        manifest.contains_path("link.txt"),
        "ReadOnly policy: symlink preimage should be captured"
    );
}

// ---------------------------------------------------------------------------
// SY-05: ReadOnly policy — rollback skips symlink restore
// ---------------------------------------------------------------------------
#[test]
fn sy_05_read_only_policy_rollback_skips_symlink_restore() {
    let ws = TempWorkspace::new();
    let target = ws.working_dir.join("target.txt");
    fs::write(&target, "symlink target").unwrap();

    let link = ws.working_dir.join("link.txt");
    if !try_create_symlink(&target, &link) {
        eprintln!("skipping: symlink creation not supported");
        return;
    }

    let interceptor = UndoInterceptor::with_symlink_policy(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        SymlinkPolicy::ReadOnly,
    );
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.delete_file(&link);
    interceptor.close_step(1).unwrap();

    // Symlink was captured (ReadOnly allows reads), now rollback
    interceptor.rollback(1, false).unwrap();

    // Symlink should NOT be restored because ReadOnly blocks write-side
    assert!(
        !link.exists() && link.symlink_metadata().is_err(),
        "ReadOnly policy: symlink should NOT be restored on rollback"
    );
    // Target file should still be there (it was never deleted)
    assert!(target.exists(), "target file should be unaffected");
}

// ---------------------------------------------------------------------------
// SY-06: ReadWrite policy — full symlink round-trip
// ---------------------------------------------------------------------------
#[test]
fn sy_06_read_write_policy_full_symlink_round_trip() {
    let ws = TempWorkspace::new();
    let target = ws.working_dir.join("target.txt");
    fs::write(&target, "symlink target").unwrap();

    let link = ws.working_dir.join("link.txt");
    if !try_create_symlink(&target, &link) {
        eprintln!("skipping: symlink creation not supported");
        return;
    }

    let interceptor = UndoInterceptor::with_symlink_policy(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        SymlinkPolicy::ReadWrite,
    );
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.delete_file(&link);
    interceptor.close_step(1).unwrap();

    assert!(
        link.symlink_metadata().is_err(),
        "symlink should be deleted"
    );

    interceptor.rollback(1, false).unwrap();

    // Symlink should be fully restored
    assert!(
        link.symlink_metadata().is_ok(),
        "ReadWrite policy: symlink should be restored on rollback"
    );
    assert!(
        link.symlink_metadata().unwrap().is_symlink(),
        "restored path should be a symlink"
    );
}

// ---------------------------------------------------------------------------
// SY-07: Default policy is Ignore
// ---------------------------------------------------------------------------
#[test]
fn sy_07_default_policy_is_ignore() {
    let ws = TempWorkspace::new();
    let target = ws.working_dir.join("target.txt");
    fs::write(&target, "content").unwrap();

    let link = ws.working_dir.join("link.txt");
    if !try_create_symlink(&target, &link) {
        eprintln!("skipping: symlink creation not supported");
        return;
    }

    // Default constructor — should use Ignore policy
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());

    interceptor.open_step(1).unwrap();
    interceptor.post_symlink(&target, &link).unwrap();
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        manifest.entries.is_empty(),
        "default policy (Ignore): post_symlink should be a no-op"
    );
}

// ---------------------------------------------------------------------------
// SY-08: Ignore policy — ensure_preimage skips existing symlinks
// ---------------------------------------------------------------------------
#[test]
fn sy_08_ignore_policy_ensure_preimage_skips_symlinks() {
    let ws = TempWorkspace::new();
    let target = ws.working_dir.join("target.txt");
    fs::write(&target, "content").unwrap();

    let link = ws.working_dir.join("link.txt");
    if !try_create_symlink(&target, &link) {
        eprintln!("skipping: symlink creation not supported");
        return;
    }

    let interceptor = UndoInterceptor::with_symlink_policy(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        SymlinkPolicy::Ignore,
    );

    interceptor.open_step(1).unwrap();
    // pre_write calls ensure_preimage — should skip the symlink silently
    interceptor.pre_write(&link).unwrap();
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        manifest.entries.is_empty(),
        "Ignore policy: pre_write on a symlink should not capture preimage"
    );
}
