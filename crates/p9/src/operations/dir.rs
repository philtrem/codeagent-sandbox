use std::fs;

use crate::error::P9Error;
use crate::fid::FidTable;
use crate::messages::*;
use crate::operations::session::qid_from_path;

/// Handle Treaddir: read directory entries.
///
/// The 9P readdir protocol returns entries packed as:
///   qid(13) + offset(8) + type(1) + name(2+len)
///
/// The `offset` field in the request indicates where to resume reading.
/// We use a simple index-based offset: offset N means "skip N entries".
pub fn handle_readdir(
    request: &Treaddir,
    fid_table: &mut FidTable,
) -> Result<Rreaddir, P9Error> {
    let state = fid_table.get(request.fid)?;
    let dir_path = state.path.clone();

    let entries: Vec<_> = fs::read_dir(&dir_path)?
        .filter_map(|e| e.ok())
        .collect();

    let mut data = Vec::new();
    let mut current_offset = request.offset;

    for entry in entries.iter().skip(current_offset as usize) {
        let name = entry.file_name().to_string_lossy().to_string();
        let entry_path = dir_path.join(&name);

        let qid = match qid_from_path(&entry_path) {
            Ok(q) => q,
            Err(_) => continue, // Skip entries we can't stat.
        };

        let dtype = if qid.is_dir() {
            DT_DIR
        } else if qid.is_symlink() {
            DT_LNK
        } else {
            DT_REG
        };

        current_offset += 1;

        // Pack the entry: qid(13) + offset(8) + type(1) + name(2+len).
        let entry_size = 13 + 8 + 1 + 2 + name.len();
        if data.len() + entry_size > request.count as usize {
            break; // No more room in the response buffer.
        }

        // QID: type(1) + version(4) + path(8)
        data.push(qid.ty);
        data.extend_from_slice(&qid.version.to_le_bytes());
        data.extend_from_slice(&qid.path.to_le_bytes());
        // Offset (next entry's offset)
        data.extend_from_slice(&current_offset.to_le_bytes());
        // Type
        data.push(dtype);
        // Name (u16 length + bytes)
        data.extend_from_slice(&(name.len() as u16).to_le_bytes());
        data.extend_from_slice(name.as_bytes());
    }

    Ok(Rreaddir { data })
}

/// Handle Tmkdir: create a directory.
pub fn handle_mkdir(
    request: &Tmkdir,
    fid_table: &FidTable,
) -> Result<Rmkdir, P9Error> {
    let parent_path = fid_table.get(request.dfid)?.path.clone();
    let new_path = parent_path.join(&request.name);

    fs::create_dir(&new_path)?;
    let qid = qid_from_path(&new_path)?;

    Ok(Rmkdir { qid })
}

/// Handle Tunlinkat: remove a file or directory.
pub fn handle_unlinkat(
    request: &Tunlinkat,
    fid_table: &FidTable,
) -> Result<(), P9Error> {
    let parent_path = fid_table.get(request.dirfid)?.path.clone();
    let target_path = parent_path.join(&request.name);

    if request.flags & AT_REMOVEDIR != 0 {
        fs::remove_dir(&target_path)?;
    } else {
        fs::remove_file(&target_path)?;
    }

    Ok(())
}

/// Handle Trenameat: rename a file or directory.
pub fn handle_renameat(
    request: &Trenameat,
    fid_table: &FidTable,
) -> Result<(), P9Error> {
    let old_parent = fid_table.get(request.olddirfid)?.path.clone();
    let new_parent = fid_table.get(request.newdirfid)?.path.clone();
    let old_path = old_parent.join(&request.oldname);
    let new_path = new_parent.join(&request.newname);

    fs::rename(&old_path, &new_path)?;

    Ok(())
}

/// Handle Tstatfs: return filesystem statistics.
///
/// Returns reasonable defaults for a passthrough filesystem.
pub fn handle_statfs(fid_table: &FidTable, fid: u32) -> Result<Rstatfs, P9Error> {
    let _state = fid_table.get(fid)?;

    // Return reasonable default values. A more complete implementation
    // would query the actual filesystem via platform-specific APIs.
    Ok(Rstatfs {
        fs_type: 0x01021997, // V9FS_MAGIC
        bsize: 4096,
        blocks: 1_000_000,
        bfree: 500_000,
        bavail: 500_000,
        files: 1_000_000,
        ffree: 500_000,
        fsid: 0,
        namelen: 255,
    })
}
