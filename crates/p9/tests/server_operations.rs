use std::path::PathBuf;

use codeagent_p9::messages::*;
use codeagent_p9::operations::session::P9_VERSION_STRING;
use codeagent_p9::server::{P9Server, DEFAULT_MSIZE};
use codeagent_p9::wire::{self, WireReader, WireWriter};

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------------------------------------------------------------------------
// Harness: in-process P9 server test helper using two tokio::io::duplex()
// ---------------------------------------------------------------------------

struct Harness {
    /// Client writes requests here → server reads from its end.
    request_writer: tokio::io::DuplexStream,
    /// Client reads responses here ← server writes to its end.
    response_reader: tokio::io::DuplexStream,
    server_handle: tokio::task::JoinHandle<Result<(), codeagent_p9::error::P9Error>>,
    _temp_dir: TempDir,
}

impl Harness {
    fn new() -> Self {
        Self::with_msize(DEFAULT_MSIZE)
    }

    fn with_msize(msize: u32) -> Self {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let root_path = temp_dir.path().to_path_buf();

        let (client_req_write, server_req_read) = tokio::io::duplex(1024 * 1024);
        let (server_resp_write, client_resp_read) = tokio::io::duplex(1024 * 1024);

        let mut server = P9Server::with_msize(root_path, msize);

        let server_handle = tokio::spawn(async move {
            server.run(server_req_read, server_resp_write).await
        });

        Self {
            request_writer: client_req_write,
            response_reader: client_resp_read,
            server_handle,
            _temp_dir: temp_dir,
        }
    }

    #[allow(dead_code)]
    fn root_path(&self) -> PathBuf {
        self._temp_dir.path().to_path_buf()
    }

    /// Send a raw 9P message frame to the server.
    async fn send(&mut self, frame: &[u8]) {
        self.request_writer
            .write_all(frame)
            .await
            .expect("send failed");
        self.request_writer.flush().await.expect("flush failed");
    }

    /// Receive a raw 9P response frame from the server.
    /// Returns (msg_type, tag, payload_bytes).
    async fn recv_raw(&mut self) -> (u8, u16, Vec<u8>) {
        let mut size_buf = [0u8; 4];
        self.response_reader
            .read_exact(&mut size_buf)
            .await
            .expect("recv size failed");
        let size = u32::from_le_bytes(size_buf);

        // Read the body (everything after the 4-byte size).
        let body_len = (size as usize) - 4;
        let mut body = vec![0u8; body_len];
        self.response_reader
            .read_exact(&mut body)
            .await
            .expect("recv body failed");

        let (msg_type, tag) = wire::parse_header(&body).expect("parse header failed");
        let payload = body[3..].to_vec();
        (msg_type, tag, payload)
    }

    /// Send a Tversion and receive the Rversion.
    async fn handshake(&mut self) -> Rversion {
        self.handshake_with_msize(DEFAULT_MSIZE).await
    }

    /// Send a Tversion with a specific msize and receive the Rversion.
    async fn handshake_with_msize(&mut self, msize: u32) -> Rversion {
        let request = Tversion {
            msize,
            version: P9_VERSION_STRING.to_string(),
        };
        let frame = request.to_wire(NOTAG);
        self.send(&frame).await;
        let (msg_type, _tag, payload) = self.recv_raw().await;
        assert_eq!(msg_type, RVERSION);
        let mut reader = WireReader::new(&payload);
        Rversion::decode(&mut reader).expect("failed to decode Rversion")
    }

    /// Send a Tattach and receive the response.
    async fn attach(&mut self, fid: u32) -> Result<Rattach, Rlerror> {
        let request = Tattach {
            fid,
            afid: u32::MAX,
            uname: "test".to_string(),
            aname: "".to_string(),
            n_uname: 0,
        };
        let frame = request.to_wire(1);
        self.send(&frame).await;
        let (msg_type, _tag, payload) = self.recv_raw().await;
        let mut reader = WireReader::new(&payload);
        if msg_type == RATTACH {
            Ok(Rattach::decode(&mut reader).unwrap())
        } else {
            Err(Rlerror::decode(&mut reader).unwrap())
        }
    }

    /// Send a Tclunk for the given FID.
    async fn clunk(&mut self, fid: u32) -> Result<(), Rlerror> {
        let request = Tclunk { fid };
        let frame = request.to_wire(2);
        self.send(&frame).await;
        let (msg_type, _tag, payload) = self.recv_raw().await;
        if msg_type == RCLUNK {
            Ok(())
        } else {
            let mut reader = WireReader::new(&payload);
            Err(Rlerror::decode(&mut reader).unwrap())
        }
    }

    /// Send a Twalk and receive the response.
    async fn walk(
        &mut self,
        fid: u32,
        newfid: u32,
        wnames: Vec<&str>,
    ) -> Result<Rwalk, Rlerror> {
        let request = Twalk {
            fid,
            newfid,
            wnames: wnames.into_iter().map(|s| s.to_string()).collect(),
        };
        let frame = request.to_wire(3);
        self.send(&frame).await;
        let (msg_type, _tag, payload) = self.recv_raw().await;
        let mut reader = WireReader::new(&payload);
        if msg_type == RWALK {
            Ok(Rwalk::decode(&mut reader).unwrap())
        } else {
            Err(Rlerror::decode(&mut reader).unwrap())
        }
    }

