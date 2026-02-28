use std::fs;

use codeagent_common::{CodeAgentError, ResourceLimitsConfig};
use codeagent_interceptor::manifest::StepManifest;
use codeagent_interceptor::preimage::capture_preimage;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_test_support::fixtures;
use codeagent_test_support::snapshot::assert_tree_eq;
use codeagent_test_support::workspace::TempWorkspace;

mod common;
use common::{compare_opts, OperationApplier};

// ---------------------------------------------------------------------------
// UI-16: Multi-step — rollback(1) restores to post-step-1 state
// ---------------------------------------------------------------------------
#[test]
fn ui_16_multi_step_rollback_restores_intermediate_state() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    let original = ws.snapshot();

    // Step 1: write "step1" to the file
    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    ops.write_file(&target, b"step1 content");
    interceptor.close_step(1).unwrap();

    let after_step_1 = ws.snapshot();

    // Step 2: write "step2" to the same file
    interceptor.open_step(2).unwrap();
    ops.write_file(&target, b"step2 content");
    interceptor.close_step(2).unwrap();

    assert_eq!(fs::read_to_string(&target).unwrap(), "step2 content");

    // Rollback 1 step → should restore to post-step-1 state
    interceptor.rollback(1, false).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "step1 content");
    let after_rollback_1 = ws.snapshot();
    assert_tree_eq(&after_step_1, &after_rollback_1, &compare_opts());

    // Rollback 1 more step → should restore to original
    interceptor.rollback(1, false).unwrap();
    let after_rollback_2 = ws.snapshot();
    assert_tree_eq(&original, &after_rollback_2, &compare_opts());
}

// ---------------------------------------------------------------------------
// UI-17: Unprotected step — exceeds max_single_step_size
// ---------------------------------------------------------------------------
#[test]
fn ui_17_unprotected_step_blocks_rollback() {
    let ws = TempWorkspace::new();

    // Create multiple files with substantial content so the cumulative preimage
    // data exceeds the threshold when we modify them all in one step.
    for i in 0..5 {
        let file = ws.working_dir.join(format!("large_{i}.bin"));
        // Random-ish data that doesn't compress well
        let data: Vec<u8> = (0..1000u32).map(|j| ((i * 1000 + j) % 251) as u8).collect();
        fs::write(&file, &data).unwrap();
    }

    let limits = ResourceLimitsConfig {
        // Threshold is 500 bytes of compressed preimage data. Each 1000-byte
        // file of pseudo-random data compresses to roughly 500-800 bytes, so
        // after 1-2 files the threshold is exceeded.
        max_single_step_size_bytes: Some(500),
        ..Default::default()
    };
    let interceptor =
        UndoInterceptor::with_resource_limits(ws.working_dir.clone(), ws.undo_dir.clone(), limits);
    let ops = OperationApplier::new(&interceptor);

    // Step 1: Modify all 5 files in one step
    interceptor.open_step(1).unwrap();
    for i in 0..5 {
        let file = ws.working_dir.join(format!("large_{i}.bin"));
        ops.write_file(&file, b"replaced");
    }
    interceptor.close_step(1).unwrap();

    // Verify the step is marked unprotected
    let manifest =
        StepManifest::read_from(&ws.undo_dir.join("steps").join("1")).unwrap();
    assert!(manifest.unprotected);

    // Rollback should fail with StepUnprotected
    let err = interceptor.rollback(1, false).unwrap_err();
    assert!(matches!(err, CodeAgentError::StepUnprotected { step_id: 1 }));

    // A subsequent protected step on top should still be rollbackable
    interceptor.open_step(2).unwrap();
    let new_file = ws.working_dir.join("brand_new.txt");
    ops.create_file(&new_file, b"tiny");
    interceptor.close_step(2).unwrap();

    assert!(new_file.exists());

    // Rollback only step 2 (not reaching step 1)
    interceptor.rollback(1, false).unwrap();
    assert!(!new_file.exists());

    // But rolling back 1 more (step 1) should still fail
    let err = interceptor.rollback(1, false).unwrap_err();
    assert!(matches!(err, CodeAgentError::StepUnprotected { .. }));
}

