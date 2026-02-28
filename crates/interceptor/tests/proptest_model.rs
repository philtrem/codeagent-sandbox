use std::fs;
use std::path::{Path, PathBuf};

use proptest::prelude::*;

use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_test_support::fixtures;
use codeagent_test_support::snapshot::assert_tree_eq;
use codeagent_test_support::workspace::TempWorkspace;

mod common;
use common::{compare_opts, OperationApplier};

// ---------------------------------------------------------------------------
// Operation model
// ---------------------------------------------------------------------------

/// A single filesystem operation to apply within a step.
#[derive(Debug, Clone)]
enum Op {
    WriteFile {
        path_index: usize,
        content: Vec<u8>,
    },
    CreateFile {
        name: String,
        content: Vec<u8>,
    },
    DeleteFile {
        path_index: usize,
    },
    DeleteTree {
        dir_index: usize,
    },
    Mkdir {
        name: String,
    },
    Rename {
        from_index: usize,
        to_name: String,
    },
    OpenTrunc {
        path_index: usize,
    },
    SetattrTruncate {
        path_index: usize,
        new_len: u64,
    },
    Fallocate {
        path_index: usize,
        new_len: u64,
    },
    CopyFileRange {
        src_index: usize,
        dst_index: usize,
    },
}

/// A group of operations to apply within a single undo step.
#[derive(Debug, Clone)]
struct StepOps {
    id: i64,
    ops: Vec<Op>,
    should_rollback: bool,
}

/// The top-level test input: a sequence of steps.
#[derive(Debug, Clone)]
struct TestPlan {
    steps: Vec<StepOps>,
}

// ---------------------------------------------------------------------------
// proptest strategies
// ---------------------------------------------------------------------------

fn op_strategy() -> impl Strategy<Value = Op> {
    let content = prop::collection::vec(any::<u8>(), 0..256);
    let index = 0..20usize;

    prop_oneof![
        3 => (index.clone(), content.clone()).prop_map(|(i, c)| Op::WriteFile {
            path_index: i,
            content: c,
        }),
        2 => ("[a-z][a-z0-9]{0,7}\\.txt", content.clone()).prop_map(|(n, c)| Op::CreateFile {
            name: n,
            content: c,
        }),
        2 => index.clone().prop_map(|i| Op::DeleteFile { path_index: i }),
        1 => index.clone().prop_map(|i| Op::DeleteTree { dir_index: i }),
        1 => "[a-z][a-z0-9]{0,5}".prop_map(|n| Op::Mkdir { name: n }),
        2 => (index.clone(), "[a-z][a-z0-9]{0,7}\\.txt").prop_map(|(i, n)| Op::Rename {
            from_index: i,
            to_name: n,
        }),
        1 => index.clone().prop_map(|i| Op::OpenTrunc { path_index: i }),
        1 => (index.clone(), 0..100u64).prop_map(|(i, l)| Op::SetattrTruncate {
            path_index: i,
            new_len: l,
        }),
        1 => (index.clone(), 100..10_000u64).prop_map(|(i, l)| Op::Fallocate {
            path_index: i,
            new_len: l,
        }),
        1 => (index.clone(), index).prop_map(|(s, d)| Op::CopyFileRange {
            src_index: s,
            dst_index: d,
        }),
    ]
}

fn test_plan_strategy() -> impl Strategy<Value = TestPlan> {
    prop::collection::vec(
        (prop::collection::vec(op_strategy(), 1..8), any::<bool>()),
        1..5,
    )
    .prop_map(|steps_data| {
        let steps = steps_data
            .into_iter()
            .enumerate()
            .map(|(i, (ops, should_rollback))| StepOps {
                id: (i + 1) as i64,
                ops,
                should_rollback,
            })
            .collect();
        TestPlan { steps }
    })
}

fn multi_step_plan_strategy() -> impl Strategy<Value = TestPlan> {
    prop::collection::vec(prop::collection::vec(op_strategy(), 1..8), 1..5).prop_map(
        |steps_data| {
            let steps = steps_data
                .into_iter()
                .enumerate()
                .map(|(i, ops)| StepOps {
                    id: (i + 1) as i64,
                    ops,
                    should_rollback: false,
                })
                .collect();
            TestPlan { steps }
        },
    )
}

// ---------------------------------------------------------------------------
// Filesystem scanning helpers
// ---------------------------------------------------------------------------

/// Collect all regular file paths under a directory, sorted for deterministic indexing.
fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files_recursive(dir, &mut files);
    files.sort();
    files
}

fn collect_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            files.push(path);
        } else if path.is_dir() {
            collect_files_recursive(&path, files);
        }
    }
}

/// Collect all directory paths under a directory (excluding root), sorted for
/// deterministic indexing.
fn collect_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    collect_dirs_recursive(dir, &mut dirs);
    dirs.sort();
    dirs
}

fn collect_dirs_recursive(dir: &Path, dirs: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path.clone());
            collect_dirs_recursive(&path, dirs);
        }
    }
}

