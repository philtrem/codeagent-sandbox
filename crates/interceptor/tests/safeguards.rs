mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use codeagent_common::{
    CodeAgentError, ExternalModificationPolicy, SafeguardConfig, SafeguardDecision,
    SafeguardEvent, SafeguardKind,
};
use codeagent_interceptor::safeguard::SafeguardHandler;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_interceptor::write_interceptor::WriteInterceptor;
use codeagent_test_support::snapshot::{assert_tree_eq, TreeSnapshot};
use codeagent_test_support::workspace::TempWorkspace;

use common::{compare_opts, OperationApplier};

// ---------------------------------------------------------------------------
// Test handler: returns a fixed decision and records all events
// ---------------------------------------------------------------------------

struct ImmediateHandler {
    decision: SafeguardDecision,
    events: Arc<Mutex<Vec<SafeguardEvent>>>,
}

impl ImmediateHandler {
    fn new(decision: SafeguardDecision) -> (Self, Arc<Mutex<Vec<SafeguardEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let handler = Self {
            decision,
            events: events.clone(),
        };
        (handler, events)
    }
}

impl SafeguardHandler for ImmediateHandler {
    fn on_safeguard_triggered(&self, event: SafeguardEvent) -> SafeguardDecision {
        self.events.lock().unwrap().push(event);
        self.decision
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_interceptor(
    ws: &TempWorkspace,
    config: SafeguardConfig,
    handler: Box<dyn SafeguardHandler>,
) -> UndoInterceptor {
    UndoInterceptor::with_safeguard(
        ws.working_dir.clone(),
        ws.undo_dir.clone(),
        ExternalModificationPolicy::default(),
        config,
        handler,
    )
}

fn snapshot(ws: &TempWorkspace) -> TreeSnapshot {
    ws.snapshot()
}

fn create_files(ws: &TempWorkspace, names: &[&str], size: usize) {
    let data = vec![0xABu8; size];
    for name in names {
        let path = ws.working_dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, &data).unwrap();
    }
}

// ---------------------------------------------------------------------------
// SG-01: Delete count reaches threshold → handler called with correct event
// ---------------------------------------------------------------------------

#[test]
fn sg_01_delete_threshold_triggers_safeguard() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"], 10);

    let (handler, events) = ImmediateHandler::new(SafeguardDecision::Allow);
    let config = SafeguardConfig {
        delete_threshold: Some(3),
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();

    // Deletes 1 and 2: below threshold, no trigger
    ops.delete_file(&ws.working_dir.join("a.txt"));
    ops.delete_file(&ws.working_dir.join("b.txt"));
    assert_eq!(events.lock().unwrap().len(), 0);

    // Delete 3: reaches threshold, triggers safeguard
    ops.delete_file(&ws.working_dir.join("c.txt"));
    let recorded = events.lock().unwrap();
    assert_eq!(recorded.len(), 1);

    let event = &recorded[0];
    assert_eq!(event.step_id, 1);
    assert!(matches!(
        &event.kind,
        SafeguardKind::DeleteThreshold {
            count: 3,
            threshold: 3
        }
    ));
    assert_eq!(event.sample_paths.len(), 3);

    drop(recorded);
    interceptor.close_step(1).unwrap();
}

// ---------------------------------------------------------------------------
// SG-02: Confirm allow → step commits; undo works
// ---------------------------------------------------------------------------

#[test]
fn sg_02_allow_step_commits_and_undo_works() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["a.txt", "b.txt", "c.txt"], 10);
    let before = snapshot(&ws);

    let (handler, _events) = ImmediateHandler::new(SafeguardDecision::Allow);
    let config = SafeguardConfig {
        delete_threshold: Some(2),
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.delete_file(&ws.working_dir.join("a.txt"));
    // 2nd delete triggers safeguard → allowed
    ops.delete_file(&ws.working_dir.join("b.txt"));
    // 3rd delete after allow — should not re-trigger
    ops.delete_file(&ws.working_dir.join("c.txt"));
    interceptor.close_step(1).unwrap();

    // All 3 files should be deleted
    assert!(!ws.working_dir.join("a.txt").exists());
    assert!(!ws.working_dir.join("b.txt").exists());
    assert!(!ws.working_dir.join("c.txt").exists());

    // Rollback restores everything
    interceptor.rollback(1, false).unwrap();
    assert_tree_eq(&before, &snapshot(&ws), &compare_opts());
}

