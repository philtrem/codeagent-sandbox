use std::fs;
use std::path::Path;

use codeagent_common::SymlinkPolicy;

use crate::manifest::StepManifest;
use crate::preimage::{PreimageFileType, PreimageMetadata, read_preimage_metadata};

/// Execute rollback for a single step.
///
/// Two-pass algorithm per the spec (testing-plan §1.2):
/// 1. Delete created paths (deepest-first), recreate dirs (shallowest-first),
///    then restore file contents and metadata.
/// 2. Restore directory metadata (deepest-first) so child operations don't
///    clobber parent mtime.
pub fn rollback_step(
    step_dir: &Path,
    working_root: &Path,
    symlink_policy: SymlinkPolicy,
) -> codeagent_common::Result<()> {
    let manifest = StepManifest::read_from(step_dir)?;
    let preimage_dir = step_dir.join("preimages");

    // Classify entries
    let mut dirs_to_restore: Vec<(String, String)> = Vec::new(); // (rel_path, path_hash)
    let mut files_to_restore: Vec<(String, String)> = Vec::new();
    let mut paths_to_delete: Vec<(String, String)> = Vec::new();

    for (rel_path, entry) in &manifest.entries {
        if entry.existed_before {
            match entry.file_type.as_str() {
                "directory" => {
                    dirs_to_restore.push((rel_path.clone(), entry.path_hash.clone()));
                }
                _ => {
                    files_to_restore.push((rel_path.clone(), entry.path_hash.clone()));
                }
            }
        } else {
            paths_to_delete.push((rel_path.clone(), entry.path_hash.clone()));
        }
    }

    // --- Pass 1a: Delete paths that were created during this step (deepest-first) ---
    paths_to_delete.sort_by(|a, b| path_depth(&b.0).cmp(&path_depth(&a.0)));

    for (rel_path, _) in &paths_to_delete {
        let full_path = working_root.join(rel_path);
        if full_path.symlink_metadata().is_ok() {
            if full_path.is_dir() {
                let _ = fs::remove_dir_all(&full_path);
            } else {
                let _ = fs::remove_file(&full_path);
            }
        }
    }

    // --- Pass 1b: Recreate directories (shallowest-first) ---
    dirs_to_restore.sort_by(|a, b| path_depth(&a.0).cmp(&path_depth(&b.0)));

    for (rel_path, _) in &dirs_to_restore {
        let full_path = working_root.join(rel_path);
        if !full_path.exists() {
            fs::create_dir_all(&full_path)?;
        }
    }

    // --- Pass 1c: Restore file contents + metadata ---
    for (rel_path, hash) in &files_to_restore {
        let meta = read_preimage_metadata(&preimage_dir, hash)?;
        let full_path = working_root.join(rel_path);

        if let Some(parent) = full_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        if meta.file_type == PreimageFileType::Symlink
            && symlink_policy != SymlinkPolicy::ReadWrite
        {
            continue;
        }

        match meta.file_type {
            PreimageFileType::Regular => {
                let compressed = fs::read(preimage_dir.join(format!("{hash}.dat")))?;
                let contents = zstd::decode_all(compressed.as_slice()).map_err(|e| {
                    codeagent_common::CodeAgentError::Decompression {
                        message: format!("failed to decompress preimage for {rel_path}: {e}"),
                    }
                })?;
                fs::write(&full_path, contents)?;
            }
            PreimageFileType::Symlink => {
                // Remove existing file/symlink at this path if present
                if full_path.symlink_metadata().is_ok() {
                    let _ = fs::remove_file(&full_path);
                }
                let target = meta.symlink_target.as_deref().ok_or_else(|| {
                    codeagent_common::CodeAgentError::Preimage {
                        path: full_path.clone(),
                        message: "symlink preimage missing target".to_string(),
                    }
                })?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &full_path)?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_file(target, &full_path)?;
            }
            PreimageFileType::Directory => {
                // Should not appear in files_to_restore, but handle gracefully
                if !full_path.exists() {
                    fs::create_dir_all(&full_path)?;
                }
            }
        }

        restore_metadata(&full_path, &meta)?;
    }

    // --- Pass 2: Restore directory metadata (deepest-first) ---
    dirs_to_restore.sort_by(|a, b| path_depth(&b.0).cmp(&path_depth(&a.0)));

    for (_, hash) in &dirs_to_restore {
        let meta = read_preimage_metadata(&preimage_dir, hash)?;
        let full_path = working_root.join(&meta.relative_path);
        if full_path.exists() {
            restore_metadata(&full_path, &meta)?;
        }
    }

    Ok(())
}

