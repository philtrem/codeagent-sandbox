use std::fs;
use std::path::Path;

use codeagent_common::StepId;

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
}
