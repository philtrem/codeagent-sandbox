//! Integration tests for undo history correctness.
//!
//! These tests verify the full flow from file operations through the
//! UndoInterceptor to on-disk manifests, validating that:
//! - Write operations produce steps with correct file counts
//! - Commands are stored in manifests
//! - Step IDs don't collide across session restarts
//! - Multi-session history is preserved
//! - Rollback works across sessions
//!
//! These are orchestrator-level tests that exercise the same code paths
//! as the production system without requiring QEMU.

use std::fs;
use std::path::Path;

use tempfile::TempDir;
use tokio::sync::mpsc;

use codeagent_sandbox::cli::CliArgs;
use codeagent_sandbox::command_classifier::CommandClassifierConfig;
use codeagent_sandbox::config::FileWatcherConfig;
use codeagent_sandbox::orchestrator::{undo_subdir_name, Orchestrator};
use codeagent_mcp::McpHandler;
use codeagent_mcp::protocol::{
    EditFileArgs, ReadFileArgs, UndoArgs, WriteFileArgs,
};
use codeagent_stdio::protocol::{
    SessionStartPayload, UndoHistoryPayload, WorkingDirectoryConfig,
};
use codeagent_stdio::{Event, RequestHandler};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_args(working_dir: &Path, undo_dir: &Path) -> CliArgs {
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

fn create_orchestrator(
    working: &Path,
    undo: &Path,
) -> (Orchestrator, mpsc::UnboundedReceiver<Event>) {
    let (event_sender, event_receiver) = mpsc::unbounded_channel();
    let args = make_args(working, undo);
    let orchestrator = Orchestrator::new(
        args,
        event_sender,
        CommandClassifierConfig::default(),
        FileWatcherConfig {
            enabled: false,
            ..FileWatcherConfig::default()
        },
    );
    (orchestrator, event_receiver)
}

/// Read step manifests directly from disk (mirrors what the Tauri frontend does).
/// Returns a list of (step_id, command, file_count) tuples.
fn read_steps_from_disk(undo_dir: &Path) -> Vec<(u64, Option<String>, usize)> {
    let mut results = Vec::new();

    if !undo_dir.exists() || !undo_dir.is_dir() {
        return results;
    }

    for entry in fs::read_dir(undo_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let steps_dir = path.join("steps");
        if !steps_dir.is_dir() {
            continue;
        }

        for step_entry in fs::read_dir(&steps_dir).unwrap() {
            let step_entry = step_entry.unwrap();
            let step_path = step_entry.path();

            if !step_path.is_dir() {
                continue;
            }

            let manifest_path = step_path.join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }

            let json = fs::read_to_string(&manifest_path).unwrap();
            let manifest: serde_json::Value = serde_json::from_str(&json).unwrap();

            let step_id = manifest["step_id"].as_u64().unwrap_or(0);
            let command = manifest["command"].as_str().map(|s| s.to_string());
            let file_count = manifest["entries"]
                .as_object()
                .map(|o| o.len())
                .unwrap_or(0);

            results.push((step_id, command, file_count));
        }
    }

    results.sort_by_key(|(id, _, _)| *id);
    results
}

// -----------------------------------------------------------------------
// UH-01: write_file creates a step with correct file count on disk
// -----------------------------------------------------------------------
#[test]
fn uh_01_write_file_creates_step_with_file_count() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Write a file via MCP (simulates what a command would do)
    orchestrator
        .write_file(WriteFileArgs {
            path: "test.txt".to_string(),
            content: "hello world".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1, "should have exactly 1 step on disk");
    let (_step_id, _command, file_count) = &steps[0];
    assert!(
        *file_count > 0,
        "step should have at least 1 file entry, got {file_count}"
    );
}

// -----------------------------------------------------------------------
// UH-02: edit_file creates a step with the edited file tracked
// -----------------------------------------------------------------------
#[test]
fn uh_02_edit_file_creates_step_with_entries() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    fs::write(working.path().join("data.txt"), "original content").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    orchestrator
        .edit_file(EditFileArgs {
            path: "data.txt".to_string(),
            old_string: "original".to_string(),
            new_string: "modified".to_string(),
            replace_all: false,
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1, "should have exactly 1 step on disk");
    let (_step_id, _command, file_count) = &steps[0];
    assert_eq!(
        *file_count, 1,
        "edit step should track the edited file"
    );
}

// -----------------------------------------------------------------------
// UH-03: MCP write_file stores command in manifest
// -----------------------------------------------------------------------
#[test]
fn uh_03_write_file_stores_command_in_manifest() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    orchestrator
        .write_file(WriteFileArgs {
            path: "file.txt".to_string(),
            content: "data".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1);
    let (_step_id, command, _file_count) = &steps[0];
    assert!(
        command.is_some(),
        "write_file step should have a command stored in manifest"
    );
    let cmd = command.as_ref().unwrap();
    assert!(
        cmd.contains("write_file"),
        "command should indicate write_file, got: {cmd}"
    );
}

// -----------------------------------------------------------------------
// UH-04: MCP edit_file stores command in manifest
// -----------------------------------------------------------------------
#[test]
fn uh_04_edit_file_stores_command_in_manifest() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    fs::write(working.path().join("src.txt"), "before").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    orchestrator
        .edit_file(EditFileArgs {
            path: "src.txt".to_string(),
            old_string: "before".to_string(),
            new_string: "after".to_string(),
            replace_all: false,
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1);
    let (_step_id, command, _file_count) = &steps[0];
    assert!(
        command.is_some(),
        "edit_file step should have a command stored in manifest"
    );
    let cmd = command.as_ref().unwrap();
    assert!(
        cmd.contains("edit_file"),
        "command should indicate edit_file, got: {cmd}"
    );
}

