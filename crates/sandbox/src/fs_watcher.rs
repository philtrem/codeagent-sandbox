use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use codeagent_stdio::Event;

use crate::recent_writes::RecentBackendWrites;

/// Configuration for the filesystem watcher.
#[derive(Debug, Clone)]
pub struct FsWatcherConfig {
    /// How long to wait after the last event before processing the batch.
    pub debounce: Duration,
    /// Path patterns to exclude from watching (substring match).
    pub exclude_patterns: Vec<String>,
    /// Whether the watcher is enabled.
    pub enabled: bool,
}

impl Default for FsWatcherConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_secs(2),
            exclude_patterns: vec![
                ".git/objects".to_string(),
                ".git/logs".to_string(),
                ".git/refs".to_string(),
                "node_modules".to_string(),
            ],
            enabled: true,
        }
    }
}

/// Spawn a filesystem watcher that monitors working directories for external
/// changes and emits `ExternalModification` events when detected.
///
/// Barriers are NOT created here — they belong only at session boundaries
/// (placed in `do_session_start`). The watcher only detects and reports.
///
/// Returns `None` if the watcher fails to initialize (non-fatal).
pub fn spawn_fs_watcher(
    working_dirs: Vec<PathBuf>,
    undo_dirs: Vec<PathBuf>,
    recent_writes: Arc<RecentBackendWrites>,
    event_sender: mpsc::UnboundedSender<Event>,
    config: FsWatcherConfig,
) -> Option<JoinHandle<()>> {
    if !config.enabled {
        return None;
    }

    // Create a channel to bridge the synchronous notify callback to async tokio.
    let (bridge_tx, bridge_rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();

    // Build the watcher with a batching event handler.
    let watcher_result = build_watcher(bridge_tx);
    let mut watcher = match watcher_result {
        Ok(w) => w,
        Err(error) => {
            let _ = event_sender.send(Event::Warning {
                code: "file_watcher_failed".to_string(),
                message: format!("Filesystem watcher failed to initialize: {error}"),
            });
            return None;
        }
    };

    // Watch each working directory recursively.
    for dir in &working_dirs {
        if let Err(error) = watcher.watch(dir, RecursiveMode::Recursive) {
            let _ = event_sender.send(Event::Warning {
                code: "file_watcher_failed".to_string(),
                message: format!(
                    "Failed to watch directory {}: {error}",
                    dir.display()
                ),
            });
        }
    }

    // Build a set of excluded prefixes: undo dirs + user-configured patterns.
    let undo_dir_prefixes: Vec<String> = undo_dirs
        .iter()
        .map(|d| d.to_string_lossy().replace('\\', "/"))
        .collect();

    let exclude_patterns = config.exclude_patterns.clone();
    let debounce = config.debounce;

    let handle = tokio::spawn(async move {
        // Keep the watcher alive for the duration of the task.
        let _watcher = watcher;

        run_watcher_loop(WatcherLoopParams {
            bridge_rx,
            debounce,
            working_dirs: &working_dirs,
            recent_writes: &recent_writes,
            event_sender: &event_sender,
            undo_dir_prefixes: &undo_dir_prefixes,
            exclude_patterns: &exclude_patterns,
        })
        .await;
    });

    Some(handle)
}

/// Build a `notify::RecommendedWatcher` that collects changed paths into a
/// sync channel. The watcher batches events internally.
fn build_watcher(
    bridge_tx: std::sync::mpsc::Sender<Vec<PathBuf>>,
) -> Result<RecommendedWatcher, notify::Error> {
    notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
        if let Ok(event) = result {
            // Only care about modification events — not access-only events.
            if is_mutation_event(&event) {
                let paths: Vec<PathBuf> = event.paths;
                if !paths.is_empty() {
                    let _ = bridge_tx.send(paths);
                }
            }
        }
    })
}

/// Check if a notify event represents a mutation (create/modify/remove/rename).
fn is_mutation_event(event: &notify::Event) -> bool {
    use notify::EventKind;
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Grouped parameters for `run_watcher_loop` to satisfy clippy's argument limit.
struct WatcherLoopParams<'a> {
    bridge_rx: std::sync::mpsc::Receiver<Vec<PathBuf>>,
    debounce: Duration,
    working_dirs: &'a [PathBuf],
    recent_writes: &'a RecentBackendWrites,
    event_sender: &'a mpsc::UnboundedSender<Event>,
    undo_dir_prefixes: &'a [String],
    exclude_patterns: &'a [String],
}

