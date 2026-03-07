use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

use crate::error::P9Error;
use crate::fid::FidTable;
use crate::messages::*;
use crate::operations::session::qid_from_path;

/// Handle Tlopen: open a file or directory for I/O.
pub fn handle_lopen(
    request: &Tlopen,
    fid_table: &mut FidTable,
    msize: u32,
) -> Result<Rlopen, P9Error> {
    let state = fid_table.get(request.fid)?;
    let path = state.path.clone();
    let qid = qid_from_path(&path)?;

    // Directories don't need a real file handle — readdir uses
    // fs::read_dir on the path directly. On Windows, opening a directory
    // with OpenOptions fails with "Access Denied".
    if !path.is_dir() {
        let file = open_with_flags(&path, request.flags)?;
        let state = fid_table.get_mut(request.fid)?;
        state.open_handle = Some(file);
    }

    let state = fid_table.get_mut(request.fid)?;
    state.open_flags = request.flags;

    let iounit = msize.saturating_sub(24);

    Ok(Rlopen { qid, iounit })
}

/// Handle Tlcreate: create a new file and open it.
///
/// Per the 9P spec, Tlcreate repurposes the parent FID to refer to the
/// newly created file.
pub fn handle_lcreate(
    request: &Tlcreate,
    fid_table: &mut FidTable,
    msize: u32,
) -> Result<Rlcreate, P9Error> {
    let parent_path = fid_table.get(request.fid)?.path.clone();
    let new_path = parent_path.join(&request.name);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&new_path)?;

    let qid = qid_from_path(&new_path)?;
    let iounit = msize.saturating_sub(24);

    let state = fid_table.get_mut(request.fid)?;
    state.path = new_path;
    state.qid = qid;
    state.open_handle = Some(file);
    state.open_flags = request.flags;

    Ok(Rlcreate { qid, iounit })
}

/// Handle Tread: read data from an open file at the given offset.
pub fn handle_read(
    request: &Tread,
    fid_table: &mut FidTable,
) -> Result<Rread, P9Error> {
    let state = fid_table.get_mut(request.fid)?;

    let file = state
        .open_handle
        .as_mut()
        .ok_or(P9Error::FidNotOpen { fid: request.fid })?;

    file.seek(SeekFrom::Start(request.offset))?;

    let mut buf = vec![0u8; request.count as usize];
    let bytes_read = file.read(&mut buf)?;
    buf.truncate(bytes_read);

    Ok(Rread { data: buf })
}

/// Handle Twrite: write data to an open file at the given offset.
pub fn handle_write(
    request: &Twrite,
    fid_table: &mut FidTable,
) -> Result<Rwrite, P9Error> {
    let state = fid_table.get_mut(request.fid)?;

    let file = state
        .open_handle
        .as_mut()
        .ok_or(P9Error::FidNotOpen { fid: request.fid })?;

    file.seek(SeekFrom::Start(request.offset))?;
    file.write_all(&request.data)?;

    Ok(Rwrite {
        count: request.data.len() as u32,
    })
}

/// Handle Tfsync: flush file data to disk.
pub fn handle_fsync(
    request: &Tfsync,
    fid_table: &mut FidTable,
) -> Result<(), P9Error> {
    let state = fid_table.get_mut(request.fid)?;

    let file = state
        .open_handle
        .as_ref()
        .ok_or(P9Error::FidNotOpen { fid: request.fid })?;

    file.sync_all()?;

    Ok(())
}

/// Open a file with the given Linux open flags.
fn open_with_flags(path: &std::path::Path, flags: u32) -> Result<File, P9Error> {
    let access_mode = flags & 0o3; // O_ACCMODE

    let mut opts = OpenOptions::new();
    match access_mode {
        0 => {
            opts.read(true);
        } // O_RDONLY
        1 => {
            opts.write(true);
        } // O_WRONLY
        2 => {
            opts.read(true).write(true);
        } // O_RDWR
        _ => {
            opts.read(true);
        }
    }

    if flags & 0o2000 != 0 {
        opts.append(true);
    }

    if flags & 0o1000 != 0 {
        opts.truncate(true);
    }

    opts.open(path).map_err(|e| P9Error::Io { source: e })
}
