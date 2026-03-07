use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use codeagent_common::{Result, StepId};
use codeagent_interceptor::write_interceptor::WriteInterceptor;

/// Default TTL for recent write records: 5 seconds.
/// Accounts for OS event delivery delay (especially macOS FSEvents).
const DEFAULT_TTL: Duration = Duration::from_secs(5);

/// Small buffer to cover the gap between the OS delivering a filesystem event
/// and our notify callback stamping `Instant::now()`. This only needs to
/// account for thread scheduling jitter, not debounce or batching delays.
const ARRIVAL_JITTER_BUFFER: Duration = Duration::from_millis(50);

/// Tracks paths recently written by the sandbox's own backends (filesystem
/// channel or MCP API). The filesystem watcher checks this map to distinguish
/// backend-originated writes from genuine external modifications.
///
/// Provides two suppression layers:
/// 1. **Per-path**: `record()` / `should_suppress()` — used by `WriteTrackingInterceptor`
///    for VM filesystem writes, and by the orchestrator for MCP `write_file`/`edit_file`
///    where we know the exact paths and can `record()` at write time.
/// 2. **Blanket**: `begin_suppression()` / `end_suppression()` — used by the
///    orchestrator for operations where we cannot enumerate the affected paths
///    (rollback, host bash).
///
/// Blanket suppression is counter-based: while any guard is held, `should_suppress`
/// returns true for all paths. When the last guard drops, the end timestamp is
/// recorded. Events are then compared by their *arrival* time (when the OS
/// delivered them to the notify callback) rather than by wall-clock time at
/// processing, eliminating races caused by debounce delays.
pub struct RecentBackendWrites {
    entries: Mutex<HashMap<PathBuf, Instant>>,
    ttl: Duration,
    /// Number of active suppression guards. While > 0, all paths are suppressed.
    active_suppressions: AtomicUsize,
    /// Timestamp when the last suppression guard dropped. Events whose arrival
    /// time is before this instant (plus a jitter buffer) are suppressed.
    suppress_ended_at: Mutex<Option<Instant>>,
}

impl RecentBackendWrites {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            active_suppressions: AtomicUsize::new(0),
            suppress_ended_at: Mutex::new(None),
        }
    }

    /// Record that the sandbox just wrote to `path`.
    ///
    /// Also records the parent directory because filesystem watchers report
    /// mtime changes on parent directories when children are created/removed,
    /// and we need to suppress those too.
    pub fn record(&self, path: &Path) {
        let canonical = normalize_path(path);
        eprintln!(
            "{{\"level\":\"trace\",\"component\":\"recent_writes\",\"action\":\"record\",\"path\":\"{}\"}}",
            canonical.display()
        );
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        if let Some(parent) = canonical.parent() {
            if !parent.as_os_str().is_empty() {
                entries.insert(parent.to_path_buf(), now);
            }
        }
        entries.insert(canonical, now);
    }

    /// Returns true if `path` should be suppressed by the watcher.
    ///
    /// `event_time` is the `Instant` when the OS delivered the filesystem event
    /// to our notify callback — NOT the current wall-clock time. This makes the
    /// check independent of debounce delays: even if processing happens seconds
    /// later, the comparison uses when the event actually arrived.
    ///
    /// Suppression triggers if any of these hold:
    /// 1. A suppression guard is currently active (counter > 0).
    /// 2. The event arrived before (or shortly after) the last guard dropped.
    /// 3. The path was recorded by `record()` and hasn't expired.
    pub fn should_suppress(&self, path: &Path, event_time: Instant) -> bool {
        if self.active_suppressions.load(Ordering::Acquire) > 0 {
            return true;
        }
        if let Some(ended_at) = *self.suppress_ended_at.lock().unwrap() {
            if event_time <= ended_at + ARRIVAL_JITTER_BUFFER {
                return true;
            }
        }
        let canonical = normalize_path(path);
        let entries = self.entries.lock().unwrap();
        entries
            .get(&canonical)
            .is_some_and(|recorded_at| {
                event_time.saturating_duration_since(*recorded_at) < self.ttl
            })
    }

    /// Increment the active suppression counter. While any guard is active,
    /// `should_suppress` returns true for all paths.
    pub fn begin_suppression(&self) {
        self.active_suppressions.fetch_add(1, Ordering::Release);
    }

    /// Decrement the active suppression counter. When the counter reaches zero,
    /// records the current instant so that events whose arrival time predates
    /// this moment (plus jitter buffer) are still suppressed.
    pub fn end_suppression(&self) {
        let prev = self.active_suppressions.fetch_sub(1, Ordering::Release);
        debug_assert!(prev > 0, "end_suppression called without matching begin_suppression");
        if prev == 1 {
            *self.suppress_ended_at.lock().unwrap() = Some(Instant::now());
        }
    }

    /// Remove entries older than the TTL.
    pub fn prune_expired(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, timestamp| timestamp.elapsed() < self.ttl);
    }

    /// Create a tracker with the given TTL (for tests).
    #[cfg(test)]
    pub fn default_with_ttl(ttl: Duration) -> Self {
        Self::new(ttl)
    }

    /// Number of tracked entries (for testing).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Whether the tracker is empty (for testing).
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.lock().unwrap().is_empty()
    }
}

