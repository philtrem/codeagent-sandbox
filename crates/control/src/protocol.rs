use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Messages sent from host to VM over the control channel.
///
/// The host sends these to instruct the VM-side shim to execute commands,
/// cancel running commands, or notify about rollbacks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum HostMessage {
    /// Execute a shell command inside the VM.
    #[serde(rename = "exec")]
    Exec {
        id: u64,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<HashMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },

    /// Cancel a running command (SIGTERM → SIGKILL).
    #[serde(rename = "cancel")]
    Cancel { id: u64 },

    /// Inform the VM-side agent that a rollback occurred.
    #[serde(rename = "rollback_notify")]
    RollbackNotify { step_id: u64 },
}

/// Messages sent from VM to host over the control channel.
///
/// The VM-side shim sends these to report step boundaries and terminal output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum VmMessage {
    /// Command execution has begun — host should open a new undo step.
    #[serde(rename = "step_started")]
    StepStarted { id: u64 },

    /// Terminal output chunk from a running command.
    #[serde(rename = "output")]
    Output {
        id: u64,
        stream: OutputStream,
        data: String,
    },

    /// Command finished — host should close the current undo step.
    #[serde(rename = "step_completed")]
    StepCompleted { id: u64, exit_code: i32 },
}

/// Which output stream a terminal output chunk came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_message_exec_round_trip() {
        let msg = HostMessage::Exec {
            id: 42,
            command: "npm install".to_string(),
            env: None,
            cwd: Some("/mnt/working".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HostMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn host_message_exec_with_env_round_trip() {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        let msg = HostMessage::Exec {
            id: 1,
            command: "echo $PATH".to_string(),
            env: Some(env),
            cwd: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HostMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn host_message_cancel_round_trip() {
        let msg = HostMessage::Cancel { id: 42 };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HostMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn host_message_rollback_notify_round_trip() {
        let msg = HostMessage::RollbackNotify { step_id: 5 };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HostMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn vm_message_step_started_round_trip() {
        let msg = VmMessage::StepStarted { id: 42 };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: VmMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn vm_message_output_round_trip() {
        let msg = VmMessage::Output {
            id: 42,
            stream: OutputStream::Stdout,
            data: "added 150 packages in 3s\n".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: VmMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn vm_message_step_completed_round_trip() {
        let msg = VmMessage::StepCompleted {
            id: 42,
            exit_code: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: VmMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn output_stream_serde_lowercase() {
        let stdout_json = serde_json::to_string(&OutputStream::Stdout).unwrap();
        assert_eq!(stdout_json, "\"stdout\"");
        let stderr_json = serde_json::to_string(&OutputStream::Stderr).unwrap();
        assert_eq!(stderr_json, "\"stderr\"");
    }

    #[test]
    fn host_message_exec_matches_spec_format() {
        let json = r#"{"type":"exec","id":42,"command":"npm install","cwd":"/mnt/working"}"#;
        let msg: HostMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            HostMessage::Exec {
                id: 42,
                command: "npm install".to_string(),
                env: None,
                cwd: Some("/mnt/working".to_string()),
            }
        );
    }

    #[test]
    fn vm_message_matches_spec_format() {
        let json = r#"{"type":"step_started","id":42}"#;
        let msg: VmMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg, VmMessage::StepStarted { id: 42 });

        let json =
            r#"{"type":"output","id":42,"stream":"stdout","data":"added 150 packages in 3s\n"}"#;
        let msg: VmMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            VmMessage::Output {
                id: 42,
                stream: OutputStream::Stdout,
                data: "added 150 packages in 3s\n".to_string(),
            }
        );

        let json = r#"{"type":"step_completed","id":42,"exit_code":0}"#;
        let msg: VmMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg, VmMessage::StepCompleted { id: 42, exit_code: 0 });
    }
}
