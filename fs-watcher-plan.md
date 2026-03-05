# Plan: External Modification Detection via Filesystem Watching

## Context

Host-side file changes (VS Code edits, `git pull`, etc.) bypass the VM filesystem channel and are never detected. The project plan (section 4.7) requires OS-native file watching to detect these and create undo barriers. The barrier infrastructure (`notify_external_modification()`, `BarrierTracker`, `Event::ExternalModification`) is already implemented and tested — only the **detection mechanism** (the actual filesystem watcher) is missing.

**Prerequisite**: Commit the pending atime fix in `crates/p9/src/server.rs` and `crates/virtiofs-backend/src/intercepted_fs.rs` before starting.

## Approach

Use the `notify` crate (v8, CC0 license) for cross-platform filesystem watching. A background task monitors working directories, debounces events, filters out writes that originated from the sandbox itself (via a `RecentBackendWrites` tracker), and calls the existing `notify_external_modification()` API to create barriers for genuine external changes.

### Correlation: Distinguishing VM Writes from External Writes

A shared `RecentBackendWrites` map records paths written by the filesystem backend or MCP API. When the watcher fires, each path is checked against this map. If present and recent (within 5s TTL), the event is suppressed. Otherwise, it's classified as external.

This is implemented via a **decorator pattern**: `WriteTrackingInterceptor` wraps `UndoInterceptor`, delegates all `WriteInterceptor` methods, and records mutated paths in `RecentBackendWrites`. The decorator is injected at the sandbox level — no changes to the `p9` or `virtiofs-backend` crates needed.

## Files to Create

### `crates/sandbox/src/recent_writes.rs`
- `RecentBackendWrites` struct: `Mutex<HashMap<PathBuf, Instant>>` + configurable TTL
  - `record(path)` — called by backend writes and MCP API writes
  - `was_recent(&path)` — called by watcher to filter
  - `prune_expired()` — cleanup, called periodically by watcher
- `WriteTrackingInterceptor` struct: implements `WriteInterceptor`, wraps `Arc<UndoInterceptor>`
  - Records paths in `RecentBackendWrites` on all mutating methods (`pre_write`, `pre_unlink`, `pre_rename`, `post_create`, `post_mkdir`, etc.)
  - Delegates all calls to inner interceptor

### `crates/sandbox/src/fs_watcher.rs`
- `FsWatcherConfig` struct: debounce duration (default 2s), exclude patterns, enabled flag
- `spawn_fs_watcher()` function: takes working dirs, interceptors, recent writes, event sender
  - Creates `notify::RecommendedWatcher` with std channel sink
  - Bridges to tokio via `spawn_blocking` reader forwarding to `tokio::sync::mpsc`
  - Main async loop: accumulates paths in a `HashSet`, resets debounce timer on each event
  - On debounce timeout: filters against `RecentBackendWrites`, excludes noisy paths (`.git/objects`, undo dir), groups by working dir, calls `interceptor.notify_external_modification()`, emits `Event::ExternalModification`

## Files to Modify

### `Cargo.toml` (workspace root)
- Add `notify = "8"` to `[workspace.dependencies]`

### `crates/sandbox/Cargo.toml`
- Add `notify = { workspace = true }`

### `crates/sandbox/src/lib.rs`
- Add `pub mod recent_writes;` and `pub mod fs_watcher;`

### `crates/sandbox/src/session.rs`
- Add to `Session` struct:
  - `pub fs_watcher_handle: Option<JoinHandle<()>>`
  - `pub recent_writes: Option<Arc<RecentBackendWrites>>`

### `crates/sandbox/src/orchestrator.rs`
- **`do_session_start()`**: After creating interceptors, create `RecentBackendWrites` and spawn watcher. Applies to both VM and non-VM code paths.
- **`do_session_stop()`**: Abort watcher handle alongside other background tasks.
- **`launch_vm()`**: Wrap interceptors in `WriteTrackingInterceptor` before passing to filesystem backends.
- **MCP `write_file`/`edit_file`**: Call `recent_writes.record(path)` after writing.
- **`create_non_vm_session()`**: Accept and store `recent_writes` + watcher handle.

### `crates/sandbox/src/config.rs`
- Add `FileWatcherConfig` to `SandboxTomlConfig` (enabled, debounce_ms, recent_write_ttl_ms, exclude_patterns).

### `crates/sandbox/src/error.rs`
- Add `FileWatcherFailed { reason: String }` variant (non-fatal — logged as warning).

## Key Design Decisions

1. **Watcher failure is non-fatal**: If the watcher can't initialize, the session continues without it. A warning event is emitted.
2. **2s debounce default**: Coalesces rapid edits (VS Code format-on-save) into a single barrier.
3. **5s TTL for RecentBackendWrites**: Accounts for OS event delivery delay (especially macOS FSEvents).
4. **Host-only mode watches too**: Without a VM, all detected changes are external — the watcher is even more useful.
5. **Exclude patterns**: `.git/objects/*`, `node_modules`, the undo directory itself are excluded by default.

## Implementation Order

1. Add `notify` dependency to workspace + sandbox
2. Create `recent_writes.rs` with `RecentBackendWrites` + `WriteTrackingInterceptor` + unit tests
3. Create `fs_watcher.rs` with `spawn_fs_watcher()`
4. Wire into `session.rs` (new fields) and `orchestrator.rs` (spawn/stop/record)
5. Add `FileWatcherConfig` to config
6. Write integration tests (FW-01..FW-12)
7. Update CLAUDE.md

## Verification

1. `cargo check -p codeagent-sandbox` — compiles
2. `cargo test -p codeagent-sandbox` — all existing + new tests pass
3. `cargo clippy -p codeagent-sandbox --tests` — no warnings
4. Manual test: start VM via desktop app, edit a file in VS Code, check Undo History tab shows a barrier
5. Manual test: execute a command via VM, verify NO spurious barriers created
