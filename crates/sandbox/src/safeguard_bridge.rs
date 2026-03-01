use codeagent_common::{SafeguardDecision, SafeguardEvent};
use codeagent_interceptor::safeguard::SafeguardHandler;
use tokio::sync::{mpsc, oneshot};

/// A pending safeguard event awaiting a user decision.
pub struct PendingSafeguard {
    pub event: SafeguardEvent,
    pub responder: oneshot::Sender<SafeguardDecision>,
}

/// Bridges the synchronous `SafeguardHandler` trait (called on the filesystem
/// backend thread) to the async orchestrator world.
///
/// When a safeguard triggers, the bridge:
/// 1. Sends the event + a oneshot response channel to the async orchestrator
/// 2. Blocks the calling (filesystem) thread until a decision arrives
///
/// The orchestrator emits an `Event::SafeguardTriggered` to the STDIO/MCP
/// client, stores the response channel, and sends the decision when
/// `safeguard.confirm` arrives.
pub struct SafeguardBridge {
    sender: mpsc::UnboundedSender<PendingSafeguard>,
}

impl SafeguardBridge {
    pub fn new(sender: mpsc::UnboundedSender<PendingSafeguard>) -> Self {
        Self { sender }
    }
}

impl SafeguardHandler for SafeguardBridge {
    fn on_safeguard_triggered(&self, event: SafeguardEvent) -> SafeguardDecision {
        let (responder, receiver) = oneshot::channel();
        let pending = PendingSafeguard { event, responder };

        if self.sender.send(pending).is_err() {
            return SafeguardDecision::Deny;
        }

        // Block this thread until the orchestrator sends a decision.
        // This is intentional — the filesystem backend thread must wait
        // for user confirmation before proceeding.
        match receiver.blocking_recv() {
            Ok(decision) => decision,
            Err(_) => SafeguardDecision::Deny,
        }
    }
}