impl Default for RecentBackendWrites {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

/// Normalize a path to a canonical form for comparison.
///
/// Converts backslashes to forward slashes and lowercases on Windows for
/// case-insensitive matching. Also strips the Windows extended-length path
/// prefix (`\\?\` / `//?/`) which `ReadDirectoryChangesW` (via the `notify`
/// crate) can prepend, causing mismatches with paths from the P9 server.
fn normalize_path(path: &Path) -> PathBuf {
    let normalized = path.to_string_lossy().replace('\\', "/");
    // Strip Windows extended-length path prefix that notify may add.
    let normalized = normalized
        .strip_prefix("//?/")
        .unwrap_or(&normalized);
    #[cfg(target_os = "windows")]
    let normalized = normalized.to_lowercase();
    PathBuf::from(normalized)
}

/// A decorator around a `WriteInterceptor` that records mutated paths in a
/// `RecentBackendWrites` tracker. This allows the filesystem watcher to
/// distinguish writes originating from the sandbox's filesystem backends
/// from genuine external modifications.
///
/// Injected at the sandbox level — no changes to the `p9` or
/// `virtiofs-backend` crates are needed.
pub struct WriteTrackingInterceptor {
    inner: std::sync::Arc<dyn WriteInterceptor>,
    recent_writes: std::sync::Arc<RecentBackendWrites>,
}

impl WriteTrackingInterceptor {
    pub fn new(
        inner: std::sync::Arc<dyn WriteInterceptor>,
        recent_writes: std::sync::Arc<RecentBackendWrites>,
    ) -> Self {
        Self {
            inner,
            recent_writes,
        }
    }
}

impl WriteInterceptor for WriteTrackingInterceptor {
    fn pre_write(&self, path: &Path) -> Result<()> {
        self.recent_writes.record(path);
        self.inner.pre_write(path)
    }

    fn pre_unlink(&self, path: &Path, is_dir: bool) -> Result<()> {
        self.recent_writes.record(path);
        self.inner.pre_unlink(path, is_dir)
    }

    fn pre_rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.recent_writes.record(from);
        self.recent_writes.record(to);
        self.inner.pre_rename(from, to)
    }

    fn post_create(&self, path: &Path) -> Result<()> {
        self.recent_writes.record(path);
        self.inner.post_create(path)
    }

    fn post_mkdir(&self, path: &Path) -> Result<()> {
        self.recent_writes.record(path);
        self.inner.post_mkdir(path)
    }

    fn pre_setattr(&self, path: &Path) -> Result<()> {
        self.recent_writes.record(path);
        self.inner.pre_setattr(path)
    }

    fn pre_link(&self, target: &Path, link_path: &Path) -> Result<()> {
        self.recent_writes.record(link_path);
        self.inner.pre_link(target, link_path)
    }

    fn post_symlink(&self, target: &Path, link_path: &Path) -> Result<()> {
        self.recent_writes.record(link_path);
        self.inner.post_symlink(target, link_path)
    }

    fn pre_xattr(&self, path: &Path) -> Result<()> {
        self.recent_writes.record(path);
        self.inner.pre_xattr(path)
    }

    fn pre_open_trunc(&self, path: &Path) -> Result<()> {
        self.recent_writes.record(path);
        self.inner.pre_open_trunc(path)
    }

    fn pre_fallocate(&self, path: &Path) -> Result<()> {
        self.recent_writes.record(path);
        self.inner.pre_fallocate(path)
    }

