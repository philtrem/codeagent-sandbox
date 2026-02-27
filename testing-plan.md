# Sandboxed Coding Agent — Testing Plan

## Preamble: Review of Proposed Strategy

This testing plan is derived from a review of the externally proposed testing strategy. That strategy is strong in several areas: it correctly identifies risk-driven quality goals, proposes a sound test pyramid with CI gating, calls for fault injection and model-based testing (both high-ROI), recommends reusing upstream test suites rather than reinventing them, and sequences TDD development to avoid "debugging QEMU" as the main loop.

The review identified the following issues and gaps that this plan addresses:

1. **Spec ambiguities the strategy correctly flagged but left open.** This plan locks them down (§1) so that tests have precise oracles.
2. **Missing coverage for ambient-step behavior.** Writes arriving outside any active command step must be tested explicitly — the strategy mentions the concept but omits test cases.
3. **Incomplete POSIX metadata overlay testing (Windows).** The strategy's Windows normalization tests don't cover the SQLite-backed overlay store added in the updated project plan for persistent `chmod` tracking.
4. **Reflink/clone detection path untested.** The strategy mentions reflinks as a storage optimization but has no tests for the detection and fallback logic.
5. **Quiescence window edge cases underspecified.** The strategy mentions quiescence tests but doesn't define the timeout, hang-prevention, or interaction with ambient steps.
6. **MCP `write_file` synthetic step lifecycle.** The strategy lists `write_file` producing an API step but doesn't specify the full lifecycle (step open → preimage → write → step close) or error paths.
7. **No explicit test for undo-barrier visibility in `undo.history`.** The strategy tests barrier blocking but not the API shape of barrier entries in history responses.
8. **Event ordering guarantee left unresolved.** This plan chooses a concrete semantics so tests can assert against it.
9. **CI infrastructure for QEMU E2E.** The strategy recommends E2E tests but doesn't address the practical question of KVM availability in CI runners.

All of these are resolved in the sections below.

---

## 1. Spec Decisions Required by Tests

Tests need precise oracles. The following decisions are locked down here and must be reflected in implementation.

### 1.1 `post_create` vs `pre_open_trunc` disambiguation

**Decision:** The filesystem backend is responsible for distinguishing "create new" from "open-and-truncate existing." If the backend's create operation opens an existing file with truncation, the backend must call `pre_open_trunc` (not `post_create`). `post_create` is only called when a genuinely new inode is created. The interceptor does not attempt to detect this itself — the backend has the information (e.g., `O_CREAT|O_TRUNC` vs `O_CREAT|O_EXCL`).

**Test implication:** Tests must verify that overwriting an existing file via `creat()` captures the preimage, while creating a truly new file records `existed_before=false`.

### 1.2 Directory restore ordering during rollback

**Decision:** Rollback processes paths in two passes:
1. **Create pass (depth-first):** Recreate directories (shallowest first), then restore file contents and metadata.
2. **Metadata pass (depth-first, leaves first):** Restore directory metadata (mode, mtime) after all children are restored, proceeding from deepest to shallowest. This prevents child restoration from updating parent directory mtime after it was already restored.

**Test implication:** Tests that delete a directory tree and roll back must assert that both file contents and directory metadata (including mtime) are restored correctly.

### 1.3 Metadata equality semantics

**Decision:** Rollback restores the following, and `TreeSnapshot` comparison asserts them:

| Attribute | Assertion | Notes |
|---|---|---|
| File contents | Byte-exact | Always |
| File type | Exact (reg/dir/symlink) | Always |
| Mode bits | Exact (all 12 bits: suid/sgid/sticky + rwx) | Always on Linux; via overlay on Windows |
| mtime | Within filesystem granularity tolerance (configurable, default 1ms) | FAT32 has 2-second granularity; ext4/APFS are sub-second |
| xattrs | Exact key-value set if the filesystem supports xattrs | Tests skip with explicit reason if unsupported |
| Symlink target | Exact string | Always |
| atime | **Not asserted** | Deliberately excluded — too volatile |

### 1.4 Undo history after rollback

**Decision:** Rollback is a **pop** operation. Rolled-back steps are removed from the history and cannot be re-applied (no "redo"). `undo.history` after `undo.rollback(2)` returns a list that no longer contains the two most recent steps.

**Rationale:** Pop is simpler and avoids the question of whether redo is valid after external modifications or new steps. Redo can be added post-MVP as a separate feature.

### 1.5 STDIO event/response ordering

**Decision:** Events and responses may interleave on stdout. The only ordering guarantee is: the `response` for a given `request_id` is sent after the corresponding operation completes (or fails). Events (`event.step_completed`, `event.terminal_output`, etc.) may arrive before or after the response they are associated with. Clients must correlate by `request_id` and `step_id`, not by position in the stream.

**Test implication:** `JsonlClient` must buffer and correlate, not assume positional ordering.

### 1.6 Ambient step behavior

**Decision:** Filesystem writes that arrive outside any active command step are attributed to a synthetic "ambient" step. Ambient steps:
- Have a system-generated step ID (negative IDs, e.g., `-1`, `-2`, to distinguish from command steps).
- Capture preimages and participate in undo like normal steps.
- Are auto-closed after a configurable inactivity timeout (default 5 seconds of no new writes).
- Appear in `undo.history` with `type: "ambient"` and no associated command.

---

## 2. Test Pyramid and CI Gating

