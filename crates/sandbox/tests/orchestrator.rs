use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;

use codeagent_sandbox::cli::CliArgs;
use codeagent_sandbox::command_classifier::CommandClassifierConfig;
use codeagent_sandbox::config::FileWatcherConfig;
use codeagent_sandbox::orchestrator::{undo_subdir_name, Orchestrator};
use codeagent_stdio::protocol::{
    FsListPayload, FsReadPayload, SessionStartPayload, UndoHistoryPayload, UndoRollbackPayload,
    WorkingDirectoryConfig,
};
use codeagent_stdio::{Event, RequestHandler};

fn make_args(working_dir: &std::path::Path, undo_dir: &std::path::Path) -> CliArgs {
    CliArgs {
        working_dirs: vec![working_dir.to_path_buf()],
        undo_dir: Some(undo_dir.to_path_buf()),
        vm_mode: "ephemeral".to_string(),
        protocol: "stdio".to_string(),
        log_level: "info".to_string(),
        qemu_binary: None,
        kernel_path: None,
        initrd_path: None,
        rootfs_path: None,
        memory_mb: 2048,
        cpus: 2,
        virtiofsd_binary: None,
        config_file: None,
        socket_path: None,
        log_file: None,
        disable_builtin_tools: false,
        auto_allow_write_tools: false,
        server_name: "codeagent-sandbox".into(),
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
    let orchestrator = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });

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
    let subdir_name = undo_subdir_name(working.path());
    let wal_dir = undo.path().join(&subdir_name).join("wal").join("in_progress");
    std::fs::create_dir_all(&wal_dir).unwrap();
    // Write version file so the interceptor doesn't treat it as fresh
    let step_base = undo.path().join(&subdir_name);
    std::fs::create_dir_all(step_base.join("steps")).unwrap();
    std::fs::write(step_base.join("version"), "1").unwrap();
    // Create empty WAL preimages dir
    std::fs::create_dir_all(wal_dir.join("preimages")).unwrap();

    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
    let args = make_args(working.path(), undo.path());
    let orchestrator = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });

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
use codeagent_mcp::protocol::{
    BashArgs, EditFileArgs, GetUndoHistoryArgs, GlobArgs, GrepArgs, ListDirectoryArgs,
    ReadFileArgs, UndoArgs, WriteFileArgs,
};

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

#[test]
fn mcp_06_edit_file() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("hello.txt"), "hello world").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .edit_file(EditFileArgs {
            path: "hello.txt".to_string(),
            old_string: "world".to_string(),
            new_string: "rust".to_string(),
            replace_all: false,
        })
        .unwrap();
    assert!(result.as_str().unwrap().contains("updated successfully"));

    let content = std::fs::read_to_string(working.path().join("hello.txt")).unwrap();
    assert_eq!(content, "hello rust");
}

#[test]
fn mcp_07_edit_file_undo() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("data.txt"), "original content").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    orchestrator
        .edit_file(EditFileArgs {
            path: "data.txt".to_string(),
            old_string: "original".to_string(),
            new_string: "modified".to_string(),
            replace_all: false,
        })
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(working.path().join("data.txt")).unwrap(),
        "modified content"
    );

    orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(working.path().join("data.txt")).unwrap(),
        "original content"
    );
}

#[test]
fn mcp_08_edit_file_not_found() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("test.txt"), "hello world").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.edit_file(EditFileArgs {
        path: "test.txt".to_string(),
        old_string: "missing".to_string(),
        new_string: "replacement".to_string(),
        replace_all: false,
    });
    assert!(result.is_err());
}

