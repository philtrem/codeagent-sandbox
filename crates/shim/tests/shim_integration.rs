//! Integration tests for the VM-side shim.
//!
//! Tests use `tokio::io::duplex()` to create in-process channels,
//! spawn the shim's `run()` on one end, and send/receive control
//! channel messages on the other.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use codeagent_control::{HostMessage, VmMessage};

/// Send a `HostMessage` as a JSON Line to the writer.
async fn send_message<W: AsyncWriteExt + Unpin>(writer: &mut W, msg: &HostMessage) {
    let json = serde_json::to_string(msg).unwrap();
    writer.write_all(json.as_bytes()).await.unwrap();
    writer.write_all(b"\n").await.unwrap();
    writer.flush().await.unwrap();
}

/// Read one `VmMessage` JSON Line from the reader.
async fn recv_message(
    lines: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
) -> VmMessage {
    let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("timed out waiting for VmMessage")
        .expect("I/O error reading VmMessage")
        .expect("unexpected EOF");
    serde_json::from_str(&line).unwrap_or_else(|err| panic!("bad VmMessage JSON: {err}\nline: {line}"))
}

/// Collect all VmMessages for a given command ID until StepCompleted.
async fn collect_until_completed(
    lines: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    expected_id: u64,
) -> (Vec<VmMessage>, VmMessage) {
    let mut messages = Vec::new();
    loop {
        let msg = recv_message(lines).await;
        match &msg {
            VmMessage::StepCompleted { id, .. } if *id == expected_id => {
                return (messages, msg);
            }
            _ => messages.push(msg),
        }
    }
}

/// Spawn the shim on a duplex pair and return (host_writer, host_reader_lines).
fn spawn_shim() -> (
    tokio::io::DuplexStream,
    tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    tokio::task::JoinHandle<()>,
) {
    // host_to_shim: host writes, shim reads
    let (host_writer, shim_reader) = tokio::io::duplex(64 * 1024);
    // shim_to_host: shim writes, host reads
    let (shim_writer, host_reader) = tokio::io::duplex(64 * 1024);

    let handle = tokio::spawn(async move {
        let _ = codeagent_shim::run(shim_reader, shim_writer).await;
    });

    let lines = BufReader::new(host_reader).lines();
    (host_writer, lines, handle)
}

/// SH-01: `echo hello` produces StepStarted → Output(stdout) → StepCompleted(0).
#[tokio::test]
async fn sh_01_echo_hello() {
    let (mut writer, mut lines, _handle) = spawn_shim();

    let msg = HostMessage::Exec {
        id: 1,
        command: "echo hello".to_string(),
        cwd: None,
        env: None,
    };
    send_message(&mut writer, &msg).await;

    let (messages, completed) = collect_until_completed(&mut lines, 1).await;

    // First message should be StepStarted
    assert!(
        matches!(&messages[0], VmMessage::StepStarted { id } if *id == 1),
        "expected StepStarted, got {:?}",
        messages[0]
    );

    // There should be at least one Output message with "hello"
    let output_data: String = messages
        .iter()
        .filter_map(|m| match m {
            VmMessage::Output { id, data, .. } if *id == 1 => Some(data.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        output_data.contains("hello"),
        "expected output to contain 'hello', got: {output_data:?}"
    );

    // StepCompleted with exit_code 0
    assert_eq!(completed, VmMessage::StepCompleted { id: 1, exit_code: 0 });
}

/// SH-02: Failing command returns correct exit code.
#[tokio::test]
async fn sh_02_failing_command() {
    let (mut writer, mut lines, _handle) = spawn_shim();

    let msg = HostMessage::Exec {
        id: 1,
        command: "exit 42".to_string(),
        cwd: None,
        env: None,
    };
    send_message(&mut writer, &msg).await;

    let (_messages, completed) = collect_until_completed(&mut lines, 1).await;
    assert_eq!(
        completed,
        VmMessage::StepCompleted {
            id: 1,
            exit_code: 42
        }
    );
}

/// SH-03: stderr output uses correct stream identifier.
#[tokio::test]
async fn sh_03_stderr_output() {
    let (mut writer, mut lines, _handle) = spawn_shim();

    let msg = HostMessage::Exec {
        id: 1,
        command: "echo err >&2".to_string(),
        cwd: None,
        env: None,
    };
    send_message(&mut writer, &msg).await;

    let (messages, _completed) = collect_until_completed(&mut lines, 1).await;

    let has_stderr = messages.iter().any(|m| {
        matches!(m, VmMessage::Output {
            id: 1,
            stream: codeagent_control::OutputStream::Stderr,
            data,
        } if data.contains("err"))
    });
    assert!(has_stderr, "expected stderr output containing 'err', got: {messages:?}");
}

/// SH-04: exec with cwd — verifies the working directory is changed.
///
/// Instead of comparing raw paths (which differ between Windows and MSYS2),
/// this test creates a marker file in a temp directory and verifies the
/// command can see it via `ls`.
#[tokio::test]
async fn sh_04_exec_with_cwd() {
    let temp_dir = tempfile::tempdir().unwrap();
    let marker_path = temp_dir.path().join("shim_test_marker.txt");
    std::fs::write(&marker_path, "marker").unwrap();

    let cwd_path = temp_dir.path().to_string_lossy().to_string();

    let (mut writer, mut lines, _handle) = spawn_shim();

    let msg = HostMessage::Exec {
        id: 1,
        command: "ls shim_test_marker.txt".to_string(),
        cwd: Some(cwd_path),
        env: None,
    };
    send_message(&mut writer, &msg).await;

    let (messages, completed) = collect_until_completed(&mut lines, 1).await;

    // The command should succeed (exit code 0), meaning the file was found
    assert_eq!(completed, VmMessage::StepCompleted { id: 1, exit_code: 0 });

    let output_data: String = messages
        .iter()
        .filter_map(|m| match m {
            VmMessage::Output { id: 1, data, .. } => Some(data.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        output_data.contains("shim_test_marker.txt"),
        "expected output to contain marker filename, got: {output_data:?}"
    );
}

/// SH-05: exec with env — environment variables are set.
#[tokio::test]
async fn sh_05_exec_with_env() {
    let (mut writer, mut lines, _handle) = spawn_shim();

    let mut env = HashMap::new();
    env.insert("MY_TEST_VAR".to_string(), "test_value_42".to_string());

    let msg = HostMessage::Exec {
        id: 1,
        command: "echo $MY_TEST_VAR".to_string(),
        cwd: None,
        env: Some(env),
    };
    send_message(&mut writer, &msg).await;

    let (messages, _completed) = collect_until_completed(&mut lines, 1).await;

    let output_data: String = messages
        .iter()
        .filter_map(|m| match m {
            VmMessage::Output { id: 1, data, .. } => Some(data.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        output_data.contains("test_value_42"),
        "expected output to contain 'test_value_42', got: {output_data:?}"
    );
}

/// SH-06: Cancel terminates a running command.
///
/// Skipped on Windows: MSYS2's Cygwin-based `sleep` process does not respond
/// to `TerminateProcess` reliably. The shim is a Linux-only binary, so process
/// group termination via SIGTERM/SIGKILL is the real cancel mechanism.
#[tokio::test]
#[cfg_attr(not(unix), ignore)]
async fn sh_06_cancel_running_command() {
    let (mut writer, mut lines, _handle) = spawn_shim();

    // Start a long-running command
    let exec_msg = HostMessage::Exec {
        id: 1,
        command: "sleep 100".to_string(),
        cwd: None,
        env: None,
    };
    send_message(&mut writer, &exec_msg).await;

    // Wait for StepStarted
    let started = recv_message(&mut lines).await;
    assert!(
        matches!(started, VmMessage::StepStarted { id: 1 }),
        "expected StepStarted, got {started:?}"
    );

    // Small delay to ensure the process is running
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send cancel
    let cancel_msg = HostMessage::Cancel { id: 1 };
    send_message(&mut writer, &cancel_msg).await;

    // Should get StepCompleted with non-zero exit code
    let (_messages, completed) = collect_until_completed(&mut lines, 1).await;
    match completed {
        VmMessage::StepCompleted { id, exit_code } => {
            assert_eq!(id, 1);
            assert_ne!(exit_code, 0, "cancelled command should have non-zero exit code");
        }
        _ => panic!("expected StepCompleted, got {completed:?}"),
    }
}

/// SH-07: Two concurrent exec commands execute and complete independently.
#[tokio::test]
async fn sh_07_concurrent_commands() {
    let (mut writer, mut lines, _handle) = spawn_shim();

    // Send two exec commands
    let msg1 = HostMessage::Exec {
        id: 1,
        command: "echo first".to_string(),
        cwd: None,
        env: None,
    };
    let msg2 = HostMessage::Exec {
        id: 2,
        command: "echo second".to_string(),
        cwd: None,
        env: None,
    };
    send_message(&mut writer, &msg1).await;
    send_message(&mut writer, &msg2).await;

    // Collect all messages until both commands complete
    let mut completed_ids = Vec::new();
    let mut all_output = HashMap::<u64, String>::new();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while completed_ids.len() < 2 {
        let msg = tokio::time::timeout_at(deadline, recv_message(&mut lines))
            .await
            .expect("timed out waiting for both commands to complete");

        match &msg {
            VmMessage::StepCompleted { id, exit_code } => {
                assert_eq!(*exit_code, 0);
                completed_ids.push(*id);
            }
            VmMessage::Output { id, data, .. } => {
                all_output.entry(*id).or_default().push_str(data);
            }
            VmMessage::StepStarted { .. } => {}
        }
    }

    // Both commands should have completed
    assert!(completed_ids.contains(&1));
    assert!(completed_ids.contains(&2));

    // Verify output
    assert!(all_output.get(&1).unwrap().contains("first"));
    assert!(all_output.get(&2).unwrap().contains("second"));
}

/// SH-08: Closing the reader end causes the shim to exit cleanly.
#[tokio::test]
async fn sh_08_graceful_shutdown() {
    let (writer, _lines, handle) = spawn_shim();

    // Drop the writer, which closes the shim's reader end
    drop(writer);

    // The shim task should complete cleanly
    let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(result.is_ok(), "shim should exit within timeout");
}
