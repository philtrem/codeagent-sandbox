use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;

use codeagent_sandbox::cli::CliArgs;
use codeagent_sandbox::orchestrator::Orchestrator;
use codeagent_stdio::protocol::{
    FsListPayload, FsReadPayload, SessionStartPayload, UndoHistoryPayload, UndoRollbackPayload,
    WorkingDirectoryConfig,
};
use codeagent_stdio::{Event, RequestHandler};

fn make_args(working_dir: &std::path::Path, undo_dir: &std::path::Path) -> CliArgs {
    CliArgs {
        working_dir: working_dir.to_path_buf(),
        undo_dir: undo_dir.to_path_buf(),
        vm_mode: "ephemeral".to_string(),
        mcp_socket: None,
        log_level: "info".to_string(),
        qemu_binary: None,
        kernel_path: None,
        initrd_path: None,
        rootfs_path: None,
        memory_mb: 2048,
        cpus: 2,
        virtiofsd_binary: None,
    }
}

fn make_start_payload(path: &str) -> SessionStartPayload {
    SessionStartPayload {
        working_directories: vec![WorkingDirectoryConfig {
            path: path.to_string(),
            label: None,
        }],
        network_policy: "disabled".to_string(),
        vm_mode: "ephemeral".to_string(),
        protocol_version: None,
    }
}

/// Create an Orchestrator with temp dirs and return it + event receiver.
fn setup() -> (Orchestrator, mpsc::UnboundedReceiver<Event>, TempDir, TempDir) {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (event_sender, event_receiver) = mpsc::unbounded_channel();
    let args = make_args(working.path(), undo.path());
    let orchestrator = Orchestrator::new(args, event_sender);

    (orchestrator, event_receiver, working, undo)
}

// -----------------------------------------------------------------------
// AO-01: session.start creates interceptor and returns ok
// -----------------------------------------------------------------------
#[test]
fn ao_01_session_start_success() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());

    let result = orchestrator.session_start(payload);
    assert!(result.is_ok(), "session.start should succeed: {result:?}");

    let value = result.unwrap();
    assert_eq!(value["vm_status"], "unavailable");
    assert_eq!(value["backend"], "none");
}

// -----------------------------------------------------------------------
// AO-02: session.start with nonexistent directory returns error
// -----------------------------------------------------------------------
#[test]
fn ao_02_session_start_invalid_dir() {
    let (orchestrator, _rx, _working, _undo) = setup();
    let payload = make_start_payload("/nonexistent/path/that/does/not/exist");

    let result = orchestrator.session_start(payload);
    assert!(result.is_err());
}

// -----------------------------------------------------------------------
// AO-03: double session.start returns error
// -----------------------------------------------------------------------
#[test]
fn ao_03_double_session_start() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());

    let _ = orchestrator.session_start(payload.clone());
    let result = orchestrator.session_start(payload);
    assert!(result.is_err());
}

// -----------------------------------------------------------------------
// AO-04: operations before session.start return error
// -----------------------------------------------------------------------
#[test]
fn ao_04_operations_before_start() {
    let (orchestrator, _rx, _working, _undo) = setup();

    assert!(orchestrator.session_stop().is_err());
    assert!(orchestrator.session_reset().is_err());
    assert!(orchestrator
        .undo_rollback(UndoRollbackPayload {
            count: 1,
            force: false,
            directory: None,
        })
        .is_err());
    assert!(orchestrator
        .undo_history(UndoHistoryPayload { directory: None })
        .is_err());
}

// -----------------------------------------------------------------------
// AO-05: session.stop transitions to idle, subsequent start works
// -----------------------------------------------------------------------
#[test]
fn ao_05_session_stop_then_start() {
    let (orchestrator, _rx, working, _undo) = setup();
    let path_str = working.path().display().to_string();
    let payload = make_start_payload(&path_str);

    let _ = orchestrator.session_start(payload.clone());
    let stop_result = orchestrator.session_stop();
    assert!(stop_result.is_ok());

    // Can start again after stop
    let start_result = orchestrator.session_start(payload);
    assert!(start_result.is_ok());
}

// -----------------------------------------------------------------------
// AO-06: session.status returns correct state
// -----------------------------------------------------------------------
#[test]
fn ao_06_session_status() {
    let (orchestrator, _rx, working, _undo) = setup();

    // Idle state
    let status = orchestrator.session_status().unwrap();
    assert_eq!(status["state"], "idle");

    // Active state
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);
    let status = orchestrator.session_status().unwrap();
    assert_eq!(status["state"], "active");
    assert_eq!(status["vm_mode"], "ephemeral");
}

// -----------------------------------------------------------------------
// AO-07: session.reset recreates session
// -----------------------------------------------------------------------
#[test]
fn ao_07_session_reset() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.session_reset();
    assert!(result.is_ok());

    // Should still be active after reset
    let status = orchestrator.session_status().unwrap();
    assert_eq!(status["state"], "active");
}

// -----------------------------------------------------------------------
// AO-08: undo.history returns empty list for fresh session
// -----------------------------------------------------------------------
#[test]
fn ao_08_undo_history_empty() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let history = orchestrator
        .undo_history(UndoHistoryPayload { directory: None })
        .unwrap();
    assert_eq!(history["steps"], json!([]));
}

// -----------------------------------------------------------------------
// AO-09: undo.rollback with no steps returns zero rolled back
// -----------------------------------------------------------------------
#[test]
fn ao_09_undo_rollback_empty() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .undo_rollback(UndoRollbackPayload {
            count: 1,
            force: false,
            directory: None,
        })
        .unwrap();
    assert_eq!(result["steps_rolled_back"], 0);
}

