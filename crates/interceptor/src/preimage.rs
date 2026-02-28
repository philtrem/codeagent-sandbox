use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use codeagent_common::CodeAgentError;

/// Compute a hex-encoded blake3 hash of a relative path string,
/// used as the filename for preimage storage on disk.
/// Normalizes path separators to forward slashes for cross-platform consistency.
pub fn path_hash(relative_path: &Path) -> String {
    let normalized = relative_path.to_string_lossy().replace('\\', "/");
    let hash = blake3::hash(normalized.as_bytes());
    hash.to_hex().to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreimageFileType {
    Regular,
    Directory,
    Symlink,
}

impl PreimageFileType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PreimageFileType::Regular => "regular",
            PreimageFileType::Directory => "directory",
            PreimageFileType::Symlink => "symlink",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreimageMetadata {
    pub relative_path: String,
    pub existed_before: bool,
    pub file_type: PreimageFileType,
    /// Unix mode bits (all 12 bits).
    pub mode: u32,
    /// mtime in nanoseconds since Unix epoch.
    pub mtime_ns: i128,
    pub size: u64,
    pub symlink_target: Option<String>,
    pub xattrs: BTreeMap<String, Vec<u8>>,
}

/// Capture the preimage of an existing path: metadata + compressed contents.
/// Writes `{path_hash}.dat` (zstd-compressed) and `{path_hash}.meta.json`
/// to `preimage_dir` using atomic temp-file-then-rename.
///
/// Returns the metadata and the number of data bytes written (compressed .dat
/// file size; 0 for directories and symlinks).
pub fn capture_preimage(
    file_path: &Path,
    working_root: &Path,
    preimage_dir: &Path,
) -> codeagent_common::Result<(PreimageMetadata, u64)> {
    let relative = file_path.strip_prefix(working_root).map_err(|_| {
        CodeAgentError::Preimage {
            path: file_path.to_path_buf(),
            message: "path is not under working root".to_string(),
        }
    })?;

    let hash = path_hash(relative);
    let meta_path = preimage_dir.join(format!("{hash}.meta.json"));
    let meta_tmp = preimage_dir.join(format!("{hash}.meta.json.tmp"));
    let data_path = preimage_dir.join(format!("{hash}.dat"));
    let data_tmp = preimage_dir.join(format!("{hash}.dat.tmp"));

    let metadata = fs::symlink_metadata(file_path)?;

    let (file_type, symlink_target) = if metadata.is_symlink() {
        let target = fs::read_link(file_path)?;
        (
            PreimageFileType::Symlink,
            Some(target.to_string_lossy().into_owned()),
        )
    } else if metadata.is_dir() {
        (PreimageFileType::Directory, None)
    } else {
        (PreimageFileType::Regular, None)
    };

    let mode = read_mode(&metadata);
    let mtime_ns = read_mtime_ns(&metadata);
    let size = metadata.len();
    let xattrs = read_xattrs(file_path);

    let preimage_meta = PreimageMetadata {
        relative_path: relative.to_string_lossy().replace('\\', "/"),
        existed_before: true,
        file_type,
        mode,
        mtime_ns,
        size,
        symlink_target,
        xattrs,
    };

    let meta_json = serde_json::to_string_pretty(&preimage_meta)?;
    fs::write(&meta_tmp, meta_json)?;
    fs::rename(&meta_tmp, &meta_path)?;

    let mut data_bytes_written: u64 = 0;
    if file_type == PreimageFileType::Regular {
        let contents = fs::read(file_path)?;
        let compressed = zstd::encode_all(contents.as_slice(), 3)
            .map_err(|e| CodeAgentError::Preimage {
                path: file_path.to_path_buf(),
                message: format!("zstd compression failed: {e}"),
            })?;
        data_bytes_written = compressed.len() as u64;
        fs::write(&data_tmp, &compressed)?;
        fs::rename(&data_tmp, &data_path)?;
    }

    Ok((preimage_meta, data_bytes_written))
}

/// Capture a "not existed" preimage marker for newly created paths.
pub fn capture_creation_marker(
    file_path: &Path,
    working_root: &Path,
    preimage_dir: &Path,
) -> codeagent_common::Result<PreimageMetadata> {
    let relative = file_path.strip_prefix(working_root).map_err(|_| {
        CodeAgentError::Preimage {
            path: file_path.to_path_buf(),
            message: "path is not under working root".to_string(),
        }
    })?;

    let hash = path_hash(relative);
    let meta_path = preimage_dir.join(format!("{hash}.meta.json"));

    let file_type = if file_path.is_dir() {
        PreimageFileType::Directory
    } else if file_path.symlink_metadata().map(|m| m.is_symlink()).unwrap_or(false) {
        PreimageFileType::Symlink
    } else {
        PreimageFileType::Regular
    };

    let preimage_meta = PreimageMetadata {
        relative_path: relative.to_string_lossy().replace('\\', "/"),
        existed_before: false,
        file_type,
        mode: 0,
        mtime_ns: 0,
        size: 0,
        symlink_target: None,
        xattrs: BTreeMap::new(),
    };

    let meta_json = serde_json::to_string_pretty(&preimage_meta)?;
    let meta_tmp = preimage_dir.join(format!("{hash}.meta.json.tmp"));
    fs::write(&meta_tmp, meta_json)?;
    fs::rename(&meta_tmp, &meta_path)?;

    Ok(preimage_meta)
}

/// Read a PreimageMetadata from a `{path_hash}.meta.json` file.
pub fn read_preimage_metadata(
    preimage_dir: &Path,
    path_hash: &str,
) -> codeagent_common::Result<PreimageMetadata> {
    let meta_path = preimage_dir.join(format!("{path_hash}.meta.json"));
    let json = fs::read_to_string(meta_path)?;
    let meta: PreimageMetadata = serde_json::from_str(&json)?;
    Ok(meta)
}

#[cfg(unix)]
fn read_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    metadata.mode()
}

