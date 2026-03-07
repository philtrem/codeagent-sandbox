use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use codeagent_common::BarrierReason;
use codeagent_interceptor::gitignore::build_gitignore;
use ignore::gitignore::Gitignore;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
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
    /// Whether to respect `.gitignore` rules when filtering external modifications.
    pub use_gitignore: bool,
}

impl Default for FsWatcherConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_secs(2),
            exclude_patterns: vec![
                ".git/".to_string(),
                "node_modules".to_string(),
            ],
            enabled: true,
            use_gitignore: true,
        }
    }
}

/// Spawn a filesystem watcher that monitors working directories for external
/// changes and emits `ExternalModification` events.
///
/// Returns `None` if the watcher fails to initialize (non-fatal).
pub fn spawn_fs_watcher(
    working_dirs: Vec<PathBuf>,
    undo_dirs: Vec<PathBuf>,
    interceptors: Vec<Arc<UndoInterceptor>>,
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

    // Build gitignore filters per working directory if enabled.
    let gitignore_filters: Vec<Option<Gitignore>> = if config.use_gitignore {
        working_dirs
            .iter()
            .map(|dir| build_gitignore(dir))
            .collect()
    } else {
        working_dirs.iter().map(|_| None).collect()
    };

    let exclude_patterns = config.exclude_patterns.clone();
    let debounce = config.debounce;

    let handle = tokio::spawn(async move {
        // Keep the watcher alive for the duration of the task.
        let _watcher = watcher;

        run_watcher_loop(WatcherLoopParams {
            bridge_rx,
            debounce,
            working_dirs: &working_dirs,
            interceptors: &interceptors,
            recent_writes: &recent_writes,
            event_sender: &event_sender,
            undo_dir_prefixes: &undo_dir_prefixes,
            exclude_patterns: &exclude_patterns,
            gitignore_filters: &gitignore_filters,
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
/// Metadata-only changes (access time updates from reads) are excluded.
fn is_mutation_event(event: &notify::Event) -> bool {
    use notify::event::ModifyKind;
    use notify::EventKind;
    match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(kind) => !matches!(kind, ModifyKind::Metadata(_)),
        _ => false,
    }
}

/// Grouped parameters for `run_watcher_loop` to satisfy clippy's argument limit.
struct WatcherLoopParams<'a> {
    bridge_rx: std::sync::mpsc::Receiver<Vec<PathBuf>>,
    debounce: Duration,
    working_dirs: &'a [PathBuf],
    interceptors: &'a [Arc<UndoInterceptor>],
    recent_writes: &'a RecentBackendWrites,
    event_sender: &'a mpsc::UnboundedSender<Event>,
    undo_dir_prefixes: &'a [String],
    exclude_patterns: &'a [String],
    gitignore_filters: &'a [Option<Gitignore>],
}

/// Main watcher loop: reads events from the bridge channel, accumulates them,
/// and processes at fixed intervals (throttle), filtering against recent
/// backend writes before emitting external modification events.
async fn run_watcher_loop(params: WatcherLoopParams<'_>) {
    let WatcherLoopParams {
        bridge_rx,
        debounce,
        working_dirs,
        interceptors,
        recent_writes,
        event_sender,
        undo_dir_prefixes,
        exclude_patterns,
        gitignore_filters,
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
                // No timer reset: use fixed-interval throttling instead of
                // debouncing. Debounce (wait for silence) starves processing
                // when the VM generates continuous filesystem activity because
                // each event resets the timer.
            }
            _ = debounce_timer.tick() => {
                if pending_paths.is_empty() {
                    continue;
                }

                process_pending_paths(
                    &mut pending_paths,
                    &ProcessParams {
                        working_dirs,
                        interceptors,
                        recent_writes,
                        event_sender,
                        undo_dir_prefixes,
                        exclude_patterns,
                        gitignore_filters,
                    },
                );
            }
            _ = prune_interval.tick() => {
                recent_writes.prune_expired();
            }
        }
    }
}

/// Grouped parameters for `process_pending_paths` to satisfy clippy's argument limit.
struct ProcessParams<'a> {
    working_dirs: &'a [PathBuf],
    interceptors: &'a [Arc<UndoInterceptor>],
    recent_writes: &'a RecentBackendWrites,
    event_sender: &'a mpsc::UnboundedSender<Event>,
    undo_dir_prefixes: &'a [String],
    exclude_patterns: &'a [String],
    gitignore_filters: &'a [Option<Gitignore>],
}

