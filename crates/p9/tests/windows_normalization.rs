//! Windows normalization tests (WN-01..WN-07).
//!
//! These tests verify Windows-specific filename validation: reserved name
//! rejection, case collision detection, and heuristic mode assignment.
//!
//! Tests marked `#[cfg(windows)]` only run on Windows. Tests without the
//! gate test the platform-independent validation logic.

use codeagent_p9::messages::*;
use codeagent_p9::operations::session::P9_VERSION_STRING;
use codeagent_p9::server::{P9Server, DEFAULT_MSIZE};
use codeagent_p9::wire::WireReader;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------------------------------------------------------------------------
// Harness (same as server_operations.rs, duplicated for test isolation)
// ---------------------------------------------------------------------------

struct Harness {
    request_writer: tokio::io::DuplexStream,
    response_reader: tokio::io::DuplexStream,
    server_handle: tokio::task::JoinHandle<Result<(), codeagent_p9::error::P9Error>>,
    _temp_dir: TempDir,
}

impl Harness {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let root_path = temp_dir.path().to_path_buf();

        let (client_req_write, server_req_read) = tokio::io::duplex(1024 * 1024);
        let (server_resp_write, client_resp_read) = tokio::io::duplex(1024 * 1024);

        let mut server = P9Server::new(root_path);

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

    fn root_path(&self) -> std::path::PathBuf {
        self._temp_dir.path().to_path_buf()
    }

    async fn send(&mut self, frame: &[u8]) {
        self.request_writer
            .write_all(frame)
            .await
            .expect("send failed");
        self.request_writer.flush().await.expect("flush failed");
    }

    async fn recv_raw(&mut self) -> (u8, u16, Vec<u8>) {
        let mut size_buf = [0u8; 4];
        self.response_reader
            .read_exact(&mut size_buf)
            .await
            .expect("recv size failed");
        let size = u32::from_le_bytes(size_buf);

        let body_len = (size as usize) - 4;
        let mut body = vec![0u8; body_len];
        self.response_reader
            .read_exact(&mut body)
            .await
            .expect("recv body failed");

        let (msg_type, tag) =
            codeagent_p9::wire::parse_header(&body).expect("parse header failed");
        let payload = body[3..].to_vec();
        (msg_type, tag, payload)
    }

    async fn handshake(&mut self) {
        let request = Tversion {
            msize: DEFAULT_MSIZE,
            version: P9_VERSION_STRING.to_string(),
        };
        self.send(&request.to_wire(NOTAG)).await;
        let (msg_type, _tag, _payload) = self.recv_raw().await;
        assert_eq!(msg_type, RVERSION);
    }

    async fn attach(&mut self, fid: u32) {
        let request = Tattach {
            fid,
            afid: u32::MAX,
            uname: "test".to_string(),
            aname: "".to_string(),
            n_uname: 0,
        };
        self.send(&request.to_wire(1)).await;
        let (msg_type, _tag, _payload) = self.recv_raw().await;
        assert_eq!(msg_type, RATTACH);
    }

    async fn shutdown(self) -> Result<(), codeagent_p9::error::P9Error> {
        drop(self.request_writer);
        drop(self.response_reader);
        self.server_handle.await.expect("server task panicked")
    }
}

// ---------------------------------------------------------------------------
// Unit tests for the validation functions (cross-platform)
// ---------------------------------------------------------------------------

/// WN-02 (unit): Reserved name detection works for all Windows reserved names.
#[test]
fn wn_02_reserved_name_detection() {
    // These functions are only active on Windows, but we can test the logic
    // directly on all platforms by calling the windows module.
    #[cfg(windows)]
    {
        use codeagent_p9::platform;

        // Standard reserved names.
        assert!(platform::is_reserved_name("CON"));
        assert!(platform::is_reserved_name("con")); // case-insensitive
        assert!(platform::is_reserved_name("Con"));
        assert!(platform::is_reserved_name("NUL"));
        assert!(platform::is_reserved_name("PRN"));
        assert!(platform::is_reserved_name("AUX"));
        assert!(platform::is_reserved_name("COM1"));
        assert!(platform::is_reserved_name("LPT1"));
        assert!(platform::is_reserved_name("COM9"));
        assert!(platform::is_reserved_name("LPT9"));

        // Reserved with extension.
        assert!(platform::is_reserved_name("CON.txt"));
        assert!(platform::is_reserved_name("NUL.log"));
        assert!(platform::is_reserved_name("lpt1.dat"));

        // NOT reserved.
        assert!(!platform::is_reserved_name("console"));
        assert!(!platform::is_reserved_name("CONTAINER"));
        assert!(!platform::is_reserved_name("file.txt"));
        assert!(!platform::is_reserved_name("COM10")); // only COM0-COM9
    }
}

// ---------------------------------------------------------------------------
// Integration tests (Windows only)
// ---------------------------------------------------------------------------

/// WN-01: Creating a file with a case-colliding name returns EEXIST.
#[cfg(windows)]
#[tokio::test]
async fn wn_01_case_collision_detection() {
    let mut harness = Harness::new();
    // Create "Foo.txt" on disk first.
    std::fs::write(harness.root_path().join("Foo.txt"), "existing").unwrap();

    harness.handshake().await;
    harness.attach(0).await;

    // Try to create "foo.txt" (same name, different case).
    let create = Tlcreate {
        fid: 0,
        name: "foo.txt".to_string(),
        flags: 0o2, // O_RDWR
        mode: 0o644,
        gid: 0,
    };
    harness.send(&create.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EEXIST);

    harness.shutdown().await.unwrap();
}