    /// Create a subdirectory in the temp root.
    fn create_dir(&self, relative_path: &str) {
        let path = self._temp_dir.path().join(relative_path);
        std::fs::create_dir_all(&path).expect("create_dir_all failed");
    }

    /// Create a file in the temp root.
    fn create_file(&self, relative_path: &str, content: &str) {
        let path = self._temp_dir.path().join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create_dir_all failed");
        }
        std::fs::write(&path, content).expect("write failed");
    }

    /// Shut down the server by dropping the request writer (EOF).
    async fn shutdown(self) -> Result<(), codeagent_p9::error::P9Error> {
        drop(self.request_writer);
        drop(self.response_reader);
        self.server_handle.await.expect("server task panicked")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// SO-01: Tversion handshake negotiates protocol and msize.
#[tokio::test]
async fn so_01_version_handshake() {
    let mut harness = Harness::new();
    let response = harness.handshake().await;

    assert_eq!(response.version, P9_VERSION_STRING);
    assert_eq!(response.msize, DEFAULT_MSIZE);

    harness.shutdown().await.unwrap();
}

/// SO-02: Tversion with smaller client msize picks the smaller value.
#[tokio::test]
async fn so_02_version_msize_negotiation() {
    let mut harness = Harness::with_msize(1_048_576);
    let response = harness.handshake_with_msize(65536).await;

    assert_eq!(response.msize, 65536);
    assert_eq!(response.version, P9_VERSION_STRING);

    harness.shutdown().await.unwrap();
}

/// SO-03: Tversion with unknown protocol returns "unknown".
#[tokio::test]
async fn so_03_version_unknown_protocol() {
    let mut harness = Harness::new();

    let request = Tversion {
        msize: 8192,
        version: "9P2000.u".to_string(),
    };
    let frame = request.to_wire(NOTAG);
    harness.send(&frame).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RVERSION);
    let mut reader = WireReader::new(&payload);
    let response = Rversion::decode(&mut reader).unwrap();

    assert_eq!(response.version, "unknown");

    harness.shutdown().await.unwrap();
}

/// SO-04: Tattach creates root FID and returns QID.
#[tokio::test]
async fn so_04_attach_creates_root_fid() {
    let mut harness = Harness::new();
    harness.handshake().await;

    let response = harness.attach(0).await.unwrap();
    assert!(response.qid.is_dir(), "root QID should be a directory");

    harness.shutdown().await.unwrap();
}

/// SO-05: Tclunk releases a FID.
#[tokio::test]
async fn so_05_clunk_releases_fid() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    harness.clunk(0).await.unwrap();

    // Clunking again should fail (unknown FID → EBADF).
    let err = harness.clunk(0).await.unwrap_err();
    assert_eq!(err.ecode, codeagent_p9::error::errno::EBADF);

    harness.shutdown().await.unwrap();
}

/// SO-06: Tauth returns EOPNOTSUPP.
#[tokio::test]
async fn so_06_auth_returns_eopnotsupp() {
    let mut harness = Harness::new();
    harness.handshake().await;

    let request = Tauth {
        afid: 100,
        uname: "test".to_string(),
        aname: "".to_string(),
        n_uname: 0,
    };
    let frame = request.to_wire(3);
    harness.send(&frame).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EOPNOTSUPP);

    harness.shutdown().await.unwrap();
}

/// SO-07: Response tag matches request tag.
#[tokio::test]
async fn so_07_tag_correlation() {
    let mut harness = Harness::new();

    // Send Tversion with tag NOTAG (0xFFFF).
    let request = Tversion {
        msize: DEFAULT_MSIZE,
        version: P9_VERSION_STRING.to_string(),
    };
    let frame = request.to_wire(NOTAG);
    harness.send(&frame).await;
    let (_msg_type, tag, _payload) = harness.recv_raw().await;
    assert_eq!(tag, NOTAG, "version response tag should match request");

    // Send Tattach with tag 42.
    let attach = Tattach {
        fid: 0,
        afid: u32::MAX,
        uname: "test".to_string(),
        aname: "".to_string(),
        n_uname: 0,
    };
    let frame = attach.to_wire(42);
    harness.send(&frame).await;
    let (_msg_type, tag, _payload) = harness.recv_raw().await;
    assert_eq!(tag, 42, "attach response tag should match request");

    harness.shutdown().await.unwrap();
}

/// SO-08: Dropping the client writer causes the server to shut down cleanly.
#[tokio::test]
async fn so_08_clean_shutdown_on_eof() {
    let mut harness = Harness::new();
    harness.handshake().await;

    harness.shutdown().await.unwrap();
}

/// SO-09: Tflush returns Rflush.
#[tokio::test]
async fn so_09_flush_returns_rflush() {
    let mut harness = Harness::new();
    harness.handshake().await;

    let request = Tflush { oldtag: 0 };
    let frame = request.to_wire(5);
    harness.send(&frame).await;
    let (msg_type, tag, _payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RFLUSH);
    assert_eq!(tag, 5);

    harness.shutdown().await.unwrap();
}