### 2.1 Layers

| Layer | Speed | Scope | Requires |
|---|---|---|---|
| **L1: Unit** | < 1s each | Pure logic: parsers, state machines, manifests, error taxonomy, step tracker, path normalization | Nothing beyond `cargo test` |
| **L2: Component integration** | < 5s each | Real host filesystem + `UndoInterceptor`, WAL, pruning, barrier logic, `TreeSnapshot` comparison | `tempfile` crate, host filesystem |
| **L3: Protocol integration** | < 5s each | 9P server and control channel with in-process clients (no kernel mount, no QEMU) | Tokio test runtime |
| **L4: System / E2E** | 10–60s each | Full agent binary, QEMU guest, STDIO/MCP clients, end-to-end undo/safeguard/barrier validation | QEMU, KVM (or TCG fallback), test guest image |
| **L5: Security fuzzing** | Continuous | `cargo-fuzz` targets for protocol parsers, path normalization, manifest parsing | `libFuzzer`, fuzz corpora |
| **L6: Stress / performance** | Minutes | fsx/fio workloads, large repo operations, sustained write pressure, watcher overflow | QEMU + KVM, `criterion` benchmarks |

### 2.2 CI Gating

**Per-PR (required, must pass before merge):**
- L1 + L2 + L3 (all unit, component, protocol tests)
- L5 fuzz smoke: each fuzz target runs for 30 seconds with existing corpus
- `cargo clippy --all-targets`, `cargo fmt --check`, `cargo deny check`, `cargo audit`
- Total budget: < 10 minutes

**Nightly (blocks release if failing):**
- L4 full QEMU E2E suite (Linux host with KVM; if KVM unavailable, run a reduced TCG subset)
- L5 extended fuzz runs: 10 minutes per target, corpus regression
- L6 performance baselines with 30% regression alert threshold
- Total budget: < 45 minutes

**Pre-release gate:**
- All of the above plus manual review of fuzz coverage report
- Phase 2/3: macOS and Windows E2E suites on dedicated runners

### 2.3 CI Infrastructure for QEMU E2E

QEMU E2E tests require KVM access. Options by CI provider:
- **Self-hosted runner (recommended for nightly):** A Linux VM with nested virtualization enabled (`/dev/kvm` available). Most cloud providers support this (GCP N2, AWS metal/nested, etc.).
- **GitHub Actions:** Use `runs-on: ubuntu-latest` with KVM enabled (available on larger runners). Alternatively, use TCG (software emulation) for a slow but functional subset.
- **Fallback for PRs:** Skip L4 tests on PR CI if KVM is unavailable; gate only on nightly. Mark L4 tests with `#[ignore]` and enable via `--ignored` flag on nightly runs.

---

## 3. Test Harness Architecture

### 3.1 Crate and directory layout

```
sandbox-agent/
  crates/
    test-support/              # Shared test utilities (library crate)
      src/
        lib.rs                 # Re-exports
        workspace.rs           # TempWorkspace: fixture trees + undo dir
        snapshot.rs            # TreeSnapshot + assert_tree_eq
        jsonl_client.rs        # STDIO API test client
        mcp_client.rs          # MCP socket test client
        fake_shim.rs           # In-process fake VM shim
        fault.rs               # Fault injection registry
        clock.rs               # Deterministic clock for tests
        fixtures.rs            # Reusable fixture tree builders
      Cargo.toml               # dev-dependency only

  tests/
    integration/               # L2 component integration tests
      undo_interceptor.rs
      wal_crash_recovery.rs
      undo_barriers.rs
      undo_pruning.rs
      undo_resource_limits.rs
      ambient_steps.rs
      multi_directory.rs
    protocol/                  # L3 protocol integration tests
      control_channel.rs
      stdio_api.rs
      mcp_server.rs
      p9_wire.rs               # Phase 3
    e2e/                       # L4 system tests (require QEMU)
      session_lifecycle.rs
      undo_roundtrip.rs
      safeguard_flow.rs
      external_modification.rs
      mcp_integration.rs
      pjdfstest_subset.rs

  fuzz/                        # L5 fuzz targets
    Cargo.toml
    corpus/
      p9_wire/
      control_jsonl/
      stdio_json/
      mcp_jsonrpc/
      undo_manifest/
      path_normalize/
    fuzz_targets/
      p9_wire.rs
      control_jsonl.rs
      stdio_json.rs
      mcp_jsonrpc.rs
      undo_manifest.rs
      path_normalize.rs

  benches/                     # L6 microbenchmarks
    preimage_capture.rs
    zstd_compression.rs
    rollback_restore.rs
    manifest_io.rs
```

### 3.2 `TempWorkspace`

```rust
/// Creates an isolated working directory + undo directory for a single test.
pub struct TempWorkspace {
    pub working_dir: PathBuf,   // The "shared folder" equivalent
    pub undo_dir: PathBuf,      // Adjacent, outside share root
    _temp: TempDir,             // Dropped on test exit
}

impl TempWorkspace {
    /// Create empty workspace.
    pub fn new() -> Self { ... }

    /// Create workspace from a fixture builder.
    pub fn with_fixture(f: impl FnOnce(&Path)) -> Self { ... }

    /// Snapshot the current state of the working directory.
    pub fn snapshot(&self) -> TreeSnapshot { ... }
}
```

### 3.3 `TreeSnapshot` and `assert_tree_eq`

