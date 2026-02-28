use std::collections::HashMap;

use crate::error::ControlChannelError;
use crate::protocol::{OutputStream, VmMessage};

/// A command that has been sent to the VM but hasn't started executing yet.
#[derive(Debug, Clone)]
pub struct PendingCommand {
    pub id: u64,
    pub command: String,
}

/// A command that is currently executing inside the VM.
#[derive(Debug, Clone)]
pub struct ActiveCommand {
    pub id: u64,
    pub command: String,
    /// Whether this command has been cancelled via a `cancel` message.
    pub cancelled: bool,
}

/// Events emitted by the state machine for the caller to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlEvent {
    /// A step has started — the caller should open an undo step.
    StepStarted { id: u64, command: String },
    /// Terminal output received from a running command.
    Output {
        id: u64,
        stream: OutputStream,
        data: String,
    },
    /// A step has completed — the caller should close the undo step.
    StepCompleted {
        id: u64,
        exit_code: i32,
        cancelled: bool,
    },
    /// A protocol violation was detected. The channel remains operational,
    /// but the caller should log this error.
    ProtocolError { error: String },
}

/// Tracks the lifecycle of commands on the control channel.
///
/// Validates message sequences and detects protocol violations such as
/// duplicate `step_started`, `step_completed` without `step_started`, etc.
/// Protocol errors do not break the state machine — it continues processing
/// subsequent messages.
#[derive(Debug, Default)]
pub struct ControlChannelState {
    /// Commands sent via `exec` that haven't received `step_started` yet.
    pending: HashMap<u64, PendingCommand>,
    /// Commands that are actively executing (between `step_started` and `step_completed`).
    active: HashMap<u64, ActiveCommand>,
}

impl ControlChannelState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a command that was sent to the VM via `exec`.
    ///
    /// This must be called when the host sends an `exec` message, so the
    /// state machine knows to expect a corresponding `step_started`.
    pub fn command_sent(&mut self, id: u64, command: String) {
        self.pending.insert(id, PendingCommand { id, command });
    }

    /// Mark a command as cancelled.
    ///
    /// If the command is pending (not yet started), it is removed immediately.
    /// If the command is active, it is marked as cancelled — the state machine
    /// will still accept `step_completed` to finalize cleanup.
    pub fn cancel_command(&mut self, id: u64) -> Result<ControlEvent, ControlChannelError> {
        if self.pending.remove(&id).is_some() {
            return Ok(ControlEvent::StepCompleted {
                id,
                exit_code: -1,
                cancelled: true,
            });
        }

        if let Some(active) = self.active.get_mut(&id) {
            active.cancelled = true;
            return Ok(ControlEvent::ProtocolError {
                error: format!("command {id} cancel sent; awaiting step_completed from VM"),
            });
        }

        Err(ControlChannelError::CancelUnknownCommand { id })
    }

    /// Process a VM→host message and return the corresponding event.
    ///
    /// Protocol violations produce `ControlEvent::ProtocolError` rather than
    /// hard errors, keeping the channel operational (CC-02, CC-05, CC-06).
    pub fn process_vm_message(&mut self, msg: VmMessage) -> ControlEvent {
        match msg {
            VmMessage::StepStarted { id } => self.handle_step_started(id),
            VmMessage::Output { id, stream, data } => self.handle_output(id, stream, data),
            VmMessage::StepCompleted { id, exit_code } => {
                self.handle_step_completed(id, exit_code)
            }
        }
    }

    /// Returns the number of pending commands (sent but not yet started).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Returns the number of active commands (started but not yet completed).
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Returns a reference to an active command by ID, if it exists.
    pub fn get_active(&self, id: u64) -> Option<&ActiveCommand> {
        self.active.get(&id)
    }

    fn handle_step_started(&mut self, id: u64) -> ControlEvent {
        // Check for duplicate step_started (CC-06)
        if self.active.contains_key(&id) {
            return ControlEvent::ProtocolError {
                error: ControlChannelError::DuplicateStepStarted { id }.to_string(),
            };
        }

        // Look for matching pending command
        if let Some(pending) = self.pending.remove(&id) {
            self.active.insert(
                id,
                ActiveCommand {
                    id,
                    command: pending.command.clone(),
                    cancelled: false,
                },
            );
            ControlEvent::StepStarted {
                id,
                command: pending.command,
            }
        } else {
            ControlEvent::ProtocolError {
                error: ControlChannelError::UnexpectedStepStarted { id }.to_string(),
            }
        }
    }

    fn handle_output(&self, id: u64, stream: OutputStream, data: String) -> ControlEvent {
        if self.active.contains_key(&id) {
            ControlEvent::Output { id, stream, data }
        } else {
            ControlEvent::ProtocolError {
                error: ControlChannelError::OutputForUnknownCommand { id }.to_string(),
            }
        }
    }

    fn handle_step_completed(&mut self, id: u64, exit_code: i32) -> ControlEvent {
        if let Some(active) = self.active.remove(&id) {
            ControlEvent::StepCompleted {
                id,
                exit_code,
                cancelled: active.cancelled,
            }
        } else {
            ControlEvent::ProtocolError {
                error: ControlChannelError::UnexpectedStepCompleted { id }.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_empty() {
        let state = ControlChannelState::new();
        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.active_count(), 0);
    }

    #[test]
    fn command_sent_adds_to_pending() {
        let mut state = ControlChannelState::new();
        state.command_sent(1, "ls".to_string());
        assert_eq!(state.pending_count(), 1);
        assert_eq!(state.active_count(), 0);
    }

    #[test]
    fn step_started_moves_pending_to_active() {
        let mut state = ControlChannelState::new();
        state.command_sent(1, "ls".to_string());

        let event = state.process_vm_message(VmMessage::StepStarted { id: 1 });
        assert_eq!(
            event,
            ControlEvent::StepStarted {
                id: 1,
                command: "ls".to_string()
            }
        );
        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.active_count(), 1);
    }

    #[test]
    fn step_completed_removes_active() {
        let mut state = ControlChannelState::new();
        state.command_sent(1, "ls".to_string());
        state.process_vm_message(VmMessage::StepStarted { id: 1 });

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
        assert_eq!(state.active_count(), 0);
    }

    #[test]
    fn output_for_active_command() {
        let mut state = ControlChannelState::new();
        state.command_sent(1, "echo hi".to_string());
        state.process_vm_message(VmMessage::StepStarted { id: 1 });

        let event = state.process_vm_message(VmMessage::Output {
            id: 1,
            stream: OutputStream::Stdout,
            data: "hi\n".to_string(),
        });
        assert_eq!(
            event,
            ControlEvent::Output {
                id: 1,
                stream: OutputStream::Stdout,
                data: "hi\n".to_string(),
            }
        );
    }

    #[test]
    fn cancel_pending_command_removes_it() {
        let mut state = ControlChannelState::new();
        state.command_sent(1, "sleep 100".to_string());

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
    }

    #[test]
    fn cancel_active_command_marks_cancelled() {
        let mut state = ControlChannelState::new();
        state.command_sent(1, "sleep 100".to_string());
        state.process_vm_message(VmMessage::StepStarted { id: 1 });

        let _event = state.cancel_command(1).unwrap();
        let active = state.get_active(1).unwrap();
        assert!(active.cancelled);
    }

    #[test]
    fn cancel_unknown_command_returns_error() {
        let mut state = ControlChannelState::new();
        let result = state.cancel_command(999);
        assert!(result.is_err());
    }
}