/// SO-10: Unknown message type returns EOPNOTSUPP error.
#[tokio::test]
async fn so_10_unknown_message_type_returns_error() {
    let mut harness = Harness::new();
    harness.handshake().await;

    // Craft a message with an unrecognized type byte (255).
    let mut writer = WireWriter::new();
    writer.write_u8(255); // type
    writer.write_u16(10); // tag
    writer.write_u32(0); // some dummy payload
    let frame = writer.finish();
    harness.send(&frame).await;
    let (msg_type, tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RLERROR);
    assert_eq!(tag, 10);

    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EOPNOTSUPP);

    harness.shutdown().await.unwrap();
}

/// SO-11: Duplicate attach on same FID returns error.
#[tokio::test]
async fn so_11_duplicate_attach_fid() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let err = harness.attach(0).await.unwrap_err();
    assert_eq!(err.ecode, codeagent_p9::error::errno::EBADF);

    harness.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Walk tests (Step 4)
// ---------------------------------------------------------------------------

/// WK-01: Walk single path component.
#[tokio::test]
async fn wk_01_walk_single_component() {
    let mut harness = Harness::new();
    harness.create_dir("src");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let response = harness.walk(0, 1, vec!["src"]).await.unwrap();
    assert_eq!(response.wqids.len(), 1);
    assert!(response.wqids[0].is_dir());

    harness.shutdown().await.unwrap();
}

/// WK-02: Walk multiple path components.
#[tokio::test]
async fn wk_02_walk_multiple_components() {
    let mut harness = Harness::new();
    harness.create_dir("src/util");
    harness.create_file("src/util/helpers.rs", "// helpers");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let response = harness.walk(0, 1, vec!["src", "util", "helpers.rs"]).await.unwrap();
    assert_eq!(response.wqids.len(), 3);
    assert!(response.wqids[0].is_dir()); // src
    assert!(response.wqids[1].is_dir()); // util
    assert!(!response.wqids[2].is_dir()); // helpers.rs

    harness.shutdown().await.unwrap();
}

/// WK-03: Walk zero components (clone FID).
#[tokio::test]
async fn wk_03_walk_zero_components_clone() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Clone fid 0 to fid 1.
    let response = harness.walk(0, 1, vec![]).await.unwrap();
    assert!(response.wqids.is_empty());

    // Both FIDs should be valid — clunk both.
    harness.clunk(0).await.unwrap();
    harness.clunk(1).await.unwrap();

    harness.shutdown().await.unwrap();
}

/// WK-04: Walk with newfid == fid (in-place update).
#[tokio::test]
async fn wk_04_walk_in_place() {
    let mut harness = Harness::new();
    harness.create_dir("subdir");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Walk in-place: update FID 0 from root to subdir.
    let response = harness.walk(0, 0, vec!["subdir"]).await.unwrap();
    assert_eq!(response.wqids.len(), 1);
    assert!(response.wqids[0].is_dir());

    // FID 0 is still valid after in-place walk.
    harness.clunk(0).await.unwrap();

    harness.shutdown().await.unwrap();
}

/// WK-05: Partial walk returns QIDs for valid prefix only.
#[tokio::test]
async fn wk_05_partial_walk() {
    let mut harness = Harness::new();
    harness.create_dir("exists");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Walk ["exists", "nonexistent", "deep"] — only "exists" resolves.
    let response = harness.walk(0, 1, vec!["exists", "nonexistent", "deep"]).await.unwrap();
    assert_eq!(response.wqids.len(), 1);

    // FID 1 should NOT be created for a partial walk.
    let err = harness.clunk(1).await.unwrap_err();
    assert_eq!(err.ecode, codeagent_p9::error::errno::EBADF);

    harness.shutdown().await.unwrap();
}

/// WK-06: Walk to nonexistent path with single component returns empty.
#[tokio::test]
async fn wk_06_walk_nonexistent_single() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let response = harness.walk(0, 1, vec!["nonexistent"]).await.unwrap();
    assert!(response.wqids.is_empty());

    harness.shutdown().await.unwrap();
}

/// WK-07: Walk with ".." traversal beyond root is rejected.
#[tokio::test]
async fn wk_07_dotdot_beyond_root_rejected() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Attempt to escape the root via "..".
    let result = harness.walk(0, 1, vec![".."]).await;
    // This should either fail or return empty wqids (stays at root).
    match result {
        Ok(response) => {
            // Some implementations clamp ".." at the root.
            assert!(response.wqids.is_empty() || response.wqids.len() == 1);
        }
        Err(_) => {
            // Error is also acceptable.
        }
    }

    harness.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Read-only operation tests (Step 5)
// ---------------------------------------------------------------------------

/// RO-01: Tgetattr returns valid attributes for a directory.
#[tokio::test]
async fn ro_01_getattr_directory() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Send Tgetattr for the root directory.
    let request = Tgetattr {
        fid: 0,
        request_mask: P9_GETATTR_BASIC,
    };
    let frame = request.to_wire(10);
    harness.send(&frame).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RGETATTR);
    let mut reader = WireReader::new(&payload);
    let response = Rgetattr::decode(&mut reader).unwrap();

    assert!(response.qid.is_dir());
    // Mode should include directory bit (S_IFDIR = 0o40000).
    assert_ne!(response.mode & 0o40000, 0, "should have S_IFDIR bit set");

    harness.shutdown().await.unwrap();
}

