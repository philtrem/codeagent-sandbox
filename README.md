# Code Agent Sandbox

A sandboxed execution environment for AI coding agents. Commands run inside a Linux VM (QEMU), with host-side filesystem interception that captures preimages of every write. This gives you N-step undo for any destructive operation — including bulk operations like `rm -rf *`, which count as a single step.

When no VM is available, the sandbox runs in **host-only mode** — filesystem tools (read, write, edit, glob, grep, undo) work directly on the host, and commands execute directly on the host via a shell (without VM isolation).

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) 1.85+ (edition 2024)
- [QEMU](https://www.qemu.org/download/) (runtime — needed to run the VM; optional for host-only mode)
- [Docker](https://docs.docker.com/get-docker/) (build-time only — needed to build the guest VM image; on Windows, keep the default "Use WSL 2" option during install — Hyper-V can conflict with QEMU's WHPX accelerator)
- [Node.js](https://nodejs.org/) (only if building the desktop app)

## CLI usage

Build the workspace, then build the guest VM image (requires Docker):

```sh
cargo build --workspace
cargo xtask build-guest                    # host architecture
cargo xtask build-guest --arch aarch64     # cross-build for aarch64
```

The guest image build produces `vmlinuz` + `initrd.img` (Alpine linux-virt kernel, busybox, statically-linked shim binary). You can skip it if you already have pre-built kernel and initrd files.

Run the sandbox:

```sh
sandbox --working-dir /path/to/project --undo-dir /tmp/undo
```

This starts in STDIO mode (JSON Lines on stdin/stdout). For MCP mode (used by Claude Code and other MCP clients):

```sh
sandbox --working-dir /path/to/project --undo-dir /tmp/undo --protocol mcp
```

In MCP mode the sandbox speaks JSON-RPC 2.0 over stdin/stdout and exposes 12 tools: `Bash`, `read_file`, `write_file`, `edit_file`, `list_directory`, `glob`, `grep`, `undo`, `get_undo_history`, `get_session_status`, `get_working_directory`, `discard_undo_history`.

Additional options: `--memory-mb` (default 2048), `--cpus` (default 2), `--qemu-binary`, `--kernel-path`, `--initrd-path`, `--virtiofsd-binary`. See `sandbox --help`.

If `--kernel-path` and `--initrd-path` are omitted and no guest images are found, the sandbox starts in host-only mode automatically.

## Desktop app

The Tauri v2 desktop app (`desktop/`) provides a GUI for managing the sandbox. It handles configuration, VM lifecycle, undo history, and Claude Code integration (MCP server registration, built-in tool blocking).

```sh
cd desktop && npm install
npm run build-sidecar   # build sandbox + guest images, copy to src-tauri/
npm run tauri dev       # development
npm run tauri build     # production installer
```

`npm run build-sidecar` builds the sandbox binary in release mode, builds the guest VM images for the host architecture (x86_64 or aarch64), and copies everything into the Tauri bundle directories. Requires Docker for the guest image build; if Docker is unavailable, the guest images are skipped with a warning. Run it before `npm run tauri dev` or `npm run tauri build`.

The desktop app and the sandbox binary are **separate executables**. The app spawns `sandbox.exe` as a child process and communicates via JSON-RPC over stdin/stdout. Rebuilding one does not rebuild the other.

### Claude Code integration

When started from the desktop app, the sandbox automatically registers itself as an MCP server in Claude Code's configuration and blocks Claude Code's built-in filesystem and execution tools (Read, Edit, Write, Glob, Grep, Bash) so all operations go through the sandbox. Tools like NotebookEdit remain available natively. On stop or app exit, the registration is removed and built-in tools are restored.

## Testing

```sh
cargo test --workspace              # ~766 tests (Windows; 24 E2E/shim/FB ignored)
cargo clippy --workspace --tests    # lint
```

E2E tests require QEMU/KVM and are ignored by default:

```sh
cargo test -p codeagent-e2e-tests --ignored
```

Fuzz targets (5 targets, requires nightly + cargo-fuzz):

```sh
cd fuzz && cargo fuzz run control_jsonl -- -max_total_time=30
```

Benchmarks (criterion):

```sh
cargo bench --workspace
```

## How it works

The host runs a Rust binary (`sandbox`) that serves the host working directory into a Linux VM through a transparent filesystem bridge. Two separate channels connect host and VM:

- **Filesystem channel** — carries POSIX filesystem operations. The VM kernel mounts a host-backed filesystem (`virtiofs` on Linux/macOS, `9P` on Windows). A write interceptor on the host side captures preimages before mutations land on disk.
- **Control channel** — carries command orchestration over virtio-serial (JSON Lines). A lightweight shim inside the VM receives commands, runs them via a shell, streams output, and signals step boundaries.

External interfaces speak either the **STDIO API** (JSON Lines over stdin/stdout) or the **MCP protocol** (JSON-RPC 2.0 over stdin/stdout), both of which expose the same capabilities: session lifecycle, command execution, filesystem access, undo, and safeguards.

### Platform-specific filesystem backends

| Host | VM arch | Accelerator | Filesystem backend | Guest mount |
|---|---|---|---|---|
| Linux x86_64 | x86_64 | KVM | Forked virtiofsd (Rust, vhost-user) | `mount -t virtiofs` |
| macOS Apple Silicon | aarch64 | HVF | Forked virtiofsd (ported to macOS) | `mount -t virtiofs` |
| Windows x86_64 | x86_64 | WHPX | Custom 9P2000.L server (Rust) | `mount -t 9p` |

Both backends call into the same `WriteInterceptor` trait. The undo log, safeguards, step tracking, and API behavior are identical across platforms.

Windows uses 9P because the vhost-user transport requires `SCM_RIGHTS`, which Windows AF_UNIX sockets do not support. Linux and macOS use virtiofs for better performance on metadata-heavy workloads.

## Project structure

```
crates/
  common/             Shared types and error definitions
  interceptor/        Undo log core (preimage capture, rollback, barriers, safeguards,
                      resource limits, gitignore filtering, symlink policy)
  control/            Control channel protocol + state machine + handler
  stdio/              STDIO API server (JSON Lines)
  mcp/                MCP server (JSON-RPC 2.0, 12 tools)
  sandbox/            Host-side binary wiring everything together
  shim/               VM-side command executor
  p9/                 9P2000.L server (Windows filesystem backend)
  p9proxy/            Guest-side 9P proxy for virtio-serial (Windows)
  virtiofs-backend/   Intercepted virtiofs filesystem backend (Linux/macOS)
  virtiofsd-fork/     Forked virtiofsd with macOS compatibility layer
  vmm-sys-util-fork/  Forked vmm-sys-util with macOS support
  test-support/       Test utilities (snapshots, temp workspaces, fixtures)
  e2e-tests/          QEMU-based end-to-end tests
guest/                Guest VM image build (Dockerfile + init script)
fuzz/                 Fuzz targets for all parsers
xtask/                Build task runner (guest image builds)
desktop/              Tauri v2 desktop app (React + TypeScript + Zustand)
```

## Key design decisions

- **Preimage-based undo.** On the first mutating touch of each file path within a step, the full file contents and metadata are captured (zstd-compressed). Rollback restores these preimages. No deltas, no diffing — works uniformly for text and binary files.
- **No guest-side caching.** Both virtiofs and 9P mount with `cache=none`. External modifications (IDE edits, `git pull`) are immediately visible inside the VM. The host is the single source of truth.
- **Safeguards.** Configurable thresholds for destructive operations (delete count, overwrite large files, rename over existing). Triggers block until explicitly allowed or denied. On deny, the current step is rolled back automatically.
- **Undo barriers.** External modifications between steps create barriers that prevent rolling back past the modification point (since the rollback would destroy the external change). `force` flag overrides.
- **Two-channel separation.** The filesystem channel and control channel are completely independent. The control channel never sees filesystem operations. Correlation happens on the host: all filesystem writes between `step_started(N)` and `step_completed(N)` belong to undo step N.
- **Host-only fallback.** When QEMU or guest images are unavailable, the sandbox operates without a VM. Filesystem tools work directly on the host with full undo support. Commands execute directly on the host via a shell (without VM isolation).

## License

MIT OR Apache-2.0
