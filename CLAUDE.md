# Agent Instructions

## General
- You are Claude Code. Actions that would be time consuming for a human — writing tests, building
  features, refactoring code — are fast and comparatively cheap for you.
- Conversation history gets compacted once the context window reaches its limit.
  Important details from earlier in the conversation — including plans, discoveries, and
  decisions — may be lost. Proactively write important information to files so it persists
  beyond context compression.

## Planning
- **Confirm before implementing**: After writing a plan but before starting implementation, always present the plan to the user and ask if they have any questions or concerns. Do not begin coding until the user confirms.

## Core Principles
- **Verify before deleting**: Before deleting any files or folders, always verify they are not referenced elsewhere in the codebase using grep or other search tools. Never assume a file is unused.
- **Verify assumptions**: Before acting on any assumption about the codebase (API signatures, available methods, file locations, type constraints, etc.), read the relevant source. Use grep, glob, or file reads to confirm. Do not assume — check.
- **Verify with builds and tests**: After making changes, build the affected project and run existing tests to confirm nothing is broken. When the correct behaviour of a piece of logic is non-obvious, write a test to verify it — including temporary/throwaway tests if that is the fastest way to confirm an assumption. Remove temporary tests once they have served their purpose.

## Code Documentation
- **Do not** add comments that merely describe the changes made (e.g., "Modified this to fix bug X").
- Comments should be reserved for explaining the **code and functionality** themselves (the "how" and "why" of the logic), adhering to standard clean code practices.

## Variable Naming
- Use clear, descriptive names for all variables.
- Avoid obscure abbreviations (e.g., use `isCollection` instead of `isColl`).

## Workflow
- **Write plans to a file before implementing**: For non-trivial tasks, write the plan to a
  markdown file in the repo before starting implementation. Delete when done.
- **Stop and reassess after repeated failures**: If consecutive fix attempts fail to resolve
  an issue, stop and reconsider the approach rather than continuing to apply further fixes.
- **Commits should be focused and well-delimited**: Each commit should represent one coherent,
  self-contained piece of work (e.g. a bug fix, a single new feature, a refactor, a docs update).
  Do not bundle unrelated changes into a single commit. Compare DIFFs to do so. When a file
  contains changes that belong in separate commits, use `git add -p` to stage specific hunks
  rather than editing the file, committing, and re-applying changes.
  Do not add any Claude attribution or co-author lines to commit messages.
- **Keep CLAUDE.md up to date**: After completing a TDD step or any significant implementation
  milestone, update the **Implementation Status** and **Workspace Structure** sections of this
  file to reflect the current state — including new files, new test counts, newly completed
  steps, and any changed conventions. This ensures future sessions start with accurate context
  rather than stale information.
- **Ask before committing**: After completing a unit of work, ask the user whether they would
  like to commit the changes rather than waiting for them to request it.

## Project-Specific Knowledge

### Project Overview
Code Agent is a sandboxed coding agent that runs inside a Linux VM (QEMU), with host-side
filesystem interception for N-step undo capability. The project will be released as open source
under MIT OR Apache-2.0 dual license.

Design documents:
- `project-plan.md` — full architecture (WriteInterceptor trait, undo log, STDIO API, MCP server, VM)
- `testing-plan.md` — 6-layer test pyramid (L1–L6), test matrix UI-01..UI-24, spec decisions

### Workspace Structure
Rust workspace at repo root: `resolver = "3"`, `edition = "2024"`, `rust-version = "1.85"`.

