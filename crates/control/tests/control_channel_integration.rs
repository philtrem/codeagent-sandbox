//! CC-08 through CC-12: Control channel integration tests with fake shim.
//!
//! These L3 tests verify that the `ControlChannelHandler` correctly integrates
//! the protocol state machine with undo step lifecycle management, including
//! quiescence window behavior and ambient step handling.
//!
//! All tests use `tokio::time::pause()` (via `start_paused = true`) for
//! deterministic time control.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use codeagent_common::StepId;
use codeagent_control::{
    ControlChannelHandler, HandlerEvent, HostMessage, InFlightTracker, OutputStream,
    QuiescenceConfig, StepManager, VmMessage,
};

// ---------------------------------------------------------------------------
// MockStepManager — records open/close calls for assertion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum StepManagerCall {
    OpenStep(StepId),
    CloseStep(StepId),
}

#[derive(Default)]
struct MockStepManager {
    calls: Mutex<Vec<StepManagerCall>>,
}

impl MockStepManager {
    fn calls(&self) -> Vec<StepManagerCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl StepManager for MockStepManager {
    fn open_step(&self, id: StepId) -> codeagent_common::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(StepManagerCall::OpenStep(id));
        Ok(())
    }

    fn close_step(&self, id: StepId) -> codeagent_common::Result<Vec<StepId>> {
        self.calls
            .lock()
            .unwrap()
            .push(StepManagerCall::CloseStep(id));
        Ok(vec![])
    }

