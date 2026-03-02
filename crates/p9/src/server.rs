use std::sync::Arc;

use codeagent_control::in_flight::InFlightTracker;
use codeagent_interceptor::write_interceptor::WriteInterceptor;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{p9_error_to_errno, P9Error};
use crate::fid::FidTable;
use crate::messages::*;
use crate::operations::{attr, dir, file, link, session, walk};
use crate::platform;
use crate::wire::{self, WireReader, WireWriter, MAX_MESSAGE_SIZE};

/// Default maximum message size (4 MB).
pub const DEFAULT_MSIZE: u32 = 4 * 1024 * 1024;

/// P9Server processes 9P2000.L messages over an async byte stream.
///
/// The server is transport-agnostic — it operates on `AsyncRead + AsyncWrite`,
/// allowing it to work over virtio-serial, Unix sockets, or test harnesses
/// using `tokio::io::duplex()`.
///
/// When an `interceptor` is provided, all mutating operations call the
/// appropriate `WriteInterceptor` pre/post hooks for undo tracking.
/// When an `in_flight` tracker is provided, each request increments the
/// counter on entry and decrements on exit (via drop guard).
pub struct P9Server {
    fid_table: FidTable,
    msize: u32,
    negotiated: bool,
    interceptor: Option<Arc<dyn WriteInterceptor>>,
    in_flight: Option<InFlightTracker>,
}

impl P9Server {
    /// Create a new P9 server rooted at the given directory.
    pub fn new(root_path: std::path::PathBuf) -> Self {
        Self {
            fid_table: FidTable::new(root_path),
            msize: DEFAULT_MSIZE,
            negotiated: false,
            interceptor: None,
            in_flight: None,
        }
    }

    /// Create a new P9 server with a custom maximum message size.
    pub fn with_msize(root_path: std::path::PathBuf, msize: u32) -> Self {
        Self {
            fid_table: FidTable::new(root_path),
            msize,
            negotiated: false,
            interceptor: None,
            in_flight: None,
        }
    }

    /// Set the write interceptor for undo tracking.
    pub fn with_interceptor(mut self, interceptor: Arc<dyn WriteInterceptor>) -> Self {
        self.interceptor = Some(interceptor);
        self
    }

    /// Set the in-flight tracker for quiescence detection.
    pub fn with_in_flight(mut self, tracker: InFlightTracker) -> Self {
        self.in_flight = Some(tracker);
        self
    }

    /// Run the server dispatch loop, reading messages from `reader` and writing
    /// responses to `writer`. Returns when the reader reaches EOF or an
    /// unrecoverable I/O error occurs.
    pub async fn run<R, W>(&mut self, mut reader: R, mut writer: W) -> Result<(), P9Error>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        loop {
            let mut size_buf = [0u8; 4];
            match reader.read_exact(&mut size_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(());
                }
                Err(e) => return Err(P9Error::Io { source: e }),
            }

            let size = u32::from_le_bytes(size_buf);

            let max = if self.negotiated {
                self.msize
            } else {
                MAX_MESSAGE_SIZE
            };
            wire::validate_message_size(size, max)?;

            let body_len = (size as usize) - 4;
            let mut body = vec![0u8; body_len];
            reader
                .read_exact(&mut body)
                .await
                .map_err(|e| P9Error::Io { source: e })?;

            let (msg_type, tag) = match wire::parse_header(&body) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let payload = &body[3..];

            // Track in-flight operations via drop guard.
            let _guard = self.in_flight.as_ref().map(InFlightGuard::new);

            let response_bytes = self.dispatch(msg_type, tag, payload);

