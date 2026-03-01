use std::sync::Arc;

use codeagent_common::StepId;
use codeagent_control::StepManager;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_interceptor::write_interceptor::WriteInterceptor;

/// Adapts `UndoInterceptor` to the `StepManager` trait expected by
/// `ControlChannelHandler`. This is a thin delegation wrapper — the
/// method signatures already match.
pub struct StepManagerAdapter {
    interceptor: Arc<UndoInterceptor>,
}

impl StepManagerAdapter {
    pub fn new(interceptor: Arc<UndoInterceptor>) -> Self {
        Self { interceptor }
    }
}

impl StepManager for StepManagerAdapter {
    fn open_step(&self, id: StepId) -> codeagent_common::Result<()> {
        self.interceptor.open_step(id)
    }

    fn close_step(&self, id: StepId) -> codeagent_common::Result<Vec<StepId>> {
        self.interceptor.close_step(id)
    }

    fn current_step(&self) -> Option<StepId> {
        self.interceptor.current_step()
    }
}