#[test]
fn mcp_09_edit_file_not_unique() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("test.txt"), "abc abc abc").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.edit_file(EditFileArgs {
        path: "test.txt".to_string(),
        old_string: "abc".to_string(),
        new_string: "xyz".to_string(),
        replace_all: false,
    });
    assert!(result.is_err());

    // But replace_all should work
    let result = orchestrator.edit_file(EditFileArgs {
        path: "test.txt".to_string(),
        old_string: "abc".to_string(),
        new_string: "xyz".to_string(),
        replace_all: true,
    });
    assert!(result.is_ok());
    assert_eq!(
        std::fs::read_to_string(working.path().join("test.txt")).unwrap(),
        "xyz xyz xyz"
    );
}

#[test]
fn mcp_10_glob() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::create_dir_all(working.path().join("src")).unwrap();
    std::fs::write(working.path().join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(working.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(working.path().join("readme.md"), "").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .glob(GlobArgs {
            pattern: "**/*.rs".to_string(),
            path: None,
            limit: None,
        })
        .unwrap();
    let output = result.as_str().unwrap();
    assert!(output.contains("src/main.rs"));
    assert!(output.contains("src/lib.rs"));
    assert!(!output.contains("readme.md"));
}

#[test]
fn mcp_11_grep_files_with_matches() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("a.txt"), "hello world\nfoo bar").unwrap();
    std::fs::write(working.path().join("b.txt"), "goodbye world").unwrap();
    std::fs::write(working.path().join("c.txt"), "no match here").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .grep(GrepArgs {
            pattern: "world".to_string(),
            path: None,
            include: None,
            output_mode: "files_with_matches".to_string(),
            context_lines: None,
            case_insensitive: false,
        })
        .unwrap();
    let output = result.as_str().unwrap();
    assert!(output.contains("a.txt"));
    assert!(output.contains("b.txt"));
    assert!(!output.contains("c.txt"));
}

#[test]
fn mcp_12_grep_content_mode() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("test.rs"), "line1\nfn main() {}\nline3").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .grep(GrepArgs {
            pattern: "main".to_string(),
            path: None,
            include: None,
            output_mode: "content".to_string(),
            context_lines: None,
            case_insensitive: false,
        })
        .unwrap();
    let output = result.as_str().unwrap();
    assert!(output.contains("test.rs"));
    assert!(output.contains("2:fn main() {}"));
}

#[test]
fn mcp_13_grep_count_mode() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("test.txt"), "foo\nfoo bar\nbaz foo").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator
        .grep(GrepArgs {
            pattern: "foo".to_string(),
            path: None,
            include: None,
            output_mode: "count".to_string(),
            context_lines: None,
            case_insensitive: false,
        })
        .unwrap();
    let output = result.as_str().unwrap();
    assert!(output.contains("test.txt:3"));
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

    let orchestrator = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });
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

// -----------------------------------------------------------------------
// AO-16: session.start rejects undo dir inside working dir
// -----------------------------------------------------------------------
#[test]
fn ao_16_undo_inside_working_dir_rejected() {
    let working = TempDir::new().unwrap();
    let undo = working.path().join("undo");
    std::fs::create_dir_all(&undo).unwrap();

    let (event_sender, _rx) = mpsc::unbounded_channel();
    let args = CliArgs {
        working_dirs: vec![working.path().to_path_buf()],
        undo_dir: Some(undo.clone()),
        vm_mode: "ephemeral".to_string(),
        protocol: "stdio".to_string(),
        log_level: "info".to_string(),
        qemu_binary: None,
        kernel_path: None,
        initrd_path: None,
        rootfs_path: None,
        memory_mb: 2048,
        cpus: 2,
        virtiofsd_binary: None,
        config_file: None,
        socket_path: None,
        log_file: None,
        disable_builtin_tools: false,
        auto_allow_write_tools: false,
        server_name: "codeagent-sandbox".into(),
    };
    let orchestrator = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });

    let payload = make_start_payload(&working.path().display().to_string());
    let result = orchestrator.session_start(payload);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("overlaps"), "expected overlap error, got: {err}");
}

