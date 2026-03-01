//! TDD Step 14 — Undo Round-Trip E2E Tests (UR-01..UR-05)
//!
//! These tests execute commands inside the VM that modify files in the shared
//! working directory, then perform undo rollback and verify the working
//! directory is restored to its pre-mutation snapshot.
//!
//! Run with: `cargo test -p codeagent-e2e-tests --ignored`

use std::time::Duration;

use codeagent_e2e_tests::messages::*;
use codeagent_e2e_tests::{
    JsonlClient, COMMAND_TIMEOUT, ROLLBACK_TIMEOUT, SESSION_START_TIMEOUT, SHUTDOWN_TIMEOUT,
};
use codeagent_test_support::fixtures;
use codeagent_test_support::{SnapshotCompareOptions, TempWorkspace, assert_tree_eq};

/// Start a session and return the client ready for commands.
async fn setup_session(ws: &TempWorkspace) -> JsonlClient {
    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "ephemeral");
    let resp = client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");
    client
}

/// Execute a command in the VM and wait for step completion.
async fn execute_and_wait(client: &mut JsonlClient, command: &str) {
    let (msg, id) = agent_execute(command);
    client.send(&msg).await.unwrap();
    client
        .recv_event("event.step_completed", COMMAND_TIMEOUT)
        .await
        .expect("step should complete");
    let _ = client.recv_response(&id, Duration::from_secs(2)).await;
}

/// E2E-specific comparison options with wide mtime tolerance for VM filesystem.
fn compare_opts() -> SnapshotCompareOptions {
    SnapshotCompareOptions {
        mtime_tolerance_ns: 2_000_000_000, // 2 seconds
        check_xattrs: false,
        exclude_patterns: vec![],
    }
}

// =========================================================================
// UR-01: Single file write → rollback → snapshot compare
// =========================================================================

#[tokio::test]
#[ignore]
async fn ur01_single_file_write_rollback() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let initial_snapshot = ws.snapshot();

    let mut client = setup_session(&ws).await;

    // Mutate a file inside the VM
    execute_and_wait(&mut client, "echo 'modified content' > /mnt/working/small.txt").await;

    // Verify mutation happened on host
    let post_content = std::fs::read_to_string(ws.working_dir.join("small.txt")).unwrap();
    assert!(post_content.contains("modified content"));

    // Rollback
    let (msg, id) = undo_rollback(1);
    let resp = client
        .request(&msg, &id, ROLLBACK_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    // Verify restoration
    assert_tree_eq(&initial_snapshot, &ws.snapshot(), &compare_opts());

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// UR-02: Multi-file mutation (one step) → rollback → snapshot match
// =========================================================================

#[tokio::test]
#[ignore]
async fn ur02_multi_file_mutation_rollback() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let initial_snapshot = ws.snapshot();

    let mut client = setup_session(&ws).await;

    // Multiple mutations in one command (all in one step)
    execute_and_wait(
        &mut client,
        "echo 'new' > /mnt/working/small.txt && \
         echo 'also new' > /mnt/working/empty.txt && \
         mkdir -p /mnt/working/newdir && \
         echo 'created' > /mnt/working/newdir/file.txt",
    )
    .await;

    // Rollback the single step
    let (msg, id) = undo_rollback(1);
    let resp = client
        .request(&msg, &id, ROLLBACK_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    assert_tree_eq(&initial_snapshot, &ws.snapshot(), &compare_opts());

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// UR-03: Multi-step → partial rollback → verify intermediate state
// =========================================================================

#[tokio::test]
#[ignore]
async fn ur03_multi_step_partial_rollback() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let initial_snapshot = ws.snapshot();

    let mut client = setup_session(&ws).await;

    // Step 1: modify small.txt
    execute_and_wait(&mut client, "echo 'step1' > /mnt/working/small.txt").await;
    let after_step1 = ws.snapshot();

    // Step 2: modify empty.txt
    execute_and_wait(&mut client, "echo 'step2' > /mnt/working/empty.txt").await;

    // Rollback only step 2
    let (msg, id) = undo_rollback(1);
    let resp = client
        .request(&msg, &id, ROLLBACK_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    // Should match after_step1 (step 1 changes remain)
    assert_tree_eq(&after_step1, &ws.snapshot(), &compare_opts());

    // Rollback step 1
    let (msg, id) = undo_rollback(1);
    let resp = client
        .request(&msg, &id, ROLLBACK_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    // Should match initial state
    assert_tree_eq(&initial_snapshot, &ws.snapshot(), &compare_opts());

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// UR-04: Delete tree → rollback → snapshot compare
// =========================================================================

#[tokio::test]
#[ignore]
async fn ur04_delete_tree_rollback() {
    let ws = TempWorkspace::with_fixture(fixtures::deep_tree);
    let initial_snapshot = ws.snapshot();

    let mut client = setup_session(&ws).await;

    // Delete the entire tree
    execute_and_wait(&mut client, "rm -rf /mnt/working/level0").await;

    // Verify deletion on host
    assert!(!ws.working_dir.join("level0").exists());

    // Rollback
    let (msg, id) = undo_rollback(1);
    let resp = client
        .request(&msg, &id, ROLLBACK_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    assert_tree_eq(&initial_snapshot, &ws.snapshot(), &compare_opts());

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// UR-05: Rename → rollback → verify source and dest restored
// =========================================================================

#[tokio::test]
#[ignore]
async fn ur05_rename_rollback() {
    let ws = TempWorkspace::with_fixture(fixtures::rename_tree);
    let initial_snapshot = ws.snapshot();

    let mut client = setup_session(&ws).await;

    // Rename a.txt -> c.txt (b.txt remains)
    execute_and_wait(&mut client, "mv /mnt/working/a.txt /mnt/working/c.txt").await;

    // Verify rename on host
    assert!(!ws.working_dir.join("a.txt").exists());
    assert!(ws.working_dir.join("c.txt").exists());

    // Rollback
    let (msg, id) = undo_rollback(1);
    let resp = client
        .request(&msg, &id, ROLLBACK_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    // a.txt should be restored, c.txt should be gone
    assert_tree_eq(&initial_snapshot, &ws.snapshot(), &compare_opts());

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}
