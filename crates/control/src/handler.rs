use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify, mpsc};

use codeagent_common::StepId;

use crate::in_flight::InFlightTracker;
use crate::protocol::{HostMessage, OutputStream, VmMessage};
use crate::state_machine::{ControlChannelState, ControlEvent};

/// Abstraction over the undo interceptor's step lifecycle.
///
/// The control channel handler uses this trait to open and close undo steps
/// in response to protocol events. In production, `UndoInterceptor` implements
/// this. In tests, a mock records calls for assertion.
pub trait StepManager: Send + Sync {
    fn open_step(&self, id: StepId) -> codeagent_common::Result<()>;
    fn close_step(&self, id: StepId) -> codeagent_common::Result<Vec<StepId>>;
    fn current_step(&self) -> Option<StepId>;
}

/// Configuration for quiescence and ambient step timeouts.
#[derive(Debug, Clone)]
pub struct QuiescenceConfig {
    /// After `step_completed`, wait this long with no filesystem activity
    /// before closing the step. Default: 100ms.
    pub idle_timeout: Duration,
    /// Maximum time to wait for in-flight operations to drain after
    /// `step_completed`. Prevents indefinite hangs. Default: 2s.
    pub max_timeout: Duration,
    /// Inactivity timeout for ambient steps. An ambient step auto-closes
    /// after this duration with no new writes. Default: 5s.
    pub ambient_inactivity_timeout: Duration,
}

impl Default for QuiescenceConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_millis(100),
            max_timeout: Duration::from_secs(2),
            ambient_inactivity_timeout: Duration::from_secs(5),
        }
    }
}

/// Events emitted by the [`ControlChannelHandler`] for the orchestration
/// layer (STDIO API, MCP server) to consume.
#[derive(Debug, Clone, PartialEq)]
pub enum HandlerEvent {
    /// A command step started executing.
    StepStarted { step_id: StepId, command: String },
    /// Terminal output from a running command.
    Output {
        step_id: StepId,
        stream: OutputStream,
        data: String,
    },
    /// A command step completed and the undo step has been closed
    /// (quiescence window elapsed).
    StepCompleted {
        step_id: StepId,
        exit_code: i32,
        cancelled: bool,
        evicted_steps: Vec<StepId>,
    },
    /// An ambient step was opened due to a write outside a command step.
    AmbientStepOpened { step_id: StepId },
    /// An ambient step was auto-closed after inactivity.
    AmbientStepClosed {
        step_id: StepId,
        evicted_steps: Vec<StepId>,
    },
    /// A protocol violation was detected but the channel remains operational.
    ProtocolError { error: String },
}

/// Internal mutable state protected by a tokio Mutex.
struct HandlerState {
    protocol: ControlChannelState,
    /// Counter for ambient step IDs (decrements: -1, -2, -3, ...)
    next_ambient_id: StepId,
    /// The currently active command step (between step_started and step close).
    active_command_step: Option<StepId>,
    /// Whether we are in a quiescence window (between step_completed and
    /// the undo step actually closing).
    in_quiescence: bool,
    /// The currently open ambient step, if any.
    ambient_step_id: Option<StepId>,
}

/// Integrates the control channel protocol state machine with the undo
/// interceptor's step lifecycle.
///
/// Responsibilities:
/// - Processes VM messages through the state machine
/// - Opens/closes undo steps at the right times
/// - Implements quiescence windows after `step_completed`
/// - Manages ambient step lifecycle for writes outside command steps
pub struct ControlChannelHandler<S: StepManager> {
    step_manager: Arc<S>,
    in_flight: InFlightTracker,
    config: QuiescenceConfig,
    state: Arc<Mutex<HandlerState>>,
    event_sender: mpsc::UnboundedSender<HandlerEvent>,
    /// Notifies the ambient timeout task that a new write arrived,
    /// so it should reset its deadline.
    ambient_reset_notify: Arc<Notify>,
}