```
Cargo.toml                         # workspace root
crates/
  common/                          # codeagent-common — shared types and errors
    src/lib.rs                     #   StepId, StepType, StepInfo, BarrierId, BarrierInfo,
                                   #   SafeguardId, SafeguardKind, SafeguardConfig, SafeguardEvent,
                                   #   SafeguardDecision, ExternalModificationPolicy, RollbackResult,
                                   #   ResourceLimitsConfig, CodeAgentError (incl. RollbackBlocked,
                                   #   SafeguardDenied, StepUnprotected, UndoDisabled), Result<T>
  control/                         # codeagent-control — control channel protocol
    src/
      lib.rs                       #   module declarations + re-exports
      error.rs                     #   ControlChannelError enum
      protocol.rs                  #   HostMessage (Exec, Cancel, RollbackNotify),
                                   #   VmMessage (StepStarted, Output, StepCompleted),
                                   #   OutputStream
      parser.rs                    #   JSONL parsing with 1MB size limit
      state_machine.rs             #   ControlChannelState, ControlEvent, PendingCommand,
                                   #   ActiveCommand — validates message sequences
    tests/
      control_channel.rs           #   CC-01..CC-07 + edge cases
  interceptor/                     # codeagent-interceptor — undo log core
    src/
      lib.rs                       #   module declarations
      write_interceptor.rs         #   WriteInterceptor trait (13 methods)
      safeguard.rs                 #   SafeguardHandler trait, SafeguardTracker (per-step counters)
      step_tracker.rs              #   StepTracker (Mutex-based step lifecycle, incl. cancel_step)
      preimage.rs                  #   path_hash, PreimageMetadata, capture/restore preimages
      manifest.rs                  #   StepManifest, ManifestEntry (JSON on disk)
      rollback.rs                  #   rollback_step (two-pass: delete→recreate→restore)
      barrier.rs                   #   BarrierTracker (in-memory + JSON persistence)
      resource_limits.rs             #   calculate_step_size, calculate_total_log_size, evict_if_needed
      gitignore.rs                 #   GitignoreFilter (opt-in .gitignore-aware preimage skipping)
      undo_interceptor.rs          #   UndoInterceptor, RecoveryInfo, recover(), WriteInterceptor impl,
                                   #   notify_external_modification(), barriers(), rollback(count, force),
                                   #   with_safeguard(), rollback_current_step(), safeguard checks in pre_*,
                                   #   with_resource_limits(), with_gitignore(), discard(), is_undo_disabled(),
                                   #   version check
    tests/
      common/mod.rs                #   shared test helpers: OperationApplier, compare_opts
      undo_interceptor.rs          #   integration tests UI-01..UI-08
      wal_crash_recovery.rs        #   crash recovery tests CR-01..CR-07 + step reconstruction
      undo_barriers.rs             #   undo barrier tests EB-01..EB-06, EB-08
      safeguards.rs                #   safeguard tests SG-01..SG-06 + edge cases
      resource_limits.rs           #   resource limit tests UI-16..UI-19, UL-01..UL-08
      gitignore.rs                 #   gitignore filter tests GI-01..GI-08
  test-support/                    # codeagent-test-support — test utilities
    src/
      lib.rs                       #   re-exports
      snapshot.rs                  #   TreeSnapshot, EntrySnapshot, assert_tree_eq
      workspace.rs                 #   TempWorkspace (isolated temp dir pairs)
      fixtures.rs                  #   small_tree, rename_tree, symlink_tree, deep_tree
```

### Key Conventions
- **Cross-platform path handling**: All internal path strings (preimage metadata, manifest keys,
  touched-paths sets, path hashes) use forward slashes. Convert with `.replace('\\', "/")`.
  The `preimage::path_hash()` function normalizes before hashing.
- **Platform-conditional compilation**: `#[cfg(unix)]` for real mode bits and symlinks,
  `#[cfg(windows)]` for synthetic mode (0o755/0o644) and `symlink_file`, `#[cfg(target_os = "linux")]`
  reserved for xattrs.
- **First-touch semantics**: `UndoInterceptor` captures a preimage only on the first mutating
  touch of a path within a step. The `touched_paths: HashSet<String>` guards against duplicates.
- **Rollback is pop**: Rolling back removes steps from history (not reversible). Two-pass algorithm:
  (1) delete created paths deepest-first, recreate dirs shallowest-first, restore files;
  (2) restore directory metadata deepest-first so child ops don't clobber parent mtime.
- **On-disk layout**:
  ```
  {undo_dir}/version            # "1"
  {undo_dir}/wal/in_progress/   # active step (promoted to steps/ on close)
  {undo_dir}/steps/{id}/        # completed steps
    manifest.json
    preimages/{hash}.dat          # zstd level 3 compressed file contents
    preimages/{hash}.meta.json    # PreimageMetadata (path, type, mode, mtime, etc.)
  ```
- **Undo barriers**: Barriers are placed "after" a specific completed step. A barrier with
  `after_step_id = S` blocks rollback of step S (because the external modification happened
  after S and rolling back S would destroy it). `rollback(count, force)` checks barriers;
  `force: true` crosses and removes them. Barriers persist in `{undo_dir}/barriers.json`.
- **Safeguards**: Configurable thresholds (delete count, overwrite-large-file, rename-over-existing)
  checked in `pre_*` methods. On trigger, calls `SafeguardHandler::on_safeguard_triggered()` which
  blocks until Allow/Deny. On Deny, `rollback_current_step()` undoes all operations in the current
  step and cancels it. Once a safeguard kind is allowed for a step, it does not re-trigger.
- **Resource limits**: `ResourceLimitsConfig` controls max log size, max step count, and max
  single-step preimage data size. On `close_step`, FIFO eviction removes oldest steps to stay
  within budget. Steps exceeding `max_single_step_size_bytes` are marked `unprotected` — they
  cannot be rolled back but do not block rollback of subsequent steps. Version mismatch
  (`version` file ≠ `CURRENT_VERSION`) disables undo; `discard()` re-enables it.
