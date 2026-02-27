use std::fs;
use std::path::PathBuf;

use codeagent_common::{CodeAgentError, ExternalModificationPolicy};
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_test_support::fixtures;
use codeagent_test_support::snapshot::assert_tree_eq;
use codeagent_test_support::workspace::TempWorkspace;

mod common;
use common::{OperationApplier, compare_opts};

// ---------------------------------------------------------------------------
// EB-01: External write during active session
// notify_external_modification creates a barrier with correct affected paths.
// ---------------------------------------------------------------------------
#[test]
fn eb_01_external_modification_creates_barrier() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    // Complete a step so there's a step to place the barrier after
    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"step 1");
    interceptor.close_step(1).unwrap();

    // Simulate external modification
    let affected = vec![
        PathBuf::from("src/main.rs"),
        PathBuf::from("Cargo.toml"),
    ];
    let result = interceptor
        .notify_external_modification(affected.clone())
        .unwrap();

    assert!(result.is_some());
    let barrier = result.unwrap();
    assert_eq!(barrier.after_step_id, 1);
    assert_eq!(barrier.affected_paths, affected);
    assert_eq!(barrier.barrier_id, 1);
}

// ---------------------------------------------------------------------------
// EB-02: Rollback across barrier (no force)
// Rollback is rejected with RollbackBlocked error; working dir unchanged.
// ---------------------------------------------------------------------------
#[test]
fn eb_02_rollback_blocked_by_barrier() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    // Step 1: modify file
    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"step 1 content");
    interceptor.close_step(1).unwrap();

    // External modification after step 1 (barrier blocks rollback of step 1)
    interceptor
        .notify_external_modification(vec![PathBuf::from("README.md")])
        .unwrap();

    let after_step1 = ws.snapshot();

    // Step 2: modify file again
    interceptor.open_step(2).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"step 2 content");
    interceptor.close_step(2).unwrap();

    // Rollback(2) should be blocked — it tries to roll back steps 2 and 1,
    // and the barrier is after step 1
    let result = interceptor.rollback(2, false);
    assert!(result.is_err());

    match result.unwrap_err() {
        CodeAgentError::RollbackBlocked { count, barriers } => {
            assert_eq!(count, 1);
            assert_eq!(barriers[0].after_step_id, 1);
            assert_eq!(barriers[0].affected_paths, vec![PathBuf::from("README.md")]);
        }
        other => panic!("expected RollbackBlocked, got: {other:?}"),
    }

    // Working dir should be unchanged (still at step 2 state)
    assert_eq!(
        fs::read_to_string(ws.working_dir.join("small.txt")).unwrap(),
        "step 2 content"
    );

    // Rollback(1) should succeed — only rolls back step 2, barrier is after step 1
    let result = interceptor.rollback(1, false).unwrap();
    assert_eq!(result.steps_rolled_back, 1);
    assert!(result.barriers_crossed.is_empty());

    // Now at step 1 state
    let after_rollback = ws.snapshot();
    assert_tree_eq(&after_step1, &after_rollback, &compare_opts());
}

// ---------------------------------------------------------------------------
// EB-03: Rollback across barrier (force=true)
// Rollback proceeds with barriers_crossed populated; working dir restored.
// ---------------------------------------------------------------------------
#[test]
fn eb_03_force_rollback_crosses_barrier() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    // Step 1: modify file
    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"step 1 content");
    interceptor.close_step(1).unwrap();

    // External modification after step 1
    interceptor
        .notify_external_modification(vec![PathBuf::from("config.toml")])
        .unwrap();

    // Force rollback should succeed
    let result = interceptor.rollback(1, true).unwrap();
    assert_eq!(result.steps_rolled_back, 1);
    assert_eq!(result.barriers_crossed.len(), 1);
    assert_eq!(result.barriers_crossed[0].after_step_id, 1);

    // Working dir restored to pre-step state
    let after = ws.snapshot();
    assert_tree_eq(&before, &after, &compare_opts());

    // Barriers should be removed after forced rollback
    assert!(interceptor.barriers().is_empty());
}

// ---------------------------------------------------------------------------
// EB-04: Barrier visible in undo.history
// Querying barriers returns entries with correct structure.
// ---------------------------------------------------------------------------
#[test]
fn eb_04_barriers_queryable_with_correct_data() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    // Complete two steps
    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"v1");
    interceptor.close_step(1).unwrap();

    interceptor.open_step(2).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"v2");
    interceptor.close_step(2).unwrap();

    // Create a barrier
    let paths = vec![PathBuf::from("externally_edited.txt")];
    interceptor
        .notify_external_modification(paths.clone())
        .unwrap();

    // Query barriers
    let barriers = interceptor.barriers();
    assert_eq!(barriers.len(), 1);

    let barrier = &barriers[0];
    assert_eq!(barrier.barrier_id, 1);
    assert_eq!(barrier.after_step_id, 2);
    assert_eq!(barrier.affected_paths, paths);
    // Timestamp should be recent (within last 5 seconds)
    let elapsed = chrono::Utc::now() - barrier.timestamp;
    assert!(elapsed.num_seconds() < 5);
}

