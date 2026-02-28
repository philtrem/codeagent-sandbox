use std::path::{Component, Path, PathBuf};

use crate::error::McpError;

/// Validate that a path is contained within the root directory.
///
/// - Relative paths are resolved relative to `root`.
/// - Absolute paths must be prefixed by `root`.
/// - `.` and `..` components are resolved logically (without touching the
///   filesystem) to prevent traversal attacks.
///
/// Returns the resolved path on success, or `PathOutsideRoot` on failure.
pub fn validate_path(path: &str, root: &Path) -> Result<PathBuf, McpError> {
    let requested = Path::new(path);

    let full_path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };

    let resolved = resolve_components(&full_path).ok_or_else(|| McpError::PathOutsideRoot {
        path: path.to_string(),
    })?;

    let canonical_root =
        resolve_components(root).ok_or_else(|| McpError::PathOutsideRoot {
            path: path.to_string(),
        })?;

    if !resolved.starts_with(&canonical_root) {
        return Err(McpError::PathOutsideRoot {
            path: path.to_string(),
        });
    }

    Ok(resolved)
}

/// Resolve `.` and `..` components logically without touching the filesystem.
///
/// Returns `None` if a `..` component would escape the path's root prefix.
fn resolve_components(path: &Path) -> Option<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\sandbox\working")
        } else {
            PathBuf::from("/sandbox/working")
        }
    }

    #[test]
    fn relative_path_within_root() {
        let result = validate_path("src/main.rs", &test_root());
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with(test_root()));
    }

    #[test]
    fn relative_path_traversal_rejected() {
        let err = validate_path("../../etc/passwd", &test_root()).unwrap_err();
        assert!(matches!(err, McpError::PathOutsideRoot { .. }));
    }

    #[test]
    fn relative_path_mixed_traversal_rejected() {
        let err = validate_path("subdir/../../etc/passwd", &test_root()).unwrap_err();
        assert!(matches!(err, McpError::PathOutsideRoot { .. }));
    }

    #[test]
    fn absolute_path_inside_root() {
        let inside = if cfg!(windows) {
            r"C:\sandbox\working\src\main.rs"
        } else {
            "/sandbox/working/src/main.rs"
        };
        let result = validate_path(inside, &test_root());
        assert!(result.is_ok());
    }

    #[test]
    fn absolute_path_outside_root() {
        let outside = if cfg!(windows) {
            r"C:\Windows\System32"
        } else {
            "/etc"
        };
        let err = validate_path(outside, &test_root()).unwrap_err();
        assert!(matches!(err, McpError::PathOutsideRoot { .. }));
    }

    #[test]
    fn dot_component_ignored() {
        let result = validate_path("./src/./main.rs", &test_root());
        assert!(result.is_ok());
    }

    #[test]
    fn dot_dot_within_root_allowed() {
        let result = validate_path("src/../lib.rs", &test_root());
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with(test_root()));
    }

    #[test]
    fn empty_path_resolves_to_root() {
        let result = validate_path("", &test_root());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), test_root());
    }
}