```rust
pub struct TreeSnapshot {
    pub entries: BTreeMap<PathBuf, EntrySnapshot>,
}

pub struct EntrySnapshot {
    pub file_type: FileType,       // Reg, Dir, Symlink
    pub content_hash: Option<[u8; 32]>,  // blake3 for regular files
    pub size: u64,
    pub mode: u32,
    pub mtime_ns: i128,
    pub symlink_target: Option<String>,
    pub xattrs: BTreeMap<String, Vec<u8>>,
}

pub struct SnapshotCompareOptions {
    pub mtime_tolerance_ns: i128,  // Default: 1_000_000 (1ms)
    pub check_xattrs: bool,        // Default: true on Linux, false on Windows
    pub exclude_patterns: Vec<String>,
}

/// Panics with a human-readable diff on mismatch.
pub fn assert_tree_eq(
    before: &TreeSnapshot,
    after: &TreeSnapshot,
    opts: &SnapshotCompareOptions,
) { ... }
```

### 3.4 `JsonlClient` and `McpClient`

```rust
/// Spawns the agent as a child process, speaks STDIO API.
pub struct JsonlClient {
    child: Child,
    // Reads stdout in a background task, demuxes events and responses
    // into separate channels keyed by request_id / event type.
}

impl JsonlClient {
    pub async fn send(&mut self, msg: Value) -> Result<()>;
    pub async fn recv_response(&mut self, request_id: &str, timeout: Duration) -> Result<Value>;
    pub async fn recv_event(&mut self, event_type: &str, timeout: Duration) -> Result<Value>;
    pub fn stderr_lines(&self) -> Vec<String>;  // For log validation
}

/// Connects to MCP socket, speaks JSON-RPC.
pub struct McpClient { ... }

impl McpClient {
    pub async fn connect(socket_path: &Path) -> Result<Self>;
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value>;
}
```

### 3.5 Deterministic `Clock` and step IDs

The `UndoInterceptor` and step finalization logic accept a `Clock` trait:

```rust
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

pub struct RealClock;
pub struct FakeClock { inner: Mutex<SystemTime> }

impl FakeClock {
    pub fn advance(&self, duration: Duration);
    pub fn set(&self, time: SystemTime);
}
```

Step IDs are host-generated and deterministic in tests (sequential integers starting from a test-provided seed).

### 3.6 Fault injection

Compile-time gated (`cfg(feature = "fault_injection")`), never in release builds.

```rust
pub struct FaultInjector {
    faults: Mutex<VecDeque<Fault>>,
}

pub enum Fault {
    FailPreimageWrite { errno: i32 },     // ENOSPC, EIO, etc.
    FailStepPromotion,                     // Rename WAL→steps fails
    TruncateManifest { after_bytes: u64 }, // Partial manifest write
    ForceWatcherOverflow,                  // Simulate inotify overflow
    ForceLateWrite { delay_ms: u64 },      // Write arrives after step_completed
}

impl FaultInjector {
    pub fn enqueue(&self, fault: Fault);
    /// Returns Some(fault) and removes it, or None.
    pub fn check(&self, point: &str) -> Option<Fault>;
}
```

Injection points are placed at the start of each fallible operation in `UndoInterceptor`:
```rust
if let Some(fault) = self.fault_injector.check("preimage_write") {
    return Err(io::Error::from_raw_os_error(fault.errno()));
}
```

---

## 4. Core Invariants (Driving All Tests)

These are the properties that, if violated, constitute a bug. Every test traces back to one or more of these.

### INV-1: Undo correctness
For any completed, protected step S in working directory D: `undo.rollback(1)` restores `TreeSnapshot(D)` to exactly the snapshot captured before step S began (within the metadata semantics of §1.3).

### INV-2: First-touch fidelity
For any path P mutated multiple times within step S, the stored preimage corresponds to the state of P before the *first* mutation within S. Subsequent mutations within S do not overwrite the preimage.

### INV-3: Crash atomicity
If the agent crashes at any point during a step, restart always produces a working directory state equal to the pre-step snapshot. The WAL `in_progress` directory is removed. An `event.recovery` is emitted.

### INV-4: Barrier integrity
Rollback cannot cross an undo barrier unless `force: true`. Barriers are visible in `undo.history`. Internal sandbox writes never create barriers.

### INV-5: Safeguard pre-operation trigger
The safeguard triggers *before* the operation that would cross the threshold executes on the host filesystem. On `deny`, zero host mutations from the paused-and-denied portion persist.

### INV-6: Path containment
No guest-originated filesystem operation, undo preimage capture, or rollback restore can read or write any host path outside the share root directory.

### INV-7: Parser robustness
No input to any protocol parser (9P wire, control channel JSONL, STDIO JSON, MCP JSON-RPC, undo manifest) causes a panic, unbounded allocation, or undefined behavior. Malformed input produces a structured error.

### INV-8: Transport isolation
STDIO API stdout never contains log output. Stderr never contains protocol messages. MCP socket traffic never appears on stdout/stderr. No cross-contamination.

---

## 5. Component Test Specifications

### 5.1 WriteInterceptor / UndoInterceptor

**Priority:** Highest. This is the core correctness path — implement and test first.

**Test scaffolding required:**
- `TempWorkspace` + `TreeSnapshot` (§3.2, §3.3)
- `StepTracker` test double: allows manual `open_step(id)` / `close_step(id)` calls
- `OperationApplier` helper: calls the interceptor hook, then performs the real `std::fs` operation, mirroring backend behavior

