use std::fs;
use std::path::Path;

use codeagent_common::{ResourceLimitsConfig, StepId};

use crate::barrier::BarrierTracker;
use crate::step_tracker::StepTracker;

/// Calculate the total size (in bytes) of all files within a step directory.
pub fn calculate_step_size(step_dir: &Path) -> codeagent_common::Result<u64> {
    let mut total: u64 = 0;

    if !step_dir.exists() {
        return Ok(0);
    }

    for entry in fs::read_dir(step_dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            total += metadata.len();
        } else if metadata.is_dir() {
            total += calculate_step_size(&entry.path())?;
        }
    }

    Ok(total)
}

/// Calculate the total size of the entire undo log (all completed step directories).
pub fn calculate_total_log_size(
    steps_dir: &Path,
    completed_steps: &[StepId],
) -> codeagent_common::Result<u64> {
    let mut total: u64 = 0;
    for step_id in completed_steps {
        let step_dir = steps_dir.join(step_id.to_string());
        total += calculate_step_size(&step_dir)?;
    }
    Ok(total)
}

/// Evict oldest completed steps to satisfy resource limits.
///
/// Returns the list of step IDs that were evicted. Eviction removes the step's
/// directory from disk, its entry from the step tracker, and any barriers
/// associated with it.
pub fn evict_if_needed(
    steps_dir: &Path,
    step_tracker: &StepTracker,
    barrier_tracker: &mut BarrierTracker,
    limits: &ResourceLimitsConfig,
    undo_dir: &Path,
) -> codeagent_common::Result<Vec<StepId>> {
    let mut evicted: Vec<StepId> = Vec::new();
    let mut completed = step_tracker.completed_steps();

    // Phase 1: Evict by step count
    if let Some(max_count) = limits.max_step_count {
        while completed.len() > max_count {
            let oldest = completed[0];
            evict_step(steps_dir, oldest)?;
            step_tracker.remove_completed_step(oldest);
            completed.remove(0);
            evicted.push(oldest);
        }
    }

    // Phase 2: Evict by total log size
    if let Some(max_size) = limits.max_log_size_bytes {
        let mut current_size = calculate_total_log_size(steps_dir, &completed)?;
        while current_size > max_size && !completed.is_empty() {
            let oldest = completed[0];
            let step_size = calculate_step_size(&steps_dir.join(oldest.to_string()))?;
            evict_step(steps_dir, oldest)?;
            step_tracker.remove_completed_step(oldest);
            completed.remove(0);
            current_size = current_size.saturating_sub(step_size);
            evicted.push(oldest);
        }
    }

    // Clean up barriers for evicted steps
    if !evicted.is_empty() {
        barrier_tracker.remove_barriers_for_steps(&evicted);
        barrier_tracker.save(undo_dir)?;
    }

    Ok(evicted)
}

/// Delete a step's directory from disk.
fn evict_step(steps_dir: &Path, step_id: StepId) -> codeagent_common::Result<()> {
    let step_dir = steps_dir.join(step_id.to_string());
    if step_dir.exists() {
        fs::remove_dir_all(&step_dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn calculate_step_size_empty_dir() {
        let dir = TempDir::new().unwrap();
        let step = dir.path().join("step");
        fs::create_dir(&step).unwrap();
        assert_eq!(calculate_step_size(&step).unwrap(), 0);
    }

    #[test]
    fn calculate_step_size_with_files() {
        let dir = TempDir::new().unwrap();
        let step = dir.path().join("step");
        fs::create_dir(&step).unwrap();
        fs::write(step.join("manifest.json"), "{}").unwrap();
        let preimages = step.join("preimages");
        fs::create_dir(&preimages).unwrap();
        fs::write(preimages.join("abc.dat"), vec![0u8; 100]).unwrap();
        fs::write(preimages.join("abc.meta.json"), vec![0u8; 50]).unwrap();

        let size = calculate_step_size(&step).unwrap();
        assert_eq!(size, 2 + 100 + 50); // "{}" is 2 bytes
    }

    #[test]
    fn calculate_step_size_nonexistent() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            calculate_step_size(&dir.path().join("nope")).unwrap(),
            0
        );
    }

    #[test]
    fn evict_by_step_count() {
        let dir = TempDir::new().unwrap();
        let steps_dir = dir.path().join("steps");
        fs::create_dir(&steps_dir).unwrap();

        let tracker = StepTracker::new();
        for id in 1..=5 {
            let step = steps_dir.join(id.to_string());
            fs::create_dir(&step).unwrap();
            fs::write(step.join("manifest.json"), "{}").unwrap();
            tracker.add_completed_step(id);
        }

        let mut barrier_tracker = BarrierTracker::load(dir.path());
        let limits = ResourceLimitsConfig {
            max_step_count: Some(3),
            ..Default::default()
        };

        let evicted =
            evict_if_needed(&steps_dir, &tracker, &mut barrier_tracker, &limits, dir.path())
                .unwrap();

        assert_eq!(evicted, vec![1, 2]);
        assert_eq!(tracker.completed_steps(), vec![3, 4, 5]);
        assert!(!steps_dir.join("1").exists());
        assert!(!steps_dir.join("2").exists());
        assert!(steps_dir.join("3").exists());
    }

    #[test]
    fn evict_by_log_size() {
        let dir = TempDir::new().unwrap();
        let steps_dir = dir.path().join("steps");
        fs::create_dir(&steps_dir).unwrap();

        let tracker = StepTracker::new();
        for id in 1..=3 {
            let step = steps_dir.join(id.to_string());
            fs::create_dir(&step).unwrap();
            // Each step ~100 bytes
            fs::write(step.join("data"), vec![0u8; 100]).unwrap();
            tracker.add_completed_step(id);
        }

        let mut barrier_tracker = BarrierTracker::load(dir.path());
        let limits = ResourceLimitsConfig {
            max_log_size_bytes: Some(200),
            ..Default::default()
        };

        let evicted =
            evict_if_needed(&steps_dir, &tracker, &mut barrier_tracker, &limits, dir.path())
                .unwrap();

        assert_eq!(evicted, vec![1]);
        assert_eq!(tracker.completed_steps(), vec![2, 3]);
    }

    #[test]
    fn evict_no_limits_does_nothing() {
        let dir = TempDir::new().unwrap();
        let steps_dir = dir.path().join("steps");
        fs::create_dir(&steps_dir).unwrap();

        let tracker = StepTracker::new();
        for id in 1..=5 {
            let step = steps_dir.join(id.to_string());
            fs::create_dir(&step).unwrap();
            tracker.add_completed_step(id);
        }

        let mut barrier_tracker = BarrierTracker::load(dir.path());
        let limits = ResourceLimitsConfig::default();

        let evicted =
            evict_if_needed(&steps_dir, &tracker, &mut barrier_tracker, &limits, dir.path())
                .unwrap();

        assert!(evicted.is_empty());
        assert_eq!(tracker.completed_steps().len(), 5);
    }
}
