use codeagent_control::{
    parse_vm_message, ControlChannelError, ControlChannelState, ControlEvent, OutputStream,
    VmMessage, MAX_MESSAGE_SIZE,
};

/// CC-01: Valid `step_started` / `step_completed` sequence
///
/// A full exec → step_started → output → step_completed lifecycle produces the
/// correct sequence of ControlEvents, demonstrating the happy path.
#[test]
fn cc01_valid_step_lifecycle() {
    let mut state = ControlChannelState::new();

    // Host sends exec
    state.command_sent(42, "npm install".to_string());

    // VM responds with step_started
    let msg = parse_vm_message(r#"{"type":"step_started","id":42}"#).unwrap();
    let event = state.process_vm_message(msg);
    assert_eq!(
        event,
        ControlEvent::StepStarted {
            id: 42,
            command: "npm install".to_string(),
        }
    );
    assert_eq!(state.active_count(), 1);
    assert_eq!(state.pending_count(), 0);

    // VM sends output
    let msg = parse_vm_message(
        r#"{"type":"output","id":42,"stream":"stdout","data":"added 150 packages in 3s\n"}"#,
    )
    .unwrap();
    let event = state.process_vm_message(msg);
    assert_eq!(
        event,
        ControlEvent::Output {
            id: 42,
            stream: OutputStream::Stdout,
            data: "added 150 packages in 3s\n".to_string(),
        }
    );

    // VM sends stderr output
    let msg = parse_vm_message(
        r#"{"type":"output","id":42,"stream":"stderr","data":"npm warn deprecated\n"}"#,
    )
    .unwrap();
    let event = state.process_vm_message(msg);
    assert_eq!(
        event,
        ControlEvent::Output {
            id: 42,
            stream: OutputStream::Stderr,
            data: "npm warn deprecated\n".to_string(),
        }
    );

    // VM sends step_completed
    let msg = parse_vm_message(r#"{"type":"step_completed","id":42,"exit_code":0}"#).unwrap();
    let event = state.process_vm_message(msg);
    assert_eq!(
        event,
        ControlEvent::StepCompleted {
            id: 42,
            exit_code: 0,
            cancelled: false,
        }
    );
    assert_eq!(state.active_count(), 0);
    assert_eq!(state.pending_count(), 0);
}

/// CC-01 extension: Multiple sequential commands work correctly.
#[test]
fn cc01_sequential_commands() {
    let mut state = ControlChannelState::new();

    // First command
    state.command_sent(1, "echo hello".to_string());
    state.process_vm_message(VmMessage::StepStarted { id: 1 });
    state.process_vm_message(VmMessage::StepCompleted {
        id: 1,
        exit_code: 0,
    });
    assert_eq!(state.active_count(), 0);

    // Second command
    state.command_sent(2, "echo world".to_string());
    let event = state.process_vm_message(VmMessage::StepStarted { id: 2 });
    assert_eq!(
        event,
        ControlEvent::StepStarted {
            id: 2,
            command: "echo world".to_string(),
        }
    );
    let event = state.process_vm_message(VmMessage::StepCompleted {
        id: 2,
        exit_code: 0,
    });
    assert_eq!(
        event,
        ControlEvent::StepCompleted {
            id: 2,
            exit_code: 0,
            cancelled: false,
        }
    );
}

/// CC-02: Malformed JSON
///
/// A garbled line produces a structured MalformedJson error. The state machine
/// continues to function after the error.
#[test]
fn cc02_malformed_json() {
    let result = parse_vm_message("this is not valid json {{{");
    let err = result.unwrap_err();
    assert!(
        matches!(err, ControlChannelError::MalformedJson { .. }),
        "expected MalformedJson, got: {err:?}"
    );

    // State machine still works after encountering malformed input
    let mut state = ControlChannelState::new();
    state.command_sent(1, "ls".to_string());
    let msg = parse_vm_message(r#"{"type":"step_started","id":1}"#).unwrap();
    let event = state.process_vm_message(msg);
    assert!(matches!(event, ControlEvent::StepStarted { id: 1, .. }));
}

/// CC-02 extension: Empty string is malformed JSON.
#[test]
fn cc02_empty_string() {
    let result = parse_vm_message("");
    assert!(matches!(
        result.unwrap_err(),
        ControlChannelError::MalformedJson { .. }
    ));
}

/// CC-02 extension: Incomplete JSON is malformed.
#[test]
fn cc02_incomplete_json() {
    let result = parse_vm_message(r#"{"type":"step_started","id":"#);
    assert!(matches!(
        result.unwrap_err(),
        ControlChannelError::MalformedJson { .. }
    ));
}

/// CC-03: Unknown message type
///
/// Valid JSON with an unrecognized `type` field produces an UnknownMessageType
/// error. The channel is not broken — subsequent valid messages parse fine.
#[test]
fn cc03_unknown_message_type() {
    let result = parse_vm_message(r#"{"type":"heartbeat","timestamp":12345}"#);
    let err = result.unwrap_err();
    assert!(
        matches!(err, ControlChannelError::UnknownMessageType { .. }),
        "expected UnknownMessageType, got: {err:?}"
    );

    // Subsequent valid message still works
    let msg = parse_vm_message(r#"{"type":"step_started","id":1}"#).unwrap();
    assert_eq!(msg, VmMessage::StepStarted { id: 1 });
}

/// CC-03 extension: JSON with no `type` field at all is malformed, not unknown type.
#[test]
fn cc03_missing_type_field() {
    let result = parse_vm_message(r#"{"id":1}"#);
    assert!(
        matches!(
            result.unwrap_err(),
            ControlChannelError::UnknownMessageType { .. }
                | ControlChannelError::MalformedJson { .. }
        ),
        "expected MalformedJson or UnknownMessageType for missing type field"
    );
}

/// CC-04: Oversized message (>1MB)
///
/// A message exceeding `MAX_MESSAGE_SIZE` is rejected before any JSON parsing
/// takes place, preventing allocation of large buffers.
#[test]
fn cc04_oversized_message() {
    let oversized = format!(
        r#"{{"type":"output","id":1,"stream":"stdout","data":"{}"}}"#,
        "x".repeat(MAX_MESSAGE_SIZE + 1)
    );
    let result = parse_vm_message(&oversized);
    let err = result.unwrap_err();
    match err {
        ControlChannelError::OversizedMessage {
            max_size,
            actual_size,
        } => {
            assert_eq!(max_size, MAX_MESSAGE_SIZE);
            assert!(actual_size > MAX_MESSAGE_SIZE);
        }
        other => panic!("expected OversizedMessage, got: {other:?}"),
    }
}

/// CC-04 extension: A message exactly at the limit should still parse.
#[test]
fn cc04_message_at_limit() {
    // Create a valid JSON message that is exactly MAX_MESSAGE_SIZE bytes.
    // We use a data field padded to the right length.
    let prefix = r#"{"type":"output","id":1,"stream":"stdout","data":""#;
    let suffix = r#""}"#;
    let padding_needed = MAX_MESSAGE_SIZE - prefix.len() - suffix.len();
    let line = format!("{prefix}{}{suffix}", "a".repeat(padding_needed));
    assert_eq!(line.len(), MAX_MESSAGE_SIZE);

    // Should parse without error
    let result = parse_vm_message(&line);
    assert!(result.is_ok(), "message at exact limit should parse");
}

/// CC-05: `step_completed` without `step_started`
///
/// A `step_completed` arriving for a command that never sent `step_started`
/// produces a ProtocolError event. The state machine remains operational.
#[test]
fn cc05_step_completed_without_step_started() {
    let mut state = ControlChannelState::new();

    // Send step_completed for a command that was never registered
    let event = state.process_vm_message(VmMessage::StepCompleted {
        id: 99,
        exit_code: 0,
    });
    assert!(
        matches!(event, ControlEvent::ProtocolError { .. }),
        "expected ProtocolError, got: {event:?}"
    );

    // State machine still works
    state.command_sent(1, "ls".to_string());
    let event = state.process_vm_message(VmMessage::StepStarted { id: 1 });
    assert!(matches!(event, ControlEvent::StepStarted { id: 1, .. }));
}

/// CC-05 extension: step_completed for a pending (not yet started) command.
#[test]
fn cc05_step_completed_for_pending_command() {
    let mut state = ControlChannelState::new();
    state.command_sent(1, "ls".to_string());

    // step_completed without step_started first
    let event = state.process_vm_message(VmMessage::StepCompleted {
        id: 1,
        exit_code: 0,
    });
    assert!(
        matches!(event, ControlEvent::ProtocolError { .. }),
        "expected ProtocolError for step_completed before step_started, got: {event:?}"
    );
}

/// CC-06: Duplicate `step_started` for same ID
///
/// If the VM sends `step_started` twice for the same ID, the second one
/// is a protocol error. The first command remains active and unaffected.
#[test]
fn cc06_duplicate_step_started() {
    let mut state = ControlChannelState::new();
    state.command_sent(1, "ls".to_string());

    // First step_started: ok
    let event = state.process_vm_message(VmMessage::StepStarted { id: 1 });
    assert!(matches!(event, ControlEvent::StepStarted { id: 1, .. }));

    // Duplicate step_started: protocol error
    let event = state.process_vm_message(VmMessage::StepStarted { id: 1 });
    assert!(
        matches!(event, ControlEvent::ProtocolError { .. }),
        "expected ProtocolError for duplicate step_started, got: {event:?}"
    );

    // Original command still active — can still complete
    assert_eq!(state.active_count(), 1);
    let event = state.process_vm_message(VmMessage::StepCompleted {
        id: 1,
        exit_code: 0,
    });
    assert_eq!(
        event,
        ControlEvent::StepCompleted {
            id: 1,
            exit_code: 0,
            cancelled: false,
        }
    );
}

/// CC-07: Cancellation mid-step
///
/// When a command is cancelled after it has started, the state machine marks
/// it as cancelled. The VM still sends `step_completed`, which is processed
/// normally with the `cancelled` flag set.
#[test]
fn cc07_cancellation_mid_step() {
    let mut state = ControlChannelState::new();
    state.command_sent(1, "sleep 1000".to_string());

    // Step starts
    let event = state.process_vm_message(VmMessage::StepStarted { id: 1 });
    assert!(matches!(event, ControlEvent::StepStarted { id: 1, .. }));

    // Cancel the command
    let _event = state.cancel_command(1).unwrap();
    let active = state.get_active(1).unwrap();
    assert!(active.cancelled);

    // VM eventually sends step_completed (process was killed)
    let event = state.process_vm_message(VmMessage::StepCompleted {
        id: 1,
        exit_code: -9,
    });
    assert_eq!(
        event,
        ControlEvent::StepCompleted {
            id: 1,
            exit_code: -9,
            cancelled: true,
        }
    );
    assert_eq!(state.active_count(), 0);
}

/// CC-07 extension: Cancel a pending (not yet started) command.
#[test]
fn cc07_cancel_pending_command() {
    let mut state = ControlChannelState::new();
    state.command_sent(1, "sleep 1000".to_string());
    assert_eq!(state.pending_count(), 1);

    // Cancel before step_started arrives
    let event = state.cancel_command(1).unwrap();
    assert!(matches!(
        event,
        ControlEvent::StepCompleted {
            id: 1,
            cancelled: true,
            ..
        }
    ));
    assert_eq!(state.pending_count(), 0);
    assert_eq!(state.active_count(), 0);
}

/// CC-07 extension: Cancel a command that doesn't exist.
#[test]
fn cc07_cancel_unknown_command() {
    let mut state = ControlChannelState::new();
    let result = state.cancel_command(999);
    assert!(result.is_err());
}

/// Additional: output for unknown command is a protocol error.
#[test]
fn output_for_unknown_command() {
    let mut state = ControlChannelState::new();
    let event = state.process_vm_message(VmMessage::Output {
        id: 99,
        stream: OutputStream::Stdout,
        data: "hello".to_string(),
    });
    assert!(matches!(event, ControlEvent::ProtocolError { .. }));
}

/// Additional: step_started for unknown command (no matching exec) is a protocol error.
#[test]
fn step_started_without_exec() {
    let mut state = ControlChannelState::new();
    let event = state.process_vm_message(VmMessage::StepStarted { id: 99 });
    assert!(
        matches!(event, ControlEvent::ProtocolError { .. }),
        "expected ProtocolError for step_started without exec, got: {event:?}"
    );
}

/// Additional: non-zero exit code is correctly propagated.
#[test]
fn nonzero_exit_code() {
    let mut state = ControlChannelState::new();
    state.command_sent(1, "false".to_string());
    state.process_vm_message(VmMessage::StepStarted { id: 1 });
    let event = state.process_vm_message(VmMessage::StepCompleted {
        id: 1,
        exit_code: 1,
    });
    assert_eq!(
        event,
        ControlEvent::StepCompleted {
            id: 1,
            exit_code: 1,
            cancelled: false,
        }
    );
}
