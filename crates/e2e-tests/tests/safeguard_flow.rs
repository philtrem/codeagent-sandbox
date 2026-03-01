//! TDD Step 16 — Safeguard Flow E2E Tests (SF-01..SF-03)
//!
//! These tests exercise the safeguard system end-to-end: triggering
//! thresholds via destructive VM commands, confirming or denying the
//! safeguard via the STDIO API, and verifying that rollback-on-deny
//! restores the working directory.
//!
//! Run with: `cargo test -p codeagent-e2e-tests --ignored`

use codeagent_e2e_tests::messages::*;
use codeagent_e2e_tests::{
    JsonlClient, COMMAND_TIMEOUT, EVENT_TIMEOUT, SESSION_START_TIMEOUT, SHUTDOWN_TIMEOUT,
};
use codeagent_test_support::fixtures;
use codeagent_test_support::{SnapshotCompareOptions, TempWorkspace, assert_tree_eq};

/// E2E-specific comparison options with wide mtime tolerance.
fn compare_opts() -> SnapshotCompareOptions {
    SnapshotCompareOptions {
        mtime_tolerance_ns: 2_000_000_000, // 2 seconds
        check_xattrs: false,
        exclude_patterns: vec![],
    }
}

/// Start a session with safeguards configured for the given delete threshold.
async fn setup_session_with_safeguards(
    ws: &TempWorkspace,
    delete_threshold: u64,
) -> JsonlClient {
    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "ephemeral");
    client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();

    let (msg, id) = safeguard_configure(Some(delete_threshold), None, false);
    let resp = client.request(&msg, &id, COMMAND_TIMEOUT).await.unwrap();
    assert_eq!(resp["status"], "ok");

    client
}

// =========================================================================
// SF-01: rm -rf triggers delete threshold → deny → verify rollback
// =========================================================================

#[tokio::test]
#[ignore]
async fn sf01_delete_threshold_deny_rollback() {
    let ws = TempWorkspace::with_fixture(fixtures::deep_tree);
    let initial_snapshot = ws.snapshot();

    // Set delete threshold low (5 deletes) so rm -rf triggers it
    let mut client = setup_session_with_safeguards(&ws, 5).await;

    // Execute rm -rf which will exceed the threshold
    let (msg, _id) = agent_execute("rm -rf /mnt/working/level0");
    client.send(&msg).await.unwrap();

    // Should receive safeguard triggered event
    let event = client
        .recv_event("event.safeguard_triggered", EVENT_TIMEOUT)
        .await
        .expect("safeguard should trigger");

    let safeguard_id = event["payload"]["safeguard_id"]
        .as_str()
        .expect("safeguard_id should be a string");
    assert_eq!(event["payload"]["kind"], "delete_threshold");

    // Deny the safeguard
    let (msg, id) = safeguard_confirm(safeguard_id, "deny");
    let resp = client.request(&msg, &id, COMMAND_TIMEOUT).await.unwrap();
    assert_eq!(resp["status"], "ok");

    // Wait for step completion (the step should be rolled back after deny)
    client
        .recv_event("event.step_completed", EVENT_TIMEOUT)
        .await
        .ok();

    // Working directory should be restored to initial state
    assert_tree_eq(&initial_snapshot, &ws.snapshot(), &compare_opts());

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// SF-02: Large file overwrite → allow → verify completion
// =========================================================================

#[tokio::test]
#[ignore]
async fn sf02_overwrite_large_file_allow() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);

    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "ephemeral");
    client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();

    // Configure safeguard: overwrite threshold at 100KB (large.bin is 1MB)
    let (msg, id) = safeguard_configure(None, Some(100_000), false);
    client.request(&msg, &id, COMMAND_TIMEOUT).await.unwrap();

    // Overwrite the large file
    let (msg, _id) = agent_execute("echo 'tiny' > /mnt/working/large.bin");
    client.send(&msg).await.unwrap();

    // Should receive safeguard triggered event
    let event = client
        .recv_event("event.safeguard_triggered", EVENT_TIMEOUT)
        .await
        .expect("safeguard should trigger for large file overwrite");

    let safeguard_id = event["payload"]["safeguard_id"]
        .as_str()
        .expect("safeguard_id should be present");

    // Allow the overwrite
    let (msg, id) = safeguard_confirm(safeguard_id, "allow");
    let resp = client.request(&msg, &id, COMMAND_TIMEOUT).await.unwrap();
    assert_eq!(resp["status"], "ok");

    // Wait for step completion
    client
        .recv_event("event.step_completed", EVENT_TIMEOUT)
        .await
        .expect("step should complete after allow");

    // Verify the overwrite went through
    let content = std::fs::read_to_string(ws.working_dir.join("large.bin")).unwrap();
    assert!(content.contains("tiny"));

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// SF-03: rename-over-existing → configure + confirm round-trip
// =========================================================================

#[tokio::test]
#[ignore]
async fn sf03_safeguard_configure_confirm_roundtrip() {
    let ws = TempWorkspace::with_fixture(fixtures::rename_tree);

    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "ephemeral");
    client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();

    // Configure all three safeguard types with tight thresholds
    let (msg, id) = safeguard_configure(Some(1), Some(1), true);
    let resp = client.request(&msg, &id, COMMAND_TIMEOUT).await.unwrap();
    assert_eq!(resp["status"], "ok");

    // Rename a.txt -> b.txt (rename-over-existing should trigger)
    let (msg, _id) = agent_execute("mv /mnt/working/a.txt /mnt/working/b.txt");
    client.send(&msg).await.unwrap();

    // Should receive safeguard triggered
    let event = client
        .recv_event("event.safeguard_triggered", EVENT_TIMEOUT)
        .await
        .expect("rename-over-existing safeguard should trigger");

    let safeguard_id = event["payload"]["safeguard_id"]
        .as_str()
        .expect("safeguard_id should be present");

    // Allow the rename
    let (msg, id) = safeguard_confirm(safeguard_id, "allow");
    let resp = client.request(&msg, &id, COMMAND_TIMEOUT).await.unwrap();
    assert_eq!(resp["status"], "ok");

    // Wait for step completion
    client
        .recv_event("event.step_completed", EVENT_TIMEOUT)
        .await
        .expect("step should complete");

    // b.txt should now contain a.txt's original content
    let content = std::fs::read_to_string(ws.working_dir.join("b.txt")).unwrap();
    assert_eq!(content, "content of a");
    assert!(!ws.working_dir.join("a.txt").exists());

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}