#### Test matrix

| ID | Category | Scenario | Assert |
|---|---|---|---|
| UI-01 | First-touch | Write same file 3× in one step | One preimage stored; rollback restores original |
| UI-02 | Create | Create new file + write content | Rollback deletes file; parent dir mtime restored |
| UI-03 | Create-dir | Create nested directory structure | Rollback removes all created dirs (deepest first) |
| UI-04 | Delete | Delete file | Rollback restores bytes + mode + mtime + xattrs |
| UI-05 | Delete-tree | `rm -rf` simulation (deep nested tree) | Rollback restores full tree with correct structure |
| UI-06 | Rename-new | Rename A→B where B doesn't exist | Rollback restores A, removes B |
| UI-07 | Rename-over | Rename A→B where B exists | Rollback restores both A and B to pre-step state |
| UI-08 | Rename-dir | Rename directory with nested files | Rollback restores all paths under old name |
| UI-09 | Truncate-open | Open existing file with O_TRUNC | `pre_open_trunc` captures preimage before truncation |
| UI-10 | Truncate-setattr | `setattr` truncate to shorter length | Preimage contains original full contents |
| UI-11 | Chmod | Flip executable bit | Rollback restores original mode |
| UI-12 | Xattr-set | Set user xattr on file | Rollback removes xattr (or restores previous value) |
| UI-13 | Xattr-remove | Remove existing xattr | Rollback restores xattr |
| UI-14 | Fallocate | Extend file via fallocate | Rollback restores original size |
| UI-15 | Copy-file-range | Copy range into existing file | Destination preimage captured; rollback restores |
| UI-16 | Multi-step | Steps 1 and 2 modify same file differently | Rollback(1) restores to post-step-1 state, not original |
| UI-17 | Unprotected | Step exceeds `max_single_step_size` | Step marked unprotected; rollback returns error |
| UI-18 | FIFO-eviction | Exceed `max_step_count` | Oldest step evicted; `event.warning` emitted |
| UI-19 | Log-size-eviction | Exceed `max_log_size_bytes` | Oldest steps evicted until within budget |
| UI-20 | Ambient-step | Write arrives outside any command step | Attributed to ambient step; undo works |
| UI-21 | Ambient-timeout | Ambient step auto-closes after inactivity | New write after timeout opens new ambient step |
| UI-22 | Multi-dir | Rollback in dir A | Dir B unmodified |
| UI-23 | Hardlink | Hardlink to file within share root | No panic; behavior documented (path-based capture) |
| UI-24 | Symlink-internal | Symlink within share root | Symlink target string captured; rollback restores |

#### Model-based test (proptest / quickcheck)

Generate random sequences of operations (`CreateFile`, `Write`, `Truncate`, `Chmod`, `Rename`, `Delete`, `Mkdir`, `Rmdir`, `SetXattr`, `RemoveXattr`) grouped into steps. After each step, optionally roll back and compare to the stored snapshot. This catches ordering bugs, rename collisions, and multi-touch edge cases that enumerated tests miss.

```rust
#[proptest]
fn undo_model(ops: Vec<StepOps>) {
    let ws = TempWorkspace::with_fixture(random_small_tree);
    let interceptor = UndoInterceptor::new(ws.undo_dir.clone(), ...);
    for step in &ops {
        let snapshot_before = ws.snapshot();
        interceptor.open_step(step.id);
        for op in &step.ops { op.apply(&ws, &interceptor); }
        interceptor.close_step(step.id);
        if step.should_rollback {
            interceptor.rollback(1);
            assert_tree_eq(&snapshot_before, &ws.snapshot(), &default_opts());
        }
    }
}
```

### 5.2 WAL and Crash Recovery

**Test scaffolding:** Fault injection (§3.6), `TempWorkspace`.

| ID | Scenario | Fault injected | Assert |
|---|---|---|---|
| CR-01 | Crash mid-step (after some preimages written) | Kill process (or return from test without closing step) | Restart rolls back; working dir equals pre-step snapshot |
| CR-02 | Crash during preimage write | `FailPreimageWrite { errno: EIO }` | Operation fails; step becomes unprotected OR write is rejected (test whichever policy is chosen) |
| CR-03 | Crash during step promotion | `FailStepPromotion` | WAL `in_progress` remains; restart rolls back |
| CR-04 | Truncated manifest | `TruncateManifest { after_bytes: 50 }` | Restart detects corruption, rolls back, emits `event.recovery` |
| CR-05 | Clean shutdown (no crash) | None | WAL empty; steps directory contains committed steps |
| CR-06 | Double recovery (restart twice without new writes) | None | Second restart is a no-op; no duplicate events |
| CR-07 | Crash with empty step (step opened but no writes) | Kill before any preimage | Restart discards empty WAL entry; no-op rollback |

### 5.3 External Modifications and Undo Barriers

