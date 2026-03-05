use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use codeagent_common::{Result, StepId};
use codeagent_interceptor::write_interceptor::WriteInterceptor;

/// Default TTL for recent write records: 5 seconds.
/// Accounts for OS event delivery delay (especially macOS FSEvents).
const DEFAULT_TTL: Duration = Duration::from_secs(5);

/// Tracks paths recently written by the sandbox's own backends (filesystem
/// channel or MCP API). The filesystem watcher checks this map to distinguish
/// backend-originated writes from genuine external modifications.
pub struct RecentBackendWrites {
    entries: Mutex<HashMap<PathBuf, Instant>>,
    ttl: Duration,
}

impl RecentBackendWrites {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Record that the sandbox just wrote to `path`.
    pub fn record(&self, path: &Path) {
        let canonical = normalize_path(path);
        self.entries.lock().unwrap().insert(canonical, Instant::now());
    }

    /// Returns true if `path` was written by the sandbox within the TTL window.
    pub fn was_recent(&self, path: &Path) -> bool {
        let canonical = normalize_path(path);
        let entries = self.entries.lock().unwrap();
        entries
            .get(&canonical)
            .is_some_and(|timestamp| timestamp.elapsed() < self.ttl)
    }

    /// Remove entries older than the TTL.
    pub fn prune_expired(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, timestamp| timestamp.elapsed() < self.ttl);
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
/// Uses forward slashes and lowercases on Windows for case-insensitive matching.
fn normalize_path(path: &Path) -> PathBuf {
    let normalized = path.to_string_lossy().replace('\\', "/");
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
        let rw = RecentBackendWrites::new(Duration::from_secs(10));
        let path = Path::new("/tmp/test/file.txt");

        assert!(!rw.was_recent(path));
        rw.record(path);
        assert!(rw.was_recent(path));
    }

    #[test]
    fn expired_entries_not_recent() {
        let rw = RecentBackendWrites::new(Duration::from_millis(1));
        let path = Path::new("/tmp/test/file.txt");

        rw.record(path);
        std::thread::sleep(Duration::from_millis(10));
        assert!(!rw.was_recent(path));
    }

    #[test]
    fn prune_removes_expired() {
        let rw = RecentBackendWrites::new(Duration::from_millis(1));
        rw.record(Path::new("/tmp/a"));
        rw.record(Path::new("/tmp/b"));
        std::thread::sleep(Duration::from_millis(10));
        rw.prune_expired();
        assert_eq!(rw.len(), 0);
    }

    #[test]
    fn prune_keeps_fresh() {
        let rw = RecentBackendWrites::new(Duration::from_secs(10));
        rw.record(Path::new("/tmp/a"));
        rw.prune_expired();
        assert_eq!(rw.len(), 1);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn case_insensitive_on_windows() {
        let rw = RecentBackendWrites::new(Duration::from_secs(10));
        rw.record(Path::new("C:\\Users\\Test\\File.txt"));
        assert!(rw.was_recent(Path::new("C:\\Users\\test\\file.txt")));
    }

    #[test]
    fn backslash_forward_slash_equivalent() {
        let rw = RecentBackendWrites::new(Duration::from_secs(10));
        rw.record(Path::new("some/path/file.txt"));
        // On Windows, Path::new("some\\path\\file.txt") would normalize too
        assert!(rw.was_recent(Path::new("some/path/file.txt")));
    }

    #[test]
    fn default_ttl_is_5s() {
        let rw = RecentBackendWrites::default();
        assert_eq!(rw.ttl, Duration::from_secs(5));
    }
}
