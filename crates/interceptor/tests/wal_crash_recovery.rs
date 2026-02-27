use std::fs;

use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_interceptor::write_interceptor::WriteInterceptor;
use codeagent_test_support::fixtures;
use codeagent_test_support::snapshot::assert_tree_eq;
use codeagent_test_support::workspace::TempWorkspace;

mod common;
use common::{OperationApplier, compare_opts};

// ---------------------------------------------------------------------------
// CR-01: Crash mid-step (after some preimages written)
// Simulate by dropping the interceptor without calling close_step().
// Recovery should roll back and restore the working dir to pre-step state.
// ---------------------------------------------------------------------------
#[test]
fn cr_01_crash_mid_step_rolls_back() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let before = ws.snapshot();

    // Phase 1: Open a step, mutate files, "crash" (drop without close)
    {
        let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
        let ops = OperationApplier::new(&interceptor);

        interceptor.open_step(1).unwrap();
        ops.write_file(&ws.working_dir.join("small.txt"), b"corrupted");
        ops.create_file(&ws.working_dir.join("crash_artifact.txt"), b"junk");
        // Do NOT call close_step — simulate crash
    }

    // Verify mutations are present (crash state)
    assert_eq!(
        fs::read_to_string(ws.working_dir.join("small.txt")).unwrap(),
        "corrupted"
    );
    assert!(ws.working_dir.join("crash_artifact.txt").exists());

    // Phase 2: Restart and recover
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let recovery = interceptor.recover().unwrap();

    assert!(recovery.is_some());
    let info = recovery.unwrap();
    assert!(info.paths_restored > 0 || info.paths_deleted > 0);
    // Manifest was not written (close_step never ran), so it was reconstructed
    assert!(!info.manifest_valid);

    // Working dir should be restored to pre-step state
    let after = ws.snapshot();
    assert_tree_eq(&before, &after, &compare_opts());

    // WAL should be cleaned up
    assert!(!ws.undo_dir.join("wal").join("in_progress").exists());
}

// ---------------------------------------------------------------------------
// CR-02: Preimage write failure
// Make the preimage directory read-only so capture_preimage fails.
// The pre_write hook should propagate the error.
// ---------------------------------------------------------------------------
#[test]
fn cr_02_preimage_write_failure() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);

    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    interceptor.open_step(1).unwrap();

    // Remove the preimage directory entirely so writes into it fail
    let preimage_dir = ws.undo_dir.join("wal").join("in_progress").join("preimages");
    fs::remove_dir_all(&preimage_dir).unwrap();

    // Attempt a write — the pre_write hook should fail on preimage capture
    let target = ws.working_dir.join("small.txt");
    let result = interceptor.pre_write(&target);

    // The operation should have failed because preimage could not be captured
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// CR-03: Crash during step promotion
// The step completes (manifest written to WAL) but the rename from
// wal/in_progress/ to steps/{id}/ fails. On restart, in_progress still
// exists with a valid manifest.
// ---------------------------------------------------------------------------
#[test]
fn cr_03_crash_during_step_promotion() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let before = ws.snapshot();

    // Phase 1: Complete a step normally, then simulate a post-manifest,
    // pre-rename crash by moving the committed step back to WAL
    {
        let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
        let ops = OperationApplier::new(&interceptor);

        interceptor.open_step(1).unwrap();
        ops.write_file(&ws.working_dir.join("small.txt"), b"modified content");
        interceptor.close_step(1).unwrap();

        // Move committed step back to wal/in_progress to simulate crash
        let step_dir = ws.undo_dir.join("steps").join("1");
        let wal_dir = ws.undo_dir.join("wal").join("in_progress");
        fs::rename(&step_dir, &wal_dir).unwrap();
    }

    // Verify mutation is present
    assert_eq!(
        fs::read_to_string(ws.working_dir.join("small.txt")).unwrap(),
        "modified content"
    );

    // Phase 2: Restart and recover
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let recovery = interceptor.recover().unwrap();

    assert!(recovery.is_some());
    let info = recovery.unwrap();
    assert!(info.manifest_valid);

    // Working dir restored
    let after = ws.snapshot();
    assert_tree_eq(&before, &after, &compare_opts());

    // WAL cleaned up
    assert!(!ws.undo_dir.join("wal").join("in_progress").exists());
}

