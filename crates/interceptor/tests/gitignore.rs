use std::fs;

use codeagent_interceptor::manifest::StepManifest;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_test_support::workspace::TempWorkspace;

mod common;
use common::OperationApplier;

/// Helper: write a `.gitignore` file at the given path.
fn write_gitignore(dir: &std::path::Path, contents: &str) {
    fs::write(dir.join(".gitignore"), contents).unwrap();
}

/// Read the step manifest back from the completed steps directory.
fn read_step_manifest(ws: &TempWorkspace, step_id: u64) -> StepManifest {
    let step_dir = ws.undo_dir.join("steps").join(step_id.to_string());
    StepManifest::read_from(&step_dir).unwrap()
}

// ---------------------------------------------------------------------------
// GI-01: Ignored file is NOT captured on pre_write (verify empty manifest)
// ---------------------------------------------------------------------------
#[test]
fn gi_01_ignored_file_not_captured_on_pre_write() {
    let ws = TempWorkspace::new();
    write_gitignore(&ws.working_dir, "*.log\n");

    // Create a .log file that matches the ignore pattern
    let log_file = ws.working_dir.join("debug.log");
    fs::write(&log_file, b"old log content").unwrap();

    let interceptor =
        UndoInterceptor::with_gitignore(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.write_file(&log_file, b"new log content");
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        manifest.entries.is_empty(),
        "ignored file should not appear in manifest"
    );
}

// ---------------------------------------------------------------------------
// GI-02: Non-ignored file IS captured normally with gitignore enabled
// ---------------------------------------------------------------------------
#[test]
fn gi_02_non_ignored_file_captured_normally() {
    let ws = TempWorkspace::new();
    write_gitignore(&ws.working_dir, "*.log\n");

    let source_file = ws.working_dir.join("main.rs");
    fs::write(&source_file, b"fn main() {}").unwrap();

    let interceptor =
        UndoInterceptor::with_gitignore(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.write_file(&source_file, b"fn main() { println!(\"hello\"); }");
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        manifest.contains_path("main.rs"),
        "non-ignored file should appear in manifest"
    );
}

// ---------------------------------------------------------------------------
// GI-03: Ignored file in directory tree is skipped during pre_unlink
// ---------------------------------------------------------------------------
#[test]
fn gi_03_ignored_file_skipped_in_tree_delete() {
    let ws = TempWorkspace::new();
    write_gitignore(&ws.working_dir, "build/\n");

    // Create a directory tree that matches the ignore pattern
    let build_dir = ws.working_dir.join("build");
    fs::create_dir_all(build_dir.join("sub")).unwrap();
    fs::write(build_dir.join("output.o"), b"object file").unwrap();
    fs::write(build_dir.join("sub").join("lib.a"), b"archive").unwrap();

    // Also create a non-ignored file
    let source_file = ws.working_dir.join("src.rs");
    fs::write(&source_file, b"source").unwrap();

    let interceptor =
        UndoInterceptor::with_gitignore(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    // Delete the ignored build directory and write to a non-ignored file
    ops.delete_tree(&build_dir);
    ops.write_file(&source_file, b"updated source");
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    // The build/ directory and its contents should not be in the manifest
    assert!(
        !manifest.contains_path("build"),
        "ignored directory should not appear in manifest"
    );
    assert!(
        !manifest.contains_path("build/output.o"),
        "file inside ignored directory should not appear in manifest"
    );
    assert!(
        !manifest.contains_path("build/sub/lib.a"),
        "nested file inside ignored directory should not appear in manifest"
    );
    // The non-ignored file should be captured
    assert!(
        manifest.contains_path("src.rs"),
        "non-ignored file should appear in manifest"
    );
}

// ---------------------------------------------------------------------------
// GI-04: Newly created file matching ignore pattern skips record_creation
// ---------------------------------------------------------------------------
#[test]
fn gi_04_created_file_matching_ignore_skips_record_creation() {
    let ws = TempWorkspace::new();
    write_gitignore(&ws.working_dir, "*.tmp\n");

    let interceptor =
        UndoInterceptor::with_gitignore(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.create_file(&ws.working_dir.join("scratch.tmp"), b"temporary data");
    ops.create_file(&ws.working_dir.join("real.txt"), b"real data");
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        !manifest.contains_path("scratch.tmp"),
        "created ignored file should not appear in manifest"
    );
    assert!(
        manifest.contains_path("real.txt"),
        "created non-ignored file should appear in manifest"
    );
}

// ---------------------------------------------------------------------------
// GI-05: Nested .gitignore in subdirectory is respected
// ---------------------------------------------------------------------------
#[test]
fn gi_05_nested_gitignore_respected() {
    let ws = TempWorkspace::new();

    // Root gitignore: ignore *.log
    write_gitignore(&ws.working_dir, "*.log\n");

    // Subdirectory with its own gitignore: ignore *.dat
    let sub_dir = ws.working_dir.join("data");
    fs::create_dir_all(&sub_dir).unwrap();
    write_gitignore(&sub_dir, "*.dat\n");

    // Create test files
    fs::write(ws.working_dir.join("app.log"), b"root log").unwrap();
    fs::write(sub_dir.join("values.dat"), b"data values").unwrap();
    fs::write(sub_dir.join("readme.txt"), b"readme").unwrap();

    let interceptor =
        UndoInterceptor::with_gitignore(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("app.log"), b"new root log");
    ops.write_file(&sub_dir.join("values.dat"), b"new data values");
    ops.write_file(&sub_dir.join("readme.txt"), b"new readme");
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        !manifest.contains_path("app.log"),
        "root-level ignored file should not be in manifest"
    );
    assert!(
        !manifest.contains_path("data/values.dat"),
        "file ignored by nested .gitignore should not be in manifest"
    );
    assert!(
        manifest.contains_path("data/readme.txt"),
        "non-ignored file in subdirectory should be in manifest"
    );
}

