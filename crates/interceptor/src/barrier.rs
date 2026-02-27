use std::collections::HashSet;
use std::fs;
use std::path::Path;

use chrono::Utc;
use codeagent_common::{BarrierId, BarrierInfo, Result, StepId};

/// Tracks undo barriers created by external modification detection.
///
/// Barriers are stored in chronological order and persisted to disk as a JSON
/// array in `{undo_dir}/barriers.json`.
pub struct BarrierTracker {
    barriers: Vec<BarrierInfo>,
    next_barrier_id: BarrierId,
}

impl Default for BarrierTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl BarrierTracker {
    pub fn new() -> Self {
        Self {
            barriers: Vec::new(),
            next_barrier_id: 1,
        }
    }

    /// Load barrier state from disk. Returns a fresh tracker if the file
    /// doesn't exist or is corrupt.
    pub fn load(undo_dir: &Path) -> Self {
        let path = undo_dir.join("barriers.json");
        if !path.exists() {
            return Self::new();
        }

        match fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<Vec<BarrierInfo>>(&json) {
                Ok(barriers) => {
                    let next_id = barriers
                        .iter()
                        .map(|b| b.barrier_id)
                        .max()
                        .unwrap_or(0)
                        + 1;
                    Self {
                        barriers,
                        next_barrier_id: next_id,
                    }
                }
                Err(_) => Self::new(),
            },
            Err(_) => Self::new(),
        }
    }

    /// Persist barrier state to disk.
    pub fn save(&self, undo_dir: &Path) -> Result<()> {
        let path = undo_dir.join("barriers.json");
        let json = serde_json::to_string_pretty(&self.barriers)?;
        fs::write(&path, json)?;
        Ok(())
    }

    /// Create a new barrier after the given step, returning the created barrier.
    pub fn create_barrier(
        &mut self,
        after_step_id: StepId,
        affected_paths: Vec<std::path::PathBuf>,
    ) -> BarrierInfo {
        let barrier = BarrierInfo {
            barrier_id: self.next_barrier_id,
            after_step_id,
            timestamp: Utc::now(),
            affected_paths,
        };
        self.next_barrier_id += 1;
        self.barriers.push(barrier.clone());
        barrier
    }

    /// Return all barriers that would block rolling back the given steps.
    ///
    /// A barrier with `after_step_id = S` blocks rollback if S is in the set of
    /// steps being rolled back — because the external modification happened
    /// after S completed and rolling back S would destroy it.
    pub fn barriers_blocking_rollback(&self, steps_to_rollback: &[StepId]) -> Vec<&BarrierInfo> {
        let step_set: HashSet<StepId> = steps_to_rollback.iter().copied().collect();
        self.barriers
            .iter()
            .filter(|b| step_set.contains(&b.after_step_id))
            .collect()
    }

    /// Remove all barriers whose `after_step_id` is in the given set.
    /// Used during forced rollback (pop semantics extends to barriers).
    pub fn remove_barriers_for_steps(&mut self, step_ids: &[StepId]) {
        let step_set: HashSet<StepId> = step_ids.iter().copied().collect();
        self.barriers
            .retain(|b| !step_set.contains(&b.after_step_id));
    }

    /// Return a snapshot of all current barriers.
    pub fn barriers(&self) -> Vec<BarrierInfo> {
        self.barriers.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_barrier_assigns_monotonic_ids() {
        let mut tracker = BarrierTracker::new();
        let b1 = tracker.create_barrier(1, vec![]);
        let b2 = tracker.create_barrier(2, vec![]);
        let b3 = tracker.create_barrier(3, vec![]);
        assert_eq!(b1.barrier_id, 1);
        assert_eq!(b2.barrier_id, 2);
        assert_eq!(b3.barrier_id, 3);
    }

    #[test]
    fn barriers_blocking_rollback_filters_correctly() {
        let mut tracker = BarrierTracker::new();
        tracker.create_barrier(1, vec![]);
        tracker.create_barrier(3, vec![]);

        // Rolling back steps [3, 4] should be blocked by barrier after step 3
        let blocking = tracker.barriers_blocking_rollback(&[3, 4]);
        assert_eq!(blocking.len(), 1);
        assert_eq!(blocking[0].after_step_id, 3);

        // Rolling back only step 4 should not be blocked
        let blocking = tracker.barriers_blocking_rollback(&[4]);
        assert!(blocking.is_empty());

        // Rolling back steps [1, 2, 3] hits both barriers
        let blocking = tracker.barriers_blocking_rollback(&[1, 2, 3]);
        assert_eq!(blocking.len(), 2);
    }

    #[test]
    fn remove_barriers_for_steps() {
        let mut tracker = BarrierTracker::new();
        tracker.create_barrier(1, vec![]);
        tracker.create_barrier(2, vec![]);
        tracker.create_barrier(3, vec![]);

        tracker.remove_barriers_for_steps(&[1, 3]);
        let remaining = tracker.barriers();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].after_step_id, 2);
    }

    #[test]
    fn save_and_load_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let undo_dir = temp.path();

        let mut tracker = BarrierTracker::new();
        tracker.create_barrier(
            5,
            vec![std::path::PathBuf::from("src/main.rs")],
        );
        tracker.create_barrier(10, vec![]);
        tracker.save(undo_dir).unwrap();

        let loaded = BarrierTracker::load(undo_dir);
        let barriers = loaded.barriers();
        assert_eq!(barriers.len(), 2);
        assert_eq!(barriers[0].after_step_id, 5);
        assert_eq!(barriers[0].affected_paths.len(), 1);
        assert_eq!(barriers[1].after_step_id, 10);
        assert_eq!(loaded.next_barrier_id, 3);
    }

    #[test]
    fn load_missing_file_returns_fresh_tracker() {
        let temp = tempfile::tempdir().unwrap();
        let tracker = BarrierTracker::load(temp.path());
        assert!(tracker.barriers().is_empty());
    }

    #[test]
    fn load_corrupt_file_returns_fresh_tracker() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("barriers.json"), "not valid json{{{").unwrap();
        let tracker = BarrierTracker::load(temp.path());
        assert!(tracker.barriers().is_empty());
    }
}
