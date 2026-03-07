mod error;
pub mod handler;
pub mod in_flight;
mod parser;
mod protocol;
mod state_machine;

pub use error::ControlChannelError;
pub use codeagent_common::StepManager;
pub use handler::{ControlChannelHandler, HandlerEvent, QuiescenceConfig};
pub use in_flight::InFlightTracker;
pub use parser::{parse_host_message, parse_vm_message, MAX_MESSAGE_SIZE};
pub use protocol::{HostMessage, OutputStream, VmMessage};
pub use state_machine::{ActiveCommand, ControlChannelState, ControlEvent, PendingCommand};
