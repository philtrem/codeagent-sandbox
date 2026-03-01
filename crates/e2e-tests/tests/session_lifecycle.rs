//! TDD Step 13 — Session Lifecycle E2E Tests (SL-01..SL-08)
//!
//! These tests exercise the full agent binary's session lifecycle through
//! the STDIO API. All tests require QEMU/KVM and are `#[ignore]` by default.
//!
//! Run with: `cargo test -p codeagent-e2e-tests --ignored`

use std::time::Duration;

use codeagent_e2e_tests::messages::*;
use codeagent_e2e_tests::{
    JsonlClient, COMMAND_TIMEOUT, EVENT_TIMEOUT, SESSION_START_TIMEOUT, SHUTDOWN_TIMEOUT,
};
use codeagent_test_support::TempWorkspace;

// =========================================================================
// SL-01: session.start with invalid working dir → structured error
// =========================================================================

#[tokio::test]
#[ignore]
async fn sl01_session_start_invalid_working_dir() {
    let ws = TempWorkspace::new();
    let nonexistent = ws.working_dir.join("nonexistent");

    let mut client = JsonlClient::spawn(&nonexistent, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&nonexistent.to_string_lossy()], "ephemeral");
    client.send(&msg).await.expect("send failed");

    let resp = client
        .recv_response(&id, SESSION_START_TIMEOUT)
        .await
        .expect("agent should respond with error");

    assert_eq!(resp["status"], "error");
    assert!(resp["error"]["code"].is_string());
    assert!(!resp["error"]["message"].as_str().unwrap_or("").is_empty());

    client.kill().await.ok();
}

// =========================================================================
// SL-02: session.start with multiple directories → each acknowledged
// =========================================================================

#[tokio::test]
#[ignore]
async fn sl02_session_start_multiple_dirs() {
    let ws = TempWorkspace::new();
    let dir_a = ws.working_dir.join("project_a");
    let dir_b = ws.working_dir.join("project_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(
        &[&dir_a.to_string_lossy(), &dir_b.to_string_lossy()],
        "ephemeral",
    );
    let resp = client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();

    assert_eq!(resp["status"], "ok");
    assert!(resp["payload"].is_object());

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// SL-03: session.stop (persistent) → VM shuts down, disk image preserved
// =========================================================================

#[tokio::test]
#[ignore]
async fn sl03_session_stop_persistent() {
    let ws = TempWorkspace::new();
    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "persistent", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "persistent");
    let resp = client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    let (msg, id) = session_stop();
    let resp = client
        .request(&msg, &id, SHUTDOWN_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    // Agent process should exit cleanly
    let status = client.child.wait().await.unwrap();
    assert!(status.success());

    // Undo dir should still exist (agent must not delete it)
    assert!(ws.undo_dir.exists());
}

// =========================================================================
// SL-04: session.stop (ephemeral) → VM destroyed, disk image deleted
// =========================================================================

#[tokio::test]
#[ignore]
async fn sl04_session_stop_ephemeral() {
    let ws = TempWorkspace::new();
    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "ephemeral");
    client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();

    // Execute a command to create VM state
    let (msg, id) = agent_execute("echo hello");
    client.send(&msg).await.unwrap();
    client
        .recv_event("event.step_completed", COMMAND_TIMEOUT)
        .await
        .ok();
    let _ = client.recv_response(&id, Duration::from_secs(2)).await;

    let (msg, id) = session_stop();
    let resp = client
        .request(&msg, &id, SHUTDOWN_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    // Agent should exit cleanly
    let status = client.child.wait().await.unwrap();
    assert!(status.success());
}

// =========================================================================
// SL-05: session.reset → persistent VM wiped and recreated
// =========================================================================

#[tokio::test]
#[ignore]
async fn sl05_session_reset() {
    let ws = TempWorkspace::new();
    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "persistent", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "persistent");
    client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();

    // Create persistent state inside the VM
    let (msg, id) = agent_execute("touch /tmp/persistent_marker");
    client.send(&msg).await.unwrap();
    client
        .recv_event("event.step_completed", COMMAND_TIMEOUT)
        .await
        .unwrap();
    let _ = client.recv_response(&id, Duration::from_secs(2)).await;

    // Reset the session — this wipes and recreates the VM
    let (msg, id) = session_reset();
    let resp = client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(resp["status"], "ok");

    // Verify persistent state is gone
    let (msg, _id) = agent_execute("test -f /tmp/persistent_marker && echo EXISTS || echo GONE");
    client.send(&msg).await.unwrap();
    let event = client
        .recv_event("event.terminal_output", EVENT_TIMEOUT)
        .await
        .unwrap();
    let output = event["payload"]["data"].as_str().unwrap_or("");
    assert!(output.contains("GONE"));

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// SL-06: QEMU launch failure → error response, agent doesn't hang
// =========================================================================

#[tokio::test]
#[ignore]
async fn sl06_qemu_launch_failure() {
    let ws = TempWorkspace::new();
    // Pass an invalid QEMU path to force a launch failure
    let mut client = JsonlClient::spawn(
        &ws.working_dir,
        &ws.undo_dir,
        "ephemeral",
        &["--qemu-binary", "/nonexistent/qemu-system-x86_64"],
    )
    .await
    .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "ephemeral");
    client.send(&msg).await.unwrap();

    // Agent must respond with an error, not hang
    let resp = client
        .recv_response(&id, SESSION_START_TIMEOUT)
        .await
        .expect("agent should respond even on QEMU failure");

    assert_eq!(resp["status"], "error");

    client.kill().await.ok();
}