- **Test pattern**: snapshot → open step → apply operations via OperationApplier → close step →
  rollback → `assert_tree_eq(before, after, opts)` with large mtime tolerance.
- **Gitignore filtering**: Opt-in via `UndoInterceptor::with_gitignore()`. When enabled, the
  `ignore` crate loads `.gitignore` files and `.git/info/exclude` once at construction time.
  Paths matching ignore rules are silently skipped in `ensure_preimage`, `record_creation`,
  and `capture_tree_preimages` — no preimage, no manifest entry.
- **Control channel protocol**: JSON Lines over virtio-serial. Host→VM messages: `exec`,
  `cancel`, `rollback_notify`. VM→host messages: `step_started`, `output`, `step_completed`.
  Messages are serde-tagged (`#[serde(tag = "type")]`). Max message size: 1 MB (rejected before
  parsing). The `ControlChannelState` validates sequences and emits `ControlEvent`s;
  protocol violations produce `ProtocolError` events without breaking the channel.
- **Dependencies** (all permissively licensed): blake3, filetime, ignore, serde (+derive),
  serde_json, tempfile, thiserror, xattr (Linux only), zstd, chrono (+serde).

### Implementation Status
The project follows a TDD sequence defined in `testing-plan.md` §5. Steps 1–8 are complete:

- **TDD Step 1 (Test Oracle Infrastructure)** — complete
  - `codeagent-common`: StepId, StepType, StepInfo, CodeAgentError, Result (4 unit tests)
  - `codeagent-test-support`: TreeSnapshot with blake3 hashing, assert_tree_eq with configurable
    mtime tolerance and exclude patterns, TempWorkspace, fixture builders (18 unit tests)

- **TDD Step 2 (UndoInterceptor Core)** — complete
  - WriteInterceptor trait (13 methods matching project-plan §4.3.3)
  - StepTracker, preimage capture (zstd + JSON metadata), StepManifest, two-pass rollback
  - UndoInterceptor wiring it all together (19 unit tests)
  - Integration tests UI-01..UI-08 covering: write 3x, create+write, nested dirs, delete file,
    delete tree, rename (dest absent), rename (dest exists), rename dir with children (8 tests)

- **TDD Step 3 (WAL + Crash Recovery)** — complete
  - `RecoveryInfo` struct reports paths restored/deleted and manifest validity
  - `UndoInterceptor::recover()` — always-rollback-incomplete policy per project-plan §4.8:
    detects `wal/in_progress/`, handles empty WAL, valid manifest, and missing/corrupt manifest
    (falls back to reconstructing manifest from `preimages/*.meta.json` files)
  - `rebuild_manifest_from_preimages()` — scans preimage metadata when manifest is unavailable
  - `UndoInterceptor::new()` now reconstructs completed steps from on-disk `steps/` directory
  - `StepTracker::add_completed_step()` — supports disk-based state reconstruction
  - Shared test helpers extracted to `tests/common/mod.rs` (OperationApplier, compare_opts)
  - Integration tests CR-01..CR-07 + step reconstruction test (8 tests)

- **TDD Step 4 (Undo Barriers)** — complete
  - `BarrierId`, `BarrierInfo`, `ExternalModificationPolicy`, `RollbackResult` types in common crate
  - `RollbackBlocked` error variant for barrier-blocked rollback
  - `BarrierTracker` module with in-memory state + JSON persistence (`barriers.json`)
  - `UndoInterceptor::notify_external_modification()` — creates barriers under `Barrier` policy
  - `UndoInterceptor::rollback(count, force)` — checks barriers, blocks or force-crosses
  - `UndoInterceptor::barriers()` — query all barriers
  - `UndoInterceptor::with_policy()` — constructor with configurable policy
  - Integration tests EB-01..EB-06, EB-08 covering: barrier creation, rollback blocking,
    force rollback, barrier querying, internal writes no-barrier, multiple barriers,
    warn policy (7 tests)

