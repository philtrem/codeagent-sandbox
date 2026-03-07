use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use codeagent_common::{AffectedPath, BarrierReason, FileChangeKind};
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
            debounce: Duration::from_millis(200),
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
    let (bridge_tx, bridge_rx) = std::sync::mpsc::channel::<Vec<AffectedPath>>();

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
    bridge_tx: std::sync::mpsc::Sender<Vec<AffectedPath>>,
) -> Result<RecommendedWatcher, notify::Error> {
    // Buffer for pairing consecutive From→To rename events (Windows delivers
    // renames as two separate events; Linux inotify uses RenameMode::Both).
    let mut pending_rename_from: Option<PathBuf> = None;

    notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
        if let Ok(event) = result {
            // Only care about modification events — not access-only events.
            if is_mutation_event(&event) {
                let entries = event_to_affected_paths(event, &mut pending_rename_from);
                if !entries.is_empty() {
                    let _ = bridge_tx.send(entries);
                }
            }
        }
    })
}

/// Convert a `notify::Event` into `AffectedPath` entries, pairing rename
/// source/destination when possible.
///
/// On Linux, inotify pairs renames into a single `RenameMode::Both` event.
/// On Windows, `ReadDirectoryChangesW` delivers them as consecutive
/// `RenameMode::From` then `RenameMode::To` events — we buffer the `From`
/// path and pair it with the next `To`. This works for both file and
/// directory renames.
fn event_to_affected_paths(
    event: notify::Event,
    pending_rename_from: &mut Option<PathBuf>,
) -> Vec<AffectedPath> {
    use notify::event::{ModifyKind, RenameMode};

    match event.kind {
        // Both paths in one event (Linux inotify).
        notify::EventKind::Modify(ModifyKind::Name(RenameMode::Both))
            if event.paths.len() == 2 =>
        {
            *pending_rename_from = None;
            vec![AffectedPath {
                path: event.paths[1].clone(),
                kind: FileChangeKind::Renamed,
                renamed_from: Some(event.paths[0].clone()),
            }]
        }

        // Source path of a rename — buffer it for pairing with the next To.
        notify::EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            // Flush any previously unpaired From as standalone.
            let mut result = Vec::new();
            if let Some(old_from) = pending_rename_from.take() {
                result.push(AffectedPath {
                    path: old_from,
                    kind: FileChangeKind::Renamed,
                    renamed_from: None,
                });
            }
            if let Some(path) = event.paths.into_iter().next() {
                *pending_rename_from = Some(path);
            }
            result
        }

        // Destination path of a rename — pair with buffered From if available.
        notify::EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            let from = pending_rename_from.take();
            if let Some(dest) = event.paths.into_iter().next() {
                vec![AffectedPath {
                    path: dest,
                    kind: FileChangeKind::Renamed,
                    renamed_from: from,
                }]
            } else {
                Vec::new()
            }
        }

        // Any other rename mode (macOS FSEvents) — emit standalone.
        notify::EventKind::Modify(ModifyKind::Name(_)) => {
            let kind = FileChangeKind::Renamed;
            event
                .paths
                .into_iter()
                .map(|p| AffectedPath {
                    path: p,
                    kind,
                    renamed_from: None,
                })
                .collect()
        }

        // Non-rename event — flush any unpaired From, then emit normally.
        _ => {
            let mut result = Vec::new();
            if let Some(old_from) = pending_rename_from.take() {
                result.push(AffectedPath {
                    path: old_from,
                    kind: FileChangeKind::Renamed,
                    renamed_from: None,
                });
            }
            let kind = event_kind_to_change_kind(&event.kind);
            result.extend(event.paths.into_iter().map(|p| AffectedPath {
                path: p,
                kind,
                renamed_from: None,
            }));
            result
        }
    }
}