// -----------------------------------------------------------------------
// UH-05: Step IDs don't collide across session restarts
// -----------------------------------------------------------------------
#[test]
fn uh_05_step_ids_unique_across_sessions() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create two steps
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        orchestrator
            .write_file(WriteFileArgs {
                path: "a.txt".to_string(),
                content: "a".to_string(),
            })
            .unwrap();

        orchestrator
            .write_file(WriteFileArgs {
                path: "b.txt".to_string(),
                content: "b".to_string(),
            })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    let steps_after_session_1 = read_steps_from_disk(undo.path());
    assert_eq!(steps_after_session_1.len(), 2);
    let session_1_ids: Vec<u64> = steps_after_session_1.iter().map(|(id, _, _)| *id).collect();

    // Session 2: create one more step
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        orchestrator
            .write_file(WriteFileArgs {
                path: "c.txt".to_string(),
                content: "c".to_string(),
            })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    let steps_after_session_2 = read_steps_from_disk(undo.path());
    assert_eq!(
        steps_after_session_2.len(),
        3,
        "should have 3 total steps from both sessions"
    );

    // All step IDs should be unique
    let all_ids: Vec<u64> = steps_after_session_2.iter().map(|(id, _, _)| *id).collect();
    let unique_ids: std::collections::HashSet<u64> = all_ids.iter().cloned().collect();
    assert_eq!(
        unique_ids.len(),
        all_ids.len(),
        "step IDs should be unique: {all_ids:?}"
    );

    // Session 2's step ID should not collide with session 1's IDs
    let session_2_id = all_ids
        .iter()
        .find(|id| !session_1_ids.contains(id))
        .expect("should find a new step ID from session 2");
    assert!(
        !session_1_ids.contains(session_2_id),
        "session 2 step ID {session_2_id} should not collide with session 1 IDs {session_1_ids:?}"
    );
}

// -----------------------------------------------------------------------
// UH-06: Multi-session rollback works
// -----------------------------------------------------------------------
#[test]
fn uh_06_multi_session_rollback_with_force() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create a file
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        orchestrator
            .write_file(WriteFileArgs {
                path: "session1.txt".to_string(),
                content: "session 1 data".to_string(),
            })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    assert!(working.path().join("session1.txt").exists());

    // Session 2: force rollback should remove the file
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        // Non-forced rollback should fail (barrier blocks it)
        let result = orchestrator.undo(UndoArgs {
            count: 1,
            force: false,
        });
        assert!(result.is_err(), "non-forced rollback should be blocked by session barrier");

        // Forced rollback should succeed
        let result = orchestrator.undo(UndoArgs {
            count: 1,
            force: true,
        });
        assert!(result.is_ok(), "forced rollback should cross barrier");

        let _ = orchestrator.session_stop();
    }

    assert!(
        !working.path().join("session1.txt").exists(),
        "file should be removed after forced rollback"
    );
}

// -----------------------------------------------------------------------
// UH-07: Multiple writes in one session produce separate steps on disk
// -----------------------------------------------------------------------
#[test]
fn uh_07_multiple_writes_produce_separate_steps() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    orchestrator
        .write_file(WriteFileArgs {
            path: "first.txt".to_string(),
            content: "1".to_string(),
        })
        .unwrap();

    orchestrator
        .write_file(WriteFileArgs {
            path: "second.txt".to_string(),
            content: "2".to_string(),
        })
        .unwrap();

    orchestrator
        .write_file(WriteFileArgs {
            path: "third.txt".to_string(),
            content: "3".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(
        steps.len(),
        3,
        "each write_file should produce a separate step"
    );

    // Each step should have at least 1 file entry
    for (step_id, _cmd, file_count) in &steps {
        assert!(
            *file_count > 0,
            "step {step_id} should have file entries, got {file_count}"
        );
    }
}

// -----------------------------------------------------------------------
// UH-08: Rollback of multiple steps restores files
// -----------------------------------------------------------------------
#[test]
fn uh_08_rollback_multiple_steps_restores_files() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Create three files in three separate steps
    orchestrator
        .write_file(WriteFileArgs {
            path: "a.txt".to_string(),
            content: "a".to_string(),
        })
        .unwrap();
    orchestrator
        .write_file(WriteFileArgs {
            path: "b.txt".to_string(),
            content: "b".to_string(),
        })
        .unwrap();
    orchestrator
        .write_file(WriteFileArgs {
            path: "c.txt".to_string(),
            content: "c".to_string(),
        })
        .unwrap();

    assert!(working.path().join("a.txt").exists());
    assert!(working.path().join("b.txt").exists());
    assert!(working.path().join("c.txt").exists());

    // Roll back 2 steps (c.txt and b.txt)
    orchestrator
        .undo(UndoArgs {
            count: 2,
            force: false,
        })
        .unwrap();

    assert!(
        working.path().join("a.txt").exists(),
        "a.txt should still exist after rolling back 2 steps"
    );
    assert!(
        !working.path().join("b.txt").exists(),
        "b.txt should be removed after rolling back 2 steps"
    );
    assert!(
        !working.path().join("c.txt").exists(),
        "c.txt should be removed after rolling back 2 steps"
    );

    // Only 1 step should remain
    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1, "1 step should remain after rolling back 2");
}

// -----------------------------------------------------------------------
// UH-09: Steps from previous session are visible in undo_history
// -----------------------------------------------------------------------
#[test]
fn uh_09_previous_session_steps_visible_in_history() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create two steps
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        orchestrator
            .write_file(WriteFileArgs {
                path: "s1_file1.txt".to_string(),
                content: "session 1".to_string(),
            })
            .unwrap();

        orchestrator
            .write_file(WriteFileArgs {
                path: "s1_file2.txt".to_string(),
                content: "session 1 part 2".to_string(),
            })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    // Session 2: verify previous steps are visible
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        let history = orchestrator
            .undo_history(UndoHistoryPayload { directory: None })
            .unwrap();
        let steps = history["steps"].as_array().unwrap();
        assert_eq!(
            steps.len(),
            2,
            "session 2 should see 2 steps from session 1"
        );

        let _ = orchestrator.session_stop();
    }

    // On-disk state should also show 2 steps
    let disk_steps = read_steps_from_disk(undo.path());
    assert_eq!(disk_steps.len(), 2, "disk should have 2 steps");
}

