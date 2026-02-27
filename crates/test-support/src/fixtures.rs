use std::fs;
use std::path::Path;

/// A small tree with files of various sizes and a nested directory structure.
pub fn small_tree(root: &Path) {
    fs::write(root.join("empty.txt"), "").unwrap();
    fs::write(root.join("small.txt"), "hello world").unwrap();
    fs::write(root.join("medium.txt"), "x".repeat(4096)).unwrap();
    fs::write(root.join("large.bin"), vec![0xABu8; 1_000_000]).unwrap();

    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(root.join("src/components/app.rs"), "pub struct App;").unwrap();

    let script_path = root.join("run.sh");
    fs::write(&script_path, "#!/bin/sh\necho ok").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Two files with distinct contents, for rename tests.
pub fn rename_tree(root: &Path) {
    fs::write(root.join("a.txt"), "content of a").unwrap();
    fs::write(root.join("b.txt"), "content of b").unwrap();
}

/// A file with a symlink (platform-conditional).
pub fn symlink_tree(root: &Path) {
    fs::write(root.join("target.txt"), "symlink target content").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("target.txt"), root.join("link.txt")).unwrap();
    }
    #[cfg(windows)]
    {
        // Windows requires developer mode for symlinks — try and skip silently if unavailable.
        let _ = std::os::windows::fs::symlink_file(root.join("target.txt"), root.join("link.txt"));
    }
}

/// A deep nested tree for delete-tree and safeguard tests.
pub fn deep_tree(root: &Path) {
    for i in 0..5 {
        let dir = root.join(format!("level0/level1/level2/level3/level4_{i}"));
        fs::create_dir_all(&dir).unwrap();
        for j in 0..3 {
            fs::write(dir.join(format!("file_{j}.txt")), format!("content {i}/{j}")).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn small_tree_structure() {
        let dir = TempDir::new().unwrap();
        small_tree(dir.path());

        assert!(dir.path().join("empty.txt").exists());
        assert!(dir.path().join("small.txt").exists());
        assert!(dir.path().join("medium.txt").exists());
        assert!(dir.path().join("large.bin").exists());
        assert!(dir.path().join("src/main.rs").exists());
        assert!(dir.path().join("src/components/app.rs").exists());
        assert!(dir.path().join("run.sh").exists());

        assert_eq!(fs::read_to_string(dir.path().join("small.txt")).unwrap(), "hello world");
        assert_eq!(fs::read(dir.path().join("large.bin")).unwrap().len(), 1_000_000);
    }

    #[test]
    fn rename_tree_structure() {
        let dir = TempDir::new().unwrap();
        rename_tree(dir.path());

        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "content of a"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "content of b"
        );
    }

    #[test]
    fn deep_tree_structure() {
        let dir = TempDir::new().unwrap();
        deep_tree(dir.path());

        // 5 level4 directories, each with 3 files = 15 files total
        let mut file_count = 0;
        for i in 0..5 {
            let level4_dir = dir
                .path()
                .join(format!("level0/level1/level2/level3/level4_{i}"));
            assert!(level4_dir.is_dir());
            for j in 0..3 {
                let file = level4_dir.join(format!("file_{j}.txt"));
                assert!(file.exists());
                file_count += 1;
            }
        }
        assert_eq!(file_count, 15);
    }
}