// ---------------------------------------------------------------------------
// Operation application (pre-filter: skip invalid ops at runtime)
// ---------------------------------------------------------------------------

fn apply_op(op: &Op, working_dir: &Path, applier: &OperationApplier) {
    match op {
        Op::WriteFile {
            path_index,
            content,
        } => {
            let files = collect_files(working_dir);
            if files.is_empty() {
                return;
            }
            let path = &files[*path_index % files.len()];
            applier.write_file(path, content);
        }
        Op::CreateFile { name, content } => {
            let path = working_dir.join(name);
            if path.exists() {
                applier.write_file(&path, content);
            } else {
                applier.create_file(&path, content);
            }
        }
        Op::DeleteFile { path_index } => {
            let files = collect_files(working_dir);
            if files.is_empty() {
                return;
            }
            let path = &files[*path_index % files.len()];
            applier.delete_file(path);
        }
        Op::DeleteTree { dir_index } => {
            let dirs = collect_dirs(working_dir);
            if dirs.is_empty() {
                return;
            }
            let path = &dirs[*dir_index % dirs.len()];
            applier.delete_tree(path);
        }
        Op::Mkdir { name } => {
            let path = working_dir.join(name);
            if !path.exists() {
                applier.mkdir(&path);
            }
        }
        Op::Rename {
            from_index,
            to_name,
        } => {
            let files = collect_files(working_dir);
            if files.is_empty() {
                return;
            }
            let from = &files[*from_index % files.len()];
            let to = from.parent().unwrap_or(working_dir).join(to_name);
            applier.rename(from, &to);
        }
        Op::OpenTrunc { path_index } => {
            let files = collect_files(working_dir);
            if files.is_empty() {
                return;
            }
            let path = &files[*path_index % files.len()];
            applier.open_trunc(path);
        }
        Op::SetattrTruncate {
            path_index,
            new_len,
        } => {
            let files = collect_files(working_dir);
            if files.is_empty() {
                return;
            }
            let path = &files[*path_index % files.len()];
            let current_len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if current_len > 0 {
                let truncate_to = *new_len % current_len;
                applier.setattr_truncate(path, truncate_to);
            }
        }
        Op::Fallocate {
            path_index,
            new_len,
        } => {
            let files = collect_files(working_dir);
            if files.is_empty() {
                return;
            }
            let path = &files[*path_index % files.len()];
            applier.fallocate(path, *new_len);
        }
        Op::CopyFileRange {
            src_index,
            dst_index,
        } => {
            let files = collect_files(working_dir);
            if files.len() < 2 {
                return;
            }
            let src_idx = *src_index % files.len();
            let mut dst_idx = *dst_index % files.len();
            if dst_idx == src_idx {
                dst_idx = (dst_idx + 1) % files.len();
            }
            let src = files[src_idx].clone();
            let dst = files[dst_idx].clone();
            applier.copy_file_range(&src, &dst);
        }
    }
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Model-based test: generate random operation sequences grouped into steps,
    /// optionally roll back each step, and verify that rollback restores the
    /// filesystem to exactly the pre-step state.
    #[test]
    fn undo_model(plan in test_plan_strategy()) {
        let ws = TempWorkspace::with_fixture(fixtures::small_tree);
        let interceptor = UndoInterceptor::new(
            ws.working_dir.clone(),
            ws.undo_dir.clone(),
        );
        let applier = OperationApplier::new(&interceptor);

        for step in &plan.steps {
            let snapshot_before = ws.snapshot();

            interceptor.open_step(step.id).unwrap();

            for op in &step.ops {
                apply_op(op, &ws.working_dir, &applier);
            }

            interceptor.close_step(step.id).unwrap();

            if step.should_rollback {
                interceptor.rollback(1, false).unwrap();
                let snapshot_after = ws.snapshot();
                assert_tree_eq(&snapshot_before, &snapshot_after, &compare_opts());
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// Model-based test: apply multiple steps without intermediate rollback, then
    /// roll back all steps at once and verify the filesystem matches the initial state.
    #[test]
    fn undo_model_multi_step_rollback(plan in multi_step_plan_strategy()) {
        let ws = TempWorkspace::with_fixture(fixtures::small_tree);
        let interceptor = UndoInterceptor::new(
            ws.working_dir.clone(),
            ws.undo_dir.clone(),
        );
        let applier = OperationApplier::new(&interceptor);

        let initial_snapshot = ws.snapshot();

        for step in &plan.steps {
            interceptor.open_step(step.id).unwrap();

            for op in &step.ops {
                apply_op(op, &ws.working_dir, &applier);
            }

            interceptor.close_step(step.id).unwrap();
        }

        let step_count = plan.steps.len();
        interceptor.rollback(step_count, false).unwrap();

        let final_snapshot = ws.snapshot();
        assert_tree_eq(&initial_snapshot, &final_snapshot, &compare_opts());
    }
}