/// Main watcher loop: reads batched events from the bridge channel, debounces
/// them, filters against recent backend writes, and emits events.
async fn run_watcher_loop(params: WatcherLoopParams<'_>) {
    let WatcherLoopParams {
        bridge_rx,
        debounce,
        working_dirs,
        recent_writes,
        event_sender,
        undo_dir_prefixes,
        exclude_patterns,
    } = params;
    // Use a tokio mpsc to forward from blocking recv to async select.
    let (async_tx, mut async_rx) = mpsc::unbounded_channel::<Vec<PathBuf>>();

    // Spawn a blocking task that reads from the sync channel and forwards.
    let _reader = tokio::task::spawn_blocking(move || {
        while let Ok(paths) = bridge_rx.recv() {
            if async_tx.send(paths).is_err() {
                break;
            }
        }
    });

    let mut pending_paths: HashSet<PathBuf> = HashSet::new();
    let mut debounce_timer = tokio::time::interval(debounce);
    debounce_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Skip the immediate first tick.
    debounce_timer.tick().await;

    // Prune expired recent writes periodically.
    let mut prune_interval = tokio::time::interval(Duration::from_secs(10));
    prune_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    prune_interval.tick().await;

    loop {
        tokio::select! {
            Some(paths) = async_rx.recv() => {
                for path in paths {
                    pending_paths.insert(path);
                }
                // Reset the debounce timer by advancing it.
                debounce_timer.reset();
            }
            _ = debounce_timer.tick() => {
                if pending_paths.is_empty() {
                    continue;
                }

                process_pending_paths(
                    &mut pending_paths,
                    working_dirs,
                    recent_writes,
                    event_sender,
                    undo_dir_prefixes,
                    exclude_patterns,
                );
            }
            _ = prune_interval.tick() => {
                recent_writes.prune_expired();
            }
        }
    }
}

/// Process accumulated paths: filter, group by working dir, emit events.
fn process_pending_paths(
    pending_paths: &mut HashSet<PathBuf>,
    working_dirs: &[PathBuf],
    recent_writes: &RecentBackendWrites,
    event_sender: &mpsc::UnboundedSender<Event>,
    undo_dir_prefixes: &[String],
    exclude_patterns: &[String],
) {
    // Collect external paths (not from backend, not excluded).
    let mut affected: Vec<PathBuf> = Vec::new();

    for path in pending_paths.drain() {
        let path_str = path.to_string_lossy().replace('\\', "/");

        // Skip paths inside undo directories.
        if undo_dir_prefixes
            .iter()
            .any(|prefix| path_str.starts_with(prefix.as_str()))
        {
            continue;
        }

        // Skip excluded patterns (substring match).
        if exclude_patterns
            .iter()
            .any(|pattern| path_str.contains(pattern.as_str()))
        {
            continue;
        }

        // Skip if this was a recent backend write.
        if recent_writes.was_recent(&path) {
            continue;
        }

        // Verify the path belongs to a watched working directory.
        if working_dirs.iter().any(|wd| path.starts_with(wd)) {
            affected.push(path);
        }
    }

    if affected.is_empty() {
        return;
    }

    let affected_strings: Vec<String> = affected
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    let _ = event_sender.send(Event::ExternalModification {
        affected_paths: affected_strings,
        barrier_id: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_events_detected() {
        use notify::{Event as NotifyEvent, EventKind, event::CreateKind};

        let create_event = NotifyEvent {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![PathBuf::from("/tmp/test")],
            attrs: Default::default(),
        };
        assert!(is_mutation_event(&create_event));
    }

    #[test]
    fn access_events_ignored() {
        use notify::{Event as NotifyEvent, EventKind, event::AccessKind};

        let access_event = NotifyEvent {
            kind: EventKind::Access(AccessKind::Read),
            paths: vec![PathBuf::from("/tmp/test")],
            attrs: Default::default(),
        };
        assert!(!is_mutation_event(&access_event));
    }

    #[test]
    fn default_config_values() {
        let config = FsWatcherConfig::default();
        assert!(config.enabled);
        assert_eq!(config.debounce, Duration::from_secs(2));
        assert!(config.exclude_patterns.contains(&".git/objects".to_string()));
        assert!(config.exclude_patterns.contains(&"node_modules".to_string()));
    }
}