#[cfg(not(unix))]
fn read_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.is_dir() {
        0o755
    } else {
        0o644
    }
}

fn read_mtime_ns(metadata: &fs::Metadata) -> i128 {
    match metadata.modified() {
        Ok(mtime) => match mtime.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos() as i128,
            Err(err) => -(err.duration().as_nanos() as i128),
        },
        Err(_) => 0,
    }
}

#[cfg(target_os = "linux")]
fn read_xattrs(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut result = BTreeMap::new();
    if let Ok(attrs) = xattr::list(path) {
        for attr in attrs {
            if let Ok(Some(value)) = xattr::get(path, &attr) {
                result.insert(attr.to_string_lossy().into_owned(), value);
            }
        }
    }
    result
}

#[cfg(not(target_os = "linux"))]
fn read_xattrs(_path: &Path) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn path_hash_deterministic() {
        let path = Path::new("src/main.rs");
        assert_eq!(path_hash(path), path_hash(path));
    }

    #[test]
    fn path_hash_different_paths() {
        let h1 = path_hash(Path::new("src/main.rs"));
        let h2 = path_hash(Path::new("src/lib.rs"));
        assert_ne!(h1, h2);
    }

    #[test]
    fn capture_preimage_regular_file() {
        let dir = TempDir::new().unwrap();
        let working = dir.path().join("working");
        let preimages = dir.path().join("preimages");
        fs::create_dir_all(&working).unwrap();
        fs::create_dir_all(&preimages).unwrap();

        let file_path = working.join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let (meta, data_size) = capture_preimage(&file_path, &working, &preimages).unwrap();

        assert!(meta.existed_before);
        assert_eq!(meta.relative_path, "test.txt");
        assert_eq!(meta.file_type, PreimageFileType::Regular);
        assert_eq!(meta.size, 11);
        assert!(data_size > 0);

        // Verify .meta.json was written
        let hash = path_hash(Path::new("test.txt"));
        assert!(preimages.join(format!("{hash}.meta.json")).exists());
        assert!(preimages.join(format!("{hash}.dat")).exists());
    }

    #[test]
    fn capture_preimage_directory() {
        let dir = TempDir::new().unwrap();
        let working = dir.path().join("working");
        let preimages = dir.path().join("preimages");
        fs::create_dir_all(working.join("subdir")).unwrap();
        fs::create_dir_all(&preimages).unwrap();

        let (meta, data_size) = capture_preimage(&working.join("subdir"), &working, &preimages).unwrap();
        assert_eq!(data_size, 0);

        assert!(meta.existed_before);
        assert_eq!(meta.file_type, PreimageFileType::Directory);

        // No .dat file for directories
        let hash = path_hash(Path::new("subdir"));
        assert!(preimages.join(format!("{hash}.meta.json")).exists());
        assert!(!preimages.join(format!("{hash}.dat")).exists());
    }

    #[test]
    fn capture_preimage_decompress_round_trip() {
        let dir = TempDir::new().unwrap();
        let working = dir.path().join("working");
        let preimages = dir.path().join("preimages");
        fs::create_dir_all(&working).unwrap();
        fs::create_dir_all(&preimages).unwrap();

        let original_content = "hello world - this is a test of zstd compression round-trip";
        let file_path = working.join("data.txt");
        fs::write(&file_path, original_content).unwrap();

        capture_preimage(&file_path, &working, &preimages).unwrap();

        let hash = path_hash(Path::new("data.txt"));
        let compressed = fs::read(preimages.join(format!("{hash}.dat"))).unwrap();
        let decompressed = zstd::decode_all(compressed.as_slice()).unwrap();
        assert_eq!(String::from_utf8(decompressed).unwrap(), original_content);
    }

    #[test]
    fn capture_creation_marker() {
        let dir = TempDir::new().unwrap();
        let working = dir.path().join("working");
        let preimages = dir.path().join("preimages");
        fs::create_dir_all(&working).unwrap();
        fs::create_dir_all(&preimages).unwrap();

        let file_path = working.join("new_file.txt");
        fs::write(&file_path, "new").unwrap();

        let meta =
            super::capture_creation_marker(&file_path, &working, &preimages).unwrap();

        assert!(!meta.existed_before);
        assert_eq!(meta.relative_path, "new_file.txt");
    }
}