    fn current_step(&self) -> Option<StepId> {
        let calls = self.calls.lock().unwrap();
        match calls.last() {
            Some(StepManagerCall::OpenStep(id)) => Some(*id),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

struct TestHarness {
    handler: ControlChannelHandler<MockStepManager>,
    events: mpsc::UnboundedReceiver<HandlerEvent>,
    step_manager: Arc<MockStepManager>,
    in_flight: InFlightTracker,
}

fn create_test_harness(config: QuiescenceConfig) -> TestHarness {
    let step_manager = Arc::new(MockStepManager::default());
    let in_flight = InFlightTracker::new();
    let (handler, events) =
        ControlChannelHandler::new(Arc::clone(&step_manager), in_flight.clone(), config);
    TestHarness {
        handler,
        events,
        step_manager,
        in_flight,
    }
}

fn default_harness() -> TestHarness {
    create_test_harness(QuiescenceConfig::default())
}

/// Drain all currently available events from the receiver without blocking.
fn drain_events(events: &mut mpsc::UnboundedReceiver<HandlerEvent>) -> Vec<HandlerEvent> {
    let mut collected = Vec::new();
    while let Ok(event) = events.try_recv() {
        collected.push(event);
    }
    collected
}

/// Advance time and yield enough for spawned tasks (like quiescence) to complete.
/// In paused mode, `advance` wakes timers but spawned tasks still need runtime
/// cycles to execute their continuations after awaiting.
async fn advance_and_settle(duration: Duration) {
    tokio::time::advance(duration).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

/// Run a full exec cycle through step_completed (does NOT wait for quiescence).
async fn run_exec_through_completed(
    harness: &TestHarness,
    id: u64,
    command: &str,
    output: &[(&str, OutputStream)],
    exit_code: i32,
) {
    harness
        .handler
        .send_exec(id, command.to_string(), None, None)
        .await;

    harness
        .handler
        .handle_vm_message(VmMessage::StepStarted { id })
        .await;

    for (data, stream) in output {
        harness
            .handler
            .handle_vm_message(VmMessage::Output {
                id,
                stream: *stream,
                data: data.to_string(),
            })
            .await;
    }

    harness
        .handler
        .handle_vm_message(VmMessage::StepCompleted { id, exit_code })
        .await;

    // Yield so the spawned quiescence task gets its first poll and
    // registers its sleep timer at the current (paused) time.
    tokio::task::yield_now().await;
}

// ---------------------------------------------------------------------------
// CC-08: Normal exec cycle
// ---------------------------------------------------------------------------

/// CC-08: Host sends exec; fake shim returns started/output/completed;
/// events forwarded; undo step opened and closed correctly.
#[tokio::test(start_paused = true)]
async fn cc08_normal_exec_cycle() {
    let mut harness = default_harness();

    // Send exec command
    let host_msg = harness
        .handler
        .send_exec(1, "echo hello".to_string(), None, None)
        .await;

    // Verify the returned HostMessage
    assert!(matches!(host_msg, HostMessage::Exec { id: 1, .. }));

    // Shim responds: step_started
    harness
        .handler
        .handle_vm_message(VmMessage::StepStarted { id: 1 })
        .await;

    let events = drain_events(&mut harness.events);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        HandlerEvent::StepStarted {
            step_id: 1,
            command: "echo hello".to_string(),
        }
    );

    // Verify step was opened
    assert_eq!(
        harness.step_manager.calls(),
        vec![StepManagerCall::OpenStep(1)]
    );

    // Shim responds: output
    harness
        .handler
        .handle_vm_message(VmMessage::Output {
            id: 1,
            stream: OutputStream::Stdout,
            data: "hello\n".to_string(),
        })
        .await;

    let events = drain_events(&mut harness.events);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        HandlerEvent::Output {
            step_id: 1,
            stream: OutputStream::Stdout,
            data: "hello\n".to_string(),
        }
    );

    // Shim responds: step_completed
    harness
        .handler
        .handle_vm_message(VmMessage::StepCompleted {
            id: 1,
            exit_code: 0,
        })
        .await;

    // Step should NOT be closed yet (in quiescence window)
    tokio::task::yield_now().await;
    assert!(harness.handler.in_quiescence().await);
    assert_eq!(
        harness.step_manager.calls(),
        vec![StepManagerCall::OpenStep(1)]
    );

    // Advance past quiescence idle timeout (100ms default)
    advance_and_settle(Duration::from_millis(100)).await;

    // Now step should be closed
    assert!(!harness.handler.in_quiescence().await);
    assert_eq!(
        harness.step_manager.calls(),
        vec![
            StepManagerCall::OpenStep(1),
            StepManagerCall::CloseStep(1),
        ]
    );

    let events = drain_events(&mut harness.events);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        HandlerEvent::StepCompleted {
            step_id: 1,
            exit_code: 0,
            cancelled: false,
            evicted_steps: vec![],
        }
    );
}

// ---------------------------------------------------------------------------
// CC-09: Quiescence window — no late writes
// ---------------------------------------------------------------------------

/// CC-09: Step closes after step_completed + quiescence idle timeout
/// when there are no in-flight or late-arriving writes.
#[tokio::test(start_paused = true)]
async fn cc09_quiescence_no_late_writes() {
    let mut harness = default_harness();

    run_exec_through_completed(&harness, 1, "ls", &[], 0).await;
    drain_events(&mut harness.events); // discard StepStarted

    // Immediately after step_completed: step NOT closed
    tokio::task::yield_now().await;
    assert!(harness.handler.in_quiescence().await);
    assert!(!harness
        .step_manager
        .calls()
        .contains(&StepManagerCall::CloseStep(1)));

    // Advance halfway through idle timeout — still not closed
    advance_and_settle(Duration::from_millis(50)).await;
    assert!(harness.handler.in_quiescence().await);

    // Advance to the idle timeout — now closed
    advance_and_settle(Duration::from_millis(50)).await;
    assert!(!harness.handler.in_quiescence().await);

    assert_eq!(
        harness.step_manager.calls(),
        vec![
            StepManagerCall::OpenStep(1),
            StepManagerCall::CloseStep(1),
        ]
    );

    let events = drain_events(&mut harness.events);
    assert!(events.iter().any(|e| matches!(
        e,
        HandlerEvent::StepCompleted {
            step_id: 1,
            exit_code: 0,
            cancelled: false,
            ..
        }
    )));
}

// ---------------------------------------------------------------------------
// CC-10: Quiescence window — late write arrives
// ---------------------------------------------------------------------------

/// CC-10: A filesystem write starts and finishes during the quiescence window.
/// The step remains open during the write and closes idle_timeout after
/// the write completes.
#[tokio::test(start_paused = true)]
async fn cc10_quiescence_late_write() {
    let mut harness = default_harness();

    run_exec_through_completed(&harness, 1, "npm install", &[], 0).await;
    drain_events(&mut harness.events);
    tokio::task::yield_now().await;

    // At T+50ms: a late filesystem write starts
    advance_and_settle(Duration::from_millis(50)).await;
    harness.in_flight.begin_operation();
    tokio::task::yield_now().await;

    // Step still not closed (in-flight > 0)
    assert!(harness.handler.in_quiescence().await);
    assert!(!harness
        .step_manager
        .calls()
        .contains(&StepManagerCall::CloseStep(1)));

    // At T+80ms: write completes
    advance_and_settle(Duration::from_millis(30)).await;
    harness.in_flight.end_operation();
    tokio::task::yield_now().await;

    // Still in quiescence — idle timer restarts from drain point
    assert!(harness.handler.in_quiescence().await);

    // Advance through idle timeout (100ms from drain)
    advance_and_settle(Duration::from_millis(100)).await;

    // Now step should be closed
    assert!(!harness.handler.in_quiescence().await);
    assert_eq!(
        harness.step_manager.calls(),
        vec![
            StepManagerCall::OpenStep(1),
            StepManagerCall::CloseStep(1),
        ]
    );
}

// ---------------------------------------------------------------------------
// CC-11: Quiescence timeout — prevent hang
// ---------------------------------------------------------------------------

/// CC-11: If in-flight ops never drain, step closes after max_timeout (2s).
#[tokio::test(start_paused = true)]
async fn cc11_quiescence_max_timeout() {
    let mut harness = default_harness();

    run_exec_through_completed(&harness, 1, "make", &[], 0).await;
    drain_events(&mut harness.events);
    tokio::task::yield_now().await;

    // Simulate a filesystem operation that never completes
    harness.in_flight.begin_operation();
    tokio::task::yield_now().await;

    // At T+100ms: step NOT closed (in-flight > 0, idle timeout insufficient)
    advance_and_settle(Duration::from_millis(100)).await;
    assert!(harness.handler.in_quiescence().await);

    // At T+1s: step still NOT closed (max_timeout is 2s)
    advance_and_settle(Duration::from_millis(900)).await;
    assert!(harness.handler.in_quiescence().await);

    // At T+2s: max_timeout reached — step IS closed
    advance_and_settle(Duration::from_millis(1000)).await;
    assert!(!harness.handler.in_quiescence().await);

    assert_eq!(
        harness.step_manager.calls(),
        vec![
            StepManagerCall::OpenStep(1),
            StepManagerCall::CloseStep(1),
        ]
    );

    let events = drain_events(&mut harness.events);
    assert!(events.iter().any(|e| matches!(
        e,
        HandlerEvent::StepCompleted {
            step_id: 1,
            exit_code: 0,
            ..
        }
    )));

    // Clean up: end the stuck operation
    harness.in_flight.end_operation();
}

// ---------------------------------------------------------------------------
// CC-12: Ambient writes after step close
// ---------------------------------------------------------------------------

/// CC-12: Writes arriving after the quiescence window go to an ambient step,
/// not the closed command step.
#[tokio::test(start_paused = true)]
async fn cc12_ambient_writes_after_step_close() {
    let mut harness = default_harness();

    // Run a full exec cycle + wait for quiescence
    run_exec_through_completed(&harness, 1, "echo done", &[], 0).await;
    drain_events(&mut harness.events);
    advance_and_settle(Duration::from_millis(100)).await;
    assert!(!harness.handler.in_quiescence().await);
    drain_events(&mut harness.events); // discard StepCompleted

    // Command step is fully closed. A new write arrives.
    harness.handler.notify_fs_write().await;
    // Yield so the spawned ambient timeout task registers its timer
    tokio::task::yield_now().await;

    let events = drain_events(&mut harness.events);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        HandlerEvent::AmbientStepOpened { step_id: -1 }
    );

