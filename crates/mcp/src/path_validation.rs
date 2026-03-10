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
    validate_path_multi(path, &[root.to_path_buf()])
}

/// Validate that a path is contained within any of the given root directories.
///
/// - Relative paths are tried against each root in order; the first match wins.
/// - Absolute paths must be prefixed by at least one root.
/// - `.` and `..` components are resolved logically (without touching the
///   filesystem) to prevent traversal attacks.
///
/// Returns the resolved path on success, or `PathOutsideRoot` on failure.
pub fn validate_path_multi(path: &str, roots: &[PathBuf]) -> Result<PathBuf, McpError> {
    let requested = Path::new(path);
    let make_err = || McpError::PathOutsideRoot {
        path: path.to_string(),
    };

    if requested.is_absolute() {
        let resolved = resolve_components(requested).ok_or_else(make_err)?;
        for root in roots {
            let canonical_root = resolve_components(root).ok_or_else(make_err)?;
            if resolved.starts_with(&canonical_root) {
                return Ok(resolved);
            }
        }
        Err(make_err())
    } else {
        for root in roots {
            let full_path = root.join(requested);
            let resolved = match resolve_components(&full_path) {
                Some(r) => r,
                None => continue,
            };
            let canonical_root = match resolve_components(root) {
                Some(r) => r,
                None => continue,
            };
            if resolved.starts_with(&canonical_root) {
                return Ok(resolved);
            }
        }
        Err(make_err())
    }
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

    // --- Multi-root tests ---

    fn test_root_secondary() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\sandbox\secondary")
        } else {
            PathBuf::from("/sandbox/secondary")
        }
    }

    fn multi_roots() -> Vec<PathBuf> {
        vec![test_root(), test_root_secondary()]
    }

    #[test]
    fn multi_root_relative_resolves_against_first() {
        let result = validate_path_multi("src/main.rs", &multi_roots());
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with(test_root()));
    }

    #[test]
    fn multi_root_absolute_in_secondary() {
        let inside = if cfg!(windows) {
            r"C:\sandbox\secondary\src\lib.rs"
        } else {
            "/sandbox/secondary/src/lib.rs"
        };
        let result = validate_path_multi(inside, &multi_roots());
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with(test_root_secondary()));
    }

    #[test]
    fn multi_root_absolute_outside_all_rejected() {
        let outside = if cfg!(windows) {
            r"C:\Windows\System32"
        } else {
            "/etc"
        };
        let err = validate_path_multi(outside, &multi_roots()).unwrap_err();
        assert!(matches!(err, McpError::PathOutsideRoot { .. }));
    }

    #[test]
    fn multi_root_traversal_rejected() {
        let err = validate_path_multi("../../etc/passwd", &multi_roots()).unwrap_err();
        assert!(matches!(err, McpError::PathOutsideRoot { .. }));
    }
}