// -----------------------------------------------------------------------
// AO-17: session.start rejects working dir inside undo dir
// -----------------------------------------------------------------------
#[test]
fn ao_17_working_inside_undo_dir_rejected() {
    let undo = TempDir::new().unwrap();
    let working = undo.path().join("project");
    std::fs::create_dir_all(&working).unwrap();

    let (event_sender, _rx) = mpsc::unbounded_channel();
    let args = CliArgs {
        working_dirs: vec![working.clone()],
        undo_dir: Some(undo.path().to_path_buf()),
        vm_mode: "ephemeral".to_string(),
        protocol: "stdio".to_string(),
        log_level: "info".to_string(),
        qemu_binary: None,
        kernel_path: None,
        initrd_path: None,
        rootfs_path: None,
        memory_mb: 2048,
        cpus: 2,
        virtiofsd_binary: None,
        config_file: None,
        socket_path: None,
        log_file: None,
        disable_builtin_tools: false,
        auto_allow_write_tools: false,
        server_name: "codeagent-sandbox".into(),
    };
    let orchestrator = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });

    let payload = make_start_payload(&working.display().to_string());
    let result = orchestrator.session_start(payload);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("overlaps"), "expected overlap error, got: {err}");
}

// -----------------------------------------------------------------------
// BA-01: Sanitization rejection returns InvalidParams error
// -----------------------------------------------------------------------
#[test]
fn ba_01_bash_sanitization_rejection() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    // Fork bomb should be rejected before reaching the VM
    let result = orchestrator.bash(BashArgs {
        command: ":(){ :|:& };:".to_string(),
        description: None,
        timeout: None,
    });
    assert!(result.is_err());
    let err = result.unwrap_err();
    let rpc_err = err.to_jsonrpc_error();
    // JSON-RPC 2.0 invalid params code
    assert_eq!(rpc_err.code, -32602);
    assert!(
        rpc_err.message.contains("fork bomb"),
        "expected fork bomb rejection, got: {}",
        rpc_err.message
    );
}

// -----------------------------------------------------------------------
// BA-02: Sudo rejection
// -----------------------------------------------------------------------
#[test]
fn ba_02_bash_sudo_rejected() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.bash(BashArgs {
        command: "sudo rm -rf /".to_string(),
        description: None,
        timeout: None,
    });
    assert!(result.is_err());
    let err = result.unwrap_err();
    let rpc_err = err.to_jsonrpc_error();
    assert!(
        rpc_err.message.contains("privilege escalation"),
        "expected privilege escalation rejection, got: {}",
        rpc_err.message
    );
}

// -----------------------------------------------------------------------
// BA-03: Empty command rejection
// -----------------------------------------------------------------------
#[test]
fn ba_03_bash_empty_command_rejected() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.bash(BashArgs {
        command: "".to_string(),
        description: None,
        timeout: None,
    });
    assert!(result.is_err());
    let err = result.unwrap_err();
    let rpc_err = err.to_jsonrpc_error();
    assert!(
        rpc_err.message.contains("empty command"),
        "expected empty command rejection, got: {}",
        rpc_err.message
    );
}

// -----------------------------------------------------------------------
// BA-04: Host-only bash execution works without VM
// -----------------------------------------------------------------------
#[test]
fn ba_04_bash_host_only() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let result = orchestrator.bash(BashArgs {
        command: "echo hello".to_string(),
        description: None,
        timeout: None,
    });
    assert!(result.is_ok(), "host-only bash should succeed: {:?}", result.err());

    let value = result.unwrap();
    assert!(
        value["host_only"].as_bool().unwrap_or(false),
        "response should include host_only flag"
    );
    assert_eq!(value["exit_code"].as_i64().unwrap(), 0);
    let output = value["output"].as_str().unwrap();
    assert!(
        output.contains("hello"),
        "expected 'hello' in output, got: {output}"
    );
}