// -----------------------------------------------------------------------
// UH-10: Overwriting an existing file creates step with preimage
// -----------------------------------------------------------------------
#[test]
fn uh_10_overwrite_file_captures_preimage_and_restores() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    // Create a file before session start
    fs::write(working.path().join("config.json"), r#"{"key": "original"}"#).unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Overwrite via MCP
    orchestrator
        .write_file(WriteFileArgs {
            path: "config.json".to_string(),
            content: r#"{"key": "modified"}"#.to_string(),
        })
        .unwrap();

    // Verify the file was modified
    let content = fs::read_to_string(working.path().join("config.json")).unwrap();
    assert_eq!(content, r#"{"key": "modified"}"#);

    // Step should exist with the file tracked
    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1);
    assert!(steps[0].2 > 0, "step should track the overwritten file");

    // Rollback should restore original content
    orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();

    let restored = fs::read_to_string(working.path().join("config.json")).unwrap();
    assert_eq!(
        restored,
        r#"{"key": "original"}"#,
        "file should be restored to original content after rollback"
    );
}

// -----------------------------------------------------------------------
// UH-11: Delete + write creates steps visible on disk
// -----------------------------------------------------------------------
#[test]
fn uh_11_write_then_delete_then_rollback() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Step 1: Create a file
    orchestrator
        .write_file(WriteFileArgs {
            path: "temp.txt".to_string(),
            content: "temporary data".to_string(),
        })
        .unwrap();
    assert!(working.path().join("temp.txt").exists());

    // Step 2: Overwrite the same file with different content
    orchestrator
        .write_file(WriteFileArgs {
            path: "temp.txt".to_string(),
            content: "updated data".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 2, "should have 2 steps on disk");

    // Rollback step 2: should restore to "temporary data"
    orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();
    assert_eq!(
        fs::read_to_string(working.path().join("temp.txt")).unwrap(),
        "temporary data"
    );

    // Rollback step 1: should remove the file entirely
    orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();
    assert!(
        !working.path().join("temp.txt").exists(),
        "file should be removed after rolling back its creation step"
    );
}

// -----------------------------------------------------------------------
// UH-12: Session boundary barrier has correct reason
// -----------------------------------------------------------------------
#[test]
fn uh_12_session_boundary_barrier_has_session_start_reason() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create a step
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        orchestrator
            .write_file(WriteFileArgs {
                path: "marker.txt".to_string(),
                content: "session 1".to_string(),
            })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    // Session 2: check the barrier on disk
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        let _ = orchestrator.session_stop();
    }

    // Read barriers from per-step files. The barrier is placed after the last
    // completed step (step 1), so it lives in steps/1/barriers.json.
    let subdir_name = undo_subdir_name(working.path());
    let steps_dir = undo.path().join(&subdir_name).join("steps");
    let step_dirs: Vec<_> = fs::read_dir(&steps_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    assert!(!step_dirs.is_empty(), "should have at least one step dir");

    let mut found_session_start = false;
    for step_entry in &step_dirs {
        let barrier_path = step_entry.path().join("barriers.json");
        if barrier_path.exists() {
            let json = fs::read_to_string(&barrier_path).unwrap();
            let barriers: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
            if barriers.iter().any(|b| b["reason"].as_str() == Some("session_start")) {
                found_session_start = true;
                break;
            }
        }
    }
    assert!(
        found_session_start,
        "at least one step should have a barrier with reason 'session_start'"
    );
}

// -----------------------------------------------------------------------
// UH-13: Write to nested path creates parent dirs and tracks them
// -----------------------------------------------------------------------
#[test]
fn uh_13_nested_write_tracks_parent_directories() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Write to a deeply nested path
    orchestrator
        .write_file(WriteFileArgs {
            path: "src/components/Button.tsx".to_string(),
            content: "export const Button = () => <button />;".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1);
    let file_count = steps[0].2;
    // Should track the file AND the created parent directories
    assert!(
        file_count >= 1,
        "should track at least the created file, got {file_count}"
    );

    // Rollback should remove the file and parent directories
    orchestrator
        .undo(UndoArgs {
            count: 1,
            force: false,
        })
        .unwrap();

    assert!(
        !working.path().join("src").exists(),
        "created parent directories should be removed after rollback"
    );
}

// -----------------------------------------------------------------------
// UH-14: Three-session scenario with cumulative history
// -----------------------------------------------------------------------
#[test]
fn uh_14_three_session_cumulative_history() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs {
                path: "s1.txt".to_string(),
                content: "session 1".to_string(),
            })
            .unwrap();
        let _ = orchestrator.session_stop();
    }

    // Session 2
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs {
                path: "s2.txt".to_string(),
                content: "session 2".to_string(),
            })
            .unwrap();
        let _ = orchestrator.session_stop();
    }

    // Session 3
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs {
                path: "s3.txt".to_string(),
                content: "session 3".to_string(),
            })
            .unwrap();
        let _ = orchestrator.session_stop();
    }

    // All 3 steps should be visible on disk
    let steps = read_steps_from_disk(undo.path());
    assert_eq!(
        steps.len(),
        3,
        "should have 3 steps across 3 sessions"
    );

    // All step IDs should be unique
    let ids: Vec<u64> = steps.iter().map(|(id, _, _)| *id).collect();
    let unique: std::collections::HashSet<u64> = ids.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        3,
        "all 3 step IDs should be unique: {ids:?}"
    );

    // All steps should have file entries
    for (id, _, count) in &steps {
        assert!(
            *count > 0,
            "step {id} should have file entries, got {count}"
        );
    }

    // All files should exist
    assert!(working.path().join("s1.txt").exists());
    assert!(working.path().join("s2.txt").exists());
    assert!(working.path().join("s3.txt").exists());
}