    // Verify ambient step was opened
    assert_eq!(harness.handler.ambient_step_id().await, Some(-1));
    assert!(harness
        .step_manager
        .calls()
        .contains(&StepManagerCall::OpenStep(-1)));

    // Wait for ambient inactivity timeout (5s)
    advance_and_settle(Duration::from_secs(5)).await;

    let events = drain_events(&mut harness.events);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        HandlerEvent::AmbientStepClosed {
            step_id: -1,
            evicted_steps: vec![],
        }
    );

    // Ambient step is closed
    assert_eq!(harness.handler.ambient_step_id().await, None);
    assert!(harness
        .step_manager
        .calls()
        .contains(&StepManagerCall::CloseStep(-1)));

    // A second write arrives → new ambient step with ID -2
    harness.handler.notify_fs_write().await;
    tokio::task::yield_now().await;

    let events = drain_events(&mut harness.events);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        HandlerEvent::AmbientStepOpened { step_id: -2 }
    );

    // Clean up: close the ambient step
    advance_and_settle(Duration::from_secs(5)).await;
}

// ---------------------------------------------------------------------------
// Edge case tests
// ---------------------------------------------------------------------------

/// Ambient timer resets on subsequent writes.
#[tokio::test(start_paused = true)]
async fn ambient_timer_resets_on_write() {
    let mut harness = default_harness();

    // Open ambient step
    harness.handler.notify_fs_write().await;
    drain_events(&mut harness.events);

    // At T+3s: another write arrives
    advance_and_settle(Duration::from_secs(3)).await;
    harness.handler.notify_fs_write().await;
    tokio::task::yield_now().await;

    // At T+5s (from start, but only 2s from last write): step should still be open
    advance_and_settle(Duration::from_secs(2)).await;
    assert_eq!(harness.handler.ambient_step_id().await, Some(-1));
    assert!(drain_events(&mut harness.events).is_empty());

    // At T+8s (5s from last write): step should be closed
    advance_and_settle(Duration::from_secs(3)).await;

    let events = drain_events(&mut harness.events);
    assert!(events.iter().any(|e| matches!(
        e,
        HandlerEvent::AmbientStepClosed { step_id: -1, .. }
    )));
}