// -----------------------------------------------------------------------
// BA-05: Host-only bash still rejects sanitization failures
// -----------------------------------------------------------------------
#[test]
fn ba_05_bash_host_sanitize() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    // Fork bomb should be rejected before host execution
    let result = orchestrator.bash(BashArgs {
        command: ":(){ :|:& };:".to_string(),
        description: None,
        timeout: None,
    });
    assert!(result.is_err(), "sanitization should still apply in host-only mode");
}

// ── MCP write_file undo tracking tests ──

/// write_file creating a new file should produce an undo step that,
/// when rolled back, removes the file.
#[test]
fn mcp_14_write_file_new_file_undo() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let target = working.path().join("new.txt");
    assert!(!target.exists());

    let result = orchestrator
        .write_file(WriteFileArgs {
            path: "new.txt".to_string(),
            content: "hello".to_string(),
        })
        .unwrap();
    assert_eq!(result["written"], true);
    assert!(target.exists());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");

    // Undo should remove the newly created file
    orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();
    assert!(
        !target.exists(),
        "newly created file should be removed after undo"
    );
}

/// write_file overwriting an existing file should capture the preimage
/// and restore it on undo.
#[test]
fn mcp_15_write_file_overwrite_undo() {
    let (orchestrator, _rx, working, _undo) = setup();
    std::fs::write(working.path().join("existing.txt"), "original").unwrap();

    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    orchestrator
        .write_file(WriteFileArgs {
            path: "existing.txt".to_string(),
            content: "replaced".to_string(),
        })
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(working.path().join("existing.txt")).unwrap(),
        "replaced"
    );

    orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(working.path().join("existing.txt")).unwrap(),
        "original"
    );
}

/// write_file to a nested path should create parent directories and
/// track them for undo.
#[test]
fn mcp_16_write_file_creates_parent_dirs_undo() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    let nested_dir = working.path().join("a").join("b");
    let target = nested_dir.join("file.txt");
    assert!(!working.path().join("a").exists());

    orchestrator
        .write_file(WriteFileArgs {
            path: "a/b/file.txt".to_string(),
            content: "deep".to_string(),
        })
        .unwrap();
    assert!(target.exists());

    // Undo should remove the file and the created directories
    orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();
    assert!(!target.exists(), "file should be removed after undo");
    assert!(
        !working.path().join("a").exists(),
        "created parent directories should be removed after undo"
    );
}

// -----------------------------------------------------------------------
// MCP-17: write_file error cancels the step so subsequent operations work
// -----------------------------------------------------------------------
#[test]
fn mcp_17_write_file_error_cancels_step() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    // Create a directory so writing a file at that path fails.
    std::fs::create_dir_all(working.path().join("blocker")).unwrap();

    let result = orchestrator.write_file(WriteFileArgs {
        path: "blocker".to_string(),
        content: "should fail".to_string(),
    });
    assert!(result.is_err(), "writing to a directory path should fail");

    // A subsequent write_file should succeed (step was cleaned up, not left open).
    let result = orchestrator.write_file(WriteFileArgs {
        path: "ok.txt".to_string(),
        content: "works".to_string(),
    });
    assert!(result.is_ok(), "subsequent write should succeed after error cleanup");
    assert_eq!(
        std::fs::read_to_string(working.path().join("ok.txt")).unwrap(),
        "works"
    );
}

