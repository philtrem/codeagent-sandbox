use codeagent_control::HandlerEvent;
use codeagent_control::OutputStream;
use codeagent_stdio::Event;
use tokio::sync::mpsc;

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

/// Reads `HandlerEvent`s from the control channel handler and forwards
/// translated events to the STDIO event stream. Run as a spawned tokio task.
pub async fn run_event_bridge(
    mut handler_events: mpsc::UnboundedReceiver<HandlerEvent>,
    stdio_event_sender: mpsc::UnboundedSender<Event>,
) {
    while let Some(event) = handler_events.recv().await {
        if let Some(stdio_event) = translate_handler_event(&event) {
            let _ = stdio_event_sender.send(stdio_event);
        }
    }
}
