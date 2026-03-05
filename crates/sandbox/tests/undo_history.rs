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
        undo_dir: undo_dir.to_path_buf(),
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

    // Read barriers from disk
    let subdir_name = undo_subdir_name(working.path());
    let barriers_path = undo.path().join(&subdir_name).join("barriers.json");
    assert!(
        barriers_path.exists(),
        "barriers.json should exist after session restart"
    );

    let json = fs::read_to_string(&barriers_path).unwrap();
    let barriers: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    assert!(!barriers.is_empty(), "should have at least one barrier");

    let has_session_start = barriers.iter().any(|b| {
        b["reason"].as_str() == Some("session_start")
    });
    assert!(
        has_session_start,
        "barrier should have reason 'session_start', got: {:?}",
        barriers.iter().map(|b| b["reason"].clone()).collect::<Vec<_>>()
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