// -----------------------------------------------------------------------
// MCP-18: edit_file error cancels the step so subsequent operations work
// -----------------------------------------------------------------------
#[test]
fn mcp_18_edit_file_error_cancels_step() {
    let (orchestrator, _rx, working, _undo) = setup();
    let payload = make_start_payload(&working.path().display().to_string());
    let _ = orchestrator.session_start(payload);

    // Create a file, then make target path a directory to cause write failure.
    std::fs::write(working.path().join("target.txt"), "original").unwrap();

    // edit_file reads the file, validates, then writes. We need the write to fail.
    // Make the file read-only so write fails on Windows (on Unix we need a different approach).
    #[cfg(windows)]
    {
        let target = working.path().join("target.txt");
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&target, perms).unwrap();

        let result = orchestrator.edit_file(EditFileArgs {
            path: "target.txt".to_string(),
            old_string: "original".to_string(),
            new_string: "modified".to_string(),
            replace_all: false,
        });
        assert!(result.is_err(), "writing to read-only file should fail");

        // Restore permissions for cleanup.
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&target, perms).unwrap();
    }

    #[cfg(not(windows))]
    {
        // On Unix, remove write permission from the parent directory to prevent write.
        use std::os::unix::fs::PermissionsExt;
        let target = working.path().join("target.txt");
        let parent = target.parent().unwrap();
        let original_perms = std::fs::metadata(parent).unwrap().permissions();
        let mut no_write = original_perms.clone();
        no_write.set_mode(0o555);
        std::fs::set_permissions(parent, no_write).unwrap();

        let result = orchestrator.edit_file(EditFileArgs {
            path: "target.txt".to_string(),
            old_string: "original".to_string(),
            new_string: "modified".to_string(),
            replace_all: false,
        });
        assert!(result.is_err(), "writing to read-only dir should fail");

        // Restore permissions for cleanup.
        std::fs::set_permissions(parent, original_perms).unwrap();
    }

    // A subsequent edit should succeed (step was cleaned up, not left open).
    let result = orchestrator.edit_file(EditFileArgs {
        path: "target.txt".to_string(),
        old_string: "original".to_string(),
        new_string: "modified".to_string(),
        replace_all: false,
    });
    assert!(result.is_ok(), "subsequent edit should succeed after error cleanup");
    assert_eq!(
        std::fs::read_to_string(working.path().join("target.txt")).unwrap(),
        "modified"
    );
}

// -----------------------------------------------------------------------
// AO-18: new session places barrier when previous steps exist
// -----------------------------------------------------------------------
#[test]
fn ao_18_session_boundary_barrier() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: write a file to create an undo step
    {
        let (event_sender, _rx) = mpsc::unbounded_channel();
        let args = make_args(working.path(), undo.path());
        let orch = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });
        let _ = orch.session_start(make_start_payload(&path_str));

        orch.write_file(WriteFileArgs {
            path: "file.txt".to_string(),
            content: "hello".to_string(),
        })
        .unwrap();
        assert!(working.path().join("file.txt").exists());

        let _ = orch.session_stop();
    }

    // Session 2: reuse same working + undo dirs
    {
        let (event_sender, mut rx) = mpsc::unbounded_channel();
        let args = make_args(working.path(), undo.path());
        let orch = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });
        let _ = orch.session_start(make_start_payload(&path_str));

        // Should have received an ExternalModification event with a barrier
        let mut got_barrier_event = false;
        while let Ok(event) = rx.try_recv() {
            if let Event::ExternalModification {
                barrier_id,
                affected_paths: _,
            } = event
            {
                assert!(barrier_id.is_some(), "barrier_id should be present");
                got_barrier_event = true;
            }
        }
        assert!(got_barrier_event, "expected ExternalModification event on session start");

        // Rollback without force should fail (barrier blocks it)
        let result = orch.undo(UndoArgs {
            count: 1,
            force: false,
        });
        assert!(result.is_err(), "rollback should be blocked by session boundary barrier");

        // Rollback with force should succeed
        let result = orch.undo(UndoArgs {
            count: 1,
            force: true,
        });
        assert!(result.is_ok(), "forced rollback should cross the barrier");
        assert!(
            !working.path().join("file.txt").exists(),
            "file should be removed after forced rollback"
        );
    }
}