// -----------------------------------------------------------------------
// UH-15: MCP read_file does NOT produce an undo step
// -----------------------------------------------------------------------
#[test]
fn uh_15_read_file_does_not_produce_step() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    // Create a file to read
    fs::write(working.path().join("readme.txt"), "hello world").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Read the file via MCP
    let result = orchestrator.read_file(ReadFileArgs {
        path: "readme.txt".to_string(),
    });
    assert!(result.is_ok(), "read_file should succeed");

    // No steps should exist on disk — read operations are not tracked
    let steps = read_steps_from_disk(undo.path());
    assert!(
        steps.is_empty(),
        "read_file should not produce any undo steps, got {} steps",
        steps.len()
    );
}

// -----------------------------------------------------------------------
// UH-16: Write then read — only the write produces a step
// -----------------------------------------------------------------------
#[test]
fn uh_16_write_then_read_only_write_produces_step() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Write a file
    orchestrator
        .write_file(WriteFileArgs {
            path: "data.txt".to_string(),
            content: "data".to_string(),
        })
        .unwrap();

    // Read the file back
    let _ = orchestrator.read_file(ReadFileArgs {
        path: "data.txt".to_string(),
    });

    // Only the write should produce a step
    let steps = read_steps_from_disk(undo.path());
    assert_eq!(
        steps.len(),
        1,
        "only the write_file should produce a step, not the read"
    );
    assert!(
        steps[0].2 > 0,
        "the write step should have file entries"
    );
}

// -----------------------------------------------------------------------
// UH-17: step IDs remain unique after rollback + new writes
// -----------------------------------------------------------------------
#[test]
fn uh_17_step_ids_unique_after_rollback() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Create 3 steps
    for i in 0..3 {
        orchestrator
            .write_file(WriteFileArgs {
                path: format!("file{i}.txt"),
                content: format!("content{i}"),
            })
            .unwrap();
    }

    let steps_before = read_steps_from_disk(undo.path());
    assert_eq!(steps_before.len(), 3);

    // Rollback 2 steps
    orchestrator
        .undo(UndoArgs {
            count: 2,
            force: false,
        })
        .unwrap();

    let steps_after_rollback = read_steps_from_disk(undo.path());
    assert_eq!(steps_after_rollback.len(), 1);

    // Create 2 more steps
    for i in 3..5 {
        orchestrator
            .write_file(WriteFileArgs {
                path: format!("file{i}.txt"),
                content: format!("content{i}"),
            })
            .unwrap();
    }

    let final_steps = read_steps_from_disk(undo.path());
    assert_eq!(final_steps.len(), 3, "should have 1 original + 2 new steps");

    // All step IDs must be unique
    let mut ids: Vec<u64> = final_steps.iter().map(|(id, _, _)| *id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        3,
        "all step IDs must be unique after rollback + new writes"
    );

    // New files should exist on disk
    assert!(working.path().join("file3.txt").exists());
    assert!(working.path().join("file4.txt").exists());
}

// -----------------------------------------------------------------------
// UH-18: read_file between writes does not create a step or consume an ID
// -----------------------------------------------------------------------
#[test]
fn uh_18_read_between_writes_no_id_gap() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    fs::write(working.path().join("existing.txt"), "data").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Write A → step 1
    orchestrator
        .write_file(WriteFileArgs {
            path: "a.txt".to_string(),
            content: "a".to_string(),
        })
        .unwrap();

    // Read (should not create a step)
    let _ = orchestrator.read_file(ReadFileArgs {
        path: "existing.txt".to_string(),
    });

    // Write B → step 2
    orchestrator
        .write_file(WriteFileArgs {
            path: "b.txt".to_string(),
            content: "b".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 2, "should have exactly 2 steps (reads produce none)");

    // Step IDs should be consecutive with no gap
    let ids: Vec<u64> = steps.iter().map(|(id, _, _)| *id).collect();
    assert_eq!(
        ids[1] - ids[0],
        1,
        "step IDs should be consecutive (no gap from read): {ids:?}"
    );
}

// -----------------------------------------------------------------------
// UH-19: multiple consecutive reads don't consume step IDs
// -----------------------------------------------------------------------
#[test]
fn uh_19_multiple_reads_no_id_consumption() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    fs::write(working.path().join("r1.txt"), "1").unwrap();
    fs::write(working.path().join("r2.txt"), "2").unwrap();
    fs::write(working.path().join("r3.txt"), "3").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Read 3 files
    let _ = orchestrator.read_file(ReadFileArgs { path: "r1.txt".to_string() });
    let _ = orchestrator.read_file(ReadFileArgs { path: "r2.txt".to_string() });
    let _ = orchestrator.read_file(ReadFileArgs { path: "r3.txt".to_string() });

    // Write one file — should get the first step ID, not the 4th
    orchestrator
        .write_file(WriteFileArgs {
            path: "w.txt".to_string(),
            content: "written".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1, "only the write should produce a step");
    // The first step ID should be 1 (or at least not 4+)
    let id = steps[0].0;
    assert!(id < 10, "step ID should be low (not inflated by reads): got {id}");
}

// -----------------------------------------------------------------------
// UH-20: interleaved read-write-read-write pattern has no ID gaps
// -----------------------------------------------------------------------
#[test]
fn uh_20_interleaved_read_write_no_gaps() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    fs::write(working.path().join("src.txt"), "source").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // read → write → read → write → read → write
    let _ = orchestrator.read_file(ReadFileArgs { path: "src.txt".to_string() });
    orchestrator
        .write_file(WriteFileArgs { path: "w1.txt".to_string(), content: "1".to_string() })
        .unwrap();
    let _ = orchestrator.read_file(ReadFileArgs { path: "src.txt".to_string() });
    orchestrator
        .write_file(WriteFileArgs { path: "w2.txt".to_string(), content: "2".to_string() })
        .unwrap();
    let _ = orchestrator.read_file(ReadFileArgs { path: "src.txt".to_string() });
    orchestrator
        .write_file(WriteFileArgs { path: "w3.txt".to_string(), content: "3".to_string() })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 3, "exactly 3 write steps, reads produce none");

    // IDs should be consecutive
    let ids: Vec<u64> = steps.iter().map(|(id, _, _)| *id).collect();
    assert_eq!(ids[1] - ids[0], 1, "IDs should be consecutive: {ids:?}");
    assert_eq!(ids[2] - ids[1], 1, "IDs should be consecutive: {ids:?}");
}

