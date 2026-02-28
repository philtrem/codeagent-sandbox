use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

/// Tracks the number of in-flight filesystem operations.
///
/// The filesystem backend calls [`begin_operation`] when it starts handling a
/// request and [`end_operation`] when it finishes. The control channel handler
/// uses [`wait_for_drain`] during the quiescence window to wait until all
/// in-flight operations complete.
///
/// This type is cheaply cloneable — the handler and filesystem backend each
/// hold their own clone, sharing the same underlying counter and notifier.
#[derive(Clone)]
pub struct InFlightTracker {
    count: Arc<AtomicUsize>,
    drain_notify: Arc<Notify>,
}

impl InFlightTracker {
    pub fn new() -> Self {
        Self {
            count: Arc::new(AtomicUsize::new(0)),
            drain_notify: Arc::new(Notify::new()),
        }
    }

    /// Called by the filesystem backend when it begins handling a request.
    pub fn begin_operation(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    /// Called by the filesystem backend when it finishes handling a request.
    ///
    /// If the count reaches zero, any task waiting in [`wait_for_drain`] is woken.
    pub fn end_operation(&self) {
        let previous = self.count.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previous > 0, "end_operation called more times than begin_operation");
        if previous == 1 {
            self.drain_notify.notify_waiters();
        }
    }

    /// Returns the current number of in-flight operations.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Wait until the in-flight count reaches zero or `timeout` elapses.
    ///
    /// Returns `true` if the count drained to zero, `false` on timeout.
    /// Compatible with `tokio::time::pause()` for deterministic tests.
    pub async fn wait_for_drain(&self, timeout: Duration) -> bool {
        if self.count() == 0 {
            return true;
        }

        tokio::select! {
            _ = self.wait_for_zero() => true,
            _ = tokio::time::sleep(timeout) => self.count() == 0,
        }
    }

    /// Internal: loops until count reaches zero, using Notify to avoid polling.
    async fn wait_for_zero(&self) {
        loop {
            if self.count() == 0 {
                return;
            }
            self.drain_notify.notified().await;
            if self.count() == 0 {
                return;
            }
        }
    }
}

impl Default for InFlightTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_has_zero_count() {
        let tracker = InFlightTracker::new();
        assert_eq!(tracker.count(), 0);
    }

    #[test]
    fn begin_increments_count() {
        let tracker = InFlightTracker::new();
        tracker.begin_operation();
        assert_eq!(tracker.count(), 1);
        tracker.begin_operation();
        assert_eq!(tracker.count(), 2);
        tracker.end_operation();
        tracker.end_operation();
    }

    #[test]
    fn end_decrements_count() {
        let tracker = InFlightTracker::new();
        tracker.begin_operation();
        tracker.begin_operation();
        tracker.end_operation();
        assert_eq!(tracker.count(), 1);
        tracker.end_operation();
        assert_eq!(tracker.count(), 0);
    }

    #[test]
    fn clone_shares_state() {
        let tracker1 = InFlightTracker::new();
        let tracker2 = tracker1.clone();
        tracker1.begin_operation();
        assert_eq!(tracker2.count(), 1);
        tracker2.end_operation();
        assert_eq!(tracker1.count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_drain_returns_immediately_when_zero() {
        let tracker = InFlightTracker::new();
        let drained = tracker.wait_for_drain(Duration::from_secs(1)).await;
        assert!(drained);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_drain_wakes_on_completion() {
        let tracker = InFlightTracker::new();
        tracker.begin_operation();

        let tracker_clone = tracker.clone();
        let handle = tokio::spawn(async move {
            tracker_clone.wait_for_drain(Duration::from_secs(10)).await
        });

        // Let the spawned task start waiting
        tokio::task::yield_now().await;

        tracker.end_operation();
        let drained = handle.await.unwrap();
        assert!(drained);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_drain_times_out() {
        let tracker = InFlightTracker::new();
        tracker.begin_operation();

        let drained = tracker.wait_for_drain(Duration::from_millis(100)).await;
        assert!(!drained);
        assert_eq!(tracker.count(), 1);

        // Clean up
        tracker.end_operation();
    }
}