/// Writes during an active command step do NOT create ambient steps.
#[tokio::test(start_paused = true)]
async fn no_ambient_during_active_command() {
    let mut harness = default_harness();

    // Start exec, get step_started
    harness
        .handler
        .send_exec(1, "cargo build".to_string(), None, None)
        .await;
    harness
        .handler
        .handle_vm_message(VmMessage::StepStarted { id: 1 })
        .await;
    drain_events(&mut harness.events);

    // Filesystem write during active command
    harness.handler.notify_fs_write().await;

    // No ambient step should be opened
    let events = drain_events(&mut harness.events);
    assert!(events.is_empty());
    assert_eq!(harness.handler.ambient_step_id().await, None);

    // Clean up
    harness
        .handler
        .handle_vm_message(VmMessage::StepCompleted {
            id: 1,
            exit_code: 0,
        })
        .await;
    advance_and_settle(Duration::from_millis(100)).await;
}

/// Writes during the quiescence window do NOT create ambient steps.
#[tokio::test(start_paused = true)]
async fn no_ambient_during_quiescence() {
    let mut harness = default_harness();

    run_exec_through_completed(&harness, 1, "ls", &[], 0).await;
    drain_events(&mut harness.events);
    tokio::task::yield_now().await;

    // We're in quiescence window now
    assert!(harness.handler.in_quiescence().await);

    // Filesystem write during quiescence
    harness.handler.notify_fs_write().await;

    // No ambient step opened
    let events = drain_events(&mut harness.events);
    assert!(events.is_empty());
    assert_eq!(harness.handler.ambient_step_id().await, None);

    // Clean up
    advance_and_settle(Duration::from_millis(100)).await;
}

/// A new exec command closes any open ambient step first.
#[tokio::test(start_paused = true)]
async fn exec_closes_ambient_step() {
    let mut harness = default_harness();

    // Open ambient step
    harness.handler.notify_fs_write().await;
    drain_events(&mut harness.events);
    assert_eq!(harness.handler.ambient_step_id().await, Some(-1));

    // Send exec command — ambient step should be closed first
    harness
        .handler
        .send_exec(1, "echo hi".to_string(), None, None)
        .await;

    let events = drain_events(&mut harness.events);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        HandlerEvent::AmbientStepClosed {
            step_id: -1,
            evicted_steps: vec![],
        }
    );

    // Ambient step should be gone
    assert_eq!(harness.handler.ambient_step_id().await, None);

    // The call sequence should show: open(-1), close(-1)
    let calls = harness.step_manager.calls();
    assert!(calls.contains(&StepManagerCall::OpenStep(-1)));
    assert!(calls.contains(&StepManagerCall::CloseStep(-1)));

    // Clean up: complete the exec cycle
    harness
        .handler
        .handle_vm_message(VmMessage::StepStarted { id: 1 })
        .await;
    harness
        .handler
        .handle_vm_message(VmMessage::StepCompleted {
            id: 1,
            exit_code: 0,
        })
        .await;
    advance_and_settle(Duration::from_millis(100)).await;
}

/// Sequential exec commands each get their own step.
#[tokio::test(start_paused = true)]
async fn sequential_exec_commands() {
    let mut harness = default_harness();

    // First command
    run_exec_through_completed(&harness, 1, "echo one", &[], 0).await;
    advance_and_settle(Duration::from_millis(100)).await;
    drain_events(&mut harness.events);

    // Second command
    run_exec_through_completed(&harness, 2, "echo two", &[], 0).await;
    advance_and_settle(Duration::from_millis(100)).await;
    drain_events(&mut harness.events);

    // Both steps opened and closed independently
    assert_eq!(
        harness.step_manager.calls(),
        vec![
            StepManagerCall::OpenStep(1),
            StepManagerCall::CloseStep(1),
            StepManagerCall::OpenStep(2),
            StepManagerCall::CloseStep(2),
        ]
    );
}