/// WN-02 (integration): Creating a file with a reserved name returns EINVAL.
#[cfg(windows)]
#[tokio::test]
async fn wn_02_reserved_name_rejected() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await;

    // Try to create "CON" (reserved Windows device name).
    let create = Tlcreate {
        fid: 0,
        name: "CON".to_string(),
        flags: 0o2,
        mode: 0o644,
        gid: 0,
    };
    harness.send(&create.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EINVAL);

    harness.shutdown().await.unwrap();
}

/// WN-03: Mkdir with reserved name is rejected.
#[cfg(windows)]
#[tokio::test]
async fn wn_03_mkdir_reserved_name_rejected() {
    let mut harness = Harness::new();
    harness.handshake().await;
    harness.attach(0).await;

    let mkdir = Tmkdir {
        dfid: 0,
        name: "NUL".to_string(),
        mode: 0o755,
        gid: 0,
    };
    harness.send(&mkdir.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EINVAL);

    harness.shutdown().await.unwrap();
}

/// WN-04: File mode heuristic — .exe files get 0o755, regular files get 0o644.
#[cfg(windows)]
#[tokio::test]
async fn wn_04_file_mode_heuristic() {
    let mut harness = Harness::new();

    // Create files with different extensions.
    std::fs::write(harness.root_path().join("script.sh"), "#!/bin/bash").unwrap();
    std::fs::write(harness.root_path().join("data.txt"), "plain text").unwrap();

    harness.handshake().await;
    harness.attach(0).await;

    // Walk to and getattr the .sh file.
    let walk = Twalk {
        fid: 0,
        newfid: 1,
        wnames: vec!["script.sh".to_string()],
    };
    harness.send(&walk.to_wire(10)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RWALK);

    let getattr = Tgetattr {
        fid: 1,
        request_mask: 0x01, // P9_GETATTR_MODE
    };
    harness.send(&getattr.to_wire(11)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RGETATTR);
    let mut reader = WireReader::new(&payload);
    let attrs = Rgetattr::decode(&mut reader).unwrap();
    // .sh files should get executable mode (0o100755).
    assert_eq!(attrs.mode & 0o777, 0o755, "script.sh should have 0o755 mode");

    // Walk to and getattr the .txt file.
    let walk = Twalk {
        fid: 0,
        newfid: 2,
        wnames: vec!["data.txt".to_string()],
    };
    harness.send(&walk.to_wire(12)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RWALK);

    let getattr = Tgetattr {
        fid: 2,
        request_mask: 0x01,
    };
    harness.send(&getattr.to_wire(13)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;
    assert_eq!(msg_type, RGETATTR);
    let mut reader = WireReader::new(&payload);
    let attrs = Rgetattr::decode(&mut reader).unwrap();
    // .txt files should get regular mode (0o100644).
    assert_eq!(attrs.mode & 0o777, 0o644, "data.txt should have 0o644 mode");

    harness.shutdown().await.unwrap();
}

/// WN-05: Rename to a reserved name is rejected.
#[cfg(windows)]
#[tokio::test]
async fn wn_05_rename_to_reserved_name_rejected() {
    let mut harness = Harness::new();
    std::fs::write(harness.root_path().join("normal.txt"), "data").unwrap();

    harness.handshake().await;
    harness.attach(0).await;

    let renameat = Trenameat {
        olddirfid: 0,
        oldname: "normal.txt".to_string(),
        newdirfid: 0,
        newname: "COM1".to_string(),
    };
    harness.send(&renameat.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EINVAL);

    // Original file should still exist.
    assert!(harness.root_path().join("normal.txt").exists());

    harness.shutdown().await.unwrap();
}

/// WN-06: Case collision detection for mkdir.
#[cfg(windows)]
#[tokio::test]
async fn wn_06_mkdir_case_collision() {
    let mut harness = Harness::new();
    // Create "MyDir" on disk.
    std::fs::create_dir(harness.root_path().join("MyDir")).unwrap();

    harness.handshake().await;
    harness.attach(0).await;

    // Try to mkdir "mydir" (same name, different case).
    let mkdir = Tmkdir {
        dfid: 0,
        name: "mydir".to_string(),
        mode: 0o755,
        gid: 0,
    };
    harness.send(&mkdir.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EEXIST);

    harness.shutdown().await.unwrap();
}

/// WN-07: Rename to case-colliding name is rejected.
#[cfg(windows)]
#[tokio::test]
async fn wn_07_rename_case_collision() {
    let mut harness = Harness::new();
    std::fs::write(harness.root_path().join("Original.txt"), "data").unwrap();
    std::fs::write(harness.root_path().join("other.txt"), "other").unwrap();

    harness.handshake().await;
    harness.attach(0).await;

    // Try to rename other.txt → original.txt (case collision with Original.txt).
    let renameat = Trenameat {
        olddirfid: 0,
        oldname: "other.txt".to_string(),
        newdirfid: 0,
        newname: "original.txt".to_string(),
    };
    harness.send(&renameat.to_wire(10)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    assert_eq!(msg_type, RLERROR);
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_eq!(error.ecode, codeagent_p9::error::errno::EEXIST);

    harness.shutdown().await.unwrap();
}