// ---------------------------------------------------------------------------
// SG-03: Confirm deny → entire step rolled back
// ---------------------------------------------------------------------------

#[test]
fn sg_03_deny_rolls_back_entire_step() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["a.txt", "b.txt", "c.txt"], 10);
    let before = snapshot(&ws);

    let (handler, _events) = ImmediateHandler::new(SafeguardDecision::Deny);
    let config = SafeguardConfig {
        delete_threshold: Some(2),
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();

    // 1st delete succeeds (below threshold)
    ops.delete_file(&ws.working_dir.join("a.txt"));
    assert!(!ws.working_dir.join("a.txt").exists());

    // 2nd delete triggers safeguard → denied → step rolled back
    let result = interceptor.pre_unlink(&ws.working_dir.join("b.txt"), false);
    assert!(matches!(
        result,
        Err(CodeAgentError::SafeguardDenied { .. })
    ));

    // a.txt should be restored (rolled back), b.txt untouched
    assert_tree_eq(&before, &snapshot(&ws), &compare_opts());

    // No active step after deny
    assert!(interceptor.current_step().is_none());
    // Step was not added to completed list
    assert!(interceptor.completed_steps().is_empty());
}

// ---------------------------------------------------------------------------
// SG-04: Timeout → auto-deny (simulated by Deny handler)
// ---------------------------------------------------------------------------

#[test]
fn sg_04_timeout_auto_deny_rolls_back() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["x.txt", "y.txt"], 10);
    let before = snapshot(&ws);

    let (handler, _events) = ImmediateHandler::new(SafeguardDecision::Deny);
    let config = SafeguardConfig {
        delete_threshold: Some(2),
        timeout_seconds: 1,
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.delete_file(&ws.working_dir.join("x.txt"));

    let result = interceptor.pre_unlink(&ws.working_dir.join("y.txt"), false);
    assert!(matches!(
        result,
        Err(CodeAgentError::SafeguardDenied { .. })
    ));

    // Working directory fully restored
    assert_tree_eq(&before, &snapshot(&ws), &compare_opts());
    assert!(interceptor.current_step().is_none());
}

// ---------------------------------------------------------------------------
// SG-05: Overwrite-large-file threshold
// ---------------------------------------------------------------------------

#[test]
fn sg_05_overwrite_large_file_triggers_safeguard() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["big.dat"], 500);

    let (handler, events) = ImmediateHandler::new(SafeguardDecision::Allow);
    let config = SafeguardConfig {
        overwrite_file_size_threshold: Some(100),
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();

    // Overwrite the large file → triggers safeguard
    ops.write_file(&ws.working_dir.join("big.dat"), b"new content");

    let recorded = events.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(matches!(
        &recorded[0].kind,
        SafeguardKind::OverwriteLargeFile {
            file_size: 500,
            threshold: 100,
            ..
        }
    ));
    assert_eq!(recorded[0].step_id, 1);

    drop(recorded);
    interceptor.close_step(1).unwrap();
}

// ---------------------------------------------------------------------------
// SG-06: Rename-over-existing threshold
// ---------------------------------------------------------------------------

#[test]
fn sg_06_rename_over_existing_triggers_safeguard() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["source.txt", "dest.txt"], 10);

    let (handler, events) = ImmediateHandler::new(SafeguardDecision::Allow);
    let config = SafeguardConfig {
        rename_over_existing: true,
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();

    // Rename source → dest (dest exists) → triggers safeguard
    ops.rename(
        &ws.working_dir.join("source.txt"),
        &ws.working_dir.join("dest.txt"),
    );

    let recorded = events.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(matches!(
        &recorded[0].kind,
        SafeguardKind::RenameOverExisting { .. }
    ));
    if let SafeguardKind::RenameOverExisting {
        ref source,
        ref destination,
    } = recorded[0].kind
    {
        assert_eq!(source, "source.txt");
        assert_eq!(destination, "dest.txt");
    }

    drop(recorded);
    interceptor.close_step(1).unwrap();
}

