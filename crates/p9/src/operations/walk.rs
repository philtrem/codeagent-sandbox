use std::path::{Path, PathBuf};

use crate::error::P9Error;
use crate::fid::{FidState, FidTable};
use crate::messages::*;
use crate::operations::session::qid_from_path;

/// Handle Twalk: walk a path from an existing FID to create a new FID.
///
/// The 9P spec defines several walk behaviors:
/// - Zero wnames: clone the source FID to newfid.
/// - One or more wnames: walk each component from the source FID's path,
///   returning a QID for each successfully resolved component.
/// - Partial walk: if only some components succeed, return the QIDs for the
///   valid prefix (but only create the new FID if ALL components succeed).
/// - newfid == fid: replace the source FID in-place.
pub fn handle_walk(
    request: &Twalk,
    fid_table: &mut FidTable,
) -> Result<Rwalk, P9Error> {
    let source_path = fid_table.get(request.fid)?.path.clone();
    let source_qid = fid_table.get(request.fid)?.qid;

    // Zero-component walk: clone the FID.
    if request.wnames.is_empty() {
        if request.newfid != request.fid {
            let state = FidState::new(source_path, source_qid);
            fid_table.insert(request.newfid, state)?;
        }
        return Ok(Rwalk { wqids: vec![] });
    }

    // Walk each component, collecting QIDs for the resolved prefix.
    let root_path = fid_table.root_path().to_path_buf();
    let mut current_path = source_path;
    let mut wqids = Vec::new();

    for name in &request.wnames {
        // ".." handling: go to the parent directory.
        let next_path = if name == ".." {
            current_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| current_path.clone())
        } else if name == "." {
            current_path.clone()
        } else {
            current_path.join(name)
        };

        // Containment check: the resolved path must remain within the root.
        if !is_contained(&next_path, &root_path) {
            // If we haven't resolved any components yet, this is an error.
            if wqids.is_empty() {
                return Err(P9Error::PathOutsideRoot {
                    path: next_path.to_string_lossy().to_string(),
                });
            }
            // Partial walk: return the QIDs resolved so far.
            break;
        }

        // Check if the path actually exists on disk.
        match qid_from_path(&next_path) {
            Ok(qid) => {
                wqids.push(qid);
                current_path = next_path;
            }
            Err(_) => {
                // Path doesn't exist — stop walking (partial walk).
                break;
            }
        }
    }

    // Only create/update the FID if ALL components were resolved.
    if wqids.len() == request.wnames.len() {
        let final_qid = *wqids.last().unwrap();
        if request.newfid == request.fid {
            // In-place update.
            let state = fid_table.get_mut(request.fid)?;
            state.path = current_path;
            state.qid = final_qid;
        } else {
            let state = FidState::new(current_path, final_qid);
            fid_table.insert(request.newfid, state)?;
        }
    }

    Ok(Rwalk { wqids })
}

/// Check whether `path` is logically contained within `root`.
///
/// Uses canonical path comparison. Both paths must be resolved to
/// their canonical forms for reliable containment checking.
fn is_contained(path: &Path, root: &Path) -> bool {
    // Try to canonicalize both. If either fails, fall back to logical check.
    match (std::fs::canonicalize(path), std::fs::canonicalize(root)) {
        (Ok(canonical_path), Ok(canonical_root)) => {
            canonical_path.starts_with(&canonical_root)
        }
        _ => {
            // Fallback: logical path prefix check. This handles the case
            // where the path doesn't exist yet (e.g., during walk).
            logical_contains(path, root)
        }
    }
}

/// Logical containment check: resolve `.` and `..` components and check
/// if the resulting path starts with the root.
fn logical_contains(path: &Path, root: &Path) -> bool {
    let resolved = match resolve_logical(path) {
        Some(p) => p,
        None => return false,
    };
    let resolved_root = match resolve_logical(root) {
        Some(p) => p,
        None => return false,
    };
    resolved.starts_with(&resolved_root)
}

/// Resolve `.` and `..` components logically without filesystem access.
fn resolve_logical(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
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

    #[test]
    fn logical_contains_within_root() {
        let root = Path::new("/tmp/root");
        let path = Path::new("/tmp/root/sub/file.txt");
        assert!(logical_contains(path, root));
    }

    #[test]
    fn logical_contains_outside_root() {
        let root = Path::new("/tmp/root");
        let path = Path::new("/tmp/other/file.txt");
        assert!(!logical_contains(path, root));
    }

    #[test]
    fn logical_contains_dotdot_escape() {
        let root = Path::new("/tmp/root");
        let path = Path::new("/tmp/root/../other");
        assert!(!logical_contains(path, root));
    }

    #[test]
    fn logical_contains_dotdot_within() {
        let root = Path::new("/tmp/root");
        let path = Path::new("/tmp/root/a/../b");
        assert!(logical_contains(path, root));
    }
}