// ---------------------------------------------------------------------------
// CR-04: Truncated manifest
// The manifest.json exists but is truncated/corrupt (not valid JSON).
// Recovery should fall back to preimage metadata scanning.
// ---------------------------------------------------------------------------
#[test]
fn cr_04_truncated_manifest_recovery() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let before = ws.snapshot();

    // Phase 1: Create crash state with preimages
    {
        let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
        let ops = OperationApplier::new(&interceptor);

        interceptor.open_step(1).unwrap();
        ops.write_file(&ws.working_dir.join("small.txt"), b"will be rolled back");
        ops.create_file(&ws.working_dir.join("new_artifact.txt"), b"artifact");
        // Drop without close — preimages are in WAL but no manifest
    }

    // Write a truncated manifest to the WAL
    let wal_dir = ws.undo_dir.join("wal").join("in_progress");
    let manifest_path = wal_dir.join("manifest.json");
    fs::write(&manifest_path, r#"{"step_id":1,"timestamp":"2026-01-01T00:00"#).unwrap();

    // Phase 2: Restart and recover
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let recovery = interceptor.recover().unwrap();

    assert!(recovery.is_some());
    let info = recovery.unwrap();
    assert!(!info.manifest_valid);

    // Working dir should still be restored via preimage metadata fallback
    let after = ws.snapshot();
    assert_tree_eq(&before, &after, &compare_opts());

    // WAL cleaned up
    assert!(!ws.undo_dir.join("wal").join("in_progress").exists());
}

// ---------------------------------------------------------------------------
// CR-05: Clean shutdown (no crash)
// Normal open/close cycle. Verify WAL is empty and steps/ has the step.
// ---------------------------------------------------------------------------
#[test]
fn cr_05_clean_shutdown_no_wal_residue() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);

    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"modified");
    interceptor.close_step(1).unwrap();

    // WAL should be empty
    assert!(!ws.undo_dir.join("wal").join("in_progress").exists());

    // Steps directory should contain the committed step
    assert!(ws.undo_dir.join("steps").join("1").exists());
    assert!(ws.undo_dir.join("steps").join("1").join("manifest.json").exists());

    // Recovery should be a no-op
    let recovery = interceptor.recover().unwrap();
    assert!(recovery.is_none());
}

// ---------------------------------------------------------------------------
// CR-06: Double recovery (restart twice without new writes)
// First recovery rolls back, second recovery is a no-op.
// ---------------------------------------------------------------------------
#[test]
fn cr_06_double_recovery_is_noop() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let before = ws.snapshot();

    // Phase 1: Create crash state
    {
        let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
        let ops = OperationApplier::new(&interceptor);
        interceptor.open_step(1).unwrap();
        ops.write_file(&ws.working_dir.join("small.txt"), b"crash data");
        // Drop without close
    }

    // Phase 2: First recovery
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let first_recovery = interceptor.recover().unwrap();
    assert!(first_recovery.is_some());

    let after_first = ws.snapshot();
    assert_tree_eq(&before, &after_first, &compare_opts());

    // Phase 3: Second recovery — create new interceptor to simulate second restart
    let interceptor2 = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let second_recovery = interceptor2.recover().unwrap();
    assert!(second_recovery.is_none());

    // Working dir unchanged
    let after_second = ws.snapshot();
    assert_tree_eq(&before, &after_second, &compare_opts());
}

// ---------------------------------------------------------------------------
// CR-07: Crash with empty step (opened but no writes)
// Step opened, no operations, then crash. Recovery discards the empty WAL.
// ---------------------------------------------------------------------------
#[test]
fn cr_07_crash_with_empty_step() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let before = ws.snapshot();

    // Phase 1: Open step, do nothing, "crash"
    {
        let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
        interceptor.open_step(1).unwrap();
        // No operations — drop without close
    }

    // WAL directory should exist (created by open_step)
    assert!(ws.undo_dir.join("wal").join("in_progress").exists());

    // Phase 2: Restart and recover
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let recovery = interceptor.recover().unwrap();

    assert!(recovery.is_some());
    let info = recovery.unwrap();
    assert_eq!(info.paths_restored, 0);
    assert_eq!(info.paths_deleted, 0);

    // Working dir unchanged (no mutations were made)
    let after = ws.snapshot();
    assert_tree_eq(&before, &after, &compare_opts());

    // WAL cleaned up
    assert!(!ws.undo_dir.join("wal").join("in_progress").exists());
}

// ---------------------------------------------------------------------------
// Additional: Verify step reconstruction from disk survives restart
// A new interceptor should recognize previously committed steps.
// ---------------------------------------------------------------------------
#[test]
fn step_reconstruction_from_disk() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);

    // Phase 1: Commit two steps
    {
        let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
        let ops = OperationApplier::new(&interceptor);

        interceptor.open_step(1).unwrap();
        ops.write_file(&ws.working_dir.join("small.txt"), b"step 1");
        interceptor.close_step(1).unwrap();

        interceptor.open_step(2).unwrap();
        ops.write_file(&ws.working_dir.join("small.txt"), b"step 2");
        interceptor.close_step(2).unwrap();

        assert_eq!(interceptor.completed_steps(), vec![1, 2]);
    }

    // Phase 2: Create new interceptor — should reconstruct completed steps
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    assert_eq!(interceptor.completed_steps(), vec![1, 2]);

    // Rollback should still work
    interceptor.rollback(1).unwrap();
    assert_eq!(interceptor.completed_steps(), vec![1]);

    // File should be back to step 1 content
    assert_eq!(
        fs::read_to_string(ws.working_dir.join("small.txt")).unwrap(),
        "step 1"
    );
}
