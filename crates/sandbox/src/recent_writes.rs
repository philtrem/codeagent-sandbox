use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use codeagent_common::{Result, StepId};
use codeagent_interceptor::write_interceptor::WriteInterceptor;

/// Default TTL for recent write records: 5 seconds.
/// Accounts for OS event delivery delay (especially macOS FSEvents).
const DEFAULT_TTL: Duration = Duration::from_secs(5);

/// Extra buffer added on top of the watcher tick interval to account for
/// OS event delivery latency (especially macOS FSEvents batching).
const OS_EVENT_DELIVERY_BUFFER: Duration = Duration::from_millis(300);

/// Tracks paths recently written by the sandbox's own backends (filesystem
/// channel or MCP API). The filesystem watcher checks this map to distinguish
/// backend-originated writes from genuine external modifications.
///
/// Also provides a deadline-based suppression mechanism that the orchestrator
/// activates during rollback. While the deadline has not expired, `was_recent`
/// returns true for all paths, causing the watcher to skip all events.
pub struct RecentBackendWrites {
    entries: Mutex<HashMap<PathBuf, Instant>>,
    ttl: Duration,
    /// Grace period for rollback suppression, derived from the watcher tick
    /// interval plus an OS event delivery buffer.
    suppress_grace: Duration,
    /// When set, all paths are suppressed until this instant passes.
    suppress_until: Mutex<Option<Instant>>,
}

impl RecentBackendWrites {
    pub fn new(ttl: Duration, watcher_tick: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            suppress_grace: watcher_tick + OS_EVENT_DELIVERY_BUFFER,
            suppress_until: Mutex::new(None),
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

    /// Returns true if `path` should be suppressed by the watcher — either
    /// because it was written by the sandbox within the TTL window, or because
    /// the suppression deadline has not yet expired (e.g. during/after rollback).
    pub fn was_recent(&self, path: &Path) -> bool {
        if let Some(deadline) = *self.suppress_until.lock().unwrap() {
            if Instant::now() < deadline {
                return true;
            }
        }
        let canonical = normalize_path(path);
        let entries = self.entries.lock().unwrap();
        entries
            .get(&canonical)
            .is_some_and(|timestamp| timestamp.elapsed() < self.ttl)
    }

    /// Begin suppressing all watcher events. Suppression lasts until the
    /// grace period (watcher tick + OS buffer) expires. Call
    /// `extend_suppression` to push the deadline further into the future
    /// (e.g. after rollback completes).
    pub fn begin_suppression(&self) {
        *self.suppress_until.lock().unwrap() = Some(Instant::now() + self.suppress_grace);
    }

    /// Extend the suppression deadline by the grace period from now. Called
    /// when rollback completes to cover late-arriving filesystem events.
    pub fn extend_suppression(&self) {
        *self.suppress_until.lock().unwrap() = Some(Instant::now() + self.suppress_grace);
    }

    /// Remove entries older than the TTL.
    pub fn prune_expired(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, timestamp| timestamp.elapsed() < self.ttl);
    }

    /// Create a tracker with the given TTL and a default watcher tick (for tests).
    #[cfg(test)]
    pub fn default_with_ttl(ttl: Duration) -> Self {
        Self::new(ttl, Duration::from_millis(200))
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
        Self::new(DEFAULT_TTL, Duration::from_millis(200))
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

        assert!(!rw.was_recent(path));
        rw.record(path);
        assert!(rw.was_recent(path));
    }

    #[test]
    fn expired_entries_not_recent() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_millis(1));
        let path = Path::new("/tmp/test/file.txt");

        rw.record(path);
        std::thread::sleep(Duration::from_millis(10));
        assert!(!rw.was_recent(path));
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
        assert!(rw.was_recent(Path::new("C:\\Users\\test\\file.txt")));
    }

    #[test]
    fn backslash_forward_slash_equivalent() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        rw.record(Path::new("some/path/file.txt"));
        // On Windows, Path::new("some\\path\\file.txt") would normalize too
        assert!(rw.was_recent(Path::new("some/path/file.txt")));
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
        assert!(rw.was_recent(Path::new("/workspace/subdir/file.txt")));
        assert!(rw.was_recent(Path::new("/workspace/subdir")));
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
        assert!(rw.was_recent(Path::new("//?/C:/Projects/test/file.txt")));
    }

    #[test]
    fn regular_path_matches_extended_length_record() {
        let rw = RecentBackendWrites::default_with_ttl(Duration::from_secs(10));
        // Recorded with extended-length prefix.
        rw.record(Path::new("//?/C:/Projects/test/file.txt"));
        // Checked with regular path.
        assert!(rw.was_recent(Path::new("C:/Projects/test/file.txt")));
    }
}
