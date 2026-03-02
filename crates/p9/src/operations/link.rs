use crate::error::P9Error;
use crate::fid::FidTable;
use crate::messages::*;
use crate::operations::session::qid_from_path;

/// Handle Tsymlink: create a symbolic link.
///
/// Creates a symlink named `request.name` in the directory referenced by
/// `request.fid`, pointing to `request.symtgt`.
#[cfg(unix)]
pub fn handle_symlink(
    request: &Tsymlink,
    fid_table: &FidTable,
) -> Result<Rsymlink, P9Error> {
    let parent_path = fid_table.get(request.fid)?.path.clone();
    let link_path = parent_path.join(&request.name);

    std::os::unix::fs::symlink(&request.symtgt, &link_path)?;

    let qid = qid_from_path(&link_path)?;
    Ok(Rsymlink { qid })
}

/// Handle Tsymlink: create a symbolic link (Windows).
///
/// On Windows, uses `symlink_file`. Directory symlinks would need
/// `symlink_dir`, but we default to file symlinks since the target
/// type may not be known at creation time.
#[cfg(windows)]
pub fn handle_symlink(
    request: &Tsymlink,
    fid_table: &FidTable,
) -> Result<Rsymlink, P9Error> {
    let parent_path = fid_table.get(request.fid)?.path.clone();
    let link_path = parent_path.join(&request.name);

    std::os::windows::fs::symlink_file(&request.symtgt, &link_path)?;

    let qid = qid_from_path(&link_path)?;
    Ok(Rsymlink { qid })
}

/// Handle Treadlink: read the target of a symbolic link.
pub fn handle_readlink(
    request: &Treadlink,
    fid_table: &FidTable,
) -> Result<Rreadlink, P9Error> {
    let state = fid_table.get(request.fid)?;
    let target = std::fs::read_link(&state.path)?;

    // Convert to a string, using lossy conversion for non-UTF-8 paths.
    let target_str = target.to_string_lossy().into_owned();

    Ok(Rreadlink { target: target_str })
}

/// Handle Tlink: create a hard link.
///
/// Creates a hard link named `request.name` in the directory referenced by
/// `request.dfid`, pointing to the file referenced by `request.fid`.
pub fn handle_link(
    request: &Tlink,
    fid_table: &FidTable,
) -> Result<(), P9Error> {
    let dir_path = fid_table.get(request.dfid)?.path.clone();
    let target_path = fid_table.get(request.fid)?.path.clone();
    let link_path = dir_path.join(&request.name);

    std::fs::hard_link(&target_path, &link_path)?;

    Ok(())
}
