use std::sync::Mutex;

use codeagent_common::{CodeAgentError, Result, StepId};

pub struct StepTracker {
    inner: Mutex<StepTrackerInner>,
}

struct StepTrackerInner {
    active_step: Option<StepId>,
    completed_steps: Vec<StepId>,
}

impl Default for StepTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl StepTracker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StepTrackerInner {
                active_step: None,
                completed_steps: Vec::new(),
            }),
        }
    }

    /// Open a new step. Returns error if a step is already active.
    pub fn open_step(&self, id: StepId) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(active) = inner.active_step {
            return Err(CodeAgentError::StepAlreadyActive { step_id: active });
        }
        inner.active_step = Some(id);
        Ok(())
    }

    /// Close the current step. Returns error if the given ID doesn't match.
    pub fn close_step(&self, id: StepId) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        match inner.active_step {
            Some(active) if active == id => {
                inner.active_step = None;
                inner.completed_steps.push(id);
                Ok(())
            }
            Some(_) => Err(CodeAgentError::StepNotActive { step_id: id }),
            None => Err(CodeAgentError::NoActiveStep),
        }
    }

    /// Returns the currently active step ID, if any.
    pub fn current_step(&self) -> Option<StepId> {
        self.inner.lock().unwrap().active_step
    }

    /// Returns completed step IDs in order.
    pub fn completed_steps(&self) -> Vec<StepId> {
        self.inner.lock().unwrap().completed_steps.clone()
    }

    /// Remove a step from the completed list (used during rollback pop).
    pub fn remove_completed_step(&self, id: StepId) {
        let mut inner = self.inner.lock().unwrap();
        inner.completed_steps.retain(|&s| s != id);
    }

    /// Add a step ID directly to the completed list.
    /// Used during initialization to reconstruct state from the on-disk steps/ directory.
    pub fn add_completed_step(&self, id: StepId) {
        let mut inner = self.inner.lock().unwrap();
        inner.completed_steps.push(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_close_step() {
        let tracker = StepTracker::new();
        assert!(tracker.current_step().is_none());

        tracker.open_step(1).unwrap();
        assert_eq!(tracker.current_step(), Some(1));

        tracker.close_step(1).unwrap();
        assert!(tracker.current_step().is_none());
        assert_eq!(tracker.completed_steps(), vec![1]);
    }

    #[test]
    fn double_open_fails() {
        let tracker = StepTracker::new();
        tracker.open_step(1).unwrap();

        let err = tracker.open_step(2).unwrap_err();
        assert!(matches!(err, CodeAgentError::StepAlreadyActive { step_id: 1 }));
    }

    #[test]
    fn close_wrong_id_fails() {
        let tracker = StepTracker::new();
        tracker.open_step(1).unwrap();

        let err = tracker.close_step(2).unwrap_err();
        assert!(matches!(err, CodeAgentError::StepNotActive { step_id: 2 }));
    }

    #[test]
    fn close_without_open_fails() {
        let tracker = StepTracker::new();
        let err = tracker.close_step(1).unwrap_err();
        assert!(matches!(err, CodeAgentError::NoActiveStep));
    }

    #[test]
    fn completed_steps_tracking() {
        let tracker = StepTracker::new();
        for id in 1..=3 {
            tracker.open_step(id).unwrap();
            tracker.close_step(id).unwrap();
        }
        assert_eq!(tracker.completed_steps(), vec![1, 2, 3]);
    }

    #[test]
    fn remove_completed_step() {
        let tracker = StepTracker::new();
        for id in 1..=3 {
            tracker.open_step(id).unwrap();
            tracker.close_step(id).unwrap();
        }
        tracker.remove_completed_step(2);
        assert_eq!(tracker.completed_steps(), vec![1, 3]);
    }

    #[test]
    fn add_completed_step() {
        let tracker = StepTracker::new();
        tracker.add_completed_step(5);
        tracker.add_completed_step(10);
        assert_eq!(tracker.completed_steps(), vec![5, 10]);
    }
}