fn restore_metadata(
    path: &Path,
    meta: &PreimageMetadata,
) -> codeagent_common::Result<()> {
    // Restore mode (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(meta.mode);
        fs::set_permissions(path, perms)?;
    }

    // Restore xattrs (Linux only)
    #[cfg(target_os = "linux")]
    {
        if let Ok(current_attrs) = xattr::list(path) {
            for attr in current_attrs {
                if !meta.xattrs.contains_key(&*attr.to_string_lossy()) {
                    let _ = xattr::remove(path, &attr);
                }
            }
        }
        for (key, value) in &meta.xattrs {
            let _ = xattr::set(path, key, value);
        }
    }

    // Restore mtime last so xattr changes don't clobber it
    restore_mtime(path, meta.mtime_ns)?;

    Ok(())
}

fn restore_mtime(path: &Path, mtime_ns: i128) -> codeagent_common::Result<()> {
    let secs = (mtime_ns / 1_000_000_000) as i64;
    let nanos = (mtime_ns % 1_000_000_000) as u32;
    let ft = filetime::FileTime::from_unix_time(secs, nanos);
    filetime::set_file_mtime(path, ft)?;
    Ok(())
}

fn path_depth(path: &str) -> usize {
    path.chars()
        .filter(|&c| c == '/' || c == '\\')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::StepManifest;
    use crate::preimage::{capture_creation_marker, capture_preimage};
    use tempfile::TempDir;

    #[test]
    fn rollback_restores_deleted_file() {
        let dir = TempDir::new().unwrap();
        let working = dir.path().join("working");
        let step_dir = dir.path().join("step");
        let preimage_dir = step_dir.join("preimages");
        fs::create_dir_all(&working).unwrap();
        fs::create_dir_all(&preimage_dir).unwrap();

        // Create a file
        let file = working.join("test.txt");
        fs::write(&file, "original content").unwrap();

        // Capture preimage manually
        let (meta, _) = capture_preimage(&file, &working, &preimage_dir).unwrap();
        let hash = crate::preimage::path_hash(Path::new("test.txt"));

        let mut manifest = StepManifest::new(1);
        manifest.add_entry("test.txt", &hash, true, meta.file_type.as_str());
        manifest.write_to(&step_dir).unwrap();

        // Delete the file
        fs::remove_file(&file).unwrap();
        assert!(!file.exists());

        // Rollback
        rollback_step(&step_dir, &working, SymlinkPolicy::default()).unwrap();
        assert!(file.exists());
        assert_eq!(fs::read_to_string(&file).unwrap(), "original content");
    }

    #[test]
    fn rollback_removes_created_file() {
        let dir = TempDir::new().unwrap();
        let working = dir.path().join("working");
        let step_dir = dir.path().join("step");
        let preimage_dir = step_dir.join("preimages");
        fs::create_dir_all(&working).unwrap();
        fs::create_dir_all(&preimage_dir).unwrap();

        // Create a new file and record it as "created"
        let file = working.join("new.txt");
        fs::write(&file, "new content").unwrap();

        let hash = crate::preimage::path_hash(Path::new("new.txt"));
        capture_creation_marker(&file, &working, &preimage_dir).unwrap();

        let mut manifest = StepManifest::new(1);
        manifest.add_entry("new.txt", &hash, false, "regular");
        manifest.write_to(&step_dir).unwrap();

        // Rollback should delete the file
        rollback_step(&step_dir, &working, SymlinkPolicy::default()).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn rollback_restores_directory_tree() {
        let dir = TempDir::new().unwrap();
        let working = dir.path().join("working");
        let step_dir = dir.path().join("step");
        let preimage_dir = step_dir.join("preimages");
        fs::create_dir_all(&working).unwrap();
        fs::create_dir_all(&preimage_dir).unwrap();

        // Create a directory with a file
        let sub_dir = working.join("mydir");
        fs::create_dir(&sub_dir).unwrap();
        let file = sub_dir.join("file.txt");
        fs::write(&file, "data").unwrap();

        // Capture preimages
        let dir_hash = crate::preimage::path_hash(Path::new("mydir"));
        let file_hash = crate::preimage::path_hash(Path::new("mydir/file.txt"));
        let (dir_meta, _) = capture_preimage(&sub_dir, &working, &preimage_dir).unwrap();
        let (file_meta, _) = capture_preimage(&file, &working, &preimage_dir).unwrap();

        let mut manifest = StepManifest::new(1);
        manifest.add_entry("mydir", &dir_hash, true, dir_meta.file_type.as_str());
        manifest.add_entry(
            "mydir/file.txt",
            &file_hash,
            true,
            file_meta.file_type.as_str(),
        );
        manifest.write_to(&step_dir).unwrap();

        // Delete everything
        fs::remove_dir_all(&sub_dir).unwrap();
        assert!(!sub_dir.exists());

        // Rollback
        rollback_step(&step_dir, &working, SymlinkPolicy::default()).unwrap();
        assert!(sub_dir.is_dir());
        assert_eq!(fs::read_to_string(&file).unwrap(), "data");
    }
}