    fn pre_copy_file_range(&self, dst_path: &Path) -> Result<()> {
        self.recent_writes.record(dst_path);
        self.inner.pre_copy_file_range(dst_path)
    }

    fn current_step(&self) -> Option<StepId> {
        self.inner.current_step()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_check_recent() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        let path = Path::new("/tmp/test/file.txt");
        let now = Instant::now();

        assert!(!rw.should_suppress(path, now));
        rw.record(path);
        assert!(rw.should_suppress(path, now));
    }

    #[test]
    fn expired_entries_not_recent() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_millis(1));
        let path = Path::new("/tmp/test/file.txt");

        rw.record(path);
        std::thread::sleep(Duration::from_millis(10));
        assert!(!rw.should_suppress(path, Instant::now()));
    }

    #[test]
    fn prune_removes_expired() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_millis(1));
        rw.record(Path::new("/tmp/a"));
        rw.record(Path::new("/tmp/b"));
        std::thread::sleep(Duration::from_millis(10));
        rw.prune_expired();
        assert_eq!(rw.len(), 0);
    }

    #[test]
    fn prune_keeps_fresh() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        rw.record(Path::new("/tmp/a"));
        rw.prune_expired();
        // record() stores both the path and its parent directory.
        assert_eq!(rw.len(), 2);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn case_insensitive_on_windows() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        rw.record(Path::new("C:\\Users\\Test\\File.txt"));
        assert!(rw.should_suppress(Path::new("C:\\Users\\test\\file.txt"), Instant::now()));
    }

    #[test]
    fn backslash_forward_slash_equivalent() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        rw.record(Path::new("some/path/file.txt"));
        // On Windows, Path::new("some\\path\\file.txt") would normalize too
        assert!(rw.should_suppress(Path::new("some/path/file.txt"), Instant::now()));
    }

    #[test]
    fn default_ttl_is_5s() {
        let rw = RecentBackendWrites::default();
        assert_eq!(rw.ttl, Duration::from_secs(5));
    }

    #[test]
    fn parent_directory_recorded() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        rw.record(Path::new("/workspace/subdir/file.txt"));
        let now = Instant::now();
        assert!(rw.should_suppress(Path::new("/workspace/subdir/file.txt"), now));
        assert!(rw.should_suppress(Path::new("/workspace/subdir"), now));
    }

    #[test]
    fn normalize_strips_extended_length_prefix() {
        // Paths with \\?\ prefix (Windows extended-length) should match regular paths.
        let with_prefix = normalize_path(Path::new("//?/C:/Projects/test/file.txt"));
        let without_prefix = normalize_path(Path::new("C:/Projects/test/file.txt"));
        assert_eq!(with_prefix, without_prefix);
    }

    #[test]
    fn extended_length_path_matches_regular() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        // P9 server records a regular path.
        rw.record(Path::new("C:/Projects/test/file.txt"));
        // Filesystem watcher checks with extended-length path (after \\?\ → //?/).
        assert!(rw.should_suppress(Path::new("//?/C:/Projects/test/file.txt"), Instant::now()));
    }

    #[test]
    fn regular_path_matches_extended_length_record() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        // Recorded with extended-length prefix.
        rw.record(Path::new("//?/C:/Projects/test/file.txt"));
        // Checked with regular path.
        assert!(rw.should_suppress(Path::new("C:/Projects/test/file.txt"), Instant::now()));
    }

    #[test]
    fn suppression_checked_by_event_time_not_wall_clock() {
        // Simulates the race condition: event arrives during suppression but is
        // processed (after debounce) long after the suppression guard drops.
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));

        // Suppression starts.
        rw.begin_suppression();

        // Event arrives while suppression is active.
        let event_time = Instant::now();

        // Suppression ends.
        rw.end_suppression();

        // Simulate debounce delay — processing happens much later.
        std::thread::sleep(Duration::from_millis(200));

        // The event should still be suppressed because it arrived during the
        // suppression window, even though we're checking long after it ended.
        assert!(rw.should_suppress(Path::new("/any/path"), event_time));
    }

    #[test]
    fn event_after_suppression_not_suppressed() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));

        rw.begin_suppression();
        rw.end_suppression();

        // Wait past the jitter buffer.
        std::thread::sleep(Duration::from_millis(100));

        // Event arrives well after suppression ended.
        let event_time = Instant::now();
        assert!(!rw.should_suppress(Path::new("/some/path"), event_time));
    }
}