/// Process accumulated paths: filter, group by working dir, and emit events.
fn process_pending_paths(pending_paths: &mut HashSet<PathBuf>, params: &ProcessParams<'_>) {
    let ProcessParams {
        working_dirs,
        interceptors,
        recent_writes,
        event_sender,
        undo_dir_prefixes,
        exclude_patterns,
        gitignore_filters,
    } = params;

    // Group external paths by working directory index.
    let mut per_dir: Vec<Vec<PathBuf>> = vec![vec![]; working_dirs.len()];

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

        // Log paths not suppressed — helps diagnose watcher vs. interceptor mismatches.
        eprintln!(
            "{{\"level\":\"debug\",\"component\":\"fs_watcher\",\"action\":\"external_candidate\",\"raw_path\":\"{}\",\"normalized_path\":\"{}\"}}",
            path.display(),
            path_str
        );

        // Find which working directory this path belongs to.
        for (index, working_dir) in working_dirs.iter().enumerate() {
            if path.starts_with(working_dir) {
                // Check gitignore rules for this working directory.
                if let Some(Some(filter)) = gitignore_filters.get(index) {
                    if let Ok(relative) = path.strip_prefix(working_dir) {
                        let relative_str = relative.to_string_lossy().replace('\\', "/");
                        let is_dir = path.is_dir();
                        if filter.matched_path_or_any_parents(&relative_str, is_dir).is_ignore() {
                            break;
                        }
                    }
                }
                per_dir[index].push(path);
                break;
            }
        }
    }

    // Remove parent directories whose mtime changed only because a child was
    // modified — the child path is already in the set and is the real change.
    for paths in &mut per_dir {
        if paths.len() > 1 {
            remove_redundant_parents(paths);
        }
    }

    // Create barriers and emit events for each working directory with external changes.
    for (index, external_paths) in per_dir.into_iter().enumerate() {
        if external_paths.is_empty() {
            continue;
        }

        let affected_strings: Vec<String> = external_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();

        // Create a barrier on the interceptor if one exists for this directory.
        let barrier_id = if let Some(interceptor) = interceptors.get(index) {
            match interceptor.notify_external_modification(
                external_paths,
                BarrierReason::ExternalModification,
            ) {
                Ok(Some(barrier)) => Some(barrier.barrier_id),
                _ => None,
            }
        } else {
            None
        };

        let _ = event_sender.send(Event::ExternalModification {
            affected_paths: affected_strings,
            barrier_id,
        });
    }
}

/// Remove paths that are a direct parent of another path in the list.
///
/// When a child file is created or modified, the OS also updates the parent
/// directory's mtime, causing `notify` to report both. The parent entry is
/// noise — the child is the real change. However, a directory path with no
/// child in the list is kept (it may be a genuine mkdir/rmdir event).
fn remove_redundant_parents(paths: &mut Vec<PathBuf>) {
    // Collect parents of all paths in the set to identify which directories
    // are only present because a child's creation/modification changed their mtime.
    let parents_of_children: HashSet<PathBuf> = paths
        .iter()
        .filter_map(|p| p.parent().map(|parent| parent.to_path_buf()))
        .collect();
    paths.retain(|path| {
        // Keep this path unless it is the parent of another path in the set.
        !parents_of_children.contains(path)
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
    fn metadata_only_events_ignored() {
        use notify::{Event as NotifyEvent, EventKind, event::{MetadataKind, ModifyKind}};

        let metadata_event = NotifyEvent {
            kind: EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            paths: vec![PathBuf::from("/tmp/test")],
            attrs: Default::default(),
        };
        assert!(!is_mutation_event(&metadata_event));

        let access_time_event = NotifyEvent {
            kind: EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)),
            paths: vec![PathBuf::from("/tmp/test")],
            attrs: Default::default(),
        };
        assert!(!is_mutation_event(&access_time_event));
    }

    #[test]
    fn data_modify_events_detected() {
        use notify::{Event as NotifyEvent, EventKind, event::{DataChange, ModifyKind}};

        let data_event = NotifyEvent {
            kind: EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            paths: vec![PathBuf::from("/tmp/test")],
            attrs: Default::default(),
        };
        assert!(is_mutation_event(&data_event));
    }

    #[test]
    fn default_config_values() {
        let config = FsWatcherConfig::default();
        assert!(config.enabled);
        assert!(config.use_gitignore);
        assert_eq!(config.debounce, Duration::from_secs(2));
        assert!(config.exclude_patterns.contains(&".git/".to_string()));
        assert!(config.exclude_patterns.contains(&"node_modules".to_string()));
    }

    #[test]
    fn redundant_parent_removed() {
        let mut paths = vec![
            PathBuf::from("/workspace/subdir"),
            PathBuf::from("/workspace/subdir/file.txt"),
        ];
        remove_redundant_parents(&mut paths);
        assert_eq!(paths, vec![PathBuf::from("/workspace/subdir/file.txt")]);
    }

    #[test]
    fn standalone_directory_kept() {
        let mut paths = vec![
            PathBuf::from("/workspace/new_dir"),
            PathBuf::from("/workspace/other_file.txt"),
        ];
        remove_redundant_parents(&mut paths);
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn multiple_children_remove_parent() {
        let mut paths = vec![
            PathBuf::from("/workspace/dir"),
            PathBuf::from("/workspace/dir/a.txt"),
            PathBuf::from("/workspace/dir/b.txt"),
        ];
        remove_redundant_parents(&mut paths);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&PathBuf::from("/workspace/dir/a.txt")));
        assert!(paths.contains(&PathBuf::from("/workspace/dir/b.txt")));
    }

    #[test]
    fn single_path_unchanged() {
        let mut paths = vec![PathBuf::from("/workspace/file.txt")];
        // remove_redundant_parents is only called when len > 1, but test the
        // function directly to verify it handles edge cases.
        remove_redundant_parents(&mut paths);
        assert_eq!(paths, vec![PathBuf::from("/workspace/file.txt")]);
    }
}
