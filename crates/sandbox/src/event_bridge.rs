use std::sync::Arc;

use codeagent_control::HandlerEvent;
use codeagent_control::OutputStream;
use codeagent_stdio::Event;
use tokio::sync::mpsc;

use crate::command_waiter::CommandWaiter;

/// Translates a `HandlerEvent` from the control channel into a STDIO `Event`.
pub fn translate_handler_event(event: &HandlerEvent) -> Option<Event> {
    match event {
        HandlerEvent::Output {
            step_id: _,
            stream,
            data,
        } => {
            let stream_name = match stream {
                OutputStream::Stdout => "stdout",
                OutputStream::Stderr => "stderr",
            };
            Some(Event::TerminalOutput {
                stream: stream_name.to_string(),
                data: data.clone(),
            })
        }
        HandlerEvent::StepCompleted {
            step_id,
            exit_code,
            evicted_steps: _,
            cancelled: _,
        } => Some(Event::StepCompleted {
            step_id: *step_id,
            affected_paths: vec![],
            exit_code: *exit_code,
        }),
        HandlerEvent::ProtocolError { error } => Some(Event::Error {
            code: "control_channel_error".to_string(),
            message: error.clone(),
        }),
        // Ambient step events are internal bookkeeping, not surfaced to the client.
        HandlerEvent::StepStarted { .. }
        | HandlerEvent::AmbientStepOpened { .. }
        | HandlerEvent::AmbientStepClosed { .. } => None,
    }
}

/// Forward a `HandlerEvent` to the `CommandWaiter` so that synchronous
/// callers (MCP `execute_command`) can collect output and wait for completion.
fn forward_to_command_waiter(event: &HandlerEvent, waiter: &CommandWaiter) {
    match event {
        HandlerEvent::Output {
            step_id,
            stream,
            data,
        } => {
            let stream_name = match stream {
                OutputStream::Stdout => "stdout",
                OutputStream::Stderr => "stderr",
            };
            waiter.append_output(*step_id as u64, stream_name, data);
        }
        HandlerEvent::StepCompleted {
            step_id,
            exit_code,
            ..
        } => {
            waiter.mark_completed(*step_id as u64, *exit_code);
        }
        _ => {}
    }
}

/// Reads `HandlerEvent`s from the control channel handler and forwards
/// translated events to the STDIO event stream. Run as a spawned tokio task.
///
/// When a `CommandWaiter` is provided, command output and completion events
/// are also forwarded to it for synchronous MCP callers.
pub async fn run_event_bridge(
    mut handler_events: mpsc::UnboundedReceiver<HandlerEvent>,
    stdio_event_sender: mpsc::UnboundedSender<Event>,
    command_waiter: Option<Arc<CommandWaiter>>,
) {
    while let Some(event) = handler_events.recv().await {
        if let Some(waiter) = &command_waiter {
            forward_to_command_waiter(&event, waiter);
        }
        if let Some(stdio_event) = translate_handler_event(&event) {
            let _ = stdio_event_sender.send(stdio_event);
        }
    }
}
