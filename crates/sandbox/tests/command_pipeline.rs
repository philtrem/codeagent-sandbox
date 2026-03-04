//! Isolated component tests for the MCP `execute_command` pipeline.
//!
//! These tests verify each stage of the pipeline that carries a command from
//! the MCP handler through to completion, targeting the `rm` hang bug where
//! filesystem-mutating commands would block indefinitely.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use codeagent_common::StepId;
use codeagent_control::{
    ControlChannelHandler, HandlerEvent, InFlightTracker, OutputStream, QuiescenceConfig,
    StepManager, VmMessage,
};
use codeagent_sandbox::command_waiter::CommandWaiter;
use codeagent_sandbox::event_bridge::run_event_bridge;

// ---------------------------------------------------------------------------
// MockStepManager — duplicated from control channel integration tests since
// test modules cannot be imported cross-crate.
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

// ===========================================================================
// Test 1: CommandWaiter Condvar Bridge
// ===========================================================================

/// Verify that the CommandWaiter's Condvar-based bridge works: register,
/// append output, mark completed from another thread, and wait_for_completion
/// returns promptly.
#[test]
fn cp_01_command_waiter_happy_path() {
    let waiter = CommandWaiter::new();
    waiter.register(1);

    let waiter_clone = Arc::clone(&waiter);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        waiter_clone.append_output(1, "stdout", "hello\n");
        waiter_clone.mark_completed(1, 0);
    });

    let result = waiter.wait_for_completion(1, Duration::from_secs(5));
    let r = result.expect("should return Some for registered command");
    assert_eq!(r.stdout, "hello\n");
    assert_eq!(r.exit_code, Some(0));
}

/// Verify that wait_for_completion returns a partial result on timeout.
#[test]
fn cp_02_command_waiter_timeout() {
    let waiter = CommandWaiter::new();
    waiter.register(2);

    let result = waiter.wait_for_completion(2, Duration::from_millis(100));
    let r = result.expect("should return Some for registered command");
    assert!(r.exit_code.is_none(), "exit_code should be None on timeout");
}

/// Verify that wait_for_completion returns None for unregistered commands.
#[test]
fn cp_03_command_waiter_unregistered() {
    let waiter = CommandWaiter::new();
    let result = waiter.wait_for_completion(999, Duration::from_millis(50));
    assert!(result.is_none(), "should return None for unregistered command");
}

// ===========================================================================
// Test 2: Event Bridge + CommandWaiter
// ===========================================================================

/// Verify that HandlerEvent::StepCompleted flowing through the event bridge
/// correctly wakes a blocked wait_for_completion call.
#[tokio::test]
async fn cp_04_event_bridge_forwards_completion() {
    let waiter = CommandWaiter::new();
    waiter.register(42);

    let (event_tx, event_rx) = mpsc::unbounded_channel::<HandlerEvent>();
    let (stdio_tx, _stdio_rx) = mpsc::unbounded_channel::<codeagent_stdio::Event>();

    // Spawn the event bridge with the command waiter.
    tokio::spawn(run_event_bridge(event_rx, stdio_tx, Some(waiter.clone())));

    // Send output followed by completion.
    event_tx
        .send(HandlerEvent::Output {
            step_id: 42,
            stream: OutputStream::Stdout,
            data: "removed\n".to_string(),
        })
        .unwrap();
    event_tx
        .send(HandlerEvent::StepCompleted {
            step_id: 42,
            exit_code: 0,
            cancelled: false,
            evicted_steps: vec![],
        })
        .unwrap();

    // Block on a separate thread (mimicking execute_command's block_in_place).
    let waiter_for_blocking = waiter.clone();
    let result = tokio::task::spawn_blocking(move || {
        waiter_for_blocking.wait_for_completion(42, Duration::from_secs(5))
    })
    .await
    .expect("spawn_blocking panicked");

    let r = result.expect("should return Some for registered command");
    assert_eq!(r.stdout, "removed\n");
    assert_eq!(r.exit_code, Some(0));
}

// ===========================================================================
// Test 3: Quiescence + InFlightTracker
// ===========================================================================

/// Helper: run a full exec cycle through the handler (send_exec → StepStarted
/// → StepCompleted) and return the handler events receiver.
async fn run_exec_cycle(
    handler: &ControlChannelHandler<MockStepManager>,
    events: &mut mpsc::UnboundedReceiver<HandlerEvent>,
    command_id: u64,
) {
    let _host_msg = handler
        .send_exec(
            command_id,
            "rm file.txt".to_string(),
            None,
            None,
        )
        .await;

    handler
        .handle_vm_message(VmMessage::StepStarted { id: command_id })
        .await;

    // Drain the StepStarted event.
    if let Some(HandlerEvent::StepStarted { .. }) = events.recv().await {}

    handler
        .handle_vm_message(VmMessage::StepCompleted {
            id: command_id,
            exit_code: 0,
        })
        .await;
}