// -----------------------------------------------------------------------
// AO-19: new session with no previous steps does NOT place a barrier
// -----------------------------------------------------------------------
#[test]
fn ao_19_no_barrier_on_fresh_session() {
    let (orchestrator, mut rx, working, _undo) = setup();
    let _ = orchestrator.session_start(make_start_payload(&working.path().display().to_string()));

    // Drain events — none should be ExternalModification
    while let Ok(event) = rx.try_recv() {
        if matches!(event, Event::ExternalModification { .. }) {
            panic!("should not emit ExternalModification on a fresh session with no steps");
        }
    }
}

// -----------------------------------------------------------------------
// AO-20: undo_subdir_name produces stable hashes
// -----------------------------------------------------------------------
#[test]
fn ao_20_stable_undo_subdir_name() {
    let dir = TempDir::new().unwrap();
    let name1 = undo_subdir_name(dir.path());
    let name2 = undo_subdir_name(dir.path());
    assert_eq!(name1, name2, "same path should produce same hash");
    assert_eq!(name1.len(), 16, "hash should be 16 hex chars");
    assert!(
        name1.chars().all(|c| c.is_ascii_hexdigit()),
        "hash should be hex: {name1}"
    );
}

// -----------------------------------------------------------------------
// AO-21: reordered directories map to the same undo subdirs
// -----------------------------------------------------------------------
#[test]
fn ao_21_reordered_dirs_stable_mapping() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let hash_a = undo_subdir_name(dir_a.path());
    let hash_b = undo_subdir_name(dir_b.path());
    assert_ne!(hash_a, hash_b, "different dirs should have different hashes");

    // Session 1: dirs in order [a, b]
    {
        let (event_sender, _rx) = mpsc::unbounded_channel();
        let args = CliArgs {
            working_dirs: vec![dir_a.path().to_path_buf(), dir_b.path().to_path_buf()],
            undo_dir: Some(undo.path().to_path_buf()),
            ..make_args(dir_a.path(), undo.path())
        };
        let orch = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });
        let payload = SessionStartPayload {
            working_directories: vec![
                WorkingDirectoryConfig { path: dir_a.path().display().to_string(), label: None },
                WorkingDirectoryConfig { path: dir_b.path().display().to_string(), label: None },
            ],
            network_policy: "disabled".to_string(),
            vm_mode: "ephemeral".to_string(),
            protocol_version: None,
        };
        let _ = orch.session_start(payload);

        // Write via MCP to create undo steps
        orch.write_file(WriteFileArgs {
            path: "marker.txt".to_string(),
            content: "from_a".to_string(),
        }).unwrap();
        let _ = orch.session_stop();
    }

    // Verify undo data landed in the hash-named directory
    assert!(undo.path().join(&hash_a).join("steps").exists());

    // Session 2: dirs in reversed order [b, a] — undo data should still match
    {
        let (event_sender, _rx) = mpsc::unbounded_channel();
        let args = CliArgs {
            working_dirs: vec![dir_b.path().to_path_buf(), dir_a.path().to_path_buf()],
            undo_dir: Some(undo.path().to_path_buf()),
            ..make_args(dir_b.path(), undo.path())
        };
        let orch = Orchestrator::new(args, event_sender, CommandClassifierConfig::default(), FileWatcherConfig { enabled: false, ..FileWatcherConfig::default() });
        let payload = SessionStartPayload {
            working_directories: vec![
                WorkingDirectoryConfig { path: dir_b.path().display().to_string(), label: None },
                WorkingDirectoryConfig { path: dir_a.path().display().to_string(), label: None },
            ],
            network_policy: "disabled".to_string(),
            vm_mode: "ephemeral".to_string(),
            protocol_version: None,
        };
        let result = orch.session_start(payload);
        assert!(result.is_ok(), "session with reordered dirs should succeed");

        // dir_a's undo history should be found (barrier placed).
        // dir_a is at index 1 in this session (reversed order).
        let history = orch.undo_history(UndoHistoryPayload { directory: Some("1".to_string()) }).unwrap();
        let steps = history["steps"].as_array().unwrap();
        assert!(!steps.is_empty(), "should find previous undo steps for dir_a");
    }
}