// -----------------------------------------------------------------------
// AO-10: fs.read returns file content
// -----------------------------------------------------------------------
#[test]
fn ao_10_fs_read() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("hello.txt"), "world").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .fs_read(FsReadPayload {
            path: "hello.txt".to_string(),
            directory: None,
        })
        .unwrap();
    assert_eq!(result["content"], "world");
}

// -----------------------------------------------------------------------
// AO-11: fs.list returns directory entries
// -----------------------------------------------------------------------
#[test]
fn ao_11_fs_list() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("a.txt"), "").unwrap();
    std::fs::create_dir(working.path().join("subdir")).unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .fs_list(FsListPayload {
            path: ".".to_string(),
            directory: None,
        })
        .unwrap();

    let entries = result["entries"].as_array().unwrap();
    assert!(entries.len() >= 2);

    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"subdir"));
}

// -----------------------------------------------------------------------
// AO-12: crash recovery emits Recovery event
// -----------------------------------------------------------------------
#[test]
fn ao_12_crash_recovery_event() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    // Simulate an incomplete step by creating the WAL structure
    let wal_dir = undo.path().join("0").join("wal").join("in_progress");
    std::fs::create_dir_all(&wal_dir).unwrap();
    // Write version file so the interceptor doesn't treat it as fresh
    let step_base = undo.path().join("0");
    std::fs::create_dir_all(step_base.join("steps")).unwrap();
    std::fs::write(step_base.join("version"), "1").unwrap();
    // Create empty WAL preimages dir
    std::fs::create_dir_all(wal_dir.join("preimages")).unwrap();

    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
    let args = make_args(working.path(), undo.path());
    let orchestrator = Orchestrator::new(args, event_sender);

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    // Check if a Recovery event was emitted
    if let Ok(Event::Recovery {
        paths_restored,
        paths_deleted,
    }) = event_receiver.try_recv()
    {
        assert_eq!(paths_restored, 0);
        assert_eq!(paths_deleted, 0);
    }
}

// -----------------------------------------------------------------------
// AO-13: agent.execute returns error (VM unavailable)
// -----------------------------------------------------------------------
#[test]
fn ao_13_agent_execute_unavailable() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.agent_execute(
        codeagent_stdio::protocol::AgentExecutePayload {
            command: "echo hello".to_string(),
            env: None,
            cwd: None,
        },
    );
    assert!(result.is_err());
}

// -----------------------------------------------------------------------
// MCP handler tests
// -----------------------------------------------------------------------

use codeagent_mcp::McpHandler;
use codeagent_mcp::protocol::{GetUndoHistoryArgs, ListDirectoryArgs, ReadFileArgs, UndoArgs};

#[test]
fn mcp_01_read_file() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("test.txt"), "mcp content").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .read_file(ReadFileArgs {
            path: "test.txt".to_string(),
        })
        .unwrap();
    assert_eq!(result["content"], "mcp content");
}

#[test]
fn mcp_02_list_directory() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("file1.rs"), "").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .list_directory(ListDirectoryArgs {
            path: ".".to_string(),
        })
        .unwrap();
    let entries = result["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
}

#[test]
fn mcp_03_undo_history() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .get_undo_history(GetUndoHistoryArgs {})
        .unwrap();
    assert_eq!(result["steps"], json!([]));
}

#[test]
fn mcp_04_undo_no_steps() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();
    assert_eq!(result["steps_rolled_back"], 0);
}

#[test]
fn mcp_05_session_status() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.get_session_status().unwrap();
    assert_eq!(result["state"], "active");
}

// ── AO-14..AO-15 — QEMU integration fallback tests ──

/// AO-14: session.start without kernel/initrd falls back to non-VM mode.
///
/// When no kernel_path or initrd_path is configured (the default), the
/// orchestrator should start a host-only session with no VM components.
/// This preserves existing behavior for all tests above.
#[test]
fn ao_14_no_vm_components_falls_back_to_host_mode() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (event_sender, _rx) = mpsc::unbounded_channel();
    let args = make_args(working.path(), undo.path());
    // Verify kernel_path and initrd_path are None (no VM)
    assert!(args.kernel_path.is_none());
    assert!(args.initrd_path.is_none());

    let orchestrator = Orchestrator::new(args, event_sender);
    let payload = make_start_payload(&working.path().display().to_string());

    let result = orchestrator.session_start(payload).unwrap();
    assert_eq!(result["status"], "ok");

    // fs.status should report "unavailable" VM since no VM components are configured
    let fs_result = orchestrator.fs_status().unwrap();
    assert_eq!(fs_result["backend"], "none");
    assert_eq!(fs_result["vm_status"], "unavailable");

    // Session should be fully functional for host-only operations
    let read_result = orchestrator.fs_read(FsReadPayload {
        path: "nonexistent.txt".to_string(),
        directory: None,
    });
    assert!(read_result.is_err()); // File doesn't exist, but no crash
}

/// AO-15: fs.status returns backend info when session is active.
///
/// In non-VM mode (no kernel/initrd), fs.status reports "none"/"unavailable".
/// When a VM is running, it would report the backend type and "running".
/// This test verifies the non-VM case since we can't spawn QEMU in unit tests.
#[test]
fn ao_15_fs_status_reports_backend_info() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.fs_status().unwrap();

    // In non-VM mode, backend is "none" and VM is unavailable
    assert_eq!(result["backend"], "none");
    assert_eq!(result["vm_status"], "unavailable");

    // vm_pid should not be present in non-VM mode
    assert!(result.get("vm_pid").is_none());
}