// -----------------------------------------------------------------------
// UH-21: undo_history reports same steps as on-disk manifests
// -----------------------------------------------------------------------
#[test]
fn uh_21_undo_history_matches_disk() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    for i in 0..4 {
        orchestrator
            .write_file(WriteFileArgs {
                path: format!("f{i}.txt"),
                content: format!("c{i}"),
            })
            .unwrap();
    }

    let disk_steps = read_steps_from_disk(undo.path());
    let history = orchestrator
        .undo_history(UndoHistoryPayload { directory: None })
        .unwrap();
    let api_steps = history["steps"].as_array().unwrap();

    assert_eq!(
        disk_steps.len(),
        api_steps.len(),
        "undo_history count should match on-disk step count"
    );

    // Step IDs should match. The API returns a plain array of step ID integers.
    let disk_ids: Vec<u64> = disk_steps.iter().map(|(id, _, _)| *id).collect();
    let mut api_ids: Vec<u64> = api_steps
        .iter()
        .map(|s| s.as_i64().unwrap() as u64)
        .collect();
    api_ids.sort();
    assert_eq!(disk_ids, api_ids, "step IDs from API and disk should match");
}

// -----------------------------------------------------------------------
// UH-22: next step ID continues from max on-disk ID after restart
// -----------------------------------------------------------------------
#[test]
fn uh_22_step_id_continues_after_restart() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create 3 steps
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        for i in 0..3 {
            orchestrator
                .write_file(WriteFileArgs {
                    path: format!("s1_{i}.txt"),
                    content: format!("data{i}"),
                })
                .unwrap();
        }

        let _ = orchestrator.session_stop();
    }

    let session_1_steps = read_steps_from_disk(undo.path());
    let max_session_1_id = session_1_steps.iter().map(|(id, _, _)| *id).max().unwrap();

    // Session 2: create 1 step — ID must be > max_session_1_id
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        orchestrator
            .write_file(WriteFileArgs {
                path: "s2.txt".to_string(),
                content: "session 2".to_string(),
            })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    let all_steps = read_steps_from_disk(undo.path());
    let session_2_id = all_steps
        .iter()
        .map(|(id, _, _)| *id)
        .max()
        .unwrap();
    assert!(
        session_2_id > max_session_1_id,
        "session 2 step ID ({session_2_id}) must be > session 1 max ({max_session_1_id})"
    );
}

// -----------------------------------------------------------------------
// UH-23: step ID counter not decremented by rollback
// -----------------------------------------------------------------------
#[test]
fn uh_23_rollback_does_not_decrement_counter() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Create 3 steps
    for i in 0..3 {
        orchestrator
            .write_file(WriteFileArgs {
                path: format!("f{i}.txt"),
                content: format!("c{i}"),
            })
            .unwrap();
    }

    let before_rollback = read_steps_from_disk(undo.path());
    let max_before = before_rollback.iter().map(|(id, _, _)| *id).max().unwrap();

    // Rollback all 3 steps
    orchestrator
        .undo(UndoArgs { count: 3, force: false })
        .unwrap();

    assert!(
        read_steps_from_disk(undo.path()).is_empty(),
        "all steps should be removed"
    );

    // Create 1 new step — its ID must be > max_before
    orchestrator
        .write_file(WriteFileArgs {
            path: "after_rollback.txt".to_string(),
            content: "new".to_string(),
        })
        .unwrap();

    let new_steps = read_steps_from_disk(undo.path());
    assert_eq!(new_steps.len(), 1);
    let new_id = new_steps[0].0;
    assert!(
        new_id > max_before,
        "new step ID ({new_id}) must be > pre-rollback max ({max_before})"
    );
}

// -----------------------------------------------------------------------
// UH-24: step ID correct after restart with rollback in previous session
// -----------------------------------------------------------------------
#[test]
fn uh_24_step_id_after_restart_with_prior_rollback() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create 5 steps, rollback 3
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        for i in 0..5 {
            orchestrator
                .write_file(WriteFileArgs {
                    path: format!("f{i}.txt"),
                    content: format!("c{i}"),
                })
                .unwrap();
        }
        orchestrator
            .undo(UndoArgs { count: 3, force: false })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    // 2 steps remain on disk after rollback
    let remaining = read_steps_from_disk(undo.path());
    assert_eq!(remaining.len(), 2);
    let max_remaining = remaining.iter().map(|(id, _, _)| *id).max().unwrap();

    // Session 2: new step must have ID > max_remaining
    // (The counter should reconstruct from on-disk max, which is step 2,
    // not step 5 which was rolled back and no longer exists)
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        orchestrator
            .write_file(WriteFileArgs {
                path: "s2.txt".to_string(),
                content: "session 2".to_string(),
            })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    let all_steps = read_steps_from_disk(undo.path());
    assert_eq!(all_steps.len(), 3);
    let new_id = all_steps.iter().map(|(id, _, _)| *id).max().unwrap();
    assert!(
        new_id > max_remaining,
        "new step ID ({new_id}) must be > remaining max ({max_remaining})"
    );

    // All IDs must be unique
    let ids: Vec<u64> = all_steps.iter().map(|(id, _, _)| *id).collect();
    let unique: std::collections::HashSet<u64> = ids.iter().cloned().collect();
    assert_eq!(unique.len(), ids.len(), "all IDs must be unique: {ids:?}");
}

