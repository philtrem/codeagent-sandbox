use std::time::Duration;

/// Environment variable for the path to the agent binary.
pub const AGENT_BIN_ENV: &str = "CODEAGENT_BIN";

/// Default binary name to search for in target/ if CODEAGENT_BIN is not set.
pub const DEFAULT_BINARY_NAME: &str = "codeagent";

/// Maximum time to wait for the agent to respond to session.start (includes VM boot).
pub const SESSION_START_TIMEOUT: Duration = Duration::from_secs(60);

/// Default timeout for a command execution response.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for an undo rollback response.
pub const ROLLBACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for receiving an event.
pub const EVENT_TIMEOUT: Duration = Duration::from_secs(15);

/// Short timeout for negative tests (expecting no response/event).
pub const NO_EVENT_TIMEOUT: Duration = Duration::from_secs(3);

/// Timeout for the agent process to shut down cleanly after session.stop.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