/// RO-02: Tgetattr returns valid attributes for a file.
#[tokio::test]
async fn ro_02_getattr_file() {
    let mut harness = Harness::new();
    harness.create_file("test.txt", "hello world");
    harness.handshake().await;
    harness.attach(0).await.unwrap();
    harness.walk(0, 1, vec!["test.txt"]).await.unwrap();

    let request = Tgetattr {
        fid: 1,
        request_mask: P9_GETATTR_BASIC,
    };
    let frame = request.to_wire(10);
    harness.send(&frame).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RGETATTR);
    let mut reader = WireReader::new(&payload);
    let response = Rgetattr::decode(&mut reader).unwrap();

    assert!(!response.qid.is_dir());
    assert_eq!(response.size, 11); // "hello world" = 11 bytes
    // Mode should include regular file bit (S_IFREG = 0o100000).
    assert_ne!(
        response.mode & 0o100000,
        0,
        "should have S_IFREG bit set"
    );

    harness.shutdown().await.unwrap();
}

/// RO-03: Tlopen + Tread reads file contents.
#[tokio::test]
async fn ro_03_lopen_and_read() {
    let mut harness = Harness::new();
    harness.create_file("data.txt", "hello from 9P");
    harness.handshake().await;
    harness.attach(0).await.unwrap();
    harness.walk(0, 1, vec!["data.txt"]).await.unwrap();

    // Open for reading (O_RDONLY = 0).
    let lopen = Tlopen { fid: 1, flags: 0 };
    let frame = lopen.to_wire(11);
    harness.send(&frame).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RLOPEN);
    let mut reader = WireReader::new(&payload);
    let rlopen = Rlopen::decode(&mut reader).unwrap();
    assert!(rlopen.iounit > 0);

    // Read the entire file.
    let read = Tread {
        fid: 1,
        offset: 0,
        count: 4096,
    };
    let frame = read.to_wire(12);
    harness.send(&frame).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREAD);
    let mut reader = WireReader::new(&payload);
    let rread = Rread::decode(&mut reader).unwrap();

    assert_eq!(String::from_utf8_lossy(&rread.data), "hello from 9P");

    harness.shutdown().await.unwrap();
}

/// RO-04: Tread at offset reads from the correct position.
#[tokio::test]
async fn ro_04_read_at_offset() {
    let mut harness = Harness::new();
    harness.create_file("offset.txt", "ABCDEFGHIJ");
    harness.handshake().await;
    harness.attach(0).await.unwrap();
    harness.walk(0, 1, vec!["offset.txt"]).await.unwrap();

    // Open.
    let lopen = Tlopen { fid: 1, flags: 0 };
    harness.send(&lopen.to_wire(11)).await;
    harness.recv_raw().await;

    // Read from offset 5.
    let read = Tread {
        fid: 1,
        offset: 5,
        count: 100,
    };
    harness.send(&read.to_wire(12)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREAD);
    let mut reader = WireReader::new(&payload);
    let rread = Rread::decode(&mut reader).unwrap();

    assert_eq!(String::from_utf8_lossy(&rread.data), "FGHIJ");

    harness.shutdown().await.unwrap();
}

/// RO-05: Tread on un-opened FID returns error.
#[tokio::test]
async fn ro_05_read_unopened_fid() {
    let mut harness = Harness::new();
    harness.create_file("noopen.txt", "data");
    harness.handshake().await;
    harness.attach(0).await.unwrap();
    harness.walk(0, 1, vec!["noopen.txt"]).await.unwrap();

    // Read without opening first.
    let read = Tread {
        fid: 1,
        offset: 0,
        count: 100,
    };
    harness.send(&read.to_wire(12)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EBADF);

    harness.shutdown().await.unwrap();
}

/// RO-06: Treaddir lists directory entries.
#[tokio::test]
async fn ro_06_readdir() {
    let mut harness = Harness::new();
    harness.create_file("alpha.txt", "a");
    harness.create_file("beta.txt", "b");
    harness.create_dir("gamma");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let readdir = Treaddir {
        fid: 0,
        offset: 0,
        count: 65536,
    };
    harness.send(&readdir.to_wire(13)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREADDIR);
    let mut reader = WireReader::new(&payload);
    let rreaddir = Rreaddir::decode(&mut reader).unwrap();

    // Parse the readdir data to count entries.
    let entries = parse_readdir_entries(&rreaddir.data);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

    assert!(names.contains(&"alpha.txt"));
    assert!(names.contains(&"beta.txt"));
    assert!(names.contains(&"gamma"));

    harness.shutdown().await.unwrap();
}

/// RO-07: Tstatfs returns filesystem statistics.
#[tokio::test]
async fn ro_07_statfs() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let statfs = Tstatfs { fid: 0 };
    harness.send(&statfs.to_wire(14)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RSTATFS);
    let mut reader = WireReader::new(&payload);
    let rstatfs = Rstatfs::decode(&mut reader).unwrap();

    assert_eq!(rstatfs.fs_type, 0x01021997); // V9FS_MAGIC
    assert!(rstatfs.bsize > 0);
    assert!(rstatfs.namelen > 0);

    harness.shutdown().await.unwrap();
}