// -----------------------------------------------------------------------
// UH-25: edit_file validation error (old_string not found) leaves no step
// -----------------------------------------------------------------------
#[test]
fn uh_25_edit_validation_error_no_step() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    fs::write(working.path().join("file.txt"), "hello world").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // old_string doesn't exist in the file — should fail before writing
    let result = orchestrator.edit_file(EditFileArgs {
        path: "file.txt".to_string(),
        old_string: "nonexistent substring".to_string(),
        new_string: "replacement".to_string(),
        replace_all: false,
    });
    assert!(result.is_err(), "edit with missing old_string should fail");

    // No step should be on disk
    let steps = read_steps_from_disk(undo.path());
    assert!(steps.is_empty(), "failed edit should produce no step on disk");

    // Subsequent write should succeed
    orchestrator
        .write_file(WriteFileArgs {
            path: "ok.txt".to_string(),
            content: "works".to_string(),
        })
        .unwrap();
    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1, "subsequent write should work after validation error");
}

// -----------------------------------------------------------------------
// UH-26: write_file error doesn't leave step on disk
// -----------------------------------------------------------------------
#[test]
fn uh_26_write_error_no_step_on_disk() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Create a directory to block write_file at that path
    fs::create_dir_all(working.path().join("blocker")).unwrap();

    let result = orchestrator.write_file(WriteFileArgs {
        path: "blocker".to_string(),
        content: "fail".to_string(),
    });
    assert!(result.is_err());

    // No step on disk from the failed write
    let steps = read_steps_from_disk(undo.path());
    assert!(steps.is_empty(), "failed write should produce no step on disk");
}

// -----------------------------------------------------------------------
// UH-27: interleaved errors and successes produce correct step count
// -----------------------------------------------------------------------
#[test]
fn uh_27_interleaved_error_success_correct_count() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    fs::create_dir_all(working.path().join("dir")).unwrap();
    fs::write(working.path().join("editable.txt"), "original content").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Success: write a.txt
    orchestrator
        .write_file(WriteFileArgs { path: "a.txt".to_string(), content: "a".to_string() })
        .unwrap();

    // Error: write to a directory path
    let _ = orchestrator.write_file(WriteFileArgs {
        path: "dir".to_string(),
        content: "fail".to_string(),
    });

    // Success: write b.txt
    orchestrator
        .write_file(WriteFileArgs { path: "b.txt".to_string(), content: "b".to_string() })
        .unwrap();

    // Error: edit with missing old_string
    let _ = orchestrator.edit_file(EditFileArgs {
        path: "editable.txt".to_string(),
        old_string: "missing".to_string(),
        new_string: "new".to_string(),
        replace_all: false,
    });

    // Success: write c.txt
    orchestrator
        .write_file(WriteFileArgs { path: "c.txt".to_string(), content: "c".to_string() })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(
        steps.len(),
        3,
        "exactly 3 successful writes, errors should leave no steps"
    );

    // All IDs unique and monotonically increasing
    let ids: Vec<u64> = steps.iter().map(|(id, _, _)| *id).collect();
    for i in 1..ids.len() {
        assert!(ids[i] > ids[i - 1], "IDs must be strictly increasing: {ids:?}");
    }
}

// -----------------------------------------------------------------------
// UH-28: error after rollback still allows new step creation
// -----------------------------------------------------------------------
#[test]
fn uh_28_error_after_rollback_allows_new_steps() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Create 2 steps, rollback 1
    orchestrator
        .write_file(WriteFileArgs { path: "a.txt".to_string(), content: "a".to_string() })
        .unwrap();
    orchestrator
        .write_file(WriteFileArgs { path: "b.txt".to_string(), content: "b".to_string() })
        .unwrap();
    orchestrator
        .undo(UndoArgs { count: 1, force: false })
        .unwrap();

    // Trigger an error
    fs::create_dir_all(working.path().join("blocker")).unwrap();
    let _ = orchestrator.write_file(WriteFileArgs {
        path: "blocker".to_string(),
        content: "fail".to_string(),
    });

    // New step should still work
    orchestrator
        .write_file(WriteFileArgs { path: "c.txt".to_string(), content: "c".to_string() })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 2, "should have step a + step c");

    let ids: Vec<u64> = steps.iter().map(|(id, _, _)| *id).collect();
    let unique: std::collections::HashSet<u64> = ids.iter().cloned().collect();
    assert_eq!(unique.len(), ids.len(), "all IDs must be unique: {ids:?}");
}

// -----------------------------------------------------------------------
// UH-29: step IDs strictly increasing after partial rollback
// -----------------------------------------------------------------------
#[test]
fn uh_29_step_ids_strictly_increasing_after_partial_rollback() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Create 4 steps (IDs 1..4)
    for i in 0..4 {
        orchestrator
            .write_file(WriteFileArgs {
                path: format!("f{i}.txt"),
                content: format!("c{i}"),
            })
            .unwrap();
    }

    let before = read_steps_from_disk(undo.path());
    let max_before = before.iter().map(|(id, _, _)| *id).max().unwrap();

    // Rollback 2 → steps 3,4 removed, steps 1,2 remain
    orchestrator
        .undo(UndoArgs { count: 2, force: false })
        .unwrap();

    // Create 1 new step — ID must be > max_before (the old step 4's ID)
    orchestrator
        .write_file(WriteFileArgs {
            path: "new.txt".to_string(),
            content: "new".to_string(),
        })
        .unwrap();

    let final_steps = read_steps_from_disk(undo.path());
    assert_eq!(final_steps.len(), 3);

    let ids: Vec<u64> = final_steps.iter().map(|(id, _, _)| *id).collect();
    let new_id = *ids.last().unwrap();
    assert!(
        new_id > max_before,
        "new step ID ({new_id}) must be > pre-rollback max ({max_before}), got IDs: {ids:?}"
    );

    // All IDs strictly increasing
    for i in 1..ids.len() {
        assert!(ids[i] > ids[i - 1], "IDs must be strictly increasing: {ids:?}");
    }
}