            writer
                .write_all(&response_bytes)
                .await
                .map_err(|e| P9Error::Io { source: e })?;
            writer
                .flush()
                .await
                .map_err(|e| P9Error::Io { source: e })?;
        }
    }

    /// Dispatch a single message and return the encoded response bytes.
    fn dispatch(&mut self, msg_type: u8, tag: u16, payload: &[u8]) -> Vec<u8> {
        match msg_type {
            TVERSION => self.handle_tversion(tag, payload),
            TAUTH => self.handle_tauth(tag),
            TATTACH => self.handle_tattach(tag, payload),
            TWALK => self.handle_twalk(tag, payload),
            TGETATTR => self.handle_tgetattr(tag, payload),
            TSETATTR => self.handle_tsetattr(tag, payload),
            TLOPEN => self.handle_tlopen(tag, payload),
            TLCREATE => self.handle_tlcreate(tag, payload),
            TREAD => self.handle_tread(tag, payload),
            TWRITE => self.handle_twrite(tag, payload),
            TREADDIR => self.handle_treaddir(tag, payload),
            TSTATFS => self.handle_tstatfs(tag, payload),
            TFSYNC => self.handle_tfsync(tag, payload),
            TMKDIR => self.handle_tmkdir(tag, payload),
            TUNLINKAT => self.handle_tunlinkat(tag, payload),
            TRENAMEAT => self.handle_trenameat(tag, payload),
            TSYMLINK => self.handle_tsymlink(tag, payload),
            TREADLINK => self.handle_treadlink(tag, payload),
            TLINK => self.handle_tlink(tag, payload),
            TMKNOD => self.handle_tmknod(tag),
            TREMOVE => self.handle_tremove(tag, payload),
            TCLUNK => self.handle_tclunk(tag, payload),
            TFLUSH => self.handle_tflush(tag),
            _ => encode_error(tag, crate::error::errno::EOPNOTSUPP),
        }
    }

    // -- Session operations --

    fn handle_tversion(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tversion::decode(&mut reader) {
            Ok(request) => {
                let response = session::handle_version(&request, self.msize);
                self.msize = response.msize;
                self.negotiated = true;
                let root_path = self.fid_table.root_path().to_path_buf();
                self.fid_table = FidTable::new(root_path);
                response.to_wire(tag)
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tauth(&mut self, tag: u16) -> Vec<u8> {
        session::handle_auth().to_wire(tag)
    }

    fn handle_tattach(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tattach::decode(&mut reader) {
            Ok(request) => match session::handle_attach(&request, &mut self.fid_table) {
                Ok(response) => response.to_wire(tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_twalk(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Twalk::decode(&mut reader) {
            Ok(request) => match walk::handle_walk(&request, &mut self.fid_table) {
                Ok(response) => response.to_wire(tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tclunk(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tclunk::decode(&mut reader) {
            Ok(request) => match session::handle_clunk(&request, &mut self.fid_table) {
                Ok(()) => encode_empty_response(RCLUNK, tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tremove(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tremove::decode(&mut reader) {
            Ok(request) => {
                // Interceptor pre-hook: check if deletion is allowed.
                if let Some(ref interceptor) = self.interceptor {
                    let path = match self.fid_table.get(request.fid) {
                        Ok(state) => state.path.clone(),
                        Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                    };
                    let is_dir = path.is_dir();
                    if interceptor.pre_unlink(&path, is_dir).is_err() {
                        // Even on denial, the FID must be clunked per the spec.
                        let _ = self.fid_table.remove(request.fid);
                        return encode_error(tag, crate::error::errno::EACCES);
                    }
                }

                match session::handle_remove(&request, &mut self.fid_table) {
                    Ok(()) => encode_empty_response(RREMOVE, tag),
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tflush(&mut self, tag: u16) -> Vec<u8> {
        session::handle_flush();
        encode_empty_response(RFLUSH, tag)
    }

    // -- Read-only operations --

    fn handle_tgetattr(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tgetattr::decode(&mut reader) {
            Ok(request) => match attr::handle_getattr(&request, &self.fid_table) {
                Ok(response) => response.to_wire(tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tlopen(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tlopen::decode(&mut reader) {
            Ok(request) => {
                // If opening with O_TRUNC, call interceptor pre-hook.
                if request.flags & 0o1000 != 0 {
                    if let Some(ref interceptor) = self.interceptor {
                        let path = match self.fid_table.get(request.fid) {
                            Ok(state) => state.path.clone(),
                            Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                        };
                        if interceptor.pre_open_trunc(&path).is_err() {
                            return encode_error(tag, crate::error::errno::EACCES);
                        }
                    }
                }
                match file::handle_lopen(&request, &mut self.fid_table, self.msize) {
                    Ok(response) => response.to_wire(tag),
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tread(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tread::decode(&mut reader) {
            Ok(request) => match file::handle_read(&request, &mut self.fid_table) {
                Ok(response) => response.to_wire(tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_treaddir(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Treaddir::decode(&mut reader) {
            Ok(request) => match dir::handle_readdir(&request, &mut self.fid_table) {
                Ok(response) => response.to_wire(tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tstatfs(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tstatfs::decode(&mut reader) {
            Ok(request) => match dir::handle_statfs(&self.fid_table, request.fid) {
                Ok(response) => response.to_wire(tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    // -- Write operations (with interceptor hooks) --

    fn handle_tsetattr(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tsetattr::decode(&mut reader) {
            Ok(request) => {
                let path = match self.fid_table.get(request.fid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };

                // Interceptor pre-hook.
                if let Some(ref interceptor) = self.interceptor {
                    if interceptor.pre_setattr(&path).is_err() {
                        return encode_error(tag, crate::error::errno::EACCES);
                    }
                }

                match attr::handle_setattr(&request, &self.fid_table) {
                    Ok(()) => encode_empty_response(RSETATTR, tag),
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tlcreate(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tlcreate::decode(&mut reader) {
            Ok(request) => {
                let parent_path = match self.fid_table.get(request.fid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };

                if let Some(err_response) = self.validate_name(tag, &parent_path, &request.name) {
                    return err_response;
                }

                let new_path = parent_path.join(&request.name);

                match file::handle_lcreate(
                    &request,
                    &mut self.fid_table,
                    self.msize,
                ) {
                    Ok(response) => {
                        // Post-create hook.
                        if let Some(ref interceptor) = self.interceptor {
                            let _ = interceptor.post_create(&new_path);
                        }
                        response.to_wire(tag)
                    }
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_twrite(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Twrite::decode(&mut reader) {
            Ok(request) => {
                // Interceptor pre-hook.
                if let Some(ref interceptor) = self.interceptor {
                    let path = match self.fid_table.get(request.fid) {
                        Ok(state) => state.path.clone(),
                        Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                    };
                    if interceptor.pre_write(&path).is_err() {
                        return encode_error(tag, crate::error::errno::EACCES);
                    }
                }

                match file::handle_write(&request, &mut self.fid_table) {
                    Ok(response) => response.to_wire(tag),
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tfsync(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tfsync::decode(&mut reader) {
            Ok(request) => match file::handle_fsync(&request, &mut self.fid_table) {
                Ok(()) => encode_empty_response(RFSYNC, tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tmkdir(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tmkdir::decode(&mut reader) {
            Ok(request) => {
                let parent_path = match self.fid_table.get(request.dfid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };

                if let Some(err_response) = self.validate_name(tag, &parent_path, &request.name) {
                    return err_response;
                }

                match dir::handle_mkdir(&request, &self.fid_table) {
                    Ok(response) => {
                        // Post-mkdir hook.
                        if let Some(ref interceptor) = self.interceptor {
                            let new_path = parent_path.join(&request.name);
                            let _ = interceptor.post_mkdir(&new_path);
                        }
                        response.to_wire(tag)
                    }
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tunlinkat(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tunlinkat::decode(&mut reader) {
            Ok(request) => {
                let parent_path = match self.fid_table.get(request.dirfid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };
                let target_path = parent_path.join(&request.name);
                let is_dir = request.flags & AT_REMOVEDIR != 0;

                // Interceptor pre-hook.
                if let Some(ref interceptor) = self.interceptor {
                    if interceptor.pre_unlink(&target_path, is_dir).is_err() {
                        return encode_error(tag, crate::error::errno::EACCES);
                    }
                }

                match dir::handle_unlinkat(&request, &self.fid_table) {
                    Ok(()) => encode_empty_response(RUNLINKAT, tag),
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_trenameat(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Trenameat::decode(&mut reader) {
            Ok(request) => {
                let old_parent = match self.fid_table.get(request.olddirfid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };
                let new_parent = match self.fid_table.get(request.newdirfid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };
                let old_path = old_parent.join(&request.oldname);
                let new_path = new_parent.join(&request.newname);

                if let Some(err_response) = self.validate_name(tag, &new_parent, &request.newname) {
                    return err_response;
                }

                // Interceptor pre-hook.
                if let Some(ref interceptor) = self.interceptor {
                    if interceptor.pre_rename(&old_path, &new_path).is_err() {
                        return encode_error(tag, crate::error::errno::EACCES);
                    }
                }

                match dir::handle_renameat(&request, &self.fid_table) {
                    Ok(()) => encode_empty_response(RRENAMEAT, tag),
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    // -- Link operations --

    fn handle_tsymlink(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tsymlink::decode(&mut reader) {
            Ok(request) => {
                let parent_path = match self.fid_table.get(request.fid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };

                if let Some(err_response) = self.validate_name(tag, &parent_path, &request.name) {
                    return err_response;
                }

                let link_path = parent_path.join(&request.name);

                match link::handle_symlink(&request, &self.fid_table) {
                    Ok(response) => {
                        // Post-symlink hook.
                        if let Some(ref interceptor) = self.interceptor {
                            let target = std::path::Path::new(&request.symtgt);
                            let _ = interceptor.post_symlink(target, &link_path);
                        }
                        response.to_wire(tag)
                    }
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_treadlink(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Treadlink::decode(&mut reader) {
            Ok(request) => match link::handle_readlink(&request, &self.fid_table) {
                Ok(response) => response.to_wire(tag),
                Err(e) => encode_error(tag, p9_error_to_errno(&e)),
            },
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tlink(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let mut reader = WireReader::new(payload);
        match Tlink::decode(&mut reader) {
            Ok(request) => {
                let dir_path = match self.fid_table.get(request.dfid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };
                let target_path = match self.fid_table.get(request.fid) {
                    Ok(state) => state.path.clone(),
                    Err(e) => return encode_error(tag, p9_error_to_errno(&e)),
                };

                if let Some(err_response) = self.validate_name(tag, &dir_path, &request.name) {
                    return err_response;
                }

                let link_path = dir_path.join(&request.name);

                // Interceptor pre-hook.
                if let Some(ref interceptor) = self.interceptor {
                    if interceptor.pre_link(&target_path, &link_path).is_err() {
                        return encode_error(tag, crate::error::errno::EACCES);
                    }
                }

                match link::handle_link(&request, &self.fid_table) {
                    Ok(()) => encode_empty_response(RLINK, tag),
                    Err(e) => encode_error(tag, p9_error_to_errno(&e)),
                }
            }
            Err(_) => encode_error(tag, crate::error::errno::EIO),
        }
    }

    fn handle_tmknod(&mut self, tag: u16) -> Vec<u8> {
        // Device nodes are not supported — return EPERM.
        encode_error(tag, crate::error::errno::EPERM)
    }

    /// Validate a filename for platform-specific restrictions.
    ///
    /// Checks for reserved names (Windows) and case collisions (case-insensitive
    /// filesystems). Returns an encoded error response if validation fails, or
    /// `None` if the name is valid.
    fn validate_name(&self, tag: u16, parent: &std::path::Path, name: &str) -> Option<Vec<u8>> {
        if platform::is_reserved_name(name) {
            return Some(encode_error(tag, p9_error_to_errno(&P9Error::ReservedName {
                name: name.to_string(),
            })));
        }

        match platform::check_case_collision(parent, name) {
            Ok(Some(existing)) => {
                Some(encode_error(tag, p9_error_to_errno(&P9Error::CaseCollision {
                    existing,
                    attempted: name.to_string(),
                })))
            }
            Ok(None) => None,
            Err(e) => Some(encode_error(tag, p9_error_to_errno(&e))),
        }
    }

    /// Returns the current negotiated maximum message size.
    pub fn msize(&self) -> u32 {
        self.msize
    }

    /// Returns whether version negotiation has been completed.
    pub fn is_negotiated(&self) -> bool {
        self.negotiated
    }

    /// Returns the number of active FIDs.
    pub fn fid_count(&self) -> usize {
        self.fid_table.count()
    }
}

/// Encode an Rlerror response with the given errno.
fn encode_error(tag: u16, ecode: u32) -> Vec<u8> {
    Rlerror { ecode }.to_wire(tag)
}

/// Encode a response with no body (just size + type + tag).
fn encode_empty_response(response_type: u8, tag: u16) -> Vec<u8> {
    let mut writer = WireWriter::new();
    writer.write_u8(response_type);
    writer.write_u16(tag);
    writer.finish()
}

/// Drop guard for in-flight operation tracking.
struct InFlightGuard {
    tracker: InFlightTracker,
}

impl InFlightGuard {
    fn new(tracker: &InFlightTracker) -> Self {
        tracker.begin_operation();
        Self {
            tracker: tracker.clone(),
        }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.tracker.end_operation();
    }
}