| ID | Scenario | Assert |
|---|---|---|
| EB-01 | External write during active session | `event.external_modification` emitted with affected paths |
| EB-02 | Rollback across barrier (no force) | Rollback rejected with error listing barrier details |
| EB-03 | Rollback across barrier (force=true) | Rollback proceeds; warning included in response |
| EB-04 | Barrier visible in `undo.history` | History entry has `type: "barrier"` with timestamp and paths |
| EB-05 | Internal sandbox write does NOT trigger barrier | Correlation logic filters backend-originated watcher events |
| EB-06 | Multiple barriers between steps | Each barrier listed; rollback blocked at nearest |
| EB-07 | Watcher overflow | Agent emits warning; degrades to conservative barrier behavior |
| EB-08 | `policy=warn` | External write emits warning but no barrier; rollback proceeds |
| EB-09 | `policy=lock` (if implemented) | External write attempt fails (CI-optional, requires permission control) |

### 5.4 Safeguards

| ID | Scenario | Assert |
|---|---|---|
| SG-01 | Delete count reaches threshold | `event.safeguard_triggered` emitted; no further host mutations while paused |
| SG-02 | Confirm allow | Command completes; step commits; undo works |
| SG-03 | Confirm deny | Entire step rolled back; tree matches pre-step snapshot |
| SG-04 | Timeout (no confirm sent) | Auto-deny; step rolled back |
| SG-05 | Overwrite-large-file threshold | Triggered when existing file > configured size is overwritten |
| SG-06 | Rename-over-existing threshold | Triggered when rename destination exists |
| SG-07 | Queue overflow (request-holding mode) | Queue cap reached; further ops get `ENOSPC`; no OOM |
| SG-08 | QMP pause mode (if available) | QMP `stop` issued on trigger; `cont` on allow/deny; VM verifiably paused |
| SG-09 | Pre-operation trigger ordering | Safeguard fires *before* the Nth deletion executes (not after) |

### 5.5 Control Channel and Step Tracking

**Unit tests (JSONL parsing):**

| ID | Scenario | Assert |
|---|---|---|
| CC-01 | Valid `step_started` / `step_completed` sequence | Step opens and closes; filesystem writes attributed correctly |
| CC-02 | Malformed JSON | Structured error logged; channel not broken |
| CC-03 | Unknown message type | Ignored or logged; channel not broken |
| CC-04 | Oversized message (>1MB) | Rejected before full allocation |
| CC-05 | `step_completed` without `step_started` | Error logged; no crash |
| CC-06 | Duplicate `step_started` for same ID | Error logged; existing step unaffected |
| CC-07 | Cancellation mid-step | Step finalized appropriately |

**Integration tests (fake shim, no QEMU):**

| ID | Scenario | Assert |
|---|---|---|
| CC-08 | Normal exec cycle | Host sends `exec`; fake shim returns started/output/completed; events forwarded |
| CC-09 | Quiescence window: no late writes | Step closes immediately after `step_completed` + quiescence timeout |
| CC-10 | Quiescence window: late write arrives | Step closure waits for in-flight ops to drain; late write included in step |
| CC-11 | Quiescence timeout: prevent hang | If in-flight ops never drain, step closes after max quiescence timeout (e.g., 2s) |
| CC-12 | Ambient writes after step close | Writes after quiescence window go to ambient step, not the closed step |

### 5.6 STDIO API

**Schema tests (unit):**

| ID | Scenario | Assert |
|---|---|---|
| SA-01 | Each request type parses correctly | Valid request → accepted |
| SA-02 | Unknown request type | Structured error: `{code: "unknown_operation", message: "..."}` |
| SA-03 | Missing required field | Structured error with field name |
| SA-04 | Version negotiation (once defined) | Mismatched version → graceful rejection |

**Stream behavior tests (integration with `JsonlClient`):**

| ID | Scenario | Assert |
|---|---|---|
| SA-05 | Response correlates to `request_id` | Response `request_id` matches request |
| SA-06 | Events interleave with responses | Client correctly demuxes both |
| SA-07 | Stderr is valid JSONL logs | Every stderr line parses as JSON with `timestamp`, `level`, `component` |
| SA-08 | Stdout contains no log lines | No line on stdout has `level` or `component` fields |
| SA-09 | Backpressure: client stops reading | Agent does not deadlock (bounded buffers or timeout) |

**Security tests:**

| ID | Scenario | Assert |
|---|---|---|
| SA-10 | `fs.read` with `../../etc/passwd` path | Rejected; resolved relative to working dir root |
| SA-11 | `fs.list` with absolute path outside root | Rejected |
| SA-12 | Oversized `write_file` payload | Size limit enforced; structured error |

### 5.7 MCP Server

| ID | Scenario | Assert |
|---|---|---|
| MC-01 | JSON-RPC compliance (id, errors, unknown method) | Correct JSON-RPC responses |
| MC-02 | `execute_command` returns exit_code/stdout/stderr | Values match what fake shim sent |
| MC-03 | `write_file` creates synthetic API step | Step appears in `undo.history` with `type: "api"` |
| MC-04 | `write_file` → rollback | Written file removed; preimage restored |
| MC-05 | `write_file` error (path outside root) | JSON-RPC error; no step created |
| MC-06 | MCP triggers safeguard → STDIO event emitted | Cross-interface consistency |
| MC-07 | Connection without auth token (if implemented) | Rejected |
| MC-08 | Concurrent MCP + STDIO operations | Shared undo/safeguard state consistent; no races |

### 5.8 Undo Log Storage and Versioning