// =========================================================================
// SL-07: Control channel disconnect → error event emitted
// =========================================================================

#[tokio::test]
#[ignore]
async fn sl07_control_channel_disconnect() {
    let ws = TempWorkspace::new();
    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "ephemeral");
    client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();

    // Crash the VM by killing PID 1 inside it — simulates control channel disconnect
    let (msg, _id) = agent_execute("kill -9 1");
    client.send(&msg).await.unwrap();

    // Agent should detect the disconnect and emit an error event
    let event = client
        .recv_event("event.error", Duration::from_secs(30))
        .await
        .expect("agent should emit error event on control channel disconnect");

    assert!(event["payload"]["code"].is_string());

    client.kill().await.ok();
}

// =========================================================================
// SL-08: Resource cleanup on stop → sockets removed, processes terminated
// =========================================================================

#[tokio::test]
#[ignore]
async fn sl08_resource_cleanup_on_stop() {
    let ws = TempWorkspace::new();
    let mut client = JsonlClient::spawn(&ws.working_dir, &ws.undo_dir, "ephemeral", &[])
        .await
        .expect("failed to spawn agent");

    let (msg, id) = session_start(&[&ws.working_dir.to_string_lossy()], "ephemeral");
    client
        .request(&msg, &id, SESSION_START_TIMEOUT)
        .await
        .unwrap();

    // Query session status to discover socket paths
    let (msg, id) = session_status();
    let status_resp = client.request(&msg, &id, COMMAND_TIMEOUT).await.unwrap();

    let (msg, id) = session_stop();
    client.request(&msg, &id, SHUTDOWN_TIMEOUT).await.ok();

    // Wait for process to exit
    let exit = tokio::time::timeout(SHUTDOWN_TIMEOUT, client.child.wait()).await;
    assert!(exit.is_ok(), "agent should exit after session.stop");

    // Verify socket files are cleaned up (if path was reported in status)
    if let Some(socket_path) = status_resp
        .get("payload")
        .and_then(|p| p.get("mcp_socket_path"))
        .and_then(|v| v.as_str())
    {
        assert!(
            !std::path::Path::new(socket_path).exists(),
            "MCP socket should be cleaned up after stop"
        );
    }
}