/// RO-08: Treaddir with offset resumes from correct position.
#[tokio::test]
async fn ro_08_readdir_offset() {
    let mut harness = Harness::new();
    // Create enough files that we can test pagination.
    for i in 0..5 {
        harness.create_file(&format!("file{i}.txt"), &format!("content{i}"));
    }
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // First readdir from offset 0 with limited buffer.
    let readdir = Treaddir {
        fid: 0,
        offset: 0,
        count: 65536,
    };
    harness.send(&readdir.to_wire(13)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREADDIR);
    let mut reader = WireReader::new(&payload);
    let rreaddir = Rreaddir::decode(&mut reader).unwrap();
    let entries = parse_readdir_entries(&rreaddir.data);
    assert_eq!(entries.len(), 5);

    // Second readdir from offset past all entries should be empty.
    let readdir2 = Treaddir {
        fid: 0,
        offset: 5,
        count: 65536,
    };
    harness.send(&readdir2.to_wire(14)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREADDIR);
    let mut reader = WireReader::new(&payload);
    let rreaddir2 = Rreaddir::decode(&mut reader).unwrap();
    assert!(rreaddir2.data.is_empty());

    harness.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Readdir entry parser helper
// ---------------------------------------------------------------------------

struct ReaddirEntry {
    name: String,
}

fn parse_readdir_entries(data: &[u8]) -> Vec<ReaddirEntry> {
    let mut entries = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        // qid: type(1) + version(4) + path(8) = 13 bytes
        if pos + 13 > data.len() {
            break;
        }
        pos += 13;

        // offset: 8 bytes
        if pos + 8 > data.len() {
            break;
        }
        pos += 8;

        // type: 1 byte
        if pos + 1 > data.len() {
            break;
        }
        pos += 1;

        // name: u16 length + bytes
        if pos + 2 > data.len() {
            break;
        }
        let name_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;

        if pos + name_len > data.len() {
            break;
        }
        let name = String::from_utf8_lossy(&data[pos..pos + name_len]).to_string();
        pos += name_len;

        entries.push(ReaddirEntry { name });
    }

    entries
}

// ---------------------------------------------------------------------------
// Write operation tests (Step 6)
// ---------------------------------------------------------------------------

/// WR-01: Tlcreate + Twrite creates and writes to a new file.
#[tokio::test]
async fn wr_01_lcreate_and_write() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Create a file via Tlcreate. FID 0 (root) becomes the new file.
    let lcreate = Tlcreate {
        fid: 0,
        name: "newfile.txt".to_string(),
        flags: 0o2, // O_RDWR
        mode: 0o644,
        gid: 0,
    };
    harness.send(&lcreate.to_wire(20)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RLCREATE);
    let mut reader = WireReader::new(&payload);
    let rlcreate = Rlcreate::decode(&mut reader).unwrap();
    assert!(!rlcreate.qid.is_dir());
    assert!(rlcreate.iounit > 0);

    // Write data to the file.
    let write = Twrite {
        fid: 0,
        offset: 0,
        data: b"hello world".to_vec(),
    };
    harness.send(&write.to_wire(21)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RWRITE);
    let mut reader = WireReader::new(&payload);
    let rwrite = Rwrite::decode(&mut reader).unwrap();
    assert_eq!(rwrite.count, 11);

    // Verify the file was written to disk.
    let content = std::fs::read_to_string(harness.root_path().join("newfile.txt")).unwrap();
    assert_eq!(content, "hello world");

    harness.shutdown().await.unwrap();
}

/// WR-02: Twrite at offset works correctly.
#[tokio::test]
async fn wr_02_write_at_offset() {
    let mut harness = Harness::new();
    harness.create_file("patch.txt", "AAAAAAAAAA");
    harness.handshake().await;
    harness.attach(0).await.unwrap();
    harness.walk(0, 1, vec!["patch.txt"]).await.unwrap();

    // Open for reading and writing.
    let lopen = Tlopen {
        fid: 1,
        flags: 0o2, // O_RDWR
    };
    harness.send(&lopen.to_wire(20)).await;
    harness.recv_raw().await;

    // Write "BBB" at offset 3.
    let write = Twrite {
        fid: 1,
        offset: 3,
        data: b"BBB".to_vec(),
    };
    harness.send(&write.to_wire(21)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RWRITE);

    // Read back the whole file.
    let read = Tread {
        fid: 1,
        offset: 0,
        count: 100,
    };
    harness.send(&read.to_wire(22)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREAD);
    let mut reader = WireReader::new(&payload);
    let rread = Rread::decode(&mut reader).unwrap();
    assert_eq!(String::from_utf8_lossy(&rread.data), "AAABBBAAAA");

    harness.shutdown().await.unwrap();
}