// ---------------------------------------------------------------------------
// UI-18: FIFO eviction by step count
// ---------------------------------------------------------------------------
#[test]
fn ui_18_fifo_eviction_by_step_count() {
    let ws = TempWorkspace::new();
    let limits = ResourceLimitsConfig {
        max_step_count: Some(2),
        ..Default::default()
    };
    let interceptor =
        UndoInterceptor::with_resource_limits(ws.working_dir.clone(), ws.undo_dir.clone(), limits);
    let ops = OperationApplier::new(&interceptor);

    // Create 3 steps
    for step_id in 1..=3 {
        interceptor.open_step(step_id).unwrap();
        let file = ws.working_dir.join(format!("file_{step_id}.txt"));
        ops.create_file(&file, format!("content {step_id}").as_bytes());
        let evicted = interceptor.close_step(step_id).unwrap();

        // Eviction happens on close of step 3 (first time we exceed max_step_count=2)
        if step_id <= 2 {
            assert!(evicted.is_empty());
        } else {
            assert_eq!(evicted, vec![1]);
        }
    }

    // Step 1 should be evicted
    assert!(!ws.undo_dir.join("steps").join("1").exists());
    assert!(ws.undo_dir.join("steps").join("2").exists());
    assert!(ws.undo_dir.join("steps").join("3").exists());

    // Only steps 2 and 3 should be in the completed list
    assert_eq!(interceptor.completed_steps(), vec![2, 3]);

    // Rollback of step 3 should work
    interceptor.rollback(1, false).unwrap();
    assert!(!ws.working_dir.join("file_3.txt").exists());
    assert_eq!(interceptor.completed_steps(), vec![2]);
}

// ---------------------------------------------------------------------------
// UI-19: Log-size eviction
// ---------------------------------------------------------------------------
#[test]
fn ui_19_log_size_eviction() {
    let ws = TempWorkspace::new();

    // First, create 4 steps without limits to measure step sizes
    // Each step modifies a file with substantial content that doesn't compress well.
    for step_id in 1..=4 {
        let file = ws.working_dir.join(format!("data_{step_id}.bin"));
        // Pseudo-random data that won't compress much
        let data: Vec<u8> = (0..2000u32).map(|j| ((step_id as u32 * 2000 + j) % 251) as u8).collect();
        fs::write(&file, &data).unwrap();
    }

    // Measure step size by creating one step without limits
    {
        let probe_ws = TempWorkspace::new();
        let probe_file = probe_ws.working_dir.join("probe.bin");
        let data: Vec<u8> = (0..2000u32).map(|j| (j % 251) as u8).collect();
        fs::write(&probe_file, &data).unwrap();

        let probe_interceptor =
            UndoInterceptor::new(probe_ws.working_dir.clone(), probe_ws.undo_dir.clone());
        let probe_ops = OperationApplier::new(&probe_interceptor);

        probe_interceptor.open_step(1).unwrap();
        probe_ops.write_file(&probe_file, b"replaced");
        probe_interceptor.close_step(1).unwrap();

        let step_size = codeagent_interceptor::resource_limits::calculate_step_size(
            &probe_ws.undo_dir.join("steps").join("1"),
        )
        .unwrap();

        // Set budget to roughly 2.5x one step — should allow 2 steps but not 4
        let budget = step_size * 5 / 2;
        let limits = ResourceLimitsConfig {
            max_log_size_bytes: Some(budget),
            ..Default::default()
        };
        let interceptor = UndoInterceptor::with_resource_limits(
            ws.working_dir.clone(),
            ws.undo_dir.clone(),
            limits,
        );
        let ops = OperationApplier::new(&interceptor);

        for step_id in 1..=4 {
            let file = ws.working_dir.join(format!("data_{step_id}.bin"));
            interceptor.open_step(step_id).unwrap();
            ops.write_file(&file, b"replaced");
            interceptor.close_step(step_id).unwrap();
        }

        let completed = interceptor.completed_steps();
        assert!(
            completed.len() < 4,
            "expected eviction with budget={budget}, steps: {completed:?}"
        );
        assert!(completed.contains(&4), "most recent step should survive");
    }
}