| ID | Scenario | Assert |
|---|---|---|
| UL-01 | Manifest correctness | Affected paths, `existed_before`, file type, metadata encoding all round-trip |
| UL-02 | Preimage atomicity | Preimage writes use temp file + atomic rename |
| UL-03 | Step promotion atomicity | `wal/in_progress/` renamed to `steps/{id}/` atomically |
| UL-04 | Version mismatch on startup | `event.undo_version_mismatch` emitted; undo disabled |
| UL-05 | `undo.discard` after mismatch | Old log wiped; new version file written; undo re-enabled |
| UL-06 | Corrupt manifest (truncated) | Graceful error; agent doesn't crash |
| UL-07 | Missing preimage file | Rollback returns error for that step; other steps unaffected |
| UL-08 | Corrupt preimage (flipped bytes) | Detected (if checksums used) or rollback produces incorrect state (documented) |
| UL-09 | Reflink detection + fallback | If `FICLONE` succeeds, preimage is a reflink; if it fails, falls back to copy+zstd |

### 5.9 Filesystem Backends

#### 5.9.1 virtiofsd fork (Linux/macOS)

**Unit/integration (no VM):**

| ID | Scenario | Assert |
|---|---|---|
| VF-01 | `InodePathMap`: insert/update/remove/rename | Lookup returns correct path |
| VF-02 | `InodePathMap`: negative lookup | Returns defined error (not panic) |
| VF-03 | `InodePathMap`: path always within root | No path returned outside share root |

**E2E (with QEMU):**

| ID | Scenario | Assert |
|---|---|---|
| VF-04 | Guest performs each primitive op | Undo step lists correct paths; rollback restores snapshot |
| VF-05 | `pjdfstest` curated subset | POSIX semantics match for create/unlink/rename/chmod/symlink |

**Reuse:** Run upstream `virtiofsd` unit tests in fork CI. Keep them passing. Add wrapper-specific tests on top.

#### 5.9.2 9P server (Phase 3)

| ID | Scenario | Assert |
|---|---|---|
| P9-01 | Wire round-trip for each message type | Serialize → deserialize = identity |
| P9-02 | Known-byte fixtures | Match crosvm test vectors (adapt licensing) |
| P9-03 | Invalid sizes/offsets/flags | Correct `Rlerror` errno |
| P9-04 | Out-of-order responses by tag | Pipelined requests handled correctly |
| P9-05 | Oversized message | Rejected before full allocation |

#### 5.9.3 Windows normalization (Phase 3)

| ID | Scenario | Assert |
|---|---|---|
| WN-01 | Case-collision detection | Create `Foo` then `foo` → error |
| WN-02 | Reserved names | Create `CON`, `NUL`, etc. → rejected |
| WN-03 | POSIX metadata overlay: chmod persistence | `chmod 755` → `getattr` returns 755 across sessions |
| WN-04 | Overlay: new file defaults | File without overlay entry gets heuristic mode |
| WN-05 | Overlay: rollback restores mode | Mode changed by step → rollback restores previous overlay entry |
| WN-06 | Reparse point escape | Create junction inside root → outside; write through it → rejected |
| WN-07 | Reparse point: read through junction | Read via junction pointing outside root → rejected |

### 5.10 Session Lifecycle

| ID | Scenario | Assert |
|---|---|---|
| SL-01 | `session.start` with invalid working dir | Structured error |
| SL-02 | `session.start` with multiple dirs | Each dir gets mount tag; backend instances created |
| SL-03 | `session.stop` (persistent) | VM shuts down; disk image preserved |
| SL-04 | `session.stop` (ephemeral) | VM destroyed; disk image deleted |
| SL-05 | `session.reset` | Persistent VM wiped and recreated |
| SL-06 | QEMU launch failure | Structured error event; agent doesn't hang |
| SL-07 | Control channel disconnect | Agent transitions to error state; emits event |
| SL-08 | Resource cleanup on stop | Sockets removed; child processes terminated |

### 5.11 Observability

| ID | Scenario | Assert |
|---|---|---|
| OB-01 | Stderr logs parse as JSONL | Every line has `timestamp`, `level`, `component` |
| OB-02 | `request_id` correlation | Log entries for a request include matching `request_id` |
| OB-03 | `step_id` correlation | Log entries during a step include matching `step_id` |
| OB-04 | No protocol frames in logs | Logs never contain raw 9P bytes or control channel messages |
| OB-05 | Log level filtering | `--log-level=warn` suppresses info/debug/trace |

---

## 6. End-to-End Test Design (QEMU, MVP Linux)

### 6.1 Test guest image

Build a minimal guest image containing:
- Busybox or Alpine base (< 50MB)
- VM-side shim (baked in)
- Core utilities: `sh`, `dd`, `truncate`, `chmod`, `ln`, `mv`, `rm`, `mkdir`, `touch`, `stat`
- Optional: `setfattr`/`getfattr` (for xattr E2E tests)
- No `node`, `cargo`, etc. — those are nightly workload tests

**Build recipe:** Alpine-based `initramfs` created via a Dockerfile or Buildroot config. The `xtask` command `cargo xtask build-guest` produces `vmlinuz` + `initrd.img` for both x86_64 and aarch64.

### 6.2 E2E test pattern

Every E2E test follows this sequence:

```rust
#[tokio::test]
#[ignore] // Only run in nightly CI with KVM
async fn test_undo_single_file_write() {
    let ws = TempWorkspace::with_fixture(small_tree);
    let initial_snapshot = ws.snapshot();

    let mut client = JsonlClient::spawn_agent(&[
        "--working-dir", ws.working_dir.to_str().unwrap(),
        "--undo-dir", ws.undo_dir.to_str().unwrap(),
        "--vm-mode", "ephemeral",
        "--backend", "virtiofs", // or "9p" for Phase 3 tests
    ]).await;

    client.send(session_start()).await;
    client.recv_response("start", Duration::from_secs(30)).await; // VM boot

    // Execute mutation
    client.send(agent_execute("echo 'hello' > /mnt/working/test.txt")).await;
    client.recv_event("event.step_completed", Duration::from_secs(10)).await;

    // Verify mutation happened
    let post_snapshot = ws.snapshot();
    assert_ne!(&initial_snapshot, &post_snapshot);

    // Rollback
    client.send(undo_rollback(1)).await;
    client.recv_response("rollback", Duration::from_secs(5)).await;

    // Verify restoration
    assert_tree_eq(&initial_snapshot, &ws.snapshot(), &default_opts());

    client.send(session_stop()).await;
}
```

### 6.3 Fixture trees

```rust
pub fn small_tree(root: &Path) {
    // Files of various sizes
    fs::write(root.join("empty.txt"), "");
    fs::write(root.join("small.txt"), "hello world");
    fs::write(root.join("medium.txt"), "x".repeat(4096));
    fs::write(root.join("large.bin"), &vec![0xABu8; 1_000_000]);
    // Nested directories
    fs::create_dir_all(root.join("src/components"));
    fs::write(root.join("src/main.rs"), "fn main() {}");
    fs::write(root.join("src/components/app.rs"), "pub struct App;");
    // Executable file
    let exec_path = root.join("run.sh");
    fs::write(&exec_path, "#!/bin/sh\necho ok");
    fs::set_permissions(&exec_path, Permissions::from_mode(0o755));
}

pub fn rename_tree(root: &Path) { /* a.txt + b.txt with distinct contents */ }
pub fn xattr_tree(root: &Path) { /* file with user.test xattr */ }
pub fn symlink_tree(root: &Path) { /* symlinks to file and dir */ }
pub fn deep_tree(root: &Path) { /* 5 levels deep, 100+ files for safeguard tests */ }
```

---

## 7. Security Testing

### 7.1 Fuzz targets

Each target is a `fuzz_target!` in `fuzz/fuzz_targets/`. Corpora are committed and grow over time.

| Target | Input | Assertions |
|---|---|---|
| `p9_wire` | Raw bytes | No panic; no alloc > 16MB; error on invalid input |
| `control_jsonl` | UTF-8 string (one line) | No panic; no alloc > 1MB; valid parse or structured error |
| `stdio_json` | UTF-8 string (one line) | No panic; no alloc > 1MB; valid parse or structured error |
| `mcp_jsonrpc` | UTF-8 string | No panic; no alloc > 1MB |
| `undo_manifest` | Raw bytes (simulated manifest file) | No panic; valid parse or error |
| `path_normalize` | `Vec<Vec<u8>>` (path components) | No panic; result is within root or error; no `..` traversal |

### 7.2 Containment tests

| ID | Scenario | Phase | Assert |
|---|---|---|---|
| SC-01 | Symlink in working dir → `/etc/passwd` (Linux virtiofsd+chroot) | MVP | Preimage capture does not access `/etc/passwd` |
| SC-02 | Rollback with symlink to outside root | MVP | Restore does not write outside root |
| SC-03 | Symlink chain (A→B→C→outside) | MVP | Entire chain resolved; access denied |
| SC-04 | macOS: symlink escape without chroot | Phase 2 | `openat`-relative containment rejects |
| SC-05 | macOS: TOCTOU during `F_GETPATH` re-open | Phase 2 | No re-open by path; use fd directly |
| SC-06 | Windows: junction to `C:\Windows\System32` | Phase 3 | 9P server rejects; no host access |
| SC-07 | Windows: reparse point during rename | Phase 3 | Rename target validated within root |

### 7.3 DoS / resource exhaustion

| ID | Scenario | Assert |
|---|---|---|
| DOS-01 | Create many unique files in one step until `max_single_step_size` hit | Step becomes unprotected; agent responsive |
| DOS-02 | Safeguard pause: flood with filesystem ops | Queue capped; `ENOSPC` beyond cap; no OOM |
| DOS-03 | Many concurrent 9P requests (pipelined) | Agent handles within bounded memory |
| DOS-04 | Giant 9P message (size field claims 2GB) | Rejected at wire parse; no allocation |

### 7.4 CI hardening checks

Run as part of per-PR CI:
- `cargo audit` — known vulnerability check
- `cargo deny check` — license and advisory policy
- `cargo clippy --all-targets` — lint
- Sanitizer jobs (nightly CI): ASan + UBSan on fuzz targets, TSAN on concurrency-heavy unit tests if feasible

---

## 8. Performance Testing

### 8.1 Microbenchmarks (`criterion`, per-PR acceptable)

| Benchmark | Sizes | Regression threshold |
|---|---|---|
| Preimage capture throughput | 4KB, 1MB, 100MB | 30% regression alerts |
| zstd compression (level 3) | 4KB, 1MB, 100MB | 30% |
| Rollback restore throughput | 4KB, 1MB, 100MB | 30% |
| Manifest write + promotion | 10 paths, 100 paths, 1000 paths | 30% |
| `TreeSnapshot` capture | 100 files, 1000 files, 10000 files | 30% |