/// Scenario A: in-flight drains quickly, StepCompleted is emitted after idle
/// timeout.
#[tokio::test(start_paused = true)]
async fn cp_05_quiescence_inflight_drains() {
    let step_manager = Arc::new(MockStepManager::default());
    let in_flight = InFlightTracker::new();
    let config = QuiescenceConfig::default();

    let (handler, mut events) = ControlChannelHandler::new(
        Arc::clone(&step_manager),
        in_flight.clone(),
        config,
    );

    // Run exec cycle: handler enters quiescence after StepCompleted.
    run_exec_cycle(&handler, &mut events, 1).await;

    // Simulate an in-flight P9 operation that started before StepCompleted.
    in_flight.begin_operation();

    // Advance 30ms — quiescence is waiting for drain.
    tokio::time::advance(Duration::from_millis(30)).await;
    tokio::task::yield_now().await;
    assert!(events.try_recv().is_err(), "no event yet while in-flight > 0");

    // P9 operation completes.
    in_flight.end_operation();

    // Advance past idle timeout (100ms default).
    tokio::time::advance(Duration::from_millis(150)).await;
    tokio::task::yield_now().await;

    // Should have StepCompleted now.
    let event = events.recv().await.expect("should receive StepCompleted");
    match event {
        HandlerEvent::StepCompleted {
            step_id,
            exit_code,
            ..
        } => {
            assert_eq!(step_id, 1);
            assert_eq!(exit_code, 0);
        }
        other => panic!("expected StepCompleted, got {other:?}"),
    }

    assert_eq!(
        step_manager.calls(),
        vec![StepManagerCall::OpenStep(1), StepManagerCall::CloseStep(1)]
    );
}

/// Scenario B: in-flight never drains, StepCompleted is still emitted at
/// max_timeout (2s default).
#[tokio::test(start_paused = true)]
async fn cp_06_quiescence_inflight_max_timeout() {
    let step_manager = Arc::new(MockStepManager::default());
    let in_flight = InFlightTracker::new();
    let config = QuiescenceConfig::default();

    let (handler, mut events) = ControlChannelHandler::new(
        Arc::clone(&step_manager),
        in_flight.clone(),
        config,
    );

    run_exec_cycle(&handler, &mut events, 2).await;

    // In-flight operation that never completes.
    in_flight.begin_operation();

    // Advance past max_timeout (2s).
    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::task::yield_now().await;

    let event = events.recv().await.expect("should receive StepCompleted at max_timeout");
    match event {
        HandlerEvent::StepCompleted {
            step_id,
            exit_code,
            ..
        } => {
            assert_eq!(step_id, 2);
            assert_eq!(exit_code, 0);
        }
        other => panic!("expected StepCompleted, got {other:?}"),
    }

    // Clean up.
    in_flight.end_operation();

    assert_eq!(
        step_manager.calls(),
        vec![StepManagerCall::OpenStep(2), StepManagerCall::CloseStep(2)]
    );
}

// ===========================================================================
// Test 5: Full Pipeline Simulation
// ===========================================================================

/// Wire together ControlChannelHandler + event bridge + CommandWaiter and
/// simulate VM messages, verifying the complete execute_command pipeline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cp_07_full_pipeline_simulation() {
    let waiter = CommandWaiter::new();
    let step_manager = Arc::new(MockStepManager::default());
    let in_flight = InFlightTracker::new();
    let config = QuiescenceConfig::default();

    let (handler, handler_events) = ControlChannelHandler::new(
        Arc::clone(&step_manager),
        in_flight.clone(),
        config,
    );

    let (stdio_tx, _stdio_rx) = mpsc::unbounded_channel::<codeagent_stdio::Event>();

    // Spawn the event bridge (this is what the orchestrator does during VM launch).
    tokio::spawn(run_event_bridge(
        handler_events,
        stdio_tx,
        Some(waiter.clone()),
    ));

    // Step 1: Register the command with the waiter (orchestrator does this).
    waiter.register(1);

    // Step 2: Register with handler state machine (orchestrator does this).
    let _host_msg = handler
        .send_exec(1, "rm -f /tmp/file".to_string(), None, None)
        .await;

    // Step 3: Simulate VM responses (control reader task does this).
    handler
        .handle_vm_message(VmMessage::StepStarted { id: 1 })
        .await;

    handler
        .handle_vm_message(VmMessage::Output {
            id: 1,
            stream: OutputStream::Stdout,
            data: "".to_string(),
        })
        .await;

    // Simulate brief in-flight from P9 operation.
    in_flight.begin_operation();
    let in_flight_clone = in_flight.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        in_flight_clone.end_operation();
    });

    handler
        .handle_vm_message(VmMessage::StepCompleted {
            id: 1,
            exit_code: 0,
        })
        .await;

    // Step 4: Block on completion (orchestrator does this with block_in_place).
    let waiter_for_blocking = waiter.clone();
    let result = tokio::task::spawn_blocking(move || {
        waiter_for_blocking.wait_for_completion(1, Duration::from_secs(10))
    })
    .await
    .expect("spawn_blocking panicked");

    let r = result.expect("should return Some for registered command");
    assert_eq!(r.exit_code, Some(0));

    // Give the quiescence task time to close_step.
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        step_manager.calls(),
        vec![StepManagerCall::OpenStep(1), StepManagerCall::CloseStep(1)]
    );
}
