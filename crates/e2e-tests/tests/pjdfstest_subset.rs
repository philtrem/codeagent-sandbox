//! TDD Step 15 — pjdfstest Subset E2E Tests (PJ-01..PJ-05)
//!
//! These tests run curated POSIX filesystem operations inside the guest VM
//! against the mounted working directory and verify basic filesystem semantics
//! (create/unlink/rename/chmod/symlink).
//!
//! Run with: `cargo test -p codeagent-e2e-tests --ignored`

use std::time::Duration;

use codeagent_e2e_tests::messages::*;
use codeagent_e2e_tests::{
    JsonlClient, COMMAND_TIMEOUT, SESSION_START_TIMEOUT, SHUTDOWN_TIMEOUT,
};
use codeagent_test_support::TempWorkspace;

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

/// Execute a command in the VM and collect all terminal output until
/// step completion. Returns the concatenated output.
async fn exec_collect_output(client: &mut JsonlClient, command: &str) -> String {
    let (msg, id) = agent_execute(command);
    client.send(&msg).await.unwrap();

    let mut output = String::new();
    let deadline = tokio::time::Instant::now() + COMMAND_TIMEOUT;

    loop {
        // Try to get terminal output or step_completed, whichever comes first
        tokio::select! {
            terminal = client.recv_event("event.terminal_output", Duration::from_millis(100)) => {
                if let Ok(evt) = terminal {
                    if let Some(data) = evt["payload"]["data"].as_str() {
                        output.push_str(data);
                    }
                }
            }
            completed = client.recv_event("event.step_completed", Duration::from_millis(100)) => {
                if completed.is_ok() {
                    // Drain any remaining terminal output that arrived before completion
                    while let Ok(evt) = client.recv_event(
                        "event.terminal_output",
                        Duration::from_millis(200),
                    ).await {
                        if let Some(data) = evt["payload"]["data"].as_str() {
                            output.push_str(data);
                        }
                    }
                    break;
                }
            }
        }

        if tokio::time::Instant::now() >= deadline {
            panic!("timeout waiting for step completion");
        }
    }

    let _ = client.recv_response(&id, Duration::from_secs(2)).await;
    output
}

// =========================================================================
// PJ-01: create + unlink basic semantics
// =========================================================================

#[tokio::test]
#[ignore]
async fn pj01_create_unlink() {
    let ws = TempWorkspace::new();
    let mut client = setup_session(&ws).await;

    let output = exec_collect_output(
        &mut client,
        "cd /mnt/working && \
         touch testfile && \
         test -f testfile && echo CREATE_OK || echo CREATE_FAIL && \
         rm testfile && \
         test -f testfile && echo DELETE_FAIL || echo DELETE_OK",
    )
    .await;

    assert!(output.contains("CREATE_OK"), "file creation should succeed");
    assert!(output.contains("DELETE_OK"), "file deletion should succeed");

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// PJ-02: rename semantics
// =========================================================================

#[tokio::test]
#[ignore]
async fn pj02_rename() {
    let ws = TempWorkspace::new();
    let mut client = setup_session(&ws).await;

    let output = exec_collect_output(
        &mut client,
        "cd /mnt/working && \
         echo content > src.txt && \
         mv src.txt dst.txt && \
         test -f src.txt && echo SRC_EXISTS || echo SRC_GONE && \
         test -f dst.txt && echo DST_EXISTS || echo DST_GONE && \
         cat dst.txt",
    )
    .await;

    assert!(output.contains("SRC_GONE"));
    assert!(output.contains("DST_EXISTS"));
    assert!(output.contains("content"));

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// PJ-03: chmod semantics
// =========================================================================

#[tokio::test]
#[ignore]
async fn pj03_chmod() {
    let ws = TempWorkspace::new();
    let mut client = setup_session(&ws).await;

    let output = exec_collect_output(
        &mut client,
        "cd /mnt/working && \
         touch script.sh && \
         chmod 755 script.sh && \
         stat -c '%a' script.sh",
    )
    .await;

    assert!(
        output.contains("755"),
        "chmod should set executable bits; got: {output}"
    );

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// PJ-04: symlink basic semantics
// =========================================================================

#[tokio::test]
#[ignore]
async fn pj04_symlink() {
    let ws = TempWorkspace::new();
    let mut client = setup_session(&ws).await;

    let output = exec_collect_output(
        &mut client,
        "cd /mnt/working && \
         echo target_data > real.txt && \
         ln -s real.txt link.txt && \
         test -L link.txt && echo IS_SYMLINK || echo NOT_SYMLINK && \
         cat link.txt",
    )
    .await;

    assert!(output.contains("IS_SYMLINK"));
    assert!(output.contains("target_data"));

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}

// =========================================================================
// PJ-05: directory operations (mkdir, rmdir)
// =========================================================================

#[tokio::test]
#[ignore]
async fn pj05_directory_ops() {
    let ws = TempWorkspace::new();
    let mut client = setup_session(&ws).await;

    let output = exec_collect_output(
        &mut client,
        "cd /mnt/working && \
         mkdir -p a/b/c && \
         test -d a/b/c && echo MKDIR_OK || echo MKDIR_FAIL && \
         rmdir a/b/c && \
         test -d a/b/c && echo RMDIR_FAIL || echo RMDIR_OK",
    )
    .await;

    assert!(output.contains("MKDIR_OK"));
    assert!(output.contains("RMDIR_OK"));

    client.shutdown(SHUTDOWN_TIMEOUT).await.ok();
}