// -----------------------------------------------------------------------
// UH-30: step IDs increasing after complete rollback (counter survives)
// -----------------------------------------------------------------------
#[test]
fn uh_30_step_ids_survive_complete_rollback() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Create 3 steps
    for i in 0..3 {
        orchestrator
            .write_file(WriteFileArgs {
                path: format!("f{i}.txt"),
                content: format!("c{i}"),
            })
            .unwrap();
    }

    let before = read_steps_from_disk(undo.path());
    let max_before = before.iter().map(|(id, _, _)| *id).max().unwrap();

    // Rollback ALL steps
    orchestrator
        .undo(UndoArgs { count: 3, force: false })
        .unwrap();
    assert!(read_steps_from_disk(undo.path()).is_empty());

    // Create 2 new steps — both IDs must be > max_before
    for i in 0..2 {
        orchestrator
            .write_file(WriteFileArgs {
                path: format!("new{i}.txt"),
                content: format!("new{i}"),
            })
            .unwrap();
    }

    let final_steps = read_steps_from_disk(undo.path());
    assert_eq!(final_steps.len(), 2);

    for (id, _, _) in &final_steps {
        assert!(
            *id > max_before,
            "new step ID ({id}) must be > pre-rollback max ({max_before})"
        );
    }

    // IDs should still be strictly increasing
    let ids: Vec<u64> = final_steps.iter().map(|(id, _, _)| *id).collect();
    assert!(ids[1] > ids[0], "IDs must be strictly increasing: {ids:?}");
}

// -----------------------------------------------------------------------
// UH-31: rollback of latest step succeeds, rollback past barrier fails
// -----------------------------------------------------------------------
#[test]
fn uh_31_rollback_past_barrier_blocked() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create 2 steps
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs { path: "a.txt".to_string(), content: "a".to_string() })
            .unwrap();
        orchestrator
            .write_file(WriteFileArgs { path: "b.txt".to_string(), content: "b".to_string() })
            .unwrap();
        let _ = orchestrator.session_stop();
    }

    // Session 2: session start creates barrier after step 2
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        // Create a step in session 2
        orchestrator
            .write_file(WriteFileArgs { path: "c.txt".to_string(), content: "c".to_string() })
            .unwrap();

        // Rollback 1 (session 2's step) should succeed
        let result = orchestrator.undo(UndoArgs { count: 1, force: false });
        assert!(result.is_ok(), "rolling back current session's step should work");

        // Rollback 1 more (session 1's step 2) should fail — barrier
        let result = orchestrator.undo(UndoArgs { count: 1, force: false });
        assert!(result.is_err(), "rolling back past session barrier should fail");

        // Force rollback should succeed
        let result = orchestrator.undo(UndoArgs { count: 1, force: true });
        assert!(result.is_ok(), "force rollback should cross barrier");

        let _ = orchestrator.session_stop();
    }
}

// -----------------------------------------------------------------------
// UH-32: multiple barriers from multiple sessions
// -----------------------------------------------------------------------
#[test]
fn uh_32_multiple_session_barriers() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // 3 sessions, each creating 1 step → 2 barriers (after sessions 1 and 2)
    for i in 0..3 {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs {
                path: format!("s{i}.txt"),
                content: format!("session {i}"),
            })
            .unwrap();
        let _ = orchestrator.session_stop();
    }

    // Session 4: 3 steps on disk, 2 barriers
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        // Rolling back 1 step (session 3's) should be blocked by the barrier
        // placed at session 4 start
        let result = orchestrator.undo(UndoArgs { count: 1, force: false });
        assert!(result.is_err(), "rollback should be blocked by session 4 barrier");

        // Force rollback 1 → removes session 3 step + barrier
        let result = orchestrator.undo(UndoArgs { count: 1, force: true });
        assert!(result.is_ok());

        // Try another — blocked by session 3 barrier
        let result = orchestrator.undo(UndoArgs { count: 1, force: false });
        assert!(result.is_err(), "should be blocked by session 3 barrier");

        let _ = orchestrator.session_stop();
    }

    // Verify at least one per-step barriers file exists (remaining barriers
    // are stored inside their step directories, not at the global level)
    let subdir_name = undo_subdir_name(working.path());
    let steps_dir = undo.path().join(&subdir_name).join("steps");
    let has_barrier_file = fs::read_dir(&steps_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.path().join("barriers.json").exists());
    assert!(has_barrier_file, "at least one step should have a barriers.json");
}

// -----------------------------------------------------------------------
// UH-33: barrier persists across session restart
// -----------------------------------------------------------------------
#[test]
fn uh_33_barrier_persists_across_restart() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create step
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs { path: "a.txt".to_string(), content: "a".to_string() })
            .unwrap();
        let _ = orchestrator.session_stop();
    }

    let subdir_name = undo_subdir_name(working.path());
    let steps_dir = undo.path().join(&subdir_name).join("steps");

    // Session 2 start creates barrier — verify it in per-step file
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        let _ = orchestrator.session_stop();
    }
    let has_barrier_after_s2 = fs::read_dir(&steps_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.path().join("barriers.json").exists());
    assert!(has_barrier_after_s2, "per-step barriers.json should exist after session 2");

    // Session 3 — barrier from session 2 should still block rollback
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        let result = orchestrator.undo(UndoArgs { count: 1, force: false });
        assert!(
            result.is_err(),
            "barrier from session 2 should persist and block rollback in session 3"
        );

        let _ = orchestrator.session_stop();
    }

    // Per-step barriers should still exist after session 3
    let has_barrier_after_s3 = fs::read_dir(&steps_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            let p = e.path().join("barriers.json");
            p.exists() && fs::read_to_string(&p).map(|s| !s.is_empty()).unwrap_or(false)
        });
    assert!(
        has_barrier_after_s3,
        "per-step barriers.json should not be empty after session 3"
    );
}