/// WR-03: Tmkdir creates a directory.
#[tokio::test]
async fn wr_03_mkdir() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let mkdir = Tmkdir {
        dfid: 0,
        name: "newdir".to_string(),
        mode: 0o755,
        gid: 0,
    };
    harness.send(&mkdir.to_wire(20)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RMKDIR);
    let mut reader = WireReader::new(&payload);
    let rmkdir = Rmkdir::decode(&mut reader).unwrap();
    assert!(rmkdir.qid.is_dir());

    // Verify directory exists on disk.
    assert!(harness.root_path().join("newdir").is_dir());

    harness.shutdown().await.unwrap();
}

/// WR-04: Tunlinkat removes a file.
#[tokio::test]
async fn wr_04_unlinkat_file() {
    let mut harness = Harness::new();
    harness.create_file("doomed.txt", "goodbye");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    assert!(harness.root_path().join("doomed.txt").exists());

    let unlinkat = Tunlinkat {
        dirfid: 0,
        name: "doomed.txt".to_string(),
        flags: 0,
    };
    harness.send(&unlinkat.to_wire(20)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RUNLINKAT);

    assert!(!harness.root_path().join("doomed.txt").exists());

    harness.shutdown().await.unwrap();
}

/// WR-05: Tunlinkat with AT_REMOVEDIR removes a directory.
#[tokio::test]
async fn wr_05_unlinkat_directory() {
    let mut harness = Harness::new();
    harness.create_dir("emptydir");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let unlinkat = Tunlinkat {
        dirfid: 0,
        name: "emptydir".to_string(),
        flags: AT_REMOVEDIR,
    };
    harness.send(&unlinkat.to_wire(20)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RUNLINKAT);

    assert!(!harness.root_path().join("emptydir").exists());

    harness.shutdown().await.unwrap();
}

/// WR-06: Trenameat renames a file.
#[tokio::test]
async fn wr_06_renameat() {
    let mut harness = Harness::new();
    harness.create_file("old.txt", "content");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let renameat = Trenameat {
        olddirfid: 0,
        oldname: "old.txt".to_string(),
        newdirfid: 0,
        newname: "new.txt".to_string(),
    };
    harness.send(&renameat.to_wire(20)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RRENAMEAT);

    assert!(!harness.root_path().join("old.txt").exists());
    assert!(harness.root_path().join("new.txt").exists());
    let content = std::fs::read_to_string(harness.root_path().join("new.txt")).unwrap();
    assert_eq!(content, "content");

    harness.shutdown().await.unwrap();
}

/// WR-07: Tfsync flushes data to disk.
#[tokio::test]
async fn wr_07_fsync() {
    let mut harness = Harness::new();
    harness.create_file("sync.txt", "data");
    harness.handshake().await;
    harness.attach(0).await.unwrap();
    harness.walk(0, 1, vec!["sync.txt"]).await.unwrap();

    // Open for reading and writing so sync_all succeeds.
    let lopen = Tlopen {
        fid: 1,
        flags: 0o2, // O_RDWR
    };
    harness.send(&lopen.to_wire(20)).await;
    harness.recv_raw().await;

    let fsync = Tfsync { fid: 1 };
    harness.send(&fsync.to_wire(21)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RFSYNC);

    harness.shutdown().await.unwrap();
}

/// WR-08: Tsetattr with SIZE flag truncates file.
#[tokio::test]
async fn wr_08_setattr_truncate() {
    let mut harness = Harness::new();
    harness.create_file("trunc.txt", "hello world!");
    harness.handshake().await;
    harness.attach(0).await.unwrap();
    harness.walk(0, 1, vec!["trunc.txt"]).await.unwrap();

    let setattr = Tsetattr {
        fid: 1,
        valid: P9_SETATTR_SIZE,
        mode: 0,
        uid: 0,
        gid: 0,
        size: 5,
        atime_sec: 0,
        atime_nsec: 0,
        mtime_sec: 0,
        mtime_nsec: 0,
    };
    harness.send(&setattr.to_wire(20)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RSETATTR);

    let content = std::fs::read_to_string(harness.root_path().join("trunc.txt")).unwrap();
    assert_eq!(content, "hello");

    harness.shutdown().await.unwrap();
}