// ---------------------------------------------------------------------------
// UL-01: Manifest correctness — all fields round-trip
// ---------------------------------------------------------------------------
#[test]
fn ul_01_manifest_round_trip_all_fields() {
    let dir = tempfile::TempDir::new().unwrap();

    let mut manifest = StepManifest::new(42);
    manifest.command = Some("rm -rf node_modules".to_string());
    manifest.unprotected = true;
    manifest.add_entry("src/main.rs", "hash_a", true, "regular");
    manifest.add_entry("new_dir", "hash_b", false, "directory");
    manifest.add_entry("link.txt", "hash_c", true, "symlink");

    manifest.write_to(dir.path()).unwrap();
    let loaded = StepManifest::read_from(dir.path()).unwrap();

    assert_eq!(loaded.step_id, 42);
    assert_eq!(loaded.command, Some("rm -rf node_modules".to_string()));
    assert!(loaded.unprotected);
    assert_eq!(loaded.entries.len(), 3);

    let src_entry = &loaded.entries["src/main.rs"];
    assert!(src_entry.existed_before);
    assert_eq!(src_entry.path_hash, "hash_a");
    assert_eq!(src_entry.file_type, "regular");

    let dir_entry = &loaded.entries["new_dir"];
    assert!(!dir_entry.existed_before);
    assert_eq!(dir_entry.file_type, "directory");

    let link_entry = &loaded.entries["link.txt"];
    assert!(link_entry.existed_before);
    assert_eq!(link_entry.file_type, "symlink");
}

// ---------------------------------------------------------------------------
// UL-02: Preimage atomicity — no .tmp files after capture
// ---------------------------------------------------------------------------
#[test]
fn ul_02_preimage_atomicity_no_tmp_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let working = dir.path().join("working");
    let preimages = dir.path().join("preimages");
    fs::create_dir_all(&working).unwrap();
    fs::create_dir_all(&preimages).unwrap();

    let file = working.join("test.txt");
    fs::write(&file, "hello world").unwrap();

    capture_preimage(&file, &working, &preimages).unwrap();

    // No .tmp files should remain
    let tmp_files: Vec<_> = fs::read_dir(&preimages)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        })
        .collect();
    assert!(tmp_files.is_empty(), "found .tmp files: {tmp_files:?}");

    // The actual files should exist
    let files: Vec<String> = fs::read_dir(&preimages)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(files.iter().any(|f| f.ends_with(".meta.json")));
    assert!(files.iter().any(|f| f.ends_with(".dat")));
}

// ---------------------------------------------------------------------------
// UL-03: Step promotion atomicity — WAL gone, step dir exists
// ---------------------------------------------------------------------------
#[test]
fn ul_03_step_promotion_atomicity() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    ops.write_file(&target, b"modified");
    interceptor.close_step(1).unwrap();

    // WAL in_progress should be gone
    assert!(!ws.undo_dir.join("wal").join("in_progress").exists());

    // Step directory should exist with manifest and preimages
    let step_dir = ws.undo_dir.join("steps").join("1");
    assert!(step_dir.exists());
    assert!(step_dir.join("manifest.json").exists());
    assert!(step_dir.join("preimages").exists());
}