- **TDD Step 5 (Safeguards — Interceptor Level)** — complete
  - `SafeguardId`, `SafeguardKind`, `SafeguardConfig`, `SafeguardEvent`, `SafeguardDecision` types in common crate
  - `SafeguardDenied` error variant in `CodeAgentError`
  - `SafeguardHandler` trait — synchronous blocking callback for safeguard decisions
  - `SafeguardTracker` — per-step counters (delete count, overwrite, rename-over), threshold checks,
    allowed-kind tracking to prevent re-triggering after Allow
  - `StepTracker::cancel_step()` — clears active step without adding to completed list
  - `UndoInterceptor::with_safeguard()` — constructor with configurable safeguard config + handler
  - `UndoInterceptor::rollback_current_step()` — mid-step rollback on deny (writes manifest,
    rolls back WAL, cancels step)
  - Safeguard checks in `pre_unlink` (delete threshold), `pre_write`/`pre_open_trunc` (overwrite
    large file), `pre_rename` (rename-over-existing)
  - Integration tests SG-01..SG-06 + 5 edge cases (11 tests)

- **TDD Step 6 (Metadata Capture)** — complete
  - `xattr` crate added as Linux-only dependency for reading/writing extended attributes
  - `read_xattrs()` implemented in `preimage.rs` and `snapshot.rs` (Linux: real xattr reads;
    other platforms: empty map)
  - `restore_metadata()` in `rollback.rs` now restores xattrs on Linux (removes stale, sets stored)
  - OperationApplier extended with: `open_trunc`, `setattr_truncate`, `chmod` (Unix),
    `fallocate`, `copy_file_range`, `set_xattr` (Linux), `remove_xattr` (Linux)
  - Integration tests UI-09..UI-15 covering: truncate-open, truncate-setattr, chmod (Unix),
    xattr-set (Linux), xattr-remove (Linux), fallocate, copy-file-range (7 tests; 4 on Windows)

- **TDD Step 7 (Resource Limits + Pruning)** — complete
  - `ResourceLimitsConfig` in common crate: `max_log_size_bytes`, `max_step_count`,
    `max_single_step_size_bytes` (all `Option`, default `None`)
  - `StepUnprotected` and `UndoDisabled` error variants in `CodeAgentError`
  - `StepManifest::unprotected` field marks steps that exceeded single-step size limit
  - Atomic preimage writes (temp-file-then-rename) for `.meta.json` and `.dat` files
  - `capture_preimage` returns `(PreimageMetadata, u64)` — includes compressed data size
  - `resource_limits` module: `calculate_step_size`, `calculate_total_log_size`, `evict_if_needed`
    (FIFO eviction by step count and log size)
  - `UndoInterceptor::with_resource_limits()` — constructor with limits config
  - `close_step` returns `Result<Vec<StepId>>` — list of evicted step IDs
  - Unprotected step tracking: skips preimage capture after threshold exceeded, blocks rollback
  - Version mismatch detection on construction, `is_undo_disabled()`, `discard()` to re-enable
  - Integration tests UI-16..UI-19, UL-01..UL-08 (12 tests)

- **TDD Step 8 (Control Channel Parsing + State Machine)** — complete
  - `codeagent-control` crate: control channel protocol types, JSONL parsing, state machine
  - `ControlChannelError` — MalformedJson, UnknownMessageType, OversizedMessage,
    UnexpectedStepCompleted, DuplicateStepStarted, OutputForUnknownCommand,
    UnexpectedStepStarted, CancelUnknownCommand
  - `HostMessage` enum (Exec, Cancel, RollbackNotify) — serde-tagged, per project-plan §4.2
  - `VmMessage` enum (StepStarted, Output, StepCompleted) — serde-tagged
  - `parse_vm_message` / `parse_host_message` — JSONL parsing with 1MB size limit (rejects
    before deserialization), distinguishes malformed JSON from unknown message types
  - `ControlChannelState` — tracks pending (exec sent) and active (step_started received)
    commands, validates sequences, emits `ControlEvent`s for the caller
  - `cancel_command` — handles cancellation of pending or active commands
  - Protocol error resilience: violations produce `ControlEvent::ProtocolError`, channel continues
  - Integration tests CC-01..CC-07 + edge cases (18 tests), unit tests (28 tests)

- **TDD Steps 9–18** — not yet started (control channel integration, STDIO API, MCP, etc.)

### Build & Test Commands
```sh
cargo check --workspace          # type-check
cargo test --workspace           # run all tests (152 on Windows, 155 on Linux)
cargo clippy --workspace --tests # lint (must be warning-free)
cargo test -p codeagent-interceptor --test undo_interceptor    # UI integration tests only
cargo test -p codeagent-interceptor --test wal_crash_recovery  # CR integration tests only
cargo test -p codeagent-interceptor --test undo_barriers       # EB barrier tests only
cargo test -p codeagent-interceptor --test safeguards          # SG safeguard tests only
cargo test -p codeagent-interceptor --test resource_limits     # UL/UI resource limit tests only
cargo test -p codeagent-interceptor --test gitignore           # GI gitignore filter tests only
cargo test -p codeagent-control --test control_channel         # CC control channel tests only
```