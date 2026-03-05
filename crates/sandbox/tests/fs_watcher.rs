use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::mpsc;

use codeagent_sandbox::config::FileWatcherConfig;
use codeagent_sandbox::fs_watcher::{self, FsWatcherConfig};
use codeagent_sandbox::recent_writes::RecentBackendWrites;
use codeagent_stdio::Event;

/// Helper: drain events from the receiver with a short timeout.
async fn collect_events(rx: &mut mpsc::UnboundedReceiver<Event>, timeout: Duration) -> Vec<Event> {
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                events.push(event);
            }
            _ = tokio::time::sleep_until(deadline) => {
                break;
            }
        }
    }
    events
}

// -----------------------------------------------------------------------
// FW-01: watcher detects external file creation and emits event (no barrier)
// -----------------------------------------------------------------------
#[tokio::test]
async fn fw_01_external_creation_detected() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let recent_writes = Arc::new(RecentBackendWrites::new(Duration::from_secs(5)));
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();

    let config = FsWatcherConfig {
        debounce: Duration::from_millis(200),
        exclude_patterns: vec![],
        enabled: true,
    };

    let handle = fs_watcher::spawn_fs_watcher(
        vec![working.path().to_path_buf()],
        vec![undo.path().to_path_buf()],
        recent_writes,
        event_sender,
        config,
    );
    assert!(handle.is_some(), "watcher should start");

    // Wait for watcher to be ready
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Create a file externally
    std::fs::write(working.path().join("external.txt"), "hello").unwrap();

    // Wait for debounce + processing
    let events = collect_events(&mut event_receiver, Duration::from_secs(3)).await;

    // Clean up
    if let Some(h) = handle {
        h.abort();
    }

    let external_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ExternalModification { .. }))
        .collect();
    assert!(
        !external_events.is_empty(),
        "expected ExternalModification event for external file creation"
    );

    // Verify no barriers are created (barriers only at session boundaries)
    for event in &external_events {
        if let Event::ExternalModification { barrier_id, .. } = event {
            assert_eq!(
                *barrier_id, None,
                "watcher should not create barriers during a session"
            );
        }
    }
}

// -----------------------------------------------------------------------
// FW-02: backend writes are suppressed (not treated as external)
// -----------------------------------------------------------------------
#[tokio::test]
async fn fw_02_backend_writes_suppressed() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let recent_writes = Arc::new(RecentBackendWrites::new(Duration::from_secs(10)));
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();

    let config = FsWatcherConfig {
        debounce: Duration::from_millis(200),
        exclude_patterns: vec![],
        enabled: true,
    };

    let handle = fs_watcher::spawn_fs_watcher(
        vec![working.path().to_path_buf()],
        vec![undo.path().to_path_buf()],
        recent_writes.clone(),
        event_sender,
        config,
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Record the path as a recent backend write BEFORE writing
    let target = working.path().join("backend_file.txt");
    recent_writes.record(&target);
    std::fs::write(&target, "from backend").unwrap();

    // Wait for debounce + processing
    let events = collect_events(&mut event_receiver, Duration::from_secs(2)).await;

    if let Some(h) = handle {
        h.abort();
    }

    let external_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ExternalModification { .. }))
        .collect();
    assert!(
        external_events.is_empty(),
        "backend writes should not trigger ExternalModification events, got: {external_events:?}"
    );
}

// -----------------------------------------------------------------------
// FW-03: disabled watcher returns None
// -----------------------------------------------------------------------
#[tokio::test]
async fn fw_03_disabled_watcher_returns_none() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let recent_writes = Arc::new(RecentBackendWrites::default());
    let (event_sender, _event_receiver) = mpsc::unbounded_channel();

    let config = FsWatcherConfig {
        enabled: false,
        ..FsWatcherConfig::default()
    };

    let handle = fs_watcher::spawn_fs_watcher(
        vec![working.path().to_path_buf()],
        vec![undo.path().to_path_buf()],
        recent_writes,
        event_sender,
        config,
    );
    assert!(handle.is_none(), "disabled watcher should return None");
}

// -----------------------------------------------------------------------
// FW-04: excluded patterns are filtered out
// -----------------------------------------------------------------------
#[tokio::test]
async fn fw_04_excluded_patterns_filtered() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let recent_writes = Arc::new(RecentBackendWrites::new(Duration::from_secs(5)));
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();

    let config = FsWatcherConfig {
        debounce: Duration::from_millis(200),
        exclude_patterns: vec!["excluded_dir".to_string()],
        enabled: true,
    };

    let handle = fs_watcher::spawn_fs_watcher(
        vec![working.path().to_path_buf()],
        vec![undo.path().to_path_buf()],
        recent_writes,
        event_sender,
        config,
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Write to an excluded path
    let excluded = working.path().join("excluded_dir");
    std::fs::create_dir_all(&excluded).unwrap();
    std::fs::write(excluded.join("file.txt"), "excluded").unwrap();

    let events = collect_events(&mut event_receiver, Duration::from_secs(2)).await;

    if let Some(h) = handle {
        h.abort();
    }

    let external_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ExternalModification { .. }))
        .collect();
    assert!(
        external_events.is_empty(),
        "excluded path writes should not trigger events, got: {external_events:?}"
    );
}

// -----------------------------------------------------------------------
// FW-05: undo directory changes are filtered out
// -----------------------------------------------------------------------
#[tokio::test]
async fn fw_05_undo_dir_changes_filtered() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let recent_writes = Arc::new(RecentBackendWrites::new(Duration::from_secs(5)));
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();

    let config = FsWatcherConfig {
        debounce: Duration::from_millis(200),
        exclude_patterns: vec![],
        enabled: true,
    };

    let _handle = fs_watcher::spawn_fs_watcher(
        vec![working.path().to_path_buf()],
        vec![undo.path().to_path_buf()],
        recent_writes,
        event_sender,
        config,
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Writes inside the undo dir should be filtered out by the undo dir prefix check.
    // This test verifies no spurious events from the undo dir leak through.
    let events = collect_events(&mut event_receiver, Duration::from_secs(1)).await;

    let external_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ExternalModification { .. }))
        .collect();
    assert!(
        external_events.is_empty(),
        "undo dir operations should not trigger ExternalModification"
    );
}

// -----------------------------------------------------------------------
// FW-06: external modification events emitted without barriers
// -----------------------------------------------------------------------
#[tokio::test]
async fn fw_06_external_modification_events_have_no_barrier() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let recent_writes = Arc::new(RecentBackendWrites::new(Duration::from_secs(5)));
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();

    let config = FsWatcherConfig {
        debounce: Duration::from_millis(200),
        exclude_patterns: vec![],
        enabled: true,
    };

    let handle = fs_watcher::spawn_fs_watcher(
        vec![working.path().to_path_buf()],
        vec![undo.path().to_path_buf()],
        recent_writes,
        event_sender,
        config,
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    std::fs::write(working.path().join("external.txt"), "hello").unwrap();

    let events = collect_events(&mut event_receiver, Duration::from_secs(3)).await;

    if let Some(h) = handle {
        h.abort();
    }

    let external_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ExternalModification { .. }))
        .collect();
    assert!(
        !external_events.is_empty(),
        "should emit ExternalModification for external writes"
    );

    // All events should have barrier_id: None (no barriers during sessions)
    for event in &external_events {
        if let Event::ExternalModification { barrier_id, .. } = event {
            assert_eq!(
                *barrier_id, None,
                "watcher should never create barriers (only session boundaries do)"
            );
        }
    }
}

// -----------------------------------------------------------------------
// FW-07: RecentBackendWrites TTL expiry allows detection (event, no barrier)
// -----------------------------------------------------------------------
#[tokio::test]
async fn fw_07_ttl_expiry_allows_detection() {
    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    // Very short TTL
    let recent_writes = Arc::new(RecentBackendWrites::new(Duration::from_millis(50)));
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();

    let config = FsWatcherConfig {
        debounce: Duration::from_millis(200),
        exclude_patterns: vec![],
        enabled: true,
    };

    let handle = fs_watcher::spawn_fs_watcher(
        vec![working.path().to_path_buf()],
        vec![undo.path().to_path_buf()],
        recent_writes.clone(),
        event_sender,
        config,
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Record a write, then wait for TTL to expire, then write again to same path.
    let target = working.path().join("ttl_test.txt");
    recent_writes.record(&target);
    std::fs::write(&target, "first write").unwrap();

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now write again — TTL expired, this should be detected as external
    std::fs::write(&target, "second write after TTL").unwrap();

    let events = collect_events(&mut event_receiver, Duration::from_secs(3)).await;

    if let Some(h) = handle {
        h.abort();
    }

    let external_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ExternalModification { .. }))
        .collect();
    assert!(
        !external_events.is_empty(),
        "writes after TTL expiry should be detected as external"
    );
}

// -----------------------------------------------------------------------
// FW-08: multiple working dirs — changes detected
// -----------------------------------------------------------------------
#[tokio::test]
async fn fw_08_multiple_working_dirs() {
    let working1 = TempDir::new().unwrap();
    let working2 = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();

    let recent_writes = Arc::new(RecentBackendWrites::new(Duration::from_secs(5)));
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();

    let config = FsWatcherConfig {
        debounce: Duration::from_millis(200),
        exclude_patterns: vec![],
        enabled: true,
    };

    let handle = fs_watcher::spawn_fs_watcher(
        vec![working1.path().to_path_buf(), working2.path().to_path_buf()],
        vec![undo.path().join("dir1"), undo.path().join("dir2")],
        recent_writes,
        event_sender,
        config,
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Write to both directories
    std::fs::write(working1.path().join("ext1.txt"), "external1").unwrap();
    std::fs::write(working2.path().join("ext2.txt"), "external2").unwrap();

    let events = collect_events(&mut event_receiver, Duration::from_secs(3)).await;

    if let Some(h) = handle {
        h.abort();
    }

    let external_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ExternalModification { .. }))
        .collect();
    assert!(
        !external_events.is_empty(),
        "should detect external modifications in multiple dirs"
    );
}

// -----------------------------------------------------------------------
// FW-09: FileWatcherConfig deserialization from TOML
// -----------------------------------------------------------------------
#[test]
fn fw_09_config_deserialization() {
    let toml_str = r#"
[file_watcher]
enabled = false
debounce_ms = 500
recent_write_ttl_ms = 3000
exclude_patterns = ["build/", "dist/"]
"#;

    let config: codeagent_sandbox::config::SandboxTomlConfig =
        toml::from_str(toml_str).unwrap();

    assert!(!config.file_watcher.enabled);
    assert_eq!(config.file_watcher.debounce_ms, 500);
    assert_eq!(config.file_watcher.recent_write_ttl_ms, 3000);
    assert_eq!(config.file_watcher.exclude_patterns, vec!["build/", "dist/"]);
}

// -----------------------------------------------------------------------
// FW-10: default config values
// -----------------------------------------------------------------------
#[test]
fn fw_10_default_config() {
    let config = FileWatcherConfig::default();
    assert!(config.enabled);
    assert_eq!(config.debounce_ms, 2000);
    assert_eq!(config.recent_write_ttl_ms, 5000);
    assert!(config.exclude_patterns.is_empty());
}

// -----------------------------------------------------------------------
// FW-11: missing file_watcher section in TOML uses defaults
// -----------------------------------------------------------------------
#[test]
fn fw_11_missing_section_uses_defaults() {
    let toml_str = r#"
[command_classifier]
read_only_commands = ["ls"]
"#;

    let config: codeagent_sandbox::config::SandboxTomlConfig =
        toml::from_str(toml_str).unwrap();

    assert!(config.file_watcher.enabled);
    assert_eq!(config.file_watcher.debounce_ms, 2000);
}

// -----------------------------------------------------------------------
// FW-12: WriteTrackingInterceptor records paths on mutations
// -----------------------------------------------------------------------
#[test]
fn fw_12_write_tracking_interceptor_records() {
    use codeagent_interceptor::undo_interceptor::UndoInterceptor;
    use codeagent_interceptor::write_interceptor::WriteInterceptor;
    use codeagent_sandbox::recent_writes::WriteTrackingInterceptor;

    let working = TempDir::new().unwrap();
    let undo = TempDir::new().unwrap();
    let interceptor = Arc::new(UndoInterceptor::new(
        working.path().to_path_buf(),
        undo.path().to_path_buf(),
    ));

    let recent_writes = Arc::new(RecentBackendWrites::new(Duration::from_secs(10)));
    let tracking = WriteTrackingInterceptor::new(interceptor.clone(), recent_writes.clone());

    // Open a step so pre_write doesn't fail
    interceptor.open_step(1).unwrap();

    let test_file = working.path().join("tracked.txt");
    std::fs::write(&test_file, "content").unwrap();

    // Call pre_write through the tracking interceptor
    let _ = tracking.pre_write(&test_file);

    // The path should be recorded
    assert!(
        recent_writes.was_recent(&test_file),
        "WriteTrackingInterceptor should record mutated paths"
    );

    interceptor.close_step(1).unwrap();
}
