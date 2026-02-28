use std::path::PathBuf;

use crate::error::StdioError;
use crate::path_validation::validate_path;
use crate::protocol::{
    AgentExecutePayload, AgentPromptPayload, FsListPayload, FsReadPayload, Request,
    ResponseEnvelope, SafeguardConfirmPayload, SafeguardConfigurePayload, SessionStartPayload,
    UndoConfigurePayload, UndoHistoryPayload, UndoRollbackPayload,
};
use crate::version::{MAX_SUPPORTED_VERSION, MIN_SUPPORTED_VERSION};

/// Trait abstracting the handling of parsed STDIO API requests.
///
/// Each method receives the typed payload and returns either a success payload
/// or a `StdioError`. For TDD Step 10, a `StubHandler` implements this trait
/// with minimal canned responses. Real implementations will be added in later
/// TDD steps.
pub trait RequestHandler: Send + Sync {
    fn session_start(
        &self,
        payload: SessionStartPayload,
    ) -> Result<serde_json::Value, StdioError>;
    fn session_stop(&self) -> Result<serde_json::Value, StdioError>;
    fn session_reset(&self) -> Result<serde_json::Value, StdioError>;
    fn session_status(&self) -> Result<serde_json::Value, StdioError>;
    fn undo_rollback(
        &self,
        payload: UndoRollbackPayload,
    ) -> Result<serde_json::Value, StdioError>;
    fn undo_history(&self, payload: UndoHistoryPayload)
        -> Result<serde_json::Value, StdioError>;
    fn undo_configure(
        &self,
        payload: UndoConfigurePayload,
    ) -> Result<serde_json::Value, StdioError>;
    fn undo_discard(&self) -> Result<serde_json::Value, StdioError>;
    fn agent_execute(
        &self,
        payload: AgentExecutePayload,
    ) -> Result<serde_json::Value, StdioError>;
    fn agent_prompt(
        &self,
        payload: AgentPromptPayload,
    ) -> Result<serde_json::Value, StdioError>;
    fn fs_list(&self, payload: FsListPayload) -> Result<serde_json::Value, StdioError>;
    fn fs_read(&self, payload: FsReadPayload) -> Result<serde_json::Value, StdioError>;
    fn fs_status(&self) -> Result<serde_json::Value, StdioError>;
    fn safeguard_configure(
        &self,
        payload: SafeguardConfigurePayload,
    ) -> Result<serde_json::Value, StdioError>;
    fn safeguard_confirm(
        &self,
        payload: SafeguardConfirmPayload,
    ) -> Result<serde_json::Value, StdioError>;
}

/// Routes parsed requests to a `RequestHandler`, performing path validation
/// for filesystem operations and protocol version checks for `session.start`.
pub struct Router {
    root_dir: PathBuf,
    handler: Box<dyn RequestHandler>,
}

impl Router {
    pub fn new(root_dir: PathBuf, handler: Box<dyn RequestHandler>) -> Self {
        Self { root_dir, handler }
    }

    /// Dispatch a parsed request, returning a response envelope.
    pub fn dispatch(&self, request: Request) -> ResponseEnvelope {
        let request_id = request.request_id().to_string();
        let result = self.dispatch_inner(request);
        match result {
            Ok(payload) => ResponseEnvelope::ok(request_id, payload),
            Err(error) => ResponseEnvelope::error(request_id, error.to_error_detail()),
        }
    }

    fn dispatch_inner(
        &self,
        request: Request,
    ) -> Result<Option<serde_json::Value>, StdioError> {
        match request {
            Request::SessionStart { payload, .. } => {
                if let Some(version) = payload.protocol_version {
                    if version < MIN_SUPPORTED_VERSION || version > MAX_SUPPORTED_VERSION {
                        return Err(StdioError::UnsupportedProtocolVersion {
                            version,
                            min: MIN_SUPPORTED_VERSION,
                            max: MAX_SUPPORTED_VERSION,
                        });
                    }
                }
                self.handler.session_start(payload).map(Some)
            }
            Request::SessionStop { .. } => self.handler.session_stop().map(Some),
            Request::SessionReset { .. } => self.handler.session_reset().map(Some),
            Request::SessionStatus { .. } => self.handler.session_status().map(Some),

            Request::UndoRollback { payload, .. } => {
                self.handler.undo_rollback(payload).map(Some)
            }
            Request::UndoHistory { payload, .. } => {
                self.handler.undo_history(payload).map(Some)
            }
            Request::UndoConfigure { payload, .. } => {
                self.handler.undo_configure(payload).map(Some)
            }
            Request::UndoDiscard { .. } => self.handler.undo_discard().map(Some),

            Request::AgentExecute { payload, .. } => {
                self.handler.agent_execute(payload).map(Some)
            }
            Request::AgentPrompt { payload, .. } => {
                self.handler.agent_prompt(payload).map(Some)
            }

            Request::FsList { payload, .. } => {
                validate_path(&payload.path, &self.root_dir)?;
                self.handler.fs_list(payload).map(Some)
            }
            Request::FsRead { payload, .. } => {
                validate_path(&payload.path, &self.root_dir)?;
                self.handler.fs_read(payload).map(Some)
            }
            Request::FsStatus { .. } => self.handler.fs_status().map(Some),

            Request::SafeguardConfigure { payload, .. } => {
                self.handler.safeguard_configure(payload).map(Some)
            }
            Request::SafeguardConfirm { payload, .. } => {
                self.handler.safeguard_confirm(payload).map(Some)
            }
        }
    }
}