// ---------------------------------------------------------------------------
// GI-06: Negation pattern (!important.log) overrides parent ignore
// ---------------------------------------------------------------------------
#[test]
fn gi_06_negation_pattern_overrides_ignore() {
    let ws = TempWorkspace::new();
    write_gitignore(&ws.working_dir, "*.log\n!important.log\n");

    fs::write(ws.working_dir.join("debug.log"), b"debug").unwrap();
    fs::write(ws.working_dir.join("important.log"), b"important").unwrap();

    let interceptor =
        UndoInterceptor::with_gitignore(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("debug.log"), b"new debug");
    ops.write_file(&ws.working_dir.join("important.log"), b"new important");
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        !manifest.contains_path("debug.log"),
        "ignored .log file should not be in manifest"
    );
    assert!(
        manifest.contains_path("important.log"),
        "negated file (!important.log) should be in manifest"
    );
}

// ---------------------------------------------------------------------------
// GI-07: .git/info/exclude patterns are respected
// ---------------------------------------------------------------------------
#[test]
fn gi_07_git_info_exclude_respected() {
    let ws = TempWorkspace::new();

    // Set up .git/info/exclude (not a .gitignore)
    let git_info_dir = ws.working_dir.join(".git").join("info");
    fs::create_dir_all(&git_info_dir).unwrap();
    fs::write(git_info_dir.join("exclude"), "secret.*\n").unwrap();

    fs::write(ws.working_dir.join("secret.key"), b"private key").unwrap();
    fs::write(ws.working_dir.join("public.txt"), b"public").unwrap();

    let interceptor =
        UndoInterceptor::with_gitignore(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("secret.key"), b"new key");
    ops.write_file(&ws.working_dir.join("public.txt"), b"new public");
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        !manifest.contains_path("secret.key"),
        "file matching .git/info/exclude should not be in manifest"
    );
    assert!(
        manifest.contains_path("public.txt"),
        "non-excluded file should be in manifest"
    );
}

// ---------------------------------------------------------------------------
// GI-08: Gitignore disabled by default — ignored files ARE captured
// ---------------------------------------------------------------------------
#[test]
fn gi_08_gitignore_disabled_by_default() {
    let ws = TempWorkspace::new();
    write_gitignore(&ws.working_dir, "*.log\n");

    let log_file = ws.working_dir.join("debug.log");
    fs::write(&log_file, b"old log").unwrap();

    // Use the default constructor (gitignore NOT enabled)
    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.write_file(&log_file, b"new log");
    interceptor.close_step(1).unwrap();

    let manifest = read_step_manifest(&ws, 1);
    assert!(
        manifest.contains_path("debug.log"),
        "with gitignore disabled, ignored files SHOULD be captured"
    );
}