// -----------------------------------------------------------------------
// UH-34: write to nested path tracks parent dirs in manifest
// -----------------------------------------------------------------------
#[test]
fn uh_34_nested_write_manifest_tracks_parents() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    // Write to a/b/c/file.txt where none of a/b/c exist
    orchestrator
        .write_file(WriteFileArgs {
            path: "a/b/c/file.txt".to_string(),
            content: "nested".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1);
    let file_count = steps[0].2;
    // Must track: a/, a/b/, a/b/c/ (3 dirs) + a/b/c/file.txt (1 file) = at least 4
    assert!(
        file_count >= 4,
        "manifest should track 3 parent dirs + 1 file = at least 4 entries, got {file_count}"
    );
}

// -----------------------------------------------------------------------
// UH-35: overwrite existing nested file only tracks the file, not parents
// -----------------------------------------------------------------------
#[test]
fn uh_35_overwrite_nested_file_tracks_only_file() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    // Pre-create the directory structure
    fs::create_dir_all(working.path().join("a/b/c")).unwrap();
    fs::write(working.path().join("a/b/c/file.txt"), "original").unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    orchestrator
        .write_file(WriteFileArgs {
            path: "a/b/c/file.txt".to_string(),
            content: "modified".to_string(),
        })
        .unwrap();

    let steps = read_steps_from_disk(undo.path());
    assert_eq!(steps.len(), 1);
    let file_count = steps[0].2;
    assert_eq!(
        file_count, 1,
        "overwrite of existing file should only track the file itself, not parent dirs"
    );
}

// -----------------------------------------------------------------------
// UH-36: manifest has valid JSON structure
// -----------------------------------------------------------------------
#[test]
fn uh_36_manifest_valid_json() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
    let _ = orchestrator.session_start(make_start_payload(
        &working.path().display().to_string(),
    ));

    orchestrator
        .write_file(WriteFileArgs {
            path: "test.txt".to_string(),
            content: "data".to_string(),
        })
        .unwrap();

    // Read manifest directly and validate structure
    let subdir_name = undo_subdir_name(working.path());
    let steps_dir = undo.path().join(&subdir_name).join("steps");
    let mut found_manifest = false;
    for entry in fs::read_dir(&steps_dir).unwrap() {
        let step_dir = entry.unwrap().path();
        let manifest_path = step_dir.join("manifest.json");
        if manifest_path.exists() {
            let json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();

            assert!(json["step_id"].is_u64(), "step_id should be a u64");
            assert!(json["entries"].is_object(), "entries should be an object");
            assert!(json["command"].is_string(), "command should be present");
            found_manifest = true;
        }
    }
    assert!(found_manifest, "should have found at least one manifest");
}

// -----------------------------------------------------------------------
// UH-37: session start recovers from incomplete WAL (crash simulation)
// -----------------------------------------------------------------------
#[test]
fn uh_37_crash_recovery_on_session_start() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Session 1: create a step normally
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs { path: "good.txt".to_string(), content: "ok".to_string() })
            .unwrap();
        let _ = orchestrator.session_stop();
    }

    let steps_before = read_steps_from_disk(undo.path());
    assert_eq!(steps_before.len(), 1);

    // Simulate a crash: create an incomplete WAL directory
    let subdir_name = undo_subdir_name(working.path());
    let wal_dir = undo.path().join(&subdir_name).join("wal").join("in_progress");
    fs::create_dir_all(wal_dir.join("preimages")).unwrap();

    // Write a file that the "crashed" step created — it should be cleaned up
    fs::write(working.path().join("crashed.txt"), "crashed data").unwrap();

    // Session 2: should recover cleanly
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));

        // New write should succeed without "step already open" errors
        orchestrator
            .write_file(WriteFileArgs { path: "s2.txt".to_string(), content: "session 2".to_string() })
            .unwrap();

        let _ = orchestrator.session_stop();
    }

    // Should have 2 steps: the original + the new one
    let steps_after = read_steps_from_disk(undo.path());
    assert_eq!(steps_after.len(), 2, "should have 1 pre-crash + 1 new step");

    // IDs should be unique
    let ids: Vec<u64> = steps_after.iter().map(|(id, _, _)| *id).collect();
    let unique: std::collections::HashSet<u64> = ids.iter().cloned().collect();
    assert_eq!(unique.len(), ids.len(), "IDs must be unique: {ids:?}");
}

// -----------------------------------------------------------------------
// UH-38: overwrite same file across sessions preserves correct preimage
// -----------------------------------------------------------------------
#[test]
fn uh_38_cross_session_overwrite_correct_preimage() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let path_str = working.path().display().to_string();

    // Create file with v0 content
    fs::write(working.path().join("data.txt"), "v0").unwrap();

    // Session 1: overwrite to v1
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs {
                path: "data.txt".to_string(),
                content: "v1".to_string(),
            })
            .unwrap();
        let _ = orchestrator.session_stop();
    }
    assert_eq!(fs::read_to_string(working.path().join("data.txt")).unwrap(), "v1");

    // Session 2: overwrite to v2
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .write_file(WriteFileArgs {
                path: "data.txt".to_string(),
                content: "v2".to_string(),
            })
            .unwrap();
        let _ = orchestrator.session_stop();
    }
    assert_eq!(fs::read_to_string(working.path().join("data.txt")).unwrap(), "v2");

    // Rollback session 2's step → should restore v1 (not v0)
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .undo(UndoArgs { count: 1, force: true })
            .unwrap();
        let _ = orchestrator.session_stop();
    }
    assert_eq!(
        fs::read_to_string(working.path().join("data.txt")).unwrap(),
        "v1",
        "rolling back session 2 should restore v1 (session 1's result)"
    );

    // Rollback session 1's step → should restore v0 (original)
    {
        let (orchestrator, _rx) = create_orchestrator(working.path(), undo.path());
        let _ = orchestrator.session_start(make_start_payload(&path_str));
        orchestrator
            .undo(UndoArgs { count: 1, force: true })
            .unwrap();
        let _ = orchestrator.session_stop();
    }
    assert_eq!(
        fs::read_to_string(working.path().join("data.txt")).unwrap(),
        "v0",
        "rolling back session 1 should restore original v0 content"
    );
}