impl<S: StepManager + 'static> ControlChannelHandler<S> {
    /// Create a new handler.
    ///
    /// Returns the handler and a receiver for [`HandlerEvent`]s.
    pub fn new(
        step_manager: Arc<S>,
        in_flight: InFlightTracker,
        config: QuiescenceConfig,
    ) -> (Self, mpsc::UnboundedReceiver<HandlerEvent>) {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let handler = Self {
            step_manager,
            in_flight,
            config,
            state: Arc::new(Mutex::new(HandlerState {
                protocol: ControlChannelState::new(),
                next_ambient_id: -1,
                active_command_step: None,
                in_quiescence: false,
                ambient_step_id: None,
            })),
            event_sender,
            ambient_reset_notify: Arc::new(Notify::new()),
        };
        (handler, event_receiver)
    }

    /// Register a command to be sent to the VM.
    ///
    /// Returns the [`HostMessage::Exec`] for the caller to serialize and send
    /// over the control channel transport.
    pub async fn send_exec(
        &self,
        id: u64,
        command: String,
        env: Option<HashMap<String, String>>,
        cwd: Option<String>,
    ) -> HostMessage {
        // If an ambient step is open, close it first
        self.close_ambient_step_if_open().await;

        let mut state = self.state.lock().await;
        state.protocol.command_sent(id, command.clone());

        HostMessage::Exec {
            id,
            command,
            env,
            cwd,
        }
    }

    /// Process a VM message through the state machine and perform
    /// step lifecycle actions.
    pub async fn handle_vm_message(&self, msg: VmMessage) {
        let event = {
            let mut state = self.state.lock().await;
            state.protocol.process_vm_message(msg)
        };

        match event {
            ControlEvent::StepStarted { id, command } => {
                let step_id = id as StepId;

                // Close any open ambient step first
                self.close_ambient_step_if_open().await;

                if let Err(error) = self.step_manager.open_step(step_id) {
                    self.emit(HandlerEvent::ProtocolError {
                        error: format!("failed to open step {step_id}: {error}"),
                    });
                    return;
                }

                {
                    let mut state = self.state.lock().await;
                    state.active_command_step = Some(step_id);
                }

                self.emit(HandlerEvent::StepStarted {
                    step_id,
                    command,
                });
            }
            ControlEvent::Output { id, stream, data } => {
                self.emit(HandlerEvent::Output {
                    step_id: id as StepId,
                    stream,
                    data,
                });
            }
            ControlEvent::StepCompleted {
                id,
                exit_code,
                cancelled,
            } => {
                let step_id = id as StepId;

                {
                    let mut state = self.state.lock().await;
                    state.active_command_step = None;
                    state.in_quiescence = true;
                }

                self.spawn_quiescence_task(step_id, exit_code, cancelled);
            }
            ControlEvent::ProtocolError { error } => {
                self.emit(HandlerEvent::ProtocolError { error });
            }
        }
    }

    /// Notify the handler that a filesystem write occurred.
    ///
    /// If no command step or quiescence window is active, this opens
    /// (or extends) an ambient step.
    pub async fn notify_fs_write(&self) {
        let should_open_ambient = {
            let state = self.state.lock().await;
            state.active_command_step.is_none()
                && !state.in_quiescence
                && state.ambient_step_id.is_none()
        };

        let should_reset_ambient = {
            let state = self.state.lock().await;
            state.active_command_step.is_none()
                && !state.in_quiescence
                && state.ambient_step_id.is_some()
        };

        if should_open_ambient {
            self.open_ambient_step().await;
        } else if should_reset_ambient {
            // Reset the ambient inactivity timer
            self.ambient_reset_notify.notify_waiters();
        }
    }

    /// Cancel a pending or active command.
    pub async fn cancel(&self, id: u64) {
        let result = {
            let mut state = self.state.lock().await;
            state.protocol.cancel_command(id)
        };

        match result {
            Ok(event) => match event {
                ControlEvent::StepCompleted {
                    id,
                    exit_code,
                    cancelled,
                } => {
                    let step_id = id as StepId;
                    self.emit(HandlerEvent::StepCompleted {
                        step_id,
                        exit_code,
                        cancelled,
                        evicted_steps: vec![],
                    });
                }
                ControlEvent::ProtocolError { error } => {
                    self.emit(HandlerEvent::ProtocolError { error });
                }
                _ => {}
            },
            Err(error) => {
                self.emit(HandlerEvent::ProtocolError {
                    error: error.to_string(),
                });
            }
        }
    }

    /// Returns `true` if the handler is currently in a quiescence window.
    pub async fn in_quiescence(&self) -> bool {
        self.state.lock().await.in_quiescence
    }

    /// Returns the currently open ambient step ID, if any.
    pub async fn ambient_step_id(&self) -> Option<StepId> {
        self.state.lock().await.ambient_step_id
    }

    fn spawn_quiescence_task(&self, step_id: StepId, exit_code: i32, cancelled: bool) {
        let step_manager = Arc::clone(&self.step_manager);
        let in_flight = self.in_flight.clone();
        let config = self.config.clone();
        let state = Arc::clone(&self.state);
        let event_sender = self.event_sender.clone();

        tokio::spawn(async move {
            let max_deadline = tokio::time::Instant::now() + config.max_timeout;

            loop {
                let now = tokio::time::Instant::now();
                let remaining = max_deadline.saturating_duration_since(now);
                if remaining.is_zero() {
                    break;
                }

                // Wait for in-flight operations to drain
                if !in_flight.wait_for_drain(remaining).await {
                    break; // max timeout reached
                }

                // In-flight is zero — wait for idle period
                let now = tokio::time::Instant::now();
                let remaining = max_deadline.saturating_duration_since(now);
                if remaining.is_zero() {
                    break;
                }
                let idle_wait = remaining.min(config.idle_timeout);
                tokio::time::sleep(idle_wait).await;

                // Check if still drained after idle period
                if in_flight.count() == 0 {
                    break; // quiescence achieved
                }
                // A new write started during idle — loop again
            }

            // Close the step
            let evicted = step_manager.close_step(step_id).unwrap_or_default();

            {
                let mut state = state.lock().await;
                state.in_quiescence = false;
            }

            let _ = event_sender.send(HandlerEvent::StepCompleted {
                step_id,
                exit_code,
                cancelled,
                evicted_steps: evicted,
            });
        });
    }

    async fn open_ambient_step(&self) {
        let ambient_id = {
            let mut state = self.state.lock().await;
            let id = state.next_ambient_id;
            state.next_ambient_id -= 1;
            state.ambient_step_id = Some(id);
            id
        };

        if let Err(error) = self.step_manager.open_step(ambient_id) {
            self.emit(HandlerEvent::ProtocolError {
                error: format!("failed to open ambient step {ambient_id}: {error}"),
            });
            let mut state = self.state.lock().await;
            state.ambient_step_id = None;
            return;
        }

        self.emit(HandlerEvent::AmbientStepOpened {
            step_id: ambient_id,
        });

        self.spawn_ambient_timeout_task(ambient_id);
    }

    fn spawn_ambient_timeout_task(&self, ambient_id: StepId) {
        let step_manager = Arc::clone(&self.step_manager);
        let config = self.config.clone();
        let state = Arc::clone(&self.state);
        let event_sender = self.event_sender.clone();
        let reset_notify = Arc::clone(&self.ambient_reset_notify);

        tokio::spawn(async move {
            let mut deadline =
                tokio::time::Instant::now() + config.ambient_inactivity_timeout;

            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {
                        // Check that this ambient step is still the active one
                        let mut state = state.lock().await;
                        if state.ambient_step_id != Some(ambient_id) {
                            return; // ambient step was already closed
                        }
                        state.ambient_step_id = None;
                        drop(state);

                        let evicted = step_manager
                            .close_step(ambient_id)
                            .unwrap_or_default();

                        let _ = event_sender.send(HandlerEvent::AmbientStepClosed {
                            step_id: ambient_id,
                            evicted_steps: evicted,
                        });
                        return;
                    }
                    _ = reset_notify.notified() => {
                        // Check that this ambient step is still the active one
                        let state = state.lock().await;
                        if state.ambient_step_id != Some(ambient_id) {
                            return; // ambient step was closed (e.g., by exec)
                        }
                        drop(state);

                        // Reset deadline and loop
                        deadline = tokio::time::Instant::now()
                            + config.ambient_inactivity_timeout;
                    }
                }
            }
        });
    }

    async fn close_ambient_step_if_open(&self) {
        let ambient_id = {
            let mut state = self.state.lock().await;
            state.ambient_step_id.take()
        };

        if let Some(ambient_id) = ambient_id {
            // Notify the ambient timeout task so it exits
            self.ambient_reset_notify.notify_waiters();

            let evicted = self
                .step_manager
                .close_step(ambient_id)
                .unwrap_or_default();

            self.emit(HandlerEvent::AmbientStepClosed {
                step_id: ambient_id,
                evicted_steps: evicted,
            });
        }
    }

    fn emit(&self, event: HandlerEvent) {
        let _ = self.event_sender.send(event);
    }
}