/// Map a `notify::EventKind` to our `FileChangeKind`.
fn event_kind_to_change_kind(kind: &notify::EventKind) -> FileChangeKind {
    use notify::EventKind;
    use notify::event::ModifyKind;
    match kind {
        EventKind::Create(_) => FileChangeKind::Created,
        EventKind::Remove(_) => FileChangeKind::Deleted,
        EventKind::Modify(ModifyKind::Name(_)) => FileChangeKind::Renamed,
        _ => FileChangeKind::Modified,
    }
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
    bridge_rx: std::sync::mpsc::Receiver<Vec<AffectedPath>>,
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
    let (async_tx, mut async_rx) = mpsc::unbounded_channel::<Vec<AffectedPath>>();

    // Spawn a blocking task that reads from the sync channel and forwards.
    let _reader = tokio::task::spawn_blocking(move || {
        while let Ok(entries) = bridge_rx.recv() {
            if async_tx.send(entries).is_err() {
                break;
            }
        }
    });

    let mut pending: Vec<AffectedPath> = Vec::new();
    let mut pending_seen: HashSet<PathBuf> = HashSet::new();
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
            Some(entries) = async_rx.recv() => {
                for ap in entries {
                    if !pending_seen.contains(&ap.path) {
                        pending_seen.insert(ap.path.clone());
                        pending.push(ap);
                    }
                }
                // No timer reset: use fixed-interval throttling instead of
                // debouncing. Debounce (wait for silence) starves processing
                // when the VM generates continuous filesystem activity because
                // each event resets the timer.
            }
            _ = debounce_timer.tick() => {
                if pending.is_empty() {
                    continue;
                }

                process_pending_paths(
                    &mut pending,
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
                pending_seen.clear();
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
fn process_pending_paths(
    pending: &mut Vec<AffectedPath>,
    params: &ProcessParams<'_>,
) {
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
    let mut per_dir: Vec<Vec<AffectedPath>> = vec![vec![]; working_dirs.len()];

    for ap in pending.drain(..) {
        let path_str = ap.path.to_string_lossy().replace('\\', "/");

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

        // Skip if this was a recent backend write (check both dest and source for renames).
        if recent_writes.was_recent(&ap.path) {
            continue;
        }
        if let Some(ref from) = ap.renamed_from {
            if recent_writes.was_recent(from) {
                continue;
            }
        }

        // Log paths not suppressed — helps diagnose watcher vs. interceptor mismatches.
        eprintln!(
            "{{\"level\":\"debug\",\"component\":\"fs_watcher\",\"action\":\"external_candidate\",\"raw_path\":\"{}\",\"normalized_path\":\"{}\",\"kind\":\"{:?}\"}}",
            ap.path.display(),
            path_str,
            ap.kind
        );

        // Find which working directory this path belongs to.
        for (index, working_dir) in working_dirs.iter().enumerate() {
            if ap.path.starts_with(working_dir) {
                // Check gitignore rules for this working directory.
                if let Some(Some(filter)) = gitignore_filters.get(index) {
                    if let Ok(relative) = ap.path.strip_prefix(working_dir) {
                        let relative_str = relative.to_string_lossy().replace('\\', "/");
                        let is_dir = ap.path.is_dir();
                        if filter.matched_path_or_any_parents(&relative_str, is_dir).is_ignore() {
                            break;
                        }
                    }
                }
                per_dir[index].push(ap);
                break;
            }
        }
    }

    // Remove parent directories whose mtime changed only because a child was
    // modified — the child path is already in the set and is the real change.
    for entries in &mut per_dir {
        if entries.len() > 1 {
            remove_redundant_parents(entries);
        }
    }

    // Create barriers and emit events for each working directory with external changes.
    for (index, external_paths) in per_dir.into_iter().enumerate() {
        if external_paths.is_empty() {
            continue;
        }

        let affected_strings: Vec<String> = external_paths
            .iter()
            .map(|ap| {
                if let Some(ref from) = ap.renamed_from {
                    let from_str = from.to_string_lossy();
                    let to_str = ap.path.to_string_lossy();
                    format!("{from_str} \u{2192} {to_str} (renamed)")
                } else {
                    let path_str = ap.path.to_string_lossy().into_owned();
                    format!("{path_str} ({kind})", kind = format_change_kind(ap.kind))
                }
            })
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

/// Format a `FileChangeKind` as a human-readable string for display.
fn format_change_kind(kind: FileChangeKind) -> &'static str {
    match kind {
        FileChangeKind::Created => "created",
        FileChangeKind::Modified => "modified",
        FileChangeKind::Deleted => "deleted",
        FileChangeKind::Renamed => "renamed",
    }
}

/// Remove paths that are a direct parent of another path in the list.
///
/// When a child file is created or modified, the OS also updates the parent
/// directory's mtime, causing `notify` to report both. The parent entry is
/// noise — the child is the real change. However, a directory path with no
/// child in the list is kept (it may be a genuine mkdir/rmdir event).
fn remove_redundant_parents(entries: &mut Vec<AffectedPath>) {
    // Collect parents of all paths in the set to identify which directories
    // are only present because a child's creation/modification changed their mtime.
    // For renames, both the destination and source paths contribute parent dirs.
    let parents_of_children: HashSet<PathBuf> = entries
        .iter()
        .flat_map(|ap| {
            let dest_parent = ap.path.parent().map(|p| p.to_path_buf());
            let src_parent = ap
                .renamed_from
                .as_ref()
                .and_then(|from| from.parent().map(|p| p.to_path_buf()));
            dest_parent.into_iter().chain(src_parent)
        })
        .collect();
    entries.retain(|ap| {
        // Keep this entry unless its path is the parent of another path in the set.
        !parents_of_children.contains(&ap.path)
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
        assert_eq!(config.debounce, Duration::from_millis(200));
        assert!(config.exclude_patterns.contains(&".git/".to_string()));
        assert!(config.exclude_patterns.contains(&"node_modules".to_string()));
    }

    #[test]
    fn redundant_parent_removed() {
        let mut entries = vec![
            AffectedPath { path: PathBuf::from("/workspace/subdir"), kind: FileChangeKind::Modified, renamed_from: None },
            AffectedPath { path: PathBuf::from("/workspace/subdir/file.txt"), kind: FileChangeKind::Created, renamed_from: None },
        ];
        remove_redundant_parents(&mut entries);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/workspace/subdir/file.txt"));
        assert_eq!(entries[0].kind, FileChangeKind::Created);
    }

    #[test]
    fn standalone_directory_kept() {
        let mut entries = vec![
            AffectedPath { path: PathBuf::from("/workspace/new_dir"), kind: FileChangeKind::Created, renamed_from: None },
            AffectedPath { path: PathBuf::from("/workspace/other_file.txt"), kind: FileChangeKind::Modified, renamed_from: None },
        ];
        remove_redundant_parents(&mut entries);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn multiple_children_remove_parent() {
        let mut entries = vec![
            AffectedPath { path: PathBuf::from("/workspace/dir"), kind: FileChangeKind::Modified, renamed_from: None },
            AffectedPath { path: PathBuf::from("/workspace/dir/a.txt"), kind: FileChangeKind::Created, renamed_from: None },
            AffectedPath { path: PathBuf::from("/workspace/dir/b.txt"), kind: FileChangeKind::Modified, renamed_from: None },
        ];
        remove_redundant_parents(&mut entries);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|ap| ap.path.as_os_str() == "/workspace/dir/a.txt"));
        assert!(entries.iter().any(|ap| ap.path.as_os_str() == "/workspace/dir/b.txt"));
    }

    #[test]
    fn single_path_unchanged() {
        let mut entries = vec![
            AffectedPath { path: PathBuf::from("/workspace/file.txt"), kind: FileChangeKind::Modified, renamed_from: None },
        ];
        // remove_redundant_parents is only called when len > 1, but test the
        // function directly to verify it handles edge cases.
        remove_redundant_parents(&mut entries);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/workspace/file.txt"));
    }

    #[test]
    fn event_kind_mapping() {
        use notify::EventKind;
        use notify::event::{CreateKind, RemoveKind, ModifyKind, RenameMode, DataChange};

        assert_eq!(
            event_kind_to_change_kind(&EventKind::Create(CreateKind::File)),
            FileChangeKind::Created,
        );
        assert_eq!(
            event_kind_to_change_kind(&EventKind::Remove(RemoveKind::File)),
            FileChangeKind::Deleted,
        );
        assert_eq!(
            event_kind_to_change_kind(&EventKind::Modify(ModifyKind::Name(RenameMode::Both))),
            FileChangeKind::Renamed,
        );
        assert_eq!(
            event_kind_to_change_kind(&EventKind::Modify(ModifyKind::Data(DataChange::Content))),
            FileChangeKind::Modified,
        );
    }

    #[test]
    fn rename_both_collapsed_into_single_entry() {
        use notify::event::{ModifyKind, RenameMode};

        let mut pending = None;
        let event = notify::Event {
            kind: notify::EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            paths: vec![
                PathBuf::from("/workspace/old_name.txt"),
                PathBuf::from("/workspace/new_name.txt"),
            ],
            attrs: Default::default(),
        };

        let entries = event_to_affected_paths(event, &mut pending);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/workspace/new_name.txt"));
        assert_eq!(entries[0].kind, FileChangeKind::Renamed);
        assert_eq!(
            entries[0].renamed_from,
            Some(PathBuf::from("/workspace/old_name.txt")),
        );
        assert!(pending.is_none());
    }

    #[test]
    fn rename_from_to_paired_on_windows() {
        use notify::event::{ModifyKind, RenameMode};

        let mut pending = None;

        // From event — buffers the source path, emits nothing.
        let from_event = notify::Event {
            kind: notify::EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            paths: vec![PathBuf::from("/workspace/old.txt")],
            attrs: Default::default(),
        };
        let entries = event_to_affected_paths(from_event, &mut pending);
        assert!(entries.is_empty());
        assert_eq!(pending, Some(PathBuf::from("/workspace/old.txt")));

        // To event — pairs with buffered From.
        let to_event = notify::Event {
            kind: notify::EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            paths: vec![PathBuf::from("/workspace/new.txt")],
            attrs: Default::default(),
        };
        let entries = event_to_affected_paths(to_event, &mut pending);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/workspace/new.txt"));
        assert_eq!(entries[0].kind, FileChangeKind::Renamed);
        assert_eq!(
            entries[0].renamed_from,
            Some(PathBuf::from("/workspace/old.txt")),
        );
        assert!(pending.is_none());
    }

    #[test]
    fn unpaired_from_flushed_by_non_rename_event() {
        use notify::event::{ModifyKind, RenameMode, DataChange};

        let mut pending = None;

        // From event with no matching To.
        let from_event = notify::Event {
            kind: notify::EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            paths: vec![PathBuf::from("/workspace/orphan.txt")],
            attrs: Default::default(),
        };
        let entries = event_to_affected_paths(from_event, &mut pending);
        assert!(entries.is_empty());

        // A non-rename event flushes the buffered From as standalone.
        let modify_event = notify::Event {
            kind: notify::EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            paths: vec![PathBuf::from("/workspace/other.txt")],
            attrs: Default::default(),
        };
        let entries = event_to_affected_paths(modify_event, &mut pending);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("/workspace/orphan.txt"));
        assert_eq!(entries[0].kind, FileChangeKind::Renamed);
        assert!(entries[0].renamed_from.is_none());
        assert_eq!(entries[1].path, PathBuf::from("/workspace/other.txt"));
        assert_eq!(entries[1].kind, FileChangeKind::Modified);
        assert!(pending.is_none());
    }

    #[test]
    fn rename_any_has_no_renamed_from() {
        use notify::event::{ModifyKind, RenameMode};

        let mut pending = None;
        let event = notify::Event {
            kind: notify::EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            paths: vec![PathBuf::from("/workspace/file.txt")],
            attrs: Default::default(),
        };

        let entries = event_to_affected_paths(event, &mut pending);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, FileChangeKind::Renamed);
        assert!(entries[0].renamed_from.is_none());
    }

    #[test]
    fn create_event_produces_created_kind() {
        use notify::event::CreateKind;

        let mut pending = None;
        let event = notify::Event {
            kind: notify::EventKind::Create(CreateKind::File),
            paths: vec![PathBuf::from("/workspace/new.txt")],
            attrs: Default::default(),
        };

        let entries = event_to_affected_paths(event, &mut pending);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, FileChangeKind::Created);
        assert!(entries[0].renamed_from.is_none());
    }
}