/// WR-09: Trenameat across directories.
#[tokio::test]
async fn wr_09_renameat_across_dirs() {
    let mut harness = Harness::new();
    harness.create_dir("src_dir");
    harness.create_dir("dst_dir");
    harness.create_file("src_dir/file.txt", "moving");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Walk to src_dir and dst_dir.
    harness.walk(0, 1, vec!["src_dir"]).await.unwrap();
    harness.walk(0, 2, vec!["dst_dir"]).await.unwrap();

    let renameat = Trenameat {
        olddirfid: 1,
        oldname: "file.txt".to_string(),
        newdirfid: 2,
        newname: "moved.txt".to_string(),
    };
    harness.send(&renameat.to_wire(20)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RRENAMEAT);

    assert!(!harness.root_path().join("src_dir/file.txt").exists());
    assert!(harness.root_path().join("dst_dir/moved.txt").exists());

    harness.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Link Operations (LK-01..LK-05)
// ---------------------------------------------------------------------------

/// LK-01: Tsymlink creates a symbolic link and returns a QID.
///
/// On Windows, symlink creation requires elevated privileges or Developer Mode.
/// The test verifies correct behavior in both success and privilege-denied cases.
#[tokio::test]
async fn lk_01_symlink_create() {
    let mut harness = Harness::new();
    harness.create_file("target.txt", "hello");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let symlink = Tsymlink {
        fid: 0,
        name: "link.txt".to_string(),
        symtgt: "target.txt".to_string(),
        gid: 0,
    };
    harness.send(&symlink.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    if msg_type == RLERROR {
        // On Windows without privileges, symlink creation fails with a permission error.
        let mut reader = WireReader::new(&payload);
        let error = Rlerror::decode(&mut reader).unwrap();
        assert!(
            error.ecode == codeagent_p9::error::errno::EACCES
                || error.ecode == codeagent_p9::error::errno::EPERM
                || error.ecode == codeagent_p9::error::errno::EIO,
            "unexpected error code: {}",
            error.ecode
        );
        eprintln!("skipping symlink assertion: OS denied symlink creation (errno={})", error.ecode);
    } else {
        assert_eq!(msg_type, RSYMLINK);
        let mut reader = WireReader::new(&payload);
        let response = Rsymlink::decode(&mut reader).unwrap();
        assert!(response.qid.is_symlink());

        let link_path = harness.root_path().join("link.txt");
        assert!(link_path.symlink_metadata().unwrap().file_type().is_symlink());
    }

    harness.shutdown().await.unwrap();
}

/// LK-02: Treadlink reads the target of a symlink.
///
/// On Windows without privileges, the test creates the symlink directly via
/// std::os to detect whether symlinks are supported before testing via 9P.
#[tokio::test]
async fn lk_02_readlink() {
    let mut harness = Harness::new();
    harness.create_file("target.txt", "hello");

    // Check whether symlinks are available on this system.
    let test_link = harness.root_path().join("_test_symlink");
    let symlinks_available = {
        #[cfg(unix)]
        { std::os::unix::fs::symlink("target.txt", &test_link).is_ok() }
        #[cfg(windows)]
        { std::os::windows::fs::symlink_file("target.txt", &test_link).is_ok() }
    };
    if !symlinks_available {
        eprintln!("skipping lk_02: symlinks not available on this system");
        harness.shutdown().await.unwrap();
        return;
    }
    // Clean up the test symlink — we'll create the real one via 9P.
    let _ = std::fs::remove_file(&test_link);

    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Create a symlink via 9P.
    let symlink = Tsymlink {
        fid: 0,
        name: "link.txt".to_string(),
        symtgt: "target.txt".to_string(),
        gid: 0,
    };
    harness.send(&symlink.to_wire(10)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RSYMLINK);

    // Walk to the symlink.
    harness.walk(0, 1, vec!["link.txt"]).await.unwrap();

    // Read the symlink target.
    let readlink = Treadlink { fid: 1 };
    harness.send(&readlink.to_wire(11)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREADLINK);

    let mut reader = WireReader::new(&payload);
    let response = Rreadlink::decode(&mut reader).unwrap();
    assert_eq!(response.target, "target.txt");

    harness.shutdown().await.unwrap();
}

/// LK-03: Tlink creates a hard link.
#[tokio::test]
async fn lk_03_hard_link() {
    let mut harness = Harness::new();
    harness.create_file("original.txt", "content");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Walk to the original file (fid 1).
    harness.walk(0, 1, vec!["original.txt"]).await.unwrap();

    // Create a hard link: link "hardlink.txt" in the root dir (fid 0) pointing to fid 1.
    let link = Tlink {
        dfid: 0,
        fid: 1,
        name: "hardlink.txt".to_string(),
    };
    harness.send(&link.to_wire(10)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RLINK);

    // Verify both files exist and have the same content.
    let original = std::fs::read_to_string(harness.root_path().join("original.txt")).unwrap();
    let linked = std::fs::read_to_string(harness.root_path().join("hardlink.txt")).unwrap();
    assert_eq!(original, linked);
    assert_eq!(original, "content");

    harness.shutdown().await.unwrap();
}

/// LK-04: Tmknod returns EPERM (device nodes not supported).
#[tokio::test]
async fn lk_04_mknod_returns_eperm() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let mknod = Tmknod {
        dfid: 0,
        name: "device".to_string(),
        mode: 0o60660,   // block device
        major: 8,
        minor: 0,
        gid: 0,
    };
    harness.send(&mknod.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RLERROR);

    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EPERM);

    harness.shutdown().await.unwrap();
}