// ---------------------------------------------------------------------------
// UL-04: Version mismatch disables undo
// ---------------------------------------------------------------------------
#[test]
fn ul_04_version_mismatch_disables_undo() {
    let ws = TempWorkspace::new();

    // Pre-initialize the undo dir with a mismatched version
    fs::create_dir_all(&ws.undo_dir).unwrap();
    fs::write(ws.undo_dir.join("version"), "999").unwrap();
    fs::create_dir_all(ws.undo_dir.join("wal")).unwrap();
    fs::create_dir_all(ws.undo_dir.join("steps")).unwrap();

    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());

    assert!(interceptor.is_undo_disabled());
    assert_eq!(
        interceptor.version_mismatch(),
        Some(("1".to_string(), "999".to_string()))
    );

    // open_step should return UndoDisabled error
    let err = interceptor.open_step(1).unwrap_err();
    assert!(
        matches!(err, CodeAgentError::UndoDisabled { .. }),
        "expected UndoDisabled, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// UL-05: Discard after version mismatch re-enables undo
// ---------------------------------------------------------------------------
#[test]
fn ul_05_discard_after_mismatch_re_enables_undo() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);

    // Pre-initialize with mismatched version
    fs::create_dir_all(&ws.undo_dir).unwrap();
    fs::write(ws.undo_dir.join("version"), "999").unwrap();
    fs::create_dir_all(ws.undo_dir.join("wal")).unwrap();
    fs::create_dir_all(ws.undo_dir.join("steps")).unwrap();

    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    assert!(interceptor.is_undo_disabled());

    // Discard the old log
    interceptor.discard().unwrap();

    // Undo should now be re-enabled
    assert!(!interceptor.is_undo_disabled());
    assert_eq!(interceptor.version_mismatch(), None);

    // Version file should be correct
    let version = fs::read_to_string(ws.undo_dir.join("version")).unwrap();
    assert_eq!(version.trim(), "1");

    // Normal operations should work
    let ops = OperationApplier::new(&interceptor);
    let before = ws.snapshot();

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    ops.write_file(&target, b"after discard");
    interceptor.close_step(1).unwrap();

    interceptor.rollback(1, false).unwrap();
    let after = ws.snapshot();
    assert_tree_eq(&before, &after, &compare_opts());
}

// ---------------------------------------------------------------------------
// UL-06: Corrupt manifest — graceful error, no panic
// ---------------------------------------------------------------------------
#[test]
fn ul_06_corrupt_manifest_graceful_error() {
    let dir = tempfile::TempDir::new().unwrap();

    // Write truncated/invalid JSON
    fs::write(dir.path().join("manifest.json"), r#"{"step_id":1,"timest"#).unwrap();

    let result = StepManifest::read_from(dir.path());
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// UL-07: Missing preimage file — rollback returns error
// ---------------------------------------------------------------------------
#[test]
fn ul_07_missing_preimage_file_rollback_error() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    ops.write_file(&target, b"modified");
    interceptor.close_step(1).unwrap();

    // Delete the .dat preimage file
    let step_dir = ws.undo_dir.join("steps").join("1");
    let preimage_dir = step_dir.join("preimages");
    for entry in fs::read_dir(&preimage_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().ends_with(".dat") {
            fs::remove_file(entry.path()).unwrap();
        }
    }

    // Rollback should fail
    let result = interceptor.rollback(1, false);
    assert!(result.is_err(), "expected error from missing preimage");
}

// ---------------------------------------------------------------------------
// UL-08: Corrupt preimage — rollback returns decompression error
// ---------------------------------------------------------------------------
#[test]
fn ul_08_corrupt_preimage_rollback_error() {
    let ws = TempWorkspace::with_fixture(fixtures::small_tree);
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    let target = ws.working_dir.join("small.txt");
    ops.write_file(&target, b"modified");
    interceptor.close_step(1).unwrap();

    // Corrupt the .dat preimage file
    let step_dir = ws.undo_dir.join("steps").join("1");
    let preimage_dir = step_dir.join("preimages");
    for entry in fs::read_dir(&preimage_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().ends_with(".dat") {
            fs::write(entry.path(), b"this is not valid zstd data").unwrap();
        }
    }

    // Rollback should fail with a decompression error
    let result = interceptor.rollback(1, false);
    assert!(result.is_err(), "expected decompression error");

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("decompress") || msg.contains("Decompression"),
        "expected decompression-related error, got: {msg}"
    );
}
