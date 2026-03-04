//! Integration test: P9 Server + UndoInterceptor + InFlightTracker.
//!
//! Verifies that a TUNLINKAT message flowing through the P9 server with a real
//! `UndoInterceptor` correctly deletes the file, tracks in-flight operations,
//! and captures the preimage for undo.

use std::path::PathBuf;
use std::sync::Arc;

use codeagent_control::InFlightTracker;
use codeagent_interceptor::undo_interceptor::UndoInterceptor;
use codeagent_interceptor::write_interceptor::WriteInterceptor;
use codeagent_p9::messages::*;
use codeagent_p9::operations::session::P9_VERSION_STRING;
use codeagent_p9::server::{P9Server, DEFAULT_MSIZE};
use codeagent_p9::wire::{self, WireReader};

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------------------------------------------------------------------------
// Harness: same duplex-stream approach as server_operations.rs but with
// interceptor and in-flight tracker wired in.
// ---------------------------------------------------------------------------

struct InterceptorHarness {
    request_writer: tokio::io::DuplexStream,
    response_reader: tokio::io::DuplexStream,
    server_handle: tokio::task::JoinHandle<Result<(), codeagent_p9::error::P9Error>>,
    working_dir: TempDir,
    _undo_dir: TempDir,
    interceptor: Arc<UndoInterceptor>,
    tracker: InFlightTracker,
}

impl InterceptorHarness {
    fn new() -> Self {
        let working_dir = TempDir::new().expect("working_dir");
        let undo_dir = TempDir::new().expect("undo_dir");
        let root_path = working_dir.path().to_path_buf();

        let interceptor = Arc::new(UndoInterceptor::new(
            working_dir.path().to_path_buf(),
            undo_dir.path().to_path_buf(),
        ));
        let tracker = InFlightTracker::new();

        let (client_req_write, server_req_read) = tokio::io::duplex(1024 * 1024);
        let (server_resp_write, client_resp_read) = tokio::io::duplex(1024 * 1024);

        let interceptor_for_server: Arc<dyn WriteInterceptor> = interceptor.clone();
        let tracker_for_server = tracker.clone();

        let mut server = P9Server::new(root_path)
            .with_interceptor(interceptor_for_server)
            .with_in_flight(tracker_for_server);

        let server_handle = tokio::spawn(async move {
            server.run(server_req_read, server_resp_write).await
        });

        Self {
            request_writer: client_req_write,
            response_reader: client_resp_read,
            server_handle,
            working_dir,
            _undo_dir: undo_dir,
            interceptor,
            tracker,
        }
    }

    fn root_path(&self) -> PathBuf {
        self.working_dir.path().to_path_buf()
    }

    fn create_file(&self, relative_path: &str, content: &str) {
        let path = self.working_dir.path().join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create_dir_all failed");
        }
        std::fs::write(&path, content).expect("write failed");
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

        let (msg_type, tag) = wire::parse_header(&body).expect("parse header failed");
        let payload = body[3..].to_vec();
        (msg_type, tag, payload)
    }

    async fn handshake(&mut self) -> Rversion {
        let request = Tversion {
            msize: DEFAULT_MSIZE,
            version: P9_VERSION_STRING.to_string(),
        };
        self.send(&request.to_wire(NOTAG)).await;
        let (msg_type, _tag, payload) = self.recv_raw().await;
        assert_eq!(msg_type, RVERSION);
        let mut reader = WireReader::new(&payload);
        Rversion::decode(&mut reader).expect("decode Rversion")
    }

    async fn attach(&mut self, fid: u32) -> Result<Rattach, Rlerror> {
        let request = Tattach {
            fid,
            afid: u32::MAX,
            uname: "test".to_string(),
            aname: "".to_string(),
            n_uname: 0,
        };
        self.send(&request.to_wire(1)).await;
        let (msg_type, _tag, payload) = self.recv_raw().await;
        let mut reader = WireReader::new(&payload);
        if msg_type == RATTACH {
            Ok(Rattach::decode(&mut reader).unwrap())
        } else {
            Err(Rlerror::decode(&mut reader).unwrap())
        }
    }

    async fn shutdown(self) -> Result<(), codeagent_p9::error::P9Error> {
        drop(self.request_writer);
        drop(self.response_reader);
        self.server_handle.await.expect("server task panicked")
    }
}

// ===========================================================================
// Test 4: P9 TUNLINKAT with real UndoInterceptor + InFlightTracker
// ===========================================================================

/// Verify that TUNLINKAT through P9 server with a real UndoInterceptor:
/// 1. Deletes the file on disk.
/// 2. InFlightTracker count returns to zero after the operation.
/// 3. UndoInterceptor captures the preimage (step can be closed without error).
#[tokio::test]
async fn cp_08_p9_tunlinkat_with_interceptor() {
    let mut harness = InterceptorHarness::new();
    harness.create_file("doomed.txt", "farewell, cruel world");

    // Open an undo step so the interceptor captures the preimage.
    harness.interceptor.open_step(1).expect("open_step");

    // 9P handshake + attach.
    harness.handshake().await;
    harness.attach(0).await.unwrap();

    assert!(harness.root_path().join("doomed.txt").exists());

    // Send TUNLINKAT.
    let unlinkat = Tunlinkat {
        dirfid: 0,
        name: "doomed.txt".to_string(),
        flags: 0,
    };
    harness.send(&unlinkat.to_wire(20)).await;
    let (msg_type, _tag, _payload) = harness.recv_raw().await;

    // Should succeed (RUNLINKAT, not Rlerror).
    assert_eq!(msg_type, RUNLINKAT, "expected RUNLINKAT response");

    // File should be deleted.
    assert!(
        !harness.root_path().join("doomed.txt").exists(),
        "doomed.txt should no longer exist"
    );

    // InFlightTracker should be back to zero (guard dropped after dispatch).
    assert_eq!(
        harness.tracker.count(),
        0,
        "in-flight count should be 0 after operation completes"
    );

    // Close the undo step — should succeed because the interceptor captured
    // the preimage during the pre_unlink hook.
    let evicted = harness.interceptor.close_step(1).expect("close_step");
    assert!(evicted.is_empty(), "no steps should have been evicted");

    harness.shutdown().await.unwrap();
}

/// Verify that TUNLINKAT for a non-existent file returns Rlerror (not a panic).
#[tokio::test]
async fn cp_09_p9_tunlinkat_nonexistent() {
    let mut harness = InterceptorHarness::new();
    harness.interceptor.open_step(1).expect("open_step");

    harness.handshake().await;
    harness.attach(0).await.unwrap();

    let unlinkat = Tunlinkat {
        dirfid: 0,
        name: "nonexistent.txt".to_string(),
        flags: 0,
    };
    harness.send(&unlinkat.to_wire(20)).await;
    let (msg_type, _tag, payload) = harness.recv_raw().await;

    // Should return Rlerror (ENOENT), not RUNLINKAT.
    assert_eq!(msg_type, RLERROR, "expected Rlerror for nonexistent file");
    let mut reader = WireReader::new(&payload);
    let error = Rlerror::decode(&mut reader).unwrap();
    assert_ne!(error.ecode, 0, "error code should be non-zero");

    // InFlightTracker should still be zero.
    assert_eq!(harness.tracker.count(), 0);

    let _ = harness.interceptor.close_step(1);
    harness.shutdown().await.unwrap();
}