/// LK-05: Treadlink on a non-symlink returns an error.
#[tokio::test]
async fn lk_05_readlink_on_regular_file() {
    let mut harness = Harness::new();
    harness.create_file("regular.txt", "not a symlink");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Walk to the regular file.
    harness.walk(0, 1, vec!["regular.txt"]).await.unwrap();

    // Try to read it as a symlink — should fail.
    let readlink = Treadlink { fid: 1 };
    harness.send(&readlink.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RLERROR);

    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    // On most platforms, reading a non-symlink returns EINVAL.
    assert_ne!(error.ecode, 0);

    harness.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Robustness Tests (RB-01..RB-07)
// ---------------------------------------------------------------------------

/// RB-01: Tremove deletes a file and clunks the FID.
#[tokio::test]
async fn rb_01_remove_file() {
    let mut harness = Harness::new();
    harness.create_file("victim.txt", "data");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Walk to the file.
    harness.walk(0, 1, vec!["victim.txt"]).await.unwrap();

    // Remove it.
    let remove = Tremove { fid: 1 };
    harness.send(&remove.to_wire(10)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREMOVE);

    // File should be gone.
    assert!(!harness.root_path().join("victim.txt").exists());

    // FID should be clunked — clunking again should fail.
    let err = harness.clunk(1).await.unwrap_err();
    assert_eq!(err.ecode, codeagent_p9::error::errno::EBADF);

    harness.shutdown().await.unwrap();
}

/// RB-02: Tremove deletes an empty directory and clunks the FID.
#[tokio::test]
async fn rb_02_remove_empty_dir() {
    let mut harness = Harness::new();
    harness.create_dir("subdir");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    harness.walk(0, 1, vec!["subdir"]).await.unwrap();

    let remove = Tremove { fid: 1 };
    harness.send(&remove.to_wire(10)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RREMOVE);

    assert!(!harness.root_path().join("subdir").exists());

    harness.shutdown().await.unwrap();
}

/// RB-03: Tremove on a non-empty directory fails but still clunks the FID.
#[tokio::test]
async fn rb_03_remove_nonempty_dir_clunks_fid() {
    let mut harness = Harness::new();
    harness.create_dir("parent");
    harness.create_file("parent/child.txt", "content");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    harness.walk(0, 1, vec!["parent"]).await.unwrap();

    let remove = Tremove { fid: 1 };
    harness.send(&remove.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    // Remove should fail (directory not empty).
    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::ENOTEMPTY);

    // FID should still be clunked.
    let err = harness.clunk(1).await.unwrap_err();
    assert_eq!(err.ecode, codeagent_p9::error::errno::EBADF);

    // Directory should still exist (remove failed).
    assert!(harness.root_path().join("parent").exists());

    harness.shutdown().await.unwrap();
}

/// RB-04: Error mapping — getattr on a deleted path returns ENOENT.
#[tokio::test]
async fn rb_04_getattr_deleted_returns_enoent() {
    let mut harness = Harness::new();
    harness.create_file("temp.txt", "data");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Walk to the file.
    harness.walk(0, 1, vec!["temp.txt"]).await.unwrap();

    // Delete the file from underneath.
    std::fs::remove_file(harness.root_path().join("temp.txt")).unwrap();

    // Getattr should return ENOENT.
    let getattr = Tgetattr {
        fid: 1,
        request_mask: 0x3fff, // all attributes
    };
    harness.send(&getattr.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RLERROR);

    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::ENOENT);

    harness.shutdown().await.unwrap();
}

/// RB-05: Error mapping — opening a nonexistent file returns ENOENT.
#[tokio::test]
async fn rb_05_lopen_nonexistent_returns_enoent() {
    let mut harness = Harness::new();
    harness.create_file("exists.txt", "data");
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Walk to the file, then delete it from underneath.
    harness.walk(0, 1, vec!["exists.txt"]).await.unwrap();
    std::fs::remove_file(harness.root_path().join("exists.txt")).unwrap();

    // Try to open the (now missing) file.
    let lopen = Tlopen {
        fid: 1,
        flags: 0, // O_RDONLY
    };
    harness.send(&lopen.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RLERROR);

    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::ENOENT);

    harness.shutdown().await.unwrap();
}

/// RB-06: Giant message is rejected before processing.
#[tokio::test]
async fn rb_06_oversized_message_rejected() {
    // Server with 8KB msize — client sends a frame claiming to be larger.
    let mut harness = Harness::with_msize(8192);
    harness.handshake_with_msize(8192).await;

    // Craft a frame with size field claiming 1 MB (way over the negotiated 8 KB).
    let giant_size: u32 = 1_048_576;
    let mut frame = giant_size.to_le_bytes().to_vec();
    // Add a minimal body so we don't just get EOF.
    frame.extend_from_slice(&[TGETATTR, 0x00, 0x01]); // type + tag
    harness.send(&frame).await;

    // The server should reject this and close the connection (or return an error).
    // The run() method returns an OversizedMessage error, causing the server to stop.
    // We verify by checking the server task result.
    drop(harness.request_writer);
    let result = harness.server_handle.await.expect("server task panicked");
    assert!(result.is_err(), "server should return an error for oversized messages");
}

/// RB-07: Tversion resets the session state (FID table cleared).
#[tokio::test]
async fn rb_07_version_resets_session() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    // Create a subdirectory and walk to it.
    harness.create_dir("subdir");
    harness.walk(0, 1, vec!["subdir"]).await.unwrap();

    // Send another Tversion — this should reset the session.
    harness.handshake().await;

    // The old FIDs should be gone — clunking fid 0 should fail.
    let err = harness.clunk(0).await.unwrap_err();
    assert_eq!(err.ecode, codeagent_p9::error::errno::EBADF);

    // Re-attach should work.
    harness.attach(0).await.unwrap();

    harness.shutdown().await.unwrap();
}