// ---------------------------------------------------------------------------
// EB-05: Internal sandbox write does NOT trigger barrier
// Performing operations through WriteInterceptor leaves barriers empty.
// ---------------------------------------------------------------------------
#[test]
fn eb_05_internal_writes_do_not_create_barriers() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    // Perform various internal operations
    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"modified");
    ops.create_file(&ws.working_dir.join("new.txt"), b"new content");
    ops.mkdir(&ws.working_dir.join("new_dir"));
    interceptor.close_step(1).unwrap();

    interceptor.open_step(2).unwrap();
    ops.delete_file(&ws.working_dir.join("new.txt"));
    ops.rename(
        &ws.working_dir.join("new_dir"),
        &ws.working_dir.join("renamed_dir"),
    );
    interceptor.close_step(2).unwrap();

    // No barriers should have been created by internal writes
    assert!(interceptor.barriers().is_empty());

    // Rollback should succeed without force
    let result = interceptor.rollback(2, false).unwrap();
    assert_eq!(result.steps_rolled_back, 2);
    assert!(result.barriers_crossed.is_empty());
}

// ---------------------------------------------------------------------------
// EB-06: Multiple barriers between steps
// Each barrier listed; rollback blocked at nearest.
// ---------------------------------------------------------------------------
#[test]
fn eb_06_multiple_barriers_all_reported() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    // Step 1
    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"step 1");
    interceptor.close_step(1).unwrap();

    // Barrier after step 1
    interceptor
        .notify_external_modification(vec![PathBuf::from("file_a.txt")])
        .unwrap();

    // Step 2
    interceptor.open_step(2).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"step 2");
    interceptor.close_step(2).unwrap();

    // Barrier after step 2
    interceptor
        .notify_external_modification(vec![PathBuf::from("file_b.txt")])
        .unwrap();

    // Step 3
    interceptor.open_step(3).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"step 3");
    interceptor.close_step(3).unwrap();

    // Rollback(3) — crosses both barriers
    let result = interceptor.rollback(3, false);
    assert!(result.is_err());

    match result.unwrap_err() {
        CodeAgentError::RollbackBlocked { count, barriers } => {
            assert_eq!(count, 2);
            // Both barriers reported
            let step_ids: Vec<_> = barriers.iter().map(|b| b.after_step_id).collect();
            assert!(step_ids.contains(&1));
            assert!(step_ids.contains(&2));
        }
        other => panic!("expected RollbackBlocked, got: {other:?}"),
    }

    // Rollback(1) — only rolls back step 3, no barrier after step 3
    let result = interceptor.rollback(1, false).unwrap();
    assert_eq!(result.steps_rolled_back, 1);
    assert!(result.barriers_crossed.is_empty());

    // Now rollback(1) again — rolls back step 2, barrier after step 2 blocks
    let result = interceptor.rollback(1, false);
    assert!(result.is_err());
    match result.unwrap_err() {
        CodeAgentError::RollbackBlocked { count, barriers } => {
            assert_eq!(count, 1);
            assert_eq!(barriers[0].after_step_id, 2);
        }
        other => panic!("expected RollbackBlocked, got: {other:?}"),
    }

    // Force rollback(1) to cross the barrier after step 2
    let result = interceptor.rollback(1, true).unwrap();
    assert_eq!(result.steps_rolled_back, 1);
    assert_eq!(result.barriers_crossed.len(), 1);

    // Now only barrier after step 1 should remain
    let remaining = interceptor.barriers();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].after_step_id, 1);
}

// ---------------------------------------------------------------------------
// EB-08: policy=warn — no barrier created, rollback succeeds
// ---------------------------------------------------------------------------
#[test]
fn eb_08_warn_policy_no_barrier() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::with_policy(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        ExternalModificationPolicy::Warn,
    );
    let ops = OperationApplier::new(&interceptor);

    let before = ws.snapshot();

    // Step 1
    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"modified");
    interceptor.close_step(1).unwrap();

    // External modification under Warn policy — no barrier
    let result = interceptor
        .notify_external_modification(vec![PathBuf::from("external.txt")])
        .unwrap();
    assert!(result.is_none());

    // No barriers stored
    assert!(interceptor.barriers().is_empty());

    // Rollback should succeed without force
    let result = interceptor.rollback(1, false).unwrap();
    assert_eq!(result.steps_rolled_back, 1);
    assert!(result.barriers_crossed.is_empty());

    let after = ws.snapshot();
    assert_tree_eq(&before, &after, &compare_opts());
}