// ---------------------------------------------------------------------------
// Edge case: Allow does not re-trigger same kind in same step
// ---------------------------------------------------------------------------

#[test]
fn sg_allow_does_not_retrigger_same_kind() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["a.txt", "b.txt", "c.txt", "d.txt"], 10);

    let (handler, events) = ImmediateHandler::new(SafeguardDecision::Allow);
    let config = SafeguardConfig {
        delete_threshold: Some(2),
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.delete_file(&ws.working_dir.join("a.txt"));
    ops.delete_file(&ws.working_dir.join("b.txt")); // triggers (count=2)
    ops.delete_file(&ws.working_dir.join("c.txt")); // no re-trigger
    ops.delete_file(&ws.working_dir.join("d.txt")); // no re-trigger
    interceptor.close_step(1).unwrap();

    // Handler should have been called exactly once
    assert_eq!(events.lock().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Edge case: Below-threshold deletes don't trigger safeguard
// ---------------------------------------------------------------------------

#[test]
fn sg_no_trigger_below_threshold() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["a.txt", "b.txt"], 10);

    let (handler, events) = ImmediateHandler::new(SafeguardDecision::Allow);
    let config = SafeguardConfig {
        delete_threshold: Some(5),
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.delete_file(&ws.working_dir.join("a.txt"));
    ops.delete_file(&ws.working_dir.join("b.txt"));
    interceptor.close_step(1).unwrap();

    assert_eq!(events.lock().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Edge case: Default config (no safeguards) never triggers
// ---------------------------------------------------------------------------

#[test]
fn sg_default_config_never_triggers() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["a.txt", "b.txt", "c.txt"], 500);

    let interceptor = UndoInterceptor::new(ws.working_dir.clone(), ws.undo_dir.clone());
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.delete_file(&ws.working_dir.join("a.txt"));
    ops.delete_file(&ws.working_dir.join("b.txt"));
    ops.delete_file(&ws.working_dir.join("c.txt"));
    interceptor.close_step(1).unwrap();

    assert!(interceptor.completed_steps().contains(&1));
}

// ---------------------------------------------------------------------------
// Edge case: Small file overwrite does not trigger overwrite safeguard
// ---------------------------------------------------------------------------

#[test]
fn sg_small_file_overwrite_no_trigger() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["small.txt"], 10);

    let (handler, events) = ImmediateHandler::new(SafeguardDecision::Allow);
    let config = SafeguardConfig {
        overwrite_file_size_threshold: Some(100),
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    interceptor.open_step(1).unwrap();
    ops.write_file(&ws.working_dir.join("small.txt"), b"new content");
    interceptor.close_step(1).unwrap();

    assert_eq!(events.lock().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Edge case: Safeguard counters reset between steps
// ---------------------------------------------------------------------------

#[test]
fn sg_counters_reset_between_steps() {
    let ws = TempWorkspace::new();
    create_files(&ws, &["a.txt", "b.txt", "c.txt", "d.txt"], 10);

    let (handler, events) = ImmediateHandler::new(SafeguardDecision::Allow);
    let config = SafeguardConfig {
        delete_threshold: Some(2),
        ..SafeguardConfig::default()
    };
    let interceptor = make_interceptor(&ws, config, Box::new(handler));
    let ops = OperationApplier::new(&interceptor);

    // Step 1: delete 1 file (below threshold)
    interceptor.open_step(1).unwrap();
    ops.delete_file(&ws.working_dir.join("a.txt"));
    interceptor.close_step(1).unwrap();
    assert_eq!(events.lock().unwrap().len(), 0);

    // Step 2: delete 1 file (below threshold — counter was reset)
    interceptor.open_step(2).unwrap();
    ops.delete_file(&ws.working_dir.join("b.txt"));
    interceptor.close_step(2).unwrap();
    assert_eq!(events.lock().unwrap().len(), 0);

    // Step 3: delete 2 files → triggers (counter started fresh)
    interceptor.open_step(3).unwrap();
    ops.delete_file(&ws.working_dir.join("c.txt"));
    ops.delete_file(&ws.working_dir.join("d.txt"));
    interceptor.close_step(3).unwrap();
    assert_eq!(events.lock().unwrap().len(), 1);
}