### 8.2 Macrobenchmarks (nightly / manual, in QEMU)

| Workload | Metrics |
|---|---|
| `git status` on large repo (Linux kernel tree) | Wall time, agent RSS |
| `rm -rf node_modules` (10,000 files) → undo | Wall time, undo log size, restore time |
| `fsx` (random filesystem exerciser) for 60s | No errors; agent RSS stable |
| `fio` sequential 1MB writes × 1000 | Throughput vs baseline (no interception) |

Record results in CI artifacts for trend analysis. Alert on >30% regression from rolling 7-day baseline.

---

## 9. Reusing Upstream Tests

| Suite | Source | How to use | Phase |
|---|---|---|---|
| virtiofsd unit tests | Upstream fork | Run in fork CI; keep passing; add wrapper tests on top | MVP |
| `pjdfstest` | `github.com/pjd/pjdfstest` | Run curated subset inside guest against `/mnt/working` | MVP |
| crosvm `p9` crate fixtures | `chromium.googlesource.com/crosvm` | Port known-byte test vectors for wire format; adapt attribution | Phase 3 |
| Mutagen test vectors | `github.com/mutagen-io/mutagen` | Port reserved-name tables, case-collision scenarios, chmod persistence behaviors as Rust table-driven tests | Phase 3 |
| `xfstests` (optional) | `github.com/kdave/xfstests` | Run small subset externally (not vendored) for extended POSIX validation | Nightly |

---

## 10. Cross-Platform Test Matrix

| Phase | Host OS | Backend | CI Runner | Must pass before moving on |
|---|---|---|---|---|
| **MVP** | Linux x86_64 | virtiofsd fork | GitHub Actions + self-hosted KVM runner | L1–L3 full; L4 E2E subset; L5 fuzz smoke; `pjdfstest` subset |
| **Phase 2** | macOS Apple Silicon | virtiofsd fork (ported) | macOS self-hosted runner (M-series) | macOS containment tests; portability layer tests; FSEvents barrier tests; E2E mount+undo |
| **Phase 3** | Windows x86_64 | 9P server | Windows self-hosted runner with WHPX | 9P wire+dispatch tests; junction/reparse containment; case/reserved-name tests; metadata overlay tests; WHPX E2E |

---

## 11. TDD Development Sequence

This ordering keeps the tight TDD loop fast and avoids "debugging QEMU" as the primary development activity.

| Step | What to build | What to test | Layer |
|---|---|---|---|
| 1 | `TreeSnapshot` + `assert_tree_eq` | Snapshot round-trip; equality and diff output | L1 |
| 2 | `UndoInterceptor` core (first-touch, preimage write, rollback) | UI-01 through UI-08 (create/write/rename/delete → rollback) | L2 |
| 3 | WAL + crash recovery | CR-01 through CR-07 (fault injection, no VM) | L2 |
| 4 | Undo barriers | EB-01 through EB-06 (external mod simulation) | L2 |
| 5 | Safeguards (interceptor level) | SG-01 through SG-06 (simulate delete counts) | L2 |
| 6 | Metadata capture (mode, mtime, xattrs) | UI-09 through UI-15 | L2 |
| 7 | Resource limits + pruning | UI-17 through UI-19, UL-01 through UL-09 | L2 |
| 8 | Control channel parsing + state machine | CC-01 through CC-07 | L1 |
| 9 | Control channel integration (fake shim) | CC-08 through CC-12 (quiescence, ambient) | L3 |
| 10 | STDIO API contract tests | SA-01 through SA-12 | L3 |
| 11 | MCP server contract tests | MC-01 through MC-08 | L3 |
| 12 | Fuzz targets (initial) | All 6 fuzz targets with seed corpus | L5 |
| 13 | QEMU E2E: session lifecycle | SL-01 through SL-08 | L4 |
| 14 | QEMU E2E: undo round-trip | `echo` → step → rollback → snapshot compare | L4 |
| 15 | QEMU E2E: `pjdfstest` subset | POSIX semantics validation | L4 |
| 16 | QEMU E2E: safeguard flow | `rm -rf` in VM → trigger → deny → verify rollback | L4 |
| 17 | Model-based / property tests | Random op sequences → rollback → snapshot | L2 |
| 18 | Performance baselines | Microbenchmarks (`criterion`) | L6 |

Steps 1–7 require no QEMU, no networking, no async runtime — pure Rust + filesystem. This is where the majority of correctness bugs will be found and fixed.

---

## 12. `cargo xtask` Commands

```
cargo xtask test-fast       # L1 + L2 + L3 (per-PR)
cargo xtask test-fuzz-smoke  # L5 short runs (per-PR)
cargo xtask test-e2e         # L4 (requires KVM; nightly)
cargo xtask test-all         # Everything
cargo xtask fuzz <target>    # Run a specific fuzz target continuously
cargo xtask bench            # L6 microbenchmarks
cargo xtask build-guest      # Build test guest image (vmlinuz + initrd)
cargo xtask ci-check         # clippy + fmt + deny + audit
```

---

## 13. Cargo Features

```toml
[features]
default = []
fault_injection = []  # Enables FaultInjector compile paths; never in release
e2e_tests = []        # Enables QEMU E2E test compilation
```

Tests use:
```
cargo test                                    # L1 + L2 + L3
cargo test --features fault_injection         # L2 with fault injection
cargo test --features e2e_tests --ignored     # L4 QEMU E2E
```
