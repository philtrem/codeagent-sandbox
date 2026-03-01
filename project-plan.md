# Sandboxed Coding Agent — Project Plan

## 1. Vision

A sandboxed execution environment for an AI-driven coding agent. The agent operates inside a Linux VM, reading and writing to a host working folder through a transparent bridge layer. All write operations are intercepted and logged, enabling N-step undo of any destructive operation — including bulk operations like `rm -rf *`, which count as a single step.

---

## 2. Architecture Overview

The system has three layers: the **External Interfaces** (frontend + LLM), the **Host-Side Agent**, and the **Virtual Machine**. The host-side agent is the single brain of the system — a Rust binary that serves the working folder to the VM via one of two filesystem backends, orchestrates command execution via a control channel, manages the undo log, and exposes STDIO-based APIs for frontends and LLMs.

### 2.1 Dual-Backend Filesystem Bridge

The filesystem bridge between host and VM uses different technologies depending on the host platform, selected automatically at startup:

| Host OS | Guest Arch | Machine Type | Accel | Filesystem Backend | Guest Mount | Why |
|---|---|---|---|---|---|---|
| **Linux** (x86_64) | x86_64 | `q35` | KVM | Forked `virtiofsd` (Rust, vhost-user) | `mount -t virtiofs` | Near-native performance, full POSIX incl. mmap. No cross-platform translation needed. |
| **macOS** (Apple Silicon) | aarch64 | `virt` | HVF | Forked `virtiofsd` (Rust, vhost-user, ported) | `mount -t virtiofs` | Same virtiofsd fork ported to macOS. HVF accelerates aarch64 guests natively on M-series. SCM_RIGHTS works on macOS — vhost-user transport is fully functional. |
| **Windows** (x86_64) | x86_64 | `q35` | WHPX | Custom 9P2000.L server (Rust) | `mount -t 9p` | Windows lacks SCM_RIGHTS — vhost-user transport cannot work. QEMU's built-in virtio-9p device avoids this entirely. 9P server handles Windows-specific normalization (permissions, case, reserved names). |

Both backends call into the same **write interceptor** — a shared Rust trait that handles undo logging, safeguard checks, and step tracking. The undo log format, safeguard logic, STDIO API, MCP server, and control channel are identical regardless of backend.

**Why not virtiofs everywhere?** The vhost-user protocol (used by virtiofs to connect QEMU to an external virtiofsd daemon) requires SCM_RIGHTS to pass file descriptors over Unix sockets. Windows has AF_UNIX sockets since Windows 10 1803, but does not implement SCM_RIGHTS. No upstream or third-party Windows vhost-user transport exists. On macOS, SCM_RIGHTS is fully supported — the only porting effort is virtiofsd's Linux-specific fd management layer (see Appendix C §C.8).

**Why not 9P everywhere?** On Linux and macOS, virtiofs with vhost-user transport delivers significantly better performance than 9P — particularly for metadata-heavy workloads (`npm install`, `git status`), `mmap`-dependent tools, and large file I/O.

### 2.2 Two Channels, One Brain

The host-side agent communicates with the VM over **two dedicated channels**:
- **Filesystem channel** — carries the filesystem protocol. On Linux and macOS hosts: a vhost-user socket carrying virtio-fs. On Windows hosts: QEMU's built-in virtio-9p device carrying 9P2000.L over a dedicated fd pair.
- **Control channel** — a virtio-serial device carrying JSON Lines messages. A lightweight **VM-side shim** receives commands, executes them, streams output, and signals step boundaries. This is the only custom software in the VM.

```
  ┌─────────────────────┐     ┌─────────────────────┐
  │   External Frontend  │     │    LLM (via MCP)     │
  │  (GUI / IDE / CLI)   │     │                      │
  └─────────┬───────────┘     └──────────┬───────────┘
            │                            │
      STDIO API                   MCP (JSON-RPC
     (JSON Lines)                 over local socket)
            │                            │
            └──────────┬─────────────────┘
                       │
┌──────────────────────┼───────────────────────────────┐
│                      │              Host              │
│             ┌────────┴────────┐                       │
│             │  Host-Side Agent │                       │
│             │  (Rust binary,   │                       │
│             │   per-platform)  │                       │
│             │                  │                       │
│             │  Filesystem Backend                     │
│             │  ┌────────────────────────────────┐     │
│             │  │ Linux/macOS: forked virtiofsd  │     │
│             │  │ Windows: custom 9P server      │     │
│             │  │                                │     │
│             │  │ Both call into:                │     │
│             │  │ ┌────────────────────────────┐ │     │
│             │  │ │ WriteInterceptor (shared)  │ │     │
│             │  │ │ • undo log                 │ │     │
│             │  │ │ • safeguards               │ │     │
│             │  │ │ • step tracking            │ │     │
│             │  │ └────────────────────────────┘ │     │
│             │  └────────────────────────────────┘     │
│             │                  │                       │
│             │  • Control chan. │                       │
│             │  • STDIO API     │                       │
│             │  • MCP server    │                       │
│             └──┬─────────┬───┘                       │
│                │         │                            │
│         ┌──────┴──┐ ┌────┴─────┐                     │
│         │ Working  │ │ Undo Log │                     │
│         │ Folder   │ │ (N steps)│                     │
│         └────┬────┘ └──────────┘                     │
└──────────────┼───────────────────────────────────────┘
               │
      ┌────────┴────────┐
      │                 │
  Filesystem        Control
  (virtio-fs or    (virtio-serial)
   9P2000.L)
      │                 │
┌─────┼─────────────────┼─────────────────────────────┐
│     │    Virtual Machine (Linux, always)              │
│     │                 │                               │
│  virtio-fs or    VM-Side Shim                        │
│  v9fs driver     (lightweight)                       │
│     │                 │                               │
│  ┌──┴──────────────┐  │                               │
│  │ /mnt/working    │  ├─ receives commands            │
│  │ (POSIX dir)     │  ├─ runs shell, captures output  │
│  └──────┬──────────┘  ├─ signals step start/end       │
│         │             └─ notifies rollback             │
│    ┌────┴────┐  ┌──────────────┐                     │
│    │Terminal │◄►│ Coding Agent  │                     │
│    └────────┘  └──────────────┘                     │
│                                                      │
│  ┌──────────────────────────────────────────┐       │
│  │ Network (configurable policy)             │       │
│  │ open / IP/CIDR allowlist / disabled       │       │
│  └──────────────────────────────────────────┘       │
└─────────────────────────────────────────────────────┘
```

---

## 3. Core Design Principles

### 3.1 Dual-Backend Filesystem Bridge

The VM accesses the host working folder through a filesystem bridge that is transparent to everything running inside the VM. The bridge uses the best available technology for each host platform:

**Linux and macOS hosts** use a **forked `virtiofsd`** — a modified version of the official Rust virtiofsd daemon (Apache 2.0 licensed, from `gitlab.com/virtio-fs/virtiofsd`). The fork adds write interception hooks to the existing `PassthroughFs` implementation. The guest mounts via the standard `virtio-fs` driver (mainline since kernel 5.4). This delivers near-native filesystem performance, full POSIX semantics including `mmap`, and requires no cross-platform translation on Linux. On macOS, virtiofsd's Linux-specific fd management layer (`/proc/self/fd`, `O_PATH`, `renameat2`) is ported to macOS equivalents (`fcntl(F_GETPATH)`, `openat()+O_NOFOLLOW`, `renameatx_np()`). Since we're already forking virtiofsd for interception hooks, the macOS portability patches land in the same fork. The vhost-user transport works on macOS because macOS fully supports SCM_RIGHTS on AF_UNIX sockets.

**Windows hosts** use a **custom 9P2000.L server** — a Rust implementation of the 9P protocol (see Appendix A). The guest mounts via the standard `v9fs` kernel module. The 9P server handles Windows-specific normalization: synthesizing POSIX metadata, detecting case collisions on case-insensitive filesystems, and translating symlinks and reserved filenames. Windows requires 9P because the vhost-user protocol depends on SCM_RIGHTS for file descriptor passing, which Windows AF_UNIX sockets do not support. QEMU's built-in virtio-9p device avoids vhost-user entirely — it uses an internal fd pair between QEMU and the agent, with no SCM_RIGHTS dependency.

**Why not virtiofs on Windows?** The vhost-user protocol requires SCM_RIGHTS to pass shared memory file descriptors from QEMU to the virtiofsd daemon. Windows has had AF_UNIX sockets since Windows 10 1803, but has never implemented SCM_RIGHTS ancillary data. No upstream QEMU patches or third-party implementations exist for a Windows vhost-user transport. The potential alternatives (named shared memory sections via `CreateFileMapping`, handle duplication via `DuplicateHandle`) would require patching QEMU's C codebase — a fundamentally different category of effort than forking the small, focused, Rust virtiofsd daemon.

**Why not 9P everywhere?** On Linux and macOS, virtiofs with vhost-user transport delivers significantly better performance than 9P — particularly for metadata-heavy workloads (`npm install`, `git status`), `mmap`-dependent tools, and large file I/O. Since Linux and macOS are the primary development hosts, optimizing these paths matters.

Both backends call into the same **write interceptor** trait (§4.3.3), so the undo log, safeguards, step tracking, STDIO API, and MCP server work identically regardless of which backend is active.

### 3.2 No Guest-Side Caching

The filesystem mount inside the VM uses no caching, regardless of backend:
- **virtio-fs:** `virtiofsd` launched with `--cache=never`, guest mounts with `-o cache=none`
- **9P:** guest mounts with `-o cache=none`

This guarantees that external modifications to the working folder (user editing in an IDE, `git pull`, etc.) are **immediately visible** inside the VM, eliminates cache coherency issues, and makes the host-side agent the single source of truth.

The trade-off is that every file operation round-trips to the host. Over a local vhost-user or fd transport, each round-trip is sub-millisecond. For typical project sizes this is acceptable. If performance becomes an issue, caching can be introduced at the agent level (where invalidation can be controlled) rather than the kernel level.

### 3.3 Primitive Write Operations Only
The host-side agent does **not** interpret or translate terminal commands. Commands run inside the VM normally. By the time a write reaches the agent (via virtio-fs or 9P), it has already been resolved by the guest kernel into primitive filesystem operations:
- `create(path, content)`
- `write(path, offset, data)`
- `open` with `O_TRUNC` (truncates before any write)
- `remove(path)`
- `rename(src, dst)`
- `setattr(path, attrs)` (chmod, chown, truncate, utimes)
- `setxattr` / `removexattr`
- `fallocate` (including hole-punch and size changes)
- `copy_file_range` (destination mutates)

This eliminates the need to understand shell semantics, glob expansion, pipes, or program-specific behavior.

### 3.4 N-Step Undo
Each terminal command constitutes one "step." On the **first mutating touch** of each file path within a step, the agent captures a full preimage (complete copy of the file's contents and metadata) before the write is applied to the host. Rolling back means restoring those preimages. The log retains the last N steps. This preimage-based approach is simpler and more reliable than delta compression, and handles all file types (text and binary) uniformly.

---

## 4. Components

### 4.1 Virtual Machine (Linux)

**Runtime:** Always Linux guest, architecture matching the host. On Apple Silicon Macs, the guest is aarch64; on x86_64 Linux and Windows hosts, the guest is x86_64. Each host uses its native hardware-accelerated virtualization — see the platform table below.

| Host Platform | Guest Arch | QEMU Binary | Machine Type | Accelerator | Direct Kernel Boot |
|---|---|---|---|---|---|
| Linux x86_64 | x86_64 | `qemu-system-x86_64` | `q35` | KVM | Yes (`-kernel vmlinuz -initrd initrd.img`) |
| macOS Apple Silicon | aarch64 | `qemu-system-aarch64` | `virt` | HVF | Yes (`-kernel Image -initrd initrd.img`) |
| Windows x86_64 | x86_64 | `qemu-system-x86_64` | `q35` | WHPX | Yes (`-kernel vmlinuz -initrd initrd.img`) |

All platforms use **direct kernel boot** — the kernel and initrd are passed directly to QEMU, bypassing BIOS/UEFI entirely. Combined with a minimal kernel config and stripped-down initrd, this achieves sub-second boot times on all platforms.

**Note on machine types:** `q35` (x86_64) and `virt` (aarch64) both provide PCI, which is required for `vhost-user-fs-pci` (virtio-fs device) and `virtio-serial-pci`. The `microvm` machine type was evaluated and rejected: it lacks PCI support (blocking virtio-fs), is KVM-only (incompatible with HVF/WHPX), and is x86_64-only. The boot time difference vs. q35 with direct kernel boot is ~100-300ms — negligible for our use case.

**Contains:**
- The coding agent (AI-driven)
- A terminal for command execution
- A guest OS and filesystem
- A filesystem mount of the host working folder (virtio-fs driver or v9fs kernel module, depending on host)
- The VM-side shim for command execution and step boundary signalling (§4.2)

**Responsibilities:**
- Execute all code, builds, tests, and terminal commands within the sandbox
- Mount the working folder (command varies by backend — see §4.3.2)
- All filesystem access to the working folder flows transparently through the mount

#### 4.1.1 Network Access Policy

The VM requires network access for common development operations (`npm install`, `pip install`, `git clone`, `apt-get`, API calls during tests, etc.). Network access is configurable per session with three modes:

| Mode | Behavior | Use case |
|---|---|---|
| `open` | Unrestricted internet access | General development, dependency installation |
| `allowlist` | Only connections to specified IP addresses or CIDR ranges are permitted | Restricted environments, corporate policy compliance |
| `disabled` | No network access | Fully offline operation, maximum isolation |

The default is `open`. The frontend configures the mode via `session.start`.

**Allowlist enforcement (MVP):** The `allowlist` mode is enforced as an **IP/CIDR allowlist** via guest-side firewall rules (iptables/nftables). Domain-based allowlisting was considered but deferred — DNS results change over time, clients can connect by IP to bypass domain logic, TLS SNI parsing is incomplete for non-TLS protocols, and package managers use a mix of HTTPS, git+ssh, etc. IP/CIDR filtering is honest, enforceable, and simple. Domain-based filtering may be added post-MVP via a transparent proxy if needed.

**Security implication:** If the agent has network access, the sandbox protects the host from both the agent's own actions *and* anything it might download or be instructed to fetch. The VM's isolation boundary must account for network-borne threats (malicious packages, compromised dependencies, exfiltration attempts). This is primarily the VM runtime's responsibility (firewall rules, network namespacing).

#### 4.1.2 VM Lifecycle

The VM can operate in two modes, configurable per session:

| Mode | Behavior | Use case |
|---|---|---|
| `persistent` (default) | VM disk state survives between sessions. Installed packages, build caches, environment configuration are retained. On `session.stop`, the VM is **shut down** (not suspended); on the next `session.start`, it is **cold-booted** from the same disk image. With sub-second direct kernel boot, the startup cost is negligible. | Ongoing development — avoids reinstalling dependencies every session |
| `ephemeral` | VM is destroyed on `session.stop` and created fresh on `session.start`. No internal state carries over. | Reproducible environments, CI/CD, untrusted workloads |

In `persistent` mode, the VM's internal state (everything *except* the working folder, which lives on the host) is stored on the host alongside the undo log. This state is separate from the undo log — rolling back the working folder does not roll back the VM's internal state (installed packages, etc.).

**Note on suspend/resume:** Persistent mode does **not** involve suspending and resuming VM RAM state. "Stop and resume" with RAM state (QEMU's `savevm`/`loadvm` or `migrate` to file) introduces significant complexity (device state serialization, compatibility across QEMU versions, large snapshot files) with marginal benefit given sub-second boot times. The VM always cold-boots; only its persistent disk image is retained.

**Reset:** A `session.reset` operation is available to destroy a persistent VM and start fresh, without requiring a mode change.

#### 4.1.3 Known Filesystem Limitations

Limitations depend on which backend is active:

**virtio-fs backend (Linux and macOS hosts):**

| Operation | Behavior |
|---|---|
| Read/write, create/delete, rename, mmap | Near-native POSIX, fully transparent |
| inotify (VM-originated changes) | Works normally |
| inotify (host-originated changes) | Immediate visibility with `--cache=never` |
| Unix domain sockets in working dir | Supported |

Note: On macOS, the host filesystem is typically case-insensitive (HFS+/APFS default). The virtiofsd fork serves whatever the host returns — case collisions are the user's responsibility, consistent with native macOS development behavior.

**9P backend (Windows hosts):**

| Operation | Behavior |
|---|---|
| Read/write, create/delete, rename | Fully transparent |
| Compilers, interpreters, test runners, package managers, git | Fully transparent |
| inotify (VM-originated changes) | Works — kernel generates events before they hit v9fs |
| inotify (host-originated changes) | Not visible via inotify — handled by external modification detection (§4.7) |
| mmap | Not available with `cache=none` — falls back to read/write. **Validation required:** A number of dev tools use `mmap` on working-tree artifacts (`target/`, `node_modules/`, `git` internals). If `mmap` failure under `v9fs` with `cache=none` breaks representative builds/tests, options include: relaxing to `cache=loose` on Windows (accepting weaker instant host visibility), or running the working tree on a Linux filesystem inside the VM with host sync. Validate early with representative workloads (`cargo build`, `npm install`, `git status`). |
| Unix domain sockets in working directory | Not supported on 9P mounts — place elsewhere in VM's local filesystem |

### 4.2 VM-Side Shim (Control Channel)

A lightweight process that runs inside the VM, communicating with the host-side agent over a **virtio-serial** device (the **control channel**). This is the only custom software in the VM. It has no filesystem coupling — all filesystem access goes through the mount as usual.

**Language:** Any (Python, shell script, or small compiled binary). Simplicity is paramount — this should be a few hundred lines at most.

**Transport:** JSON Lines over virtio-serial. In the guest, the shim reads/writes a character device (e.g., `/dev/virtio-ports/control`). On the host, QEMU exposes a Unix socket that the host-side agent connects to.

**Responsibilities:**
- Receive command execution requests from the host
- Execute commands in a shell, capturing stdout, stderr, and exit code
- Stream terminal output back to the host in real time
- Signal **step boundaries** — `step_started` when a command begins, `step_completed` when it finishes — enabling the host-side agent to group filesystem writes into undo steps
- Receive rollback notifications so the coding agent can re-read the filesystem
- (Future) Receive agent prompts and relay agent output

**Protocol (host → VM):**

| Message | Fields | Purpose |
|---|---|---|
| `exec` | `id`, `command`, `env`, `cwd` | Execute a shell command |
| `cancel` | `id` | Cancel a running command (SIGTERM → SIGKILL) |
| `rollback_notify` | `step_id` | Inform the agent that a rollback occurred |

**Protocol (VM → host):**

| Message | Fields | Purpose |
|---|---|---|
| `step_started` | `id` | Command execution has begun — host opens a new undo step |
| `output` | `id`, `stream` (stdout/stderr), `data` | Terminal output chunk |
| `step_completed` | `id`, `exit_code` | Command finished — host closes the current undo step |

**Example exchange:**
```json
→ {"type":"exec","id":42,"command":"npm install","cwd":"/mnt/working"}
← {"type":"step_started","id":42}
← {"type":"output","id":42,"stream":"stdout","data":"added 150 packages in 3s\n"}
← {"type":"step_completed","id":42,"exit_code":0}
```

**Design notes:**
- The shim is intentionally minimal. It does not interpret commands, manage state, or touch the filesystem directly. It is a thin shell executor with structured I/O.
- The control channel is separate from the filesystem channel because they carry fundamentally different traffic: filesystem protocol vs. structured orchestration messages. Multiplexing would add complexity and fragility.
- The host-side agent correlates step boundaries with filesystem writes: all writes between `step_started(42)` and `step_completed(42)` belong to undo step 42.
- **Trust model:** The host-side agent does not trust the VM for step boundary authority. Step IDs are created by the host; a step becomes active when the agent processes `agent.execute` and the shim's `step_started` is treated as an acknowledgement, not an authority. A malicious process inside the VM could access `/dev/virtio-ports/control` unless it is locked down via guest file permissions.
- **Late-arriving writes:** Even with `cache=none`, writes may be delivered after the process exits (writeback flushes, close-time flushes). The agent enforces a short **quiescence window** (configurable, default 100ms) after receiving `step_completed` — it waits for in-flight filesystem operations to drain before finalizing the step. Alternatively, the agent tracks the count of in-flight filesystem requests and only closes the step when it reaches zero.
- **Ambient/background writes:** Background daemons inside the VM (language servers, file watchers) may write during a step and get attributed to it. FUSE provides per-request PID information, but 9P does not. This is a known limitation — document it and consider a future "transaction protocol" if it becomes painful.

### 4.3 Host-Side Agent

**Language:** Rust

**Runs on:** Windows, macOS, Linux (three platform builds)

**The host-side agent is the single central component of the system.** It implements:

1. **Filesystem backend** — the forked virtiofsd on Linux and macOS, or the custom 9P server on Windows. Selected automatically based on host platform. See §4.3.2.
2. **Write interceptor** — shared trait called by both backends for undo logging, safeguard checks, and step tracking. See §4.3.3.
3. **Control channel handler** — receives step boundary signals and terminal output from the VM-side shim, sends commands and notifications.
4. **STDIO API** (§4.5) — JSON Lines interface for frontends (on stdin/stdout).
5. **MCP server** (§4.6) — JSON-RPC interface for LLMs (on a separate local socket).
6. **Undo log manager** (§4.4) — preimage capture, compression, WAL, rollback execution.
7. **External modification detector** (§4.7) — OS-native file watching with undo-barrier semantics.
8. **Cross-platform translation layer** (§5) — normalizes host filesystem semantics to POSIX. Only active in the 9P backend.
9. **Structured logging** (§4.9) — `tracing`-based JSON Lines output on stderr for all subsystems.
10. **Multiple working directory management** (§4.10) — spawns and manages per-directory filesystem backend instances.

#### 4.3.1 Concurrency Model

The host-side agent uses **tokio** as its async runtime, with **`spawn_blocking`** for host filesystem I/O, and **Rayon** for CPU-bound bulk operations.

**Event loop (tokio):** A single `tokio::select!` loop multiplexes all I/O sources:
- Filesystem backend events — for virtiofsd: the vhost-user socket is handled internally by the fork's own event loop, with interception callbacks crossing into the agent's async context via channels. For 9P: the fd pair is read via `AsyncFd`.
- Control channel (via `AsyncFd` wrapping the virtio-serial Unix socket) — step boundary signals, terminal output from the shim
- stdin (via `tokio::io::stdin`) — STDIO API messages from the frontend
- File watcher events (via channel from OS-native watcher)

**9P request handling:** The kernel v9fs client pipelines 9P requests — it sends multiple T-messages without waiting for each R-message. The agent reads each message, spawns a tokio task to handle it, and immediately reads the next. Responses are tagged and can be sent out of order per the 9P protocol.

**Host filesystem I/O:** All `std::fs` operations are blocking. They are dispatched via `tokio::task::spawn_blocking`, which runs them on tokio's blocking thread pool. This prevents filesystem latency from stalling the event loop or other in-flight requests.

```rust
async fn handle_read(&self, fid: u32, offset: u64, count: u32) -> Result<Rread> {
    let handle = self.fid_table.get_handle(fid)?;
    tokio::task::spawn_blocking(move || {
        let mut buf = vec![0u8; count as usize];
        let n = handle.read_at(&mut buf, offset)?;
        buf.truncate(n);
        Ok(Rread { data: buf })
    }).await?
}
```

**Bulk operations (Rayon):** Directory scanning for case collision indexing on startup and large rollback operations (restoring many preimages) benefit from data-parallel work-stealing. These are dispatched from `spawn_blocking` into Rayon's thread pool.

| Layer | Technology | Handles |
|---|---|---|
| Event loop | tokio | Multiplexing filesystem backend, control channel, STDIO API, MCP, file watcher events |
| 9P request handling | tokio tasks | One task per in-flight 9P request, enabling pipelining |
| virtiofsd threading | virtiofsd's own thread pool | Request handling in the fork; interception calls cross into agent via channels |
| Host filesystem I/O | `spawn_blocking` | Blocking `std::fs` calls on tokio's thread pool |
| Bulk operations | Rayon (from `spawn_blocking`) | Directory scanning, large rollback operations |

#### 4.3.2 Filesystem Backend Selection

The host-side agent selects the filesystem backend at startup based on the host platform:

**Linux host — Forked virtiofsd:**
- The agent spawns the forked virtiofsd as either a child process or an in-process library.
- virtiofsd communicates with QEMU via a vhost-user socket.
- The guest mounts with: `mount -t virtiofs -o cache=none working /mnt/working`
- virtiofsd is launched with `--cache=never --shared-dir=/path/to/working --socket-path=/tmp/virtiofsd.sock`
- Write interception hooks in the fork call into the shared `WriteInterceptor` trait.
- See Appendix C for the fork implementation guide.

**macOS host (Apple Silicon) — Forked virtiofsd (ported):**
- Same forked virtiofsd as Linux, with a macOS portability layer for fd management (see Appendix C §C.8).
- virtiofsd communicates with QEMU via a vhost-user socket (SCM_RIGHTS is fully supported on macOS).
- QEMU runs `qemu-system-aarch64` with HVF acceleration, machine type `virt`.
- Guest is aarch64 Linux; mounts with: `mount -t virtiofs -o cache=none working /mnt/working`
- QEMU memory backend uses `memory-backend-shm` (POSIX `shm_open()`; merged in QEMU ~9.1) instead of Linux-only `memory-backend-memfd`.
- Write interception hooks are identical to Linux — same `WriteInterceptor` trait, same `InterceptedFs` wrapper.
- See Appendix A for the 9P implementation guide (not used on macOS, but available as fallback).

**Windows host — Custom 9P server:**
- The agent runs the 9P server in-process.
- QEMU uses its built-in `virtio-9p-pci` device with WHPX acceleration — no vhost-user, no SCM_RIGHTS dependency.
- QEMU plumbs a dedicated fd pair between the VM and the agent for the 9P transport.
- The guest mounts with: `mount -t 9p -o version=9p2000.L,trans=fd,cache=none host0 /mnt/working`
- The 9P server handles Windows-specific normalization (POSIX metadata synthesis, reserved filenames, symlink type mapping, case collision detection).
- See Appendix A for the 9P implementation guide.

#### 4.3.3 Write Interceptor (Shared Trait)

The write interceptor is the core abstraction that makes both backends share the same undo/safeguard logic. Both the forked virtiofsd and the custom 9P server call into it for every mutating filesystem operation.

```rust
/// Shared write interception logic. Called by both filesystem backends.
/// Implementations handle undo logging, safeguard checks, and step tracking.
///
/// **First-touch semantics:** On the first mutating touch of a path within a
/// step, the interceptor captures the full preimage (file contents + metadata).
/// Subsequent touches to the same path within the same step are no-ops for
/// capture purposes.
///
/// **Metadata scope:** Preimages capture file type (reg/dir/symlink),
/// permissions (mode), timestamps (mtime at minimum), and extended attributes
/// (xattrs, if present). This ensures rollbacks restore not just bytes but also
/// metadata like the executable bit.
trait WriteInterceptor: Send + Sync {
    /// Called before a file is written or truncated.
    /// Records pre-mutation state (contents + metadata) for undo on first touch.
    fn pre_write(&self, path: &Path) -> Result<()>;

    /// Called before a file or directory is deleted.
    /// Records pre-mutation state, increments delete counter, checks safeguard.
    fn pre_unlink(&self, path: &Path, is_dir: bool) -> Result<()>;

    /// Called before a rename. Records state of both source and destination.
    fn pre_rename(&self, from: &Path, to: &Path) -> Result<()>;

    /// Called after a file is created. Records that this path was created
    /// (undo = delete it).
    fn post_create(&self, path: &Path) -> Result<()>;

    /// Called after a directory is created.
    fn post_mkdir(&self, path: &Path) -> Result<()>;

    /// Called before attributes are changed (chmod, chown, truncate, utimes).
    /// Captures metadata preimage.
    fn pre_setattr(&self, path: &Path) -> Result<()>;

    /// Called before a hard link is created.
    fn pre_link(&self, target: &Path, link_path: &Path) -> Result<()>;

    /// Called after a symlink is created.
    fn post_symlink(&self, target: &Path, link_path: &Path) -> Result<()>;

    /// Called before extended attributes are set or removed.
    /// Captures the xattr preimage for the affected path.
    fn pre_xattr(&self, path: &Path) -> Result<()>;

    /// Called before an open with O_TRUNC, which truncates the file before
    /// any write occurs. Captures the full preimage.
    fn pre_open_trunc(&self, path: &Path) -> Result<()>;

    /// Called before fallocate (including hole-punch and size changes).
    fn pre_fallocate(&self, path: &Path) -> Result<()>;

    /// Called before copy_file_range (destination path mutates).
    fn pre_copy_file_range(&self, dst_path: &Path) -> Result<()>;

    /// Query whether the current step is active (between step_started and
    /// step_completed). Writes outside a step go to an "ambient" step.
    fn current_step(&self) -> Option<StepId>;
}
```

Both backends hold an `Arc<dyn WriteInterceptor>` and call the appropriate method before/after each mutating operation. The concrete implementation (`UndoInterceptor`) owns the undo log, safeguard state, and a reference to the step tracker (which receives signals from the control channel).

**How the forked virtiofsd calls it (in `PassthroughFs` methods):**
```rust
fn write(&self, ..., data: ...) -> io::Result<usize> {
    let path = self.fd_to_path(inode)?;
    self.interceptor.pre_write(&path)?;
    // ... original virtiofsd write logic ...
}

fn unlink(&self, ..., name: &CStr) -> io::Result<()> {
    let path = self.resolve_path(parent, name)?;
    self.interceptor.pre_unlink(&path, false)?;
    // ... original virtiofsd unlink logic ...
}
```

**How the 9P server calls it (in operation handlers):**
```rust
fn handle_write(&mut self, fid: u32, offset: u64, data: &[u8]) -> Result<u32> {
    let path = self.fid_table.get_path(fid)?;
    self.interceptor.pre_write(&path)?;
    // ... write to host filesystem ...
}

fn handle_unlinkat(&mut self, dirfid: u32, name: &str, flags: u32) -> Result<()> {
    let path = self.fid_table.get_path(dirfid)?.join(name);
    self.interceptor.pre_unlink(&path, flags & AT_REMOVEDIR != 0)?;
    // ... unlink on host filesystem ...
}
```

The interception patterns are structurally identical — only the method signatures differ to match each backend's conventions.

#### 4.3.4 Filesystem Server Hardening

Both the forked virtiofsd and the custom 9P server are directly exposed to an untrusted guest kernel client. A compromised or malicious guest can send arbitrary protocol messages. These servers must be treated as security-critical attack surfaces.

**Least-privilege execution:**
- **Linux:** Run virtiofsd under a dedicated unprivileged user with no capabilities beyond what's needed for the shared directory. The existing `--sandbox=chroot` mode provides additional confinement via mount namespaces.
- **macOS:** Run virtiofsd under a macOS sandbox profile (see §C.8.1) and as a dedicated unprivileged user. Since macOS lacks chroot-equivalent containment for virtiofsd, the sandbox profile is the primary defense after the VM boundary.
- **Windows:** Run the 9P server with minimal privileges. The server must defensively open all paths relative to a root directory handle, avoid following reparse points (junctions) unless intended, and validate that all final resolved paths are within the shared directory root. Reparse points on Windows can escape directory boundaries similarly to symlinks on Unix — this is security-critical and requires dedicated tests.

**Fuzzing:** Both the virtiofsd FUSE message parser and the 9P wire protocol parser should be fuzz-tested (e.g., via `cargo-fuzz` / `libFuzzer`) with randomized and malformed inputs. This is especially important for the 9P server, which is a from-scratch implementation.

**Input validation:** Both servers must validate all incoming protocol fields (path components, sizes, offsets, flags) before processing, and reject malformed requests with appropriate error codes rather than panicking or invoking undefined behavior.

### 4.4 Undo Log

**Location:** Host-side, adjacent to (but separate from) the working folder.

**Structure per step:**
- Step ID / sequence number (matches the `id` from the control channel's `step_started`/`step_completed`)
- Timestamp
- Command that produced this step (from the `exec` message)
- List of affected paths, each with:
  - `existed_before: bool`
  - If existed: full preimage (complete copy of file contents + metadata snapshot)
  - If not existed: marker indicating the file was created in this step (undo = delete)
- Metadata snapshot per path: file type, permissions (mode), mtime, xattrs (if present)
- Exit code of the command

**Behavior:**
- One step = one terminal command's worth of write operations (bounded by `step_started` and `step_completed` signals from the control channel)
- On the **first mutating touch** of a path within a step, the `WriteInterceptor` captures the full preimage before the write proceeds. Subsequent touches to the same path within the same step do not re-capture.
- Rolling back step N restores the preimage of each affected file (or deletes it if it was created in that step)
- Log retains the last N steps; oldest entries are pruned by simple FIFO deletion
- Writes use a WAL (Write-Ahead Logging) pattern: the preimage is persisted before the host filesystem is modified

#### 4.4.1 Preimage Capture

Instead of storing deltas (diffs) between file states, the log stores a **full copy of each affected file** at the moment before the step's first mutation to that path. This is a classic transactional undo pattern (preimage capture).

**Why preimages instead of deltas:**
- **Simpler:** No format decisions (text vs binary deltas, partial writes, rename ordering), no reconstruction logic, no dependency chains between steps.
- **More reliable:** Rollback is a direct file restore, not a reverse-apply that can fail if intermediate state is inconsistent. Crash recovery is trivial (see §4.8).
- **Backend-agnostic:** Works identically for text, binary, small, and large files.
- **Handles worst cases well:** Mass deletions (the primary destructive risk) require storing full content regardless of approach. Preimage capture handles this naturally.

**Trade-off:** Disk usage can be higher for "tiny edit to huge file" — a one-line change to a 10MB file stores 10MB. In practice, most files in coding workloads are small, and the storage efficiency tricks below mitigate the cost. Delta compression can be added as a future optimization once correctness is solid.

**Storage efficiency (low-complexity optimizations):**
- **Compression:** zstd on preimages before writing to the undo store. Especially effective for text files (source code, configs, JSON).
- **Reflinks/clones when available:** On supported filesystems, a reflink is a near-instant, zero-copy snapshot:
  - Linux: `ioctl(FICLONE)` on btrfs, XFS, etc.
  - macOS/APFS: `clonefile()`
  - Windows: limited support; fall back to copy + compression
- **Deduplication within a step:** If the same path is touched multiple times in a step, only one preimage is captured (first-touch semantics).

#### 4.4.2 Pruning

With preimage capture, there are no dependency chains between steps — each step is self-contained. Pruning is simple FIFO: delete the oldest step's directory and all its preimage files.

No checkpoints are needed for the MVP. Checkpoints matter when you want deep history with bounded storage *and* random access reconstruction via delta chains — not required for "undo last N steps" with self-contained preimages.

#### 4.4.3 Undo Log Resource Limits

A runaway or malicious process in the VM could generate enormous write traffic — a loop writing random data to new files, or a build system producing large binary artifacts. Each first-touch triggers preimage capture, and compression may be ineffective for binary or random content. Without limits, the undo log could consume all available host disk space.

**Configurable limits:**

| Limit | Default | Behavior when exceeded |
|---|---|---|
| `max_log_size` | 1 GB | Evict oldest steps (FIFO) until the log is within budget |
| `max_step_count` | 100 | Evict oldest steps using FIFO order |
| `max_single_step_size` | 200 MB | If a single step's preimage captures exceed this threshold, stop capturing for the remainder of the step and mark it as "unprotected" in the log — the step cannot be individually rolled back, but subsequent steps can still be |

When the log exceeds `max_log_size` or `max_step_count`, the agent evicts the oldest steps. Since each step is self-contained (no delta chains), eviction is a simple directory deletion with no dependency concerns.

The agent emits an `event.warning` on the STDIO API when eviction occurs, so the frontend can inform the user that old undo history has been discarded.

**Configuration via STDIO API:**
```json
→ {"type":"undo.configure","request_id":"5","payload":{
     "max_log_size_bytes": 1073741824,
     "max_step_count": 100,
     "max_single_step_size_bytes": 209715200
   }}
← {"type":"response","request_id":"5","status":"ok"}
```

#### 4.4.4 Undo Log Versioning

The undo log format (preimage storage, compression, WAL structure) may change between agent versions. Rather than implementing migration logic for every format change, the agent uses an explicit discard-on-upgrade policy:

1. The undo log directory contains a `version` file with the current format version (integer, starting at 1).
2. On startup, the agent reads the version file and compares it to its expected version.
3. If the versions do not match, the agent **does not** automatically discard the log. Instead, it emits an `event.undo_version_mismatch` event on the STDIO API, indicating that the existing undo history is incompatible and requesting user confirmation to discard it.
4. The frontend presents this to the user. Until the user confirms, undo operations are unavailable but the session can otherwise proceed normally.
5. On confirmation, the agent deletes the old undo log directory and initializes a fresh one with the current version.

This ensures that users are never surprised by silent loss of undo history after an upgrade.

### 4.5 STDIO API (Host Integration Interface)

The host-side agent exposes a **STDIO-based API** over stdin/stdout, making it a headless backend process that any frontend can drive. A GUI application, an IDE plugin, a CLI wrapper, or a web-based interface can spawn the host-side agent as a child process and communicate with it through this API.

**Design:**
- **Transport:** JSON messages over stdin/stdout, one message per line (JSON Lines / NDJSON format). Simple to parse from any language, easy to debug by reading the stream.
- **Stderr:** Reserved for diagnostic logs. Never carries protocol messages.
- **Message structure:** Each message has a `type` field identifying the operation, a `request_id` for correlating responses, and a `payload` containing operation-specific data.

**Operations the frontend can invoke:**

| Category | Operation | Description |
|---|---|---|
| Session | `session.start` | Start a new sandbox session, specifying one or more working directory paths, network policy, VM lifecycle mode, and other configuration |
| Session | `session.stop` | Stop the VM (persistent mode) or destroy it (ephemeral mode) and clean up |
| Session | `session.reset` | Destroy a persistent VM and start fresh |
| Session | `session.status` | Query current session state (running, idle, error), active filesystem backend |
| Undo | `undo.rollback` | Undo the most recent N steps (blocked by undo barriers unless `force: true`) |
| Undo | `undo.history` | List recent steps with metadata (timestamp, affected paths, operation summary, undo barriers) |
| Undo | `undo.configure` | Configure undo log resource limits (max log size, max step count, max single step size) |
| Undo | `undo.discard` | Confirm discarding an incompatible undo log after a version mismatch |
| Agent | `agent.execute` | Send a command to the terminal inside the VM (relayed to the VM-side shim via the control channel) |
| Agent | `agent.prompt` | Send a prompt to the coding agent |
| FS | `fs.list` | List directory contents in the working folder |
| FS | `fs.read` | Read a file's contents |
| FS | `fs.status` | Get filesystem translation warnings (case collisions, symlink issues, etc.) |
| Safeguard | `safeguard.configure` | Configure destructive operation thresholds (e.g., max delete count before confirmation is required) |
| Safeguard | `safeguard.confirm` | Confirm or reject a paused destructive operation |

**Events the agent emits (unsolicited):**

| Event | Description |
|---|---|
| `event.step_completed` | A terminal command finished; includes step ID, affected paths, and exit code |
| `event.agent_output` | Coding agent produced output (text, code, etc.) |
| `event.terminal_output` | Terminal stdout/stderr from the running command (relayed from VM-side shim) |
| `event.warning` | Filesystem translation warning (case collision, permission degradation, undo log eviction, etc.) |
| `event.error` | Unrecoverable error in the agent or VM |
| `event.safeguard_triggered` | A destructive operation hit the configured threshold; execution is paused pending confirmation |
| `event.external_modification` | Files in the working folder were changed by something other than the sandbox; an undo barrier has been created (if barrier policy is active) |
| `event.recovery` | Crash recovery was performed on startup; indicates the incomplete step was rolled back and how many paths were restored |
| `event.undo_version_mismatch` | On startup, the existing undo log was created by a different agent version; user confirmation required to discard it |

**Example exchange:**
```json
→ {"type":"agent.execute","request_id":"1","payload":{"command":"npm install"}}
← {"type":"event.terminal_output","payload":{"stream":"stdout","data":"added 150 packages..."}}
← {"type":"event.step_completed","payload":{"step_id":7,"affected_paths":["package-lock.json","node_modules/..."],"exit_code":0}}
← {"type":"response","request_id":"1","status":"ok"}
```

**Under the hood**, `agent.execute` is relayed from the STDIO API → host-side agent → control channel → VM-side shim. Terminal output flows back in reverse. Step boundary signals from the shim are correlated with filesystem writes (intercepted by the `WriteInterceptor`) to produce undo steps, and the `event.step_completed` event is emitted to the frontend.

#### 4.5.1 Destructive Operation Safeguard

The STDIO API supports a subscription-based safeguard that **pauses execution** when a step's write operations contain a number of deletions meeting or exceeding a configurable threshold. This prevents an errant `rm -rf` or aggressive cleanup script from wiping files without the frontend (and ultimately the user) having a chance to intervene.

**How it works:**

1. The frontend configures a threshold via `safeguard.configure`, specifying the maximum number of delete operations allowed in a single step before confirmation is required.
2. During a step, the `WriteInterceptor` counts delete operations as they arrive from the VM. When the count meets or exceeds the threshold, the interceptor:
   - **Holds responses** — the filesystem backend stops sending replies for incoming write/delete requests from the VM. Both 9P and FUSE/vhost-user are request-response protocols: the guest kernel sends a request and blocks the calling process until it receives a reply. By holding the reply, the guest process is transparently frozen mid-syscall — no errors, no partial state, no corruption. Requests are queued internally.
   - **Emits** an `event.safeguard_triggered` event containing the step ID, the current delete count, and a summary of the paths targeted so far.
3. The frontend receives the event and presents it to the user (e.g., a confirmation dialog).
4. The frontend responds with `safeguard.confirm`:
   - `action: "allow"` — the interceptor processes all queued requests, sends their responses, and resumes normal operation. From the guest's perspective, the operations experienced a brief latency spike.
   - `action: "deny"` — the interceptor rolls back any writes already applied in the current step, then sends error responses for all queued requests. Since the entire step is being abandoned, the guest process's reaction to the errors is irrelevant — the user is discarding this command's output.
5. If no response is received within a configurable timeout, the agent defaults to `deny`.

**Why hold responses instead of returning errors?** Returning errors immediately would cause the guest process to see a mix of successful and failed I/O calls, potentially leaving VM-side state (e.g., a partially-completed `npm install`) in a confused and unrecoverable state. Holding responses keeps the process cleanly frozen: on "allow" it resumes exactly where it left off, on "deny" the entire step is rolled back cleanly.

**Configuration:**
```json
→ {"type":"safeguard.configure","request_id":"2","payload":{
     "delete_threshold": 50,
     "timeout_seconds": 30
   }}
← {"type":"response","request_id":"2","status":"ok"}
```

**Trigger and confirmation flow:**
```json
← {"type":"event.safeguard_triggered","payload":{
     "step_id": 12,
     "safeguard_id": "sg_001",
     "delete_count": 87,
     "sample_paths": ["src/old-module/", "tests/old-module/", "docs/deprecated/..."],
     "message": "Step 12 is attempting to delete 87 files/directories. Awaiting confirmation."
   }}
→ {"type":"safeguard.confirm","request_id":"3","payload":{
     "safeguard_id": "sg_001",
     "action": "allow"
   }}
← {"type":"response","request_id":"3","status":"ok"}
```

**Design notes:**
- The threshold is for delete operations specifically (not all writes), since deletes are the primary destructive risk. Two additional safeguard types are planned for the MVP:
  - **Overwrite/truncate threshold:** Triggers when existing files over a configurable size (default: 1 MB) are overwritten or truncated within a single step, using the same hold/confirm/deny pattern.
  - **Rename-over-existing threshold:** Triggers when a rename operation would overwrite an existing destination file.
  Additional safeguard types can be added later using the same pattern.
- The safeguard operates at the host-side agent level via the `WriteInterceptor`. The VM's command continues running in the guest, but its filesystem requests are held at the backend — the guest process blocks on the pending syscall. If denied, the entire step is rolled back.
- **Important ordering note:** The safeguard must trigger **before executing** the operation that crosses the threshold — not after. The interceptor checks the threshold, holds the current request, and emits the event before proceeding.
- **Unbounded queue mitigation:** While the VM's filesystem requests are held, the guest kernel may continue issuing new requests. The agent must cap the queued request count (e.g., 10,000) and reject further requests with `ENOSPC` if the queue overflows during a safeguard hold.
- **Alternative: QMP VM pause.** Instead of holding individual filesystem responses, the agent can pause the entire VM via QEMU's QMP `stop` command when a threshold is reached. This freezes all vCPUs, preventing new filesystem requests entirely. On `allow`, the agent sends QMP `cont`; on `deny`, it rolls back the step, then sends `cont`. This is simpler (no request queueing) but requires a QMP connection to QEMU (which is also useful for VM lifecycle/status). Both approaches are valid; the implementation should support QMP-based pause as the primary mechanism if a QMP connection is available.
- The undo log provides a second safety net, but the safeguard prevents the destructive operation from reaching the host in the first place, which matters for operations that are expensive to roll back (e.g., thousands of files in `node_modules`).

**Principles:**
- The STDIO API is the primary way external applications interact with the sandbox.
- The host-side agent can be used entirely through this API — no GUI or TUI is built into the agent itself.
- The API is stateful (a session must be started before other operations), but individual messages are self-contained.
- The protocol should be versioned from the start to allow evolution without breaking existing integrations.

### 4.6 MCP Server (LLM Integration Interface)

The host-side agent also runs an **MCP (Model Context Protocol) server**, allowing any MCP-compatible LLM to interact with the sandbox directly. This is the primary interface for AI-driven use — rather than building a custom integration for each LLM provider, the sandbox speaks a standard protocol that any compliant model can use out of the box.

**Transport:** JSON-RPC over a **separate local socket** — Unix domain socket on Linux/macOS, named pipe on Windows. The MCP server does **not** share stdin/stdout with the STDIO API. This avoids protocol multiplexing complexity and makes debugging easier. The socket path is printed to stderr on startup and can be configured via `--mcp-socket`.

**Exposed tools:**

| Tool | Description |
|---|---|
| `execute_command` | Run a terminal command inside the VM (relayed via control channel to the shim). Returns stdout, stderr, and exit code. |
| `read_file` | Read a file's contents from the working folder. |
| `write_file` | Write content to a file in the working folder. **Goes through the same undo/safeguard machinery** — participates in step accounting as its own "API step" if no command is running. |
| `list_directory` | List directory contents. |
| `undo` | Roll back the most recent N steps. |
| `get_undo_history` | List recent steps with metadata. |
| `get_session_status` | Query current session state. |

**`write_file` and undo integration:** When the MCP `write_file` tool is invoked outside of an active command step, the agent creates a synthetic "API step" for the write. This ensures all mutations — whether from VM commands or MCP API calls — flow through the same undo log and safeguard system. Without this, `write_file` would create an untracked mutation path that breaks undo assumptions.

**Relationship to the STDIO API (§4.5):**
- The MCP server and the STDIO API are two separate interfaces to the same underlying host-side agent.
- The MCP server is for LLMs — it exposes sandbox operations as callable tools using the standard MCP protocol. It listens on a separate local socket.
- The STDIO API is for frontends (GUIs, IDEs, CLIs) — it exposes the full management surface including session lifecycle, safeguards, and event streaming. It uses stdin/stdout.
- A typical deployment runs both: the frontend connects via the STDIO API to manage the session and display output, while an LLM connects via the MCP server to drive the coding agent.
- Both interfaces share the same undo log, safeguards, and filesystem translation layer. A destructive operation safeguard triggered by an LLM's `execute_command` will pause and emit an event on the STDIO API for the frontend to present to the user.

### 4.7 External Modification Detection

The working folder lives on the host filesystem, and the user (or other tools) may modify it while a sandbox session is active — editing files in an IDE, running `git pull`, triggering a file watcher, etc. The sandbox needs to detect these external modifications because:
- A rollback could silently destroy the user's own edits if external changes aren't properly guarded.

**Detection mechanism:** The host-side agent monitors the working folder using OS-native file watching (`inotify` on Linux, `FSEvents` on macOS, `ReadDirectoryChangesW` on Windows). Any change that did not originate from the filesystem backend's own write path is classified as an external modification.

**On detection, the agent:**
1. Emits an `event.external_modification` event on the STDIO API, listing the affected paths.
2. Creates an **undo barrier** — a marker in the undo history that prevents rollback from crossing it.

With no guest-side caching (`cache=none` / `--cache=never`), external changes are **immediately visible** to VM processes on their next read — no notification or invalidation mechanism is needed inside the VM.

#### 4.7.1 Undo Barrier Semantics

An undo barrier prevents rollback from silently clobbering user edits. The problem it solves:

1. Step 7 (sandbox) changes `foo` from `A → B`.
2. User edits `foo` on host: `B → C` (external modification, undo barrier created).
3. User calls `undo.rollback(1)` (wants to undo step 7).

Without barriers, rollback would restore `foo` to `A`, silently destroying the user's edit `C`. With barriers, the rollback is refused unless explicitly forced.

**Implementation:**
- Maintain a monotonically increasing `barrier_id`. Each external modification event increments it and stamps the barrier in the undo history.
- When `undo.rollback(N)` is requested, check if any barriers exist between the current state and the target step.
- If a barrier exists: refuse the rollback and return an error with a description of the external modifications that created the barrier.
- If the user explicitly confirms (via `undo.rollback` with `force: true`): proceed with the rollback, with a strong warning that external edits will be lost.

**Undo history visibility:** Barriers are visible in `undo.history` responses, showing the timestamp, affected paths, and a note that they block rollback.

**Policy options** (configurable per session):

| Policy | Behavior |
|---|---|
| `barrier` (default) | External modifications create an undo barrier. Rollback cannot cross barriers without explicit `force: true`. |
| `warn` | Emit a warning event but do not create a barrier. Rollback may inadvertently undo external changes. |
| `lock` | Reject external modifications by setting the working folder to read-only for non-sandbox processes (requires appropriate host permissions). |

### 4.8 Crash Recovery

If the host-side agent crashes or is killed mid-step — some writes applied to the host, some not — the system must recover to a consistent state on restart.

**Mechanism:** The WAL (Write-Ahead Logging) pattern ensures recovery. Unlike a traditional "manifest-up-front" WAL, the agent cannot know all operations in a step before they happen. Instead, the WAL **appends entries as operations occur**:

1. **On each first-touch of a path within a step:** the `WriteInterceptor` persists the preimage to the WAL directory before allowing the write to proceed. The WAL entry records the path, `existed_before` flag, and (if the file existed) the full preimage contents plus metadata.
2. **Paths created during the step** are recorded in the WAL as "created" entries (undo = delete).
3. **On normal step completion:** the WAL directory is promoted to a permanent undo log entry (renamed from `wal/in_progress/` to `steps/{step_id}/`).

**Recovery on restart (always-rollback-incomplete):**

1. The agent checks for a WAL directory marked `in_progress`.
2. If one exists, the agent **always rolls it back**: restores all captured preimages to their original paths, deletes all paths marked as "created", then discards the incomplete WAL entry.
3. This restores the working folder to a known-consistent state (the state before the interrupted step began).

**Why always-rollback instead of replay:**
- Avoids the complexity of determining "which operations were applied and which were not."
- Avoids ambiguous "did the VM see success?" questions for partially-completed operations.
- A crashed session didn't "complete" — rolling back to a clean state is the safest default.
- With preimage capture, rollback is a simple file restore (not a reverse-delta computation), so it's fast and reliable.

**Trade-off:** If the agent crashed *after* the guest observed success for some operations but *before* step commit, those operations are rolled back. This is acceptable: a crashed session didn't complete, and the user can re-run the command.

**Durability scope (MVP):** The WAL targets recovery from **agent crash / SIGKILL**, not host power loss. This means:
- The agent does not `fsync` on every preimage write (significant performance benefit; relies on OS page cache).
- Power-loss safety can be added post-MVP by enabling `fsync`/group-commit semantics if needed.

The STDIO API emits an `event.recovery` event on startup if crash recovery was performed, indicating that the incomplete step was rolled back and how many paths were restored.

### 4.9 Observability

The host-side agent has multiple concurrent subsystems (filesystem backend, control channel, undo log, file watcher, STDIO API, MCP server) that must be debuggable in production. Structured logging is specified upfront to avoid costly retrofitting.

**Logging framework:** The agent uses `tracing` (Rust) with structured fields. All log entries include:

| Field | Description |
|---|---|
| `timestamp` | ISO 8601 with microsecond precision |
| `level` | `error`, `warn`, `info`, `debug`, `trace` |
| `component` | Subsystem originating the log: `fs_backend`, `control`, `undo`, `file_watcher`, `stdio_api`, `mcp`, `session`, `safeguard` |
| `request_id` | Correlation ID linking a log entry to the STDIO API or MCP request that triggered it (if applicable) |
| `step_id` | Current step ID (if within an active step) |

**Output:** All diagnostic output goes to stderr as JSON Lines (one JSON object per line), never to stdout (which carries the STDIO API protocol). The log level is configurable at startup via a `--log-level` flag (default: `info`).

**Example log entries:**
```json
{"timestamp":"2025-03-01T12:00:01.234567Z","level":"info","component":"fs_backend","step_id":42,"message":"pre_write intercepted","path":"src/main.rs"}
{"timestamp":"2025-03-01T12:00:01.235000Z","level":"warn","component":"undo","step_id":42,"message":"step size approaching limit","current_bytes":180000000,"max_bytes":209715200}
{"timestamp":"2025-03-01T12:00:02.000000Z","level":"error","component":"control","message":"control channel read timeout","timeout_ms":5000}
```

**Key logging points per subsystem:**

| Subsystem | Info-level events | Warn/Error events |
|---|---|---|
| `fs_backend` | Session start/stop, backend type selected | Backend initialization failure, unexpected protocol errors |
| `control` | Step started/completed, command dispatched | Shim unresponsive, malformed messages, channel disconnection |
| `undo` | Step committed, preimage captured, rollback executed | Step size limit approached, log eviction triggered, WAL recovery |
| `file_watcher` | External modification detected | Watcher initialization failure, event overflow |
| `stdio_api` | Request received, response sent | Malformed request, unknown operation |
| `mcp` | Tool invocation, response sent | Protocol errors, unknown tool |
| `safeguard` | Safeguard triggered, user confirmed/denied | Safeguard timeout (auto-deny) |
| `session` | VM launched, VM stopped, health check passed | VM crash detected, QEMU exit with error |

**Debug and trace levels** add per-operation detail: individual filesystem calls (debug), wire-level protocol bytes (trace). These are disabled by default and enabled for troubleshooting.

### 4.10 Session Scope and Multiple Working Directories

The host-side agent manages a **single session** at a time — one QEMU instance, one VM. Concurrent sessions (multiple QEMU instances from a single agent process) are not supported, to avoid resource contention and complexity.

However, a single session can expose **multiple working directories** to the VM. This supports the common case of an IDE or workflow that needs access to several project directories simultaneously (e.g., a main project and a shared library).

**Implementation:**

- `session.start` accepts a list of working directories instead of a single path. Each directory is assigned a unique mount tag (e.g., `working0`, `working1`, ...) and mounted at a corresponding path inside the VM (`/mnt/working/0`, `/mnt/working/1`, ...).
- For the virtiofsd backend (Linux/macOS), each working directory gets its own virtiofsd instance and vhost-user socket. QEMU is configured with multiple `vhost-user-fs-pci` devices.
- For the 9P backend (Windows), each working directory gets a separate `virtio-9p-pci` device.
- Each working directory has its own `WriteInterceptor` instance and undo log. Undo operations are per-directory — rolling back step N in directory A does not affect directory B.
- Each working directory has an **access mode**: `read_write` (default) or `read_only`. This is enforced at two levels:
  - **Mount level:** The filesystem backend (virtiofsd or 9P server) is configured with read-only mount options for `read_only` directories, preventing writes at the transport layer.
  - **Interceptor level:** The `WriteInterceptor` rejects write operations targeting `read_only` directories, providing a second layer of enforcement.
  - **Undo scope:** `read_only` directories have no undo tracking — no `WriteInterceptor` instance, no preimage capture, no manifest entries. Since nothing should be written there, undo is not applicable.
- The STDIO API and MCP server operations accept a `directory` parameter (index or path) to disambiguate which working directory an operation targets. If omitted, the first (primary) directory is assumed.

**Configuration:**
```json
→ {"type":"session.start","request_id":"1","payload":{
     "working_directories": [
       {"path": "/home/user/project", "label": "project", "access": "read_write"},
       {"path": "/home/user/shared-lib", "label": "shared-lib", "access": "read_only"}
     ],
     "network_policy": "open",
     "vm_mode": "persistent"
   }}
```

**Resource note:** Each additional working directory adds a virtiofsd process (or 9P device) and a file watcher. For most use cases, 1-3 directories is typical. The agent should enforce a reasonable upper limit (e.g., 8) to prevent resource exhaustion.

---

## 5. Cross-Platform Filesystem Translation

This section applies **only to the 9P backend** (Windows hosts). On Linux and macOS hosts, the forked virtiofsd serves the filesystem directly — Linux-to-Linux requires no translation, and macOS-to-Linux requires no translation at the virtiofsd level because virtiofsd operates on host-native POSIX APIs and serves the results as-is.

**macOS note:** macOS is POSIX-compliant for permissions, ownership, and symlinks. The only macOS-specific concern is case sensitivity (default HFS+/APFS is case-insensitive), but this is a host filesystem property that virtiofsd transparently passes through — the same way any native macOS development tool works. Case collisions are the user's responsibility, consistent with normal macOS development.

The VM always runs Linux. The Windows host requires translation in both directions via the 9P server.

### 5.1 Known Edge Cases

| Issue | Linux | macOS | Windows |
|---|---|---|---|
| Case sensitivity | Case-sensitive | Case-insensitive (default) | Case-insensitive |
| Path separator | `/` | `/` | `\` |
| Symlinks | Always available | Always available | Requires dev mode or admin |
| Symlink types | Unified | Unified | Separate file/dir symlinks |
| Permissions | POSIX (rwx) | POSIX (rwx) | ACL-based, no execute bit equiv |
| Reserved names | None | None | `CON`, `PRN`, `AUX`, `NUL`, `COM0-9`, `LPT0-9` |
| Max path length | ~4096 | ~1024 | 260 (default) / 32767 (extended) |
| Line endings | LF | LF | CRLF (convention, not enforced) |

### 5.2 Read Normalization (9P Responses)

When the VM issues 9P `Tgetattr`, `Treaddir`, or `Tread` requests, the 9P server reads from the host via `std::fs` and normalizes the response:

**Permissions:**
- **Windows host:** Synthesize mode bits using a **POSIX metadata overlay store** — a small SQLite database (or flat file store) located outside the shared folder (e.g., `.sandbox/metadata.db`). The overlay persistently records:
  - `mode` bits for each path (set via `chmod` from the VM)
  - symlink targets and types (if emulating POSIX symlinks)
  - uid/gid (constant `1000:1000`, but stored for completeness)
  - timestamps if Windows precision differs from POSIX requirements
  
  The overlay is consulted on every `Tgetattr` and updated on every `Tsetattr`. This replaces the heuristic-only approach (`.sh` gets `0755`) which does not cover common cases like `node_modules/.bin/*` (executables without extensions) or explicit `chmod +x` calls from build scripts. The heuristic serves as the **default** for files not yet in the overlay: `0644` for files, `0755` for directories, `0755` for files with known executable extensions. Once a `chmod` is applied from the VM, the overlay takes precedence. Mutagen's approach is a good reference for this design.

**Ownership (uid/gid):**
- **Windows host:** Synthesize a consistent uid/gid (e.g., `1000:1000`) for all files. Windows ACLs don't map to POSIX ownership in a meaningful way.

**Symlinks:**
- **Windows host:** Unify file symlinks and directory symlinks into a single POSIX-style symlink. If symlink capability is unavailable (no dev mode, no admin), report the target as a regular file/directory and log a warning at session start.

**Case sensitivity:**
- **Windows host:** The server serves whatever the host returns. Case collisions are detected proactively on writes (see §5.4).

**File names:**
- **Windows host:** Reject or sanitize filenames containing characters illegal on Windows (`<`, `>`, `:`, `"`, `|`, `?`, `*`) or matching reserved names (`CON`, `NUL`, etc.) at write time. At read time, these files cannot exist on the host, so no translation is needed.

### 5.3 Write Translation (9P Requests)

When the VM issues 9P write operations (`Tcreate`, `Twrite`, `Tremove`, `Trename`, `Tmkdir`, etc.), the 9P server translates and applies them:

- **Case collisions:** Detect when the VM creates two files differing only in case. Surface as an error rather than silently clobbering one.
- **Symlinks on Windows:** Detect capability at agent startup. If unavailable, either degrade gracefully (copy instead of link) or warn the user.
- **Reserved names:** Reject or escape Windows-reserved filenames when the host is Windows.
- **Permissions:** Map POSIX permissions to the closest host equivalent. Accept that fidelity will be lost on Windows (e.g., execute bit has no effect on native Windows). Use the POSIX metadata overlay (§5.2) to persistently store mode bits set from the VM.
- **Path translation:** 9P uses `/`-separated paths. The host-side agent converts to native separators on apply.
- **Reparse point / junction containment (security-critical):** Windows has junctions and reparse points that can escape the shared directory root, similar to symlink escape attacks on Unix. The 9P server must defensively:
  - Open all paths **relative to a root directory handle** (using `NtCreateFile` with a root directory parameter, or `CreateFile` with proper relative path handling).
  - **Never follow reparse points** unless the target resolves within the shared directory.
  - **Validate all final resolved paths** are within the shared directory root before proceeding.
  - This requires dedicated tests and fuzzing — a compromised guest creating malicious reparse points is a real attack vector.

### 5.4 Case Collision Detection

On the Windows host (case-insensitive NTFS), the 9P server maintains an in-memory index of the canonical casing for every path in the working folder. When the VM issues a 9P create for a path whose case conflicts with an existing entry, the server returns a 9P error, which surfaces to the coding agent as a filesystem error.

Note: macOS (also case-insensitive by default) uses the virtiofs backend, where case behavior is the host filesystem's native behavior — same as any macOS development tool. The virtiofsd fork does not add case collision detection.

### 5.5 Reference Implementation

Mutagen's `pkg/filesystem` package (Go, MIT-licensed) handles all of the above for real-world cross-platform file sync. Use it as the primary reference when implementing the Rust translation layer.

---

## 6. Step Granularity & Transaction Model

### 6.1 Definition of a Step

One step = one terminal command execution. Step boundaries are determined by signals from the VM-side shim over the control channel:

1. The host-side agent sends an `exec` message to the shim.
2. The shim sends `step_started` — the agent opens a new undo step.
3. While the command runs, any filesystem writes (intercepted by the `WriteInterceptor` in whichever backend is active) are attributed to this step.
4. The shim sends `step_completed` — the agent closes the step, finalizing the undo log entry.

This correlation is straightforward because commands are sequential — only one command runs at a time in the MVP. Filesystem writes arriving between `step_started(N)` and `step_completed(N)` belong to step N. Any writes arriving *outside* an active step (e.g., from background processes still running after a command completes) are attributed to a synthetic "ambient" step.

### 6.2 Logical Step Grouping / Transaction Protocol (Deferred)

Sometimes the coding agent needs to run multiple terminal commands that together form one coherent change — for example, "refactor the auth module" might involve moving files, renaming imports, and updating tests across several commands. With the current model (one command = one step), undoing that refactor requires hitting undo multiple times, and a partial undo could leave the codebase in a broken intermediate state.

A transaction protocol would let the agent declare that a sequence of commands should be treated as a single logical unit for undo purposes:
- `begin_step` — opens a new step; all subsequent writes across any number of terminal commands are grouped
- `commit_step` — closes the step; all grouped writes are finalized as a single undo unit
- `rollback_step` — discards all writes in the current open step without committing

**Deferred because:** The MVP's one-command-one-step model is sufficient for initial use. The control channel protocol should be designed with extensibility in mind so this can be added without breaking changes, but the implementation is not required for the first version.

---

## 7. Rollback Mechanics

### 7.1 Single-Step Undo
1. Read the most recent step from the undo log.
2. Check for undo barriers between the current state and the target step. If a barrier exists, refuse rollback unless `force: true` is specified.
3. For each affected path in the step:
   - If `existed_before` is true: restore the preimage (file contents + metadata) from the undo store.
   - If `existed_before` is false (file was created in this step): delete the file.
4. With no guest-side caching, the VM immediately sees the restored state on the next read — no invalidation needed.

### 7.2 Multi-Step Undo
Apply single-step undo repeatedly, from most recent step backward. Since each step's preimages are self-contained (no delta chains or checkpoint dependencies), multi-step rollback is straightforward — each step is reversed independently.

### 7.3 VM State After Rollback
After a rollback, the VM's in-memory state (open file handles, running processes, cached paths) may be stale. However, with no guest-side caching, there are no stale kernel caches — the next filesystem access will fetch the current state from the agent. The main risk is processes holding open file descriptors to deleted or modified files.

The host-side agent sends a `rollback_notify` message to the VM-side shim over the control channel, which can relay this to the coding agent so it re-reads the filesystem.

For MVP, this notification is the simplest path. The no-cache mount eliminates the most common staleness issues.

### 7.4 Concurrent Write Handling During Rollback (Deferred)

If a background process inside the VM is actively writing files while a rollback is being applied, the filesystem write requests and the rollback's host-side writes race. The host-side agent can manage this by temporarily returning errors for incoming filesystem write requests during rollback, effectively freezing the VM's writes until rollback completes.

**Deferred because:** For the MVP, rollback is expected to be a deliberate, user-initiated action that occurs when the agent is idle. The risk is low as long as rollbacks are not triggered mid-execution.

---

## 8. Technology Decisions

| Component | Technology | Rationale |
|---|---|---|
| VM guest OS | Linux (always) | Supports both virtio-fs driver (5.4+) and v9fs kernel module |
| Guest arch (Linux host) | x86_64 | Matches host, KVM hardware acceleration |
| Guest arch (macOS host) | aarch64 | Matches Apple Silicon, HVF hardware acceleration |
| Guest arch (Windows host) | x86_64 | Matches host, WHPX hardware acceleration |
| Filesystem backend (Linux/macOS hosts) | Forked virtiofsd (Rust, Apache 2.0) | Near-native perf, full POSIX incl. mmap, single daemon. macOS port adds fd management layer. |
| Filesystem backend (Windows hosts) | Custom 9P2000.L server (Rust) | Windows lacks SCM_RIGHTS — vhost-user transport cannot work. 9P via QEMU built-in device avoids the issue entirely. |
| Write interception | Shared `WriteInterceptor` trait | Both backends call the same undo/safeguard logic; no duplication |
| virtio-fs transport | vhost-user socket | Standard QEMU virtio-fs plumbing; works on Linux (memfd) and macOS (shm_open) |
| virtio-fs cache mode | `--cache=never` + guest `cache=none` | Immediate external change visibility |
| 9P transport | QEMU built-in `virtio-9p-pci` + fd pair | Sub-millisecond local latency, no vhost-user dependency |
| 9P cache mode | `cache=none` | Immediate external change visibility |
| Control channel | virtio-serial | Character device in guest, Unix socket on host; structured JSON Lines messages |
| VM machine type (Linux/Windows) | `q35` | PCI bus required for virtio-fs-pci and virtio-serial-pci; works with KVM and WHPX |
| VM machine type (macOS) | `virt` | ARM's lightweight VM platform with PCI (ecam); works with HVF on Apple Silicon |
| VM boot method | Direct kernel boot | Bypasses BIOS/UEFI; sub-second boot on all platforms |
| VM memory backend (Linux) | `memory-backend-memfd` | Standard for vhost-user shared memory on Linux |
| VM memory backend (macOS) | `memory-backend-shm` | POSIX `shm_open()`, portable alternative to Linux-only memfd (merged QEMU ~9.1) |
| VM runtime (Linux host) | QEMU + KVM | Best virtio-fs + virtio-serial support, near-native performance |
| VM runtime (macOS host) | QEMU + HVF acceleration | Native hardware acceleration for aarch64 guests on Apple Silicon |
| VM runtime (Windows host) | QEMU + WHPX acceleration | Native hardware acceleration via Hyper-V |
| VM-side shim | Lightweight script/binary | Minimal shell executor with structured I/O; only custom software in the VM |
| Host-side agent | Rust (3 platform builds) | Cross-platform `std::fs`, safety, single binary per platform |
| Async runtime | tokio | Multiplexes all I/O channels in one event loop |
| Blocking FS I/O | `tokio::spawn_blocking` | Dispatches `std::fs` calls to tokio's thread pool |
| Parallel bulk ops | Rayon | Work-stealing for directory scanning, large rollback operations |
| Cross-platform FS reference | Mutagen source (MIT) | Battle-tested edge case handling (Windows only, since macOS now uses virtiofs) |
| Undo log format | Per-step preimage files (zstd compressed, optional reflinks); evaluate SQLite post-MVP | Simpler than deltas, reliable rollback, no dependency chains; see §4.4 |
| Structured logging | `tracing` (Rust) with JSON Lines on stderr | Structured fields, component tags, correlation IDs; essential for debugging concurrent subsystems |
| Host integration API | STDIO (JSON Lines) | Language-agnostic, trivial to consume from any frontend, easy to debug |
| LLM integration | MCP (JSON-RPC over local socket: Unix domain socket on Linux/macOS, named pipe on Windows) | Standard protocol, separate transport from STDIO API, compatible with any MCP-capable LLM |
| External modification detection | OS-native file watching | `inotify` (Linux), `FSEvents` (macOS), `ReadDirectoryChangesW` (Windows) |

---

## 9. Open Questions

1. **Undo log on-disk layout details:** Preimage capture is the chosen approach (see §4.4.1). The exact directory structure (`.sandbox/undo/steps/{step_id}/` with one file per affected path?), naming scheme, WAL entry format, and zstd compression configuration still need specification. See Appendix B §B.2.

2. **Rollback trigger:** Is rollback initiated by the agent itself, by the user, or by an external supervisor? Each has different UX and safety implications.

3. **Large binary files:** Preimage capture stores full copies, which may be expensive for very large binaries. Consider a configurable size threshold above which preimage capture is skipped and the path is marked "unprotected." Alternatively, reflinks can mitigate storage cost on supported filesystems.

4. **Persistent VM storage location and size limits:** For persistent VMs, where does the VM disk image live on the host, and how large can it grow? Users may accumulate significant state (installed packages, build caches) over time.

5. **Coding agent pluggability:** Is the coding agent a fixed component or pluggable? If pluggable, the MCP server and STDIO API may need operations for selecting and configuring which agent runs inside the VM.

6. **virtiofsd fork maintenance strategy:** How closely to track upstream. Pin to a specific release and cherry-pick security fixes? Maintain as a thin patch series? The macOS portability layer (§C.8) adds a second axis of maintenance. See Appendix C §C.5.

7. **Multiple directory cross-references:** When a session exposes multiple working directories (§4.10), operations that span directories (e.g., moving a file from one working directory to another) are not supported through the filesystem bridge — each mount is independent. Should the agent detect and warn about cross-directory operations, or is this a documented limitation?

8. **macOS symlink containment strategy:** The symlink escape security analysis (§C.8.1) identifies risks but does not prescribe a single solution. The `openat()`-relative approach is recommended, but the exact implementation depends on how deeply `PassthroughFs` can be refactored. This must be resolved before Phase 2 begins.

9. **Path-based vs inode-based undo tracking:** The current design is path-oriented. Hardlinks mean "one inode, multiple paths" and renames mean "same inode migrates paths during a step." Path-based preimages work for most cases, but edge cases exist. Define "first-touch per canonical path at the time of touch" and document limitations. Consider inode-based tracking as a future enhancement if hardlink-heavy workloads cause issues.

10. **Windows 9P `mmap` validation:** If `mmap` truly fails under `v9fs` with `cache=none`, this may break representative workloads. Must validate early (see §4.1.3). If it's a blocker, options include relaxing caching on Windows or a WSL2-based architecture.

11. **Filesystem server hardening:** Both virtiofsd and the 9P server are directly exposed to an untrusted guest kernel client. What level of sandboxing and fuzzing is appropriate for MVP? See §4.3.4.

---

## 10. MVP Scope

For the first working version, target:

- [ ] Linux host (x86_64) + QEMU q35 with KVM, direct kernel boot, virtio-fs (forked virtiofsd) and virtio-serial
- [ ] Forked virtiofsd with `WriteInterceptor` hooks for undo/safeguard integration, including coverage for: write, create, mkdir, unlink, rmdir, rename, setattr, link, symlink, setxattr/removexattr, open with O_TRUNC, fallocate, copy_file_range
- [ ] `WriteInterceptor` trait and `UndoInterceptor` implementation (shared between backends), with first-touch preimage capture including metadata (mode, mtime, xattrs)
- [ ] tokio-based async event loop multiplexing all I/O channels
- [ ] VM-side shim for command execution and step boundary signalling over virtio-serial
- [ ] Control channel protocol (JSON Lines over virtio-serial) with host-authoritative step IDs and quiescence window for late-arriving writes
- [ ] Step boundary detection: correlate control channel signals with filesystem writes for undo grouping
- [ ] Undo log with per-step preimage capture: full-file backups with zstd compression and optional reflink/clone support
- [ ] N-step retention with simple FIFO pruning (no checkpoints needed)
- [ ] Undo log resource limits: configurable maximum log size with oldest-step eviction to prevent host disk exhaustion
- [ ] Single-step undo/redo triggered by user command, with undo-barrier enforcement for external modifications
- [ ] STDIO API (JSON Lines over stdin/stdout) for external integration
- [ ] MCP server (JSON-RPC over Unix domain socket) for LLM integration — separate transport from STDIO API
- [ ] MCP `write_file` tool routed through undo/safeguard machinery as synthetic "API steps"
- [ ] Destructive operation safeguard: configurable delete threshold with pause/confirm/deny flow, plus overwrite-large-file and rename-over-existing thresholds. QMP-based VM pause as primary mechanism when QMP is available.
- [ ] VM network access with configurable policy (open / IP/CIDR allowlist / disabled)
- [ ] VM lifecycle management: persistent (disk only, no suspend/resume) and ephemeral modes
- [ ] External modification detection via inotify, with undo-barrier semantics (barrier / warn / lock policy)
- [ ] Crash recovery: append-based WAL with always-rollback-incomplete policy (no replay); targets agent crash, not power loss
- [ ] Structured logging: `tracing`-based JSON Lines output on stderr with component tags and request correlation IDs
- [ ] Undo log versioning: version file with discard-on-mismatch policy (user confirmation required)
- [ ] Multiple working directories per session: each with independent virtiofsd instance, WriteInterceptor, and undo log
- [ ] Protocol version negotiation and stable error taxonomy (code, message, data) for both STDIO API and MCP
- [ ] Filesystem server hardening: virtiofsd runs least-privilege with chroot sandbox; fuzz testing for FUSE and 9P wire parsers

**Phase 2 (macOS Apple Silicon support):**
- [ ] Port virtiofsd fork to macOS: replace `/proc/self/fd` + `O_PATH` with `fcntl(F_GETPATH)` + `openat()+O_NOFOLLOW`, `renameat2(RENAME_EXCHANGE)` with `renameatx_np(RENAME_SWAP)`, `statx()` with `fstatat()`, drop mount namespace sandboxing (VM is our sandbox) — see Appendix C §C.8
- [ ] Symlink escape security review: audit every `PassthroughFs` path resolution code path on macOS, implement path containment checks, test with symlink escape attempts — see Appendix C §C.8.1
- [ ] **macOS host sandboxing:** Run virtiofsd under a macOS sandbox profile and as a dedicated unprivileged user — treat "guest can exploit virtiofsd" as a real threat since it's exposed to untrusted input
- [ ] **Case-collision detection on macOS:** Optional detection for Linux-in-VM creating case-conflicting paths (e.g., `Foo` and `foo`) on case-insensitive APFS, since the mismatch between the case-sensitive guest and case-insensitive host is more dangerous than on native macOS development
- [ ] QEMU `qemu-system-aarch64` with HVF acceleration, machine type `virt`, `memory-backend-shm`
- [ ] aarch64 Linux kernel + rootfs for macOS guest
- [ ] Verify vhost-user transport works on macOS AF_UNIX (SCM_RIGHTS)
- [ ] External modification detection via FSEvents

**Phase 3 (Windows support):**
- [ ] Custom 9P2000.L server (Appendix A) sharing `WriteInterceptor` with virtiofsd fork
- [ ] QEMU + WHPX acceleration on Windows, machine type `q35`, built-in `virtio-9p-pci` device
- [ ] Windows-specific normalization in 9P server: **POSIX metadata overlay store** (SQLite DB) for persistent mode/xattr/ownership tracking, reserved name handling, symlink type mapping, case collision detection
- [ ] **Reparse point / junction security:** 9P server must open paths relative to root directory handle, avoid following reparse points, validate resolved paths are within root — requires dedicated tests and fuzzing
- [ ] **mmap validation:** Test representative workloads (`cargo build`, `npm install`, `git status`) with `cache=none` on v9fs; determine if mmap failure is a blocker and select mitigation strategy
- [ ] External modification detection via ReadDirectoryChangesW
- [ ] MCP server transport via named pipe (Windows equivalent of Unix domain socket)

Defer to post-MVP:
- Logical step grouping / transaction protocol (§6.2)
- Concurrent write handling during rollback (§7.4)
- Delta compression as an optimization over preimage capture
- Domain-based network allowlist (via transparent proxy)
- Per-path invalidation for external modifications (more granular alternative to undo barriers)
- Suspend/resume VM RAM state (unnecessary given sub-second boot)

---

## 11. Implementation Guide

This section provides the information an implementing agent needs to begin work efficiently. A companion **testing plan** (`testing-plan.md`) should be loaded alongside this project plan — it contains the full test harness architecture, 130+ enumerated test cases, and the TDD development sequence referenced below.

### 11.1 What Is Ready to Build Immediately

Steps 1–7 of the TDD sequence (see testing plan §11) require **no QEMU, no networking, no async runtime** — pure Rust + `std::fs` on temp directories. This is the tightest possible development loop and where the majority of correctness bugs will be found:

1. `TreeSnapshot` + `assert_tree_eq` (test oracle infrastructure)
2. `UndoInterceptor` core: first-touch preimage capture, rollback for create/write/rename/delete
3. WAL + crash recovery with fault injection
4. Undo barrier logic for external modifications
5. Safeguard threshold checking at the interceptor level
6. Metadata capture: mode bits, mtime, xattrs
7. Resource limits + FIFO pruning

Only at step 13 (session lifecycle) does QEMU enter the picture. Build and test the undo/safeguard/WAL core thoroughly before integrating the VM.

### 11.2 Decisions to Resolve Early

The following items from Appendix B are not yet fully specified. They are listed in order of when they'll block implementation progress:

**Needed before writing the first `Cargo.toml` (step 1):**
- **B.1 Cargo workspace structure:** Rust edition (2021 vs 2024), MSRV, how the virtiofsd fork is vendored (git subtree, submodule, or full copy). The recommended layout in B.1 is solid — just commit to it.

**Needed at TDD step 2 (UndoInterceptor):**
- **B.2 Undo log on-disk layout:** The plan specifies preimage capture with zstd compression and optional reflinks, but the exact decisions needed are:
  - Directory structure: recommended `.sandbox/undo/steps/{step_id}/` with one preimage file per affected path, plus a `manifest.json` per step.
  - WAL layout: `wal/in_progress/` promoted to `steps/{step_id}/` via atomic directory rename.
  - Manifest format: JSON is recommended for inspectability during development; can migrate to bincode post-MVP if performance warrants.
  - Compression: zstd level 3 (good speed/ratio trade-off). Per-file compression.
  - Metadata sidecar: `.meta.json` alongside each preimage file, containing mode, mtime, xattrs, file_type, symlink_target.

**Needed at TDD step 8 (control channel):**
- No unresolved decisions — the protocol is fully specified in §4.2.

**Needed at TDD step 10 (STDIO API):**
- **B.5 Message schemas:** The operation tables in §4.5 list types and fields but no formal JSON schemas. Define concrete field names and nesting as you implement. The testing plan (§1.5) locks down event/response ordering: events and responses may interleave; clients correlate by `request_id`.

**Needed at TDD step 13 (QEMU E2E):**
- **B.3 QEMU configuration:** Templates are provided in Appendix C §C.7 for Linux and macOS. Windows template still needs derivation.
- **B.8 VM-side shim:** Language (recommend Python or compiled Go for a small, dependency-free binary), packaging (baked into guest image), and startup (systemd service or init script).

### 11.3 Spec Decisions Locked Down in the Testing Plan

The testing plan resolves several ambiguities that the project plan left open. These decisions are authoritative:

1. **`post_create` vs `pre_open_trunc`:** The filesystem backend is responsible for distinguishing "create new" from "open-and-truncate existing." If create opens an existing file with truncation, the backend calls `pre_open_trunc` (not `post_create`). `post_create` is only for genuinely new inodes.

2. **Directory restore ordering:** Rollback uses two passes — (1) recreate directories shallowest-first, then restore file contents/metadata; (2) restore directory metadata deepest-first (so child restoration doesn't clobber parent mtime).

3. **Metadata equality:** Rollback restores and tests assert: file contents (byte-exact), file type (exact), mode bits (all 12 bits), mtime (within FS granularity, default 1ms tolerance), xattrs (exact if supported), symlink target (exact). atime is deliberately **not** restored or asserted.

4. **Rollback is pop:** Rolled-back steps are removed from history. No redo. `undo.history` after `rollback(2)` no longer contains the two most recent steps.

5. **Event/response ordering:** Events and responses may interleave on STDIO stdout. Only guarantee: the `response` for a `request_id` is sent after the operation completes. Clients correlate by ID, not by stream position.

6. **Ambient steps:** Writes outside any active command step go to synthetic "ambient" steps with negative IDs (e.g., `-1`, `-2`). Ambient steps auto-close after 5 seconds of inactivity. They appear in `undo.history` with `type: "ambient"`.

### 11.4 Recommended Cargo Workspace

```
sandbox-agent/
  Cargo.toml                   # Workspace root
  crates/
    agent/                     # Host-side agent binary (depends on all below)
    interceptor/               # WriteInterceptor trait + UndoInterceptor impl + WAL
    virtiofsd-fork/            # Forked virtiofsd with interception hooks + macOS compat
    p9/                        # 9P2000.L protocol + server (Windows only)
    control/                   # Control channel protocol types and handler
    shim/                      # VM-side shim (separate binary, minimal deps)
    common/                    # Shared types (step types, error taxonomy, event types)
    test-support/              # Test utilities (TempWorkspace, TreeSnapshot, clients, fault injection)
  tests/
    integration/               # Component integration tests (L2)
    protocol/                  # Protocol integration tests (L3)
    e2e/                       # QEMU system tests (L4)
  fuzz/                        # Fuzz targets and corpora (L5)
  benches/                     # Criterion benchmarks (L6)
```

Start with `interceptor/`, `common/`, and `test-support/` crates. The `agent/` binary and `virtiofsd-fork/` come later.

### 11.5 Cargo Features

```toml
[features]
default = []
fault_injection = []  # Enables FaultInjector; never in release builds
e2e_tests = []        # Enables QEMU E2E test compilation
```

Commands:
```bash
cargo test                                    # L1 + L2 + L3 (fast, per-PR)
cargo test --features fault_injection         # L2 with crash/fault tests
cargo test --features e2e_tests --ignored     # L4 QEMU E2E (nightly, needs KVM)
```

---

## Appendix A: 9P2000.L Server Implementation Guide

This appendix provides enough detail for an LLM coding agent to implement the custom 9P2000.L server used on Windows hosts. The primary reference is the crosvm `p9` crate source (BSD-licensed, `chromium.googlesource.com/crosvm`) and the Linux kernel's `net/9p/` and `fs/9p/` source.

**Relationship to the virtiofsd fork:** The 9P server and the forked virtiofsd are separate filesystem backends. They share the `WriteInterceptor` trait (§4.3.3) but have completely different wire protocols and codebases. The 9P server is a from-scratch implementation used only on Windows (where vhost-user transport is unavailable due to missing SCM_RIGHTS); the virtiofsd fork is a modification of an existing project used on Linux and macOS (see Appendix C).

### A.1 Wire Format

Every 9P message has the same envelope:

```
[4 bytes: size (little-endian u32, includes these 4 bytes)]
[1 byte:  type (u8, identifies the message)]
[2 bytes: tag  (little-endian u16, client-chosen request ID)]
[... message-specific fields ...]
```

- **size** includes itself, so minimum message size is 7 bytes (header only).
- **tag** correlates requests with responses. `NOTAG` (0xFFFF) is used only for `Tversion`.
- Each T-message (client request) has a corresponding R-message (server response).
- On error, the server replies with `Rlerror` instead of the expected R-message.

**Primitive types on the wire:**

| Type | Encoding |
|---|---|
| `u8` | 1 byte |
| `u16` | 2 bytes, little-endian |
| `u32` | 4 bytes, little-endian |
| `u64` | 8 bytes, little-endian |
| `string` | `u16` length prefix + UTF-8 bytes (no null terminator) |
| `qid` | 13 bytes: `u8` type + `u32` version + `u64` path |
| `stat` | Variable, see Tgetattr/Rgetattr |
| `data` | `u32` length prefix + raw bytes |

### A.2 Message Types (9P2000.L)

The type byte identifies the message. T-messages are even, R-messages are odd.

#### A.2.1 Session Management

| Type | Name | Fields | Purpose |
|---|---|---|---|
| 8/9 | `Tstatfs`/`Rstatfs` | `fid` → `type bsize blocks bfree bavail files ffree fsid namelen` | Filesystem stats |
| 12/13 | `Tlopen`/`Rlopen` | `fid flags` → `qid iounit` | Open a file |
| 14/15 | `Tlcreate`/`Rlcreate` | `fid name flags mode gid` → `qid iounit` | Create + open file |
| 16/17 | `Tsymlink`/`Rsymlink` | `fid name symtgt gid` → `qid` | Create symlink |
| 18/19 | `Tmknod`/`Rmknod` | `fid name mode major minor gid` → `qid` | Create device node |
| 20/21 | `Trename`/`Rrename` | `fid dfid name` → (empty) | Rename file |
| 22/23 | `Treadlink`/`Rreadlink` | `fid` → `target` | Read symlink target |
| 24/25 | `Tgetattr`/`Rgetattr` | `fid request_mask` → `valid qid mode uid gid nlink rdev size blksize blocks atime_sec atime_nsec mtime_sec mtime_nsec ctime_sec ctime_nsec btime_sec btime_nsec gen data_version` | Get file attributes |
| 26/27 | `Tsetattr`/`Rsetattr` | `fid valid mode uid gid size atime_sec atime_nsec mtime_sec mtime_nsec` → (empty) | Set file attributes |
| 30/31 | `Txattrwalk`/`Rxattrwalk` | `fid newfid name` → `size` | Walk to xattr |
| 32/33 | `Txattrcreate`/`Rxattrcreate` | `fid name attr_size flags` → (empty) | Create xattr |
| 40/41 | `Treaddir`/`Rreaddir` | `fid offset count` → `data` | Read directory entries |
| 50/51 | `Tfsync`/`Rfsync` | `fid` → (empty) | Flush file to disk |
| 52/53 | `Tlock`/`Rlock` | `fid type flags start length proc_id client_id` → `status` | POSIX file lock |
| 54/55 | `Tgetlock`/`Rgetlock` | `fid type start length proc_id client_id` → `type start length proc_id client_id` | Query lock state |
| 70/71 | `Tlink`/`Rlink` | `dfid fid name` → (empty) | Create hard link |
| 72/73 | `Tmkdir`/`Rmkdir` | `dfid name mode gid` → `qid` | Create directory |
| 74/75 | `Trenameat`/`Rrenameat` | `olddirfid oldname newdirfid newname` → (empty) | Rename (at-style) |
| 76/77 | `Tunlinkat`/`Runlinkat` | `dirfid name flags` → (empty) | Unlink (at-style) |

#### A.2.2 Core Protocol (inherited from 9P2000)

| Type | Name | Fields | Purpose |
|---|---|---|---|
| 6/7 | `Rlerror` | `ecode` | Error response (Linux errno) |
| 100/101 | `Tversion`/`Rversion` | `msize version` → `msize version` | Protocol negotiation |
| 104/105 | `Tauth`/`Rauth` | `afid uname aname n_uname` → `aqid` | Authentication (optional) |
| 108/109 | `Tattach`/`Rattach` | `fid afid uname aname n_uname` → `qid` | Connect to filesystem root |
| 110 | `Terror` | — | Invalid (never sent) |
| 112/113 | `Tflush`/`Rflush` | `oldtag` → (empty) | Cancel pending request |
| 114/115 | `Twalk`/`Rwalk` | `fid newfid nwname wname[nwname]` → `nwqid wqid[nwqid]` | Traverse directory path |
| 116/117 | `Tread`/`Rread` | `fid offset count` → `data` | Read file contents |
| 118/119 | `Twrite`/`Rwrite` | `fid offset data` → `count` | Write file contents |
| 120/121 | `Tclunk`/`Rclunk` | `fid` → (empty) | Close a fid |
| 122/123 | `Tremove`/`Rremove` | `fid` → (empty) | Remove file + clunk fid |

### A.3 Key Concepts

**QID** (unique identifier for a filesystem object):
```rust
struct Qid {
    ty: u8,     // QTDIR=0x80, QTAPPEND=0x40, QTEXCL=0x20, QTSYMLINK=0x02, QTFILE=0x00
    version: u32, // Incremented on each modification (can use mtime or a counter)
    path: u64,    // Unique ID for this object (inode number or synthetic)
}
```

**FID** (client-assigned handle):
- A u32 chosen by the client to reference a filesystem object.
- The server maintains a `HashMap<u32, FidState>` mapping fids to internal state.
- `FidState` holds: host path, open file handle (if opened), qid, directory position (for readdir).
- `Tattach` establishes the root fid. `Twalk` clones/walks fids. `Tclunk` releases them.

**iounit** (returned by `Tlopen`/`Tlcreate`):
- Tells the client the maximum data payload per read/write. Set this to `msize - 24` (header overhead). A large `msize` (several MB) improves throughput for bulk I/O.

### A.4 Module Structure

```
crates/
  p9/
    src/
      lib.rs           -- Public API: P9Server struct, Config
      wire.rs          -- Serialize/deserialize all message types
      messages.rs      -- Struct definitions for each T/R message pair
      fid.rs           -- FidTable: HashMap<u32, FidState>, fid lifecycle
      server.rs        -- Main dispatch: read message → match type → handle → write response
      operations/
        mod.rs
        session.rs     -- version, auth, attach, flush, clunk
        walk.rs        -- walk (path traversal, fid cloning)
        file.rs        -- lopen, lcreate, read, write, fsync
        dir.rs         -- mkdir, readdir, unlinkat, renameat
        attr.rs        -- getattr, setattr
        link.rs        -- symlink, readlink, link, mknod
        statfs.rs      -- statfs
      platform/
        mod.rs         -- Platform detection, trait for platform-specific behavior
        windows.rs     -- Synthesize POSIX metadata from Windows APIs, case collision index
```

Note: no `linux.rs` or `macos.rs` in the platform module. On Linux and macOS, the forked virtiofsd is used instead of the 9P server. The 9P server is only compiled and used on Windows.

### A.5 Implementation Order

Build and test incrementally. Each phase produces a mountable (though incomplete) filesystem:

**Phase 1: Read-only mount (minimum viable 9P)**
1. `wire.rs` — message parsing and serialization
2. `messages.rs` — struct definitions
3. `Tversion`/`Rversion` — handshake (agree on msize, respond with `"9P2000.L"`)
4. `Tattach`/`Rattach` — bind root fid to working folder, return root qid
5. `Twalk`/`Rwalk` — traverse paths, clone fids
6. `Tgetattr`/`Rgetattr` — stat files (mode, size, times, uid, gid)
7. `Treaddir`/`Rreaddir` — list directories
8. `Tlopen`/`Rlopen` — open files for reading
9. `Tread`/`Rread` — read file contents
10. `Tclunk`/`Rclunk` — release fids
11. `Tstatfs`/`Rstatfs` — filesystem stats (can return hardcoded reasonable values)

**Test:** Mount with `mount -t 9p -o version=9p2000.L,trans=fd,cache=none`, run `ls`, `cat`, `find`, `stat`.

**Phase 2: Write support**
12. `Tlcreate`/`Rlcreate` — create files (call `interceptor.post_create`)
13. `Twrite`/`Rwrite` — write file contents (call `interceptor.pre_write`)
14. `Tmkdir`/`Rmkdir` — create directories (call `interceptor.post_mkdir`)
15. `Tunlinkat`/`Runlinkat` — delete files and directories (call `interceptor.pre_unlink`)
16. `Trenameat`/`Rrenameat` — rename files (call `interceptor.pre_rename`)
17. `Tsetattr`/`Rsetattr` — chmod, chown, truncate, utimes (call `interceptor.pre_setattr`)
18. `Tfsync`/`Rfsync` — flush to disk

**Test:** Mount, run `touch`, `echo "x" > file`, `mkdir`, `rm`, `mv`, `cp`.

**Phase 3: Links and special files**
19. `Tsymlink`/`Rsymlink` — create symlinks (call `interceptor.pre_link`)
20. `Treadlink`/`Rreadlink` — read symlink targets
21. `Tlink`/`Rlink` — hard links (call `interceptor.pre_link`)
22. `Tmknod`/`Rmknod` — device nodes (return EPERM on non-Linux hosts)

**Test:** `ln -s`, `readlink`, `ln`.

**Phase 4: Robustness**
23. `Tflush`/`Rflush` — cancel in-flight requests
24. `Tlock`/`Rlock`, `Tgetlock`/`Rgetlock` — POSIX advisory locking
25. Error handling sweep — every operation returns correct errno values
26. Concurrent access — multiple fids to the same path

**Test:** Full development workflow: `git clone`, `npm install`, `cargo build`, `pytest`.

### A.6 Readdir Format

The `Rreaddir` data payload is a sequence of directory entries:

```
[13 bytes: qid]
[8 bytes:  offset (u64, opaque cookie for next read)]
[1 byte:   type (DT_DIR=4, DT_REG=8, DT_LNK=10, etc.)]
[2 bytes:  name length (u16)]
[N bytes:  name (UTF-8, no null terminator)]
```

Entries are packed contiguously. The server fills the buffer up to the requested `count` bytes. If an entry won't fit, stop and return what fits. The client will call `Treaddir` again with the `offset` from the last entry it received.

### A.7 Getattr Request/Response Mask

The `request_mask` in `Tgetattr` is a bitmask indicating which fields the client wants:

```
P9_GETATTR_MODE        = 0x00000001
P9_GETATTR_NLINK       = 0x00000002
P9_GETATTR_UID         = 0x00000004
P9_GETATTR_GID         = 0x00000008
P9_GETATTR_RDEV        = 0x00000010
P9_GETATTR_ATIME       = 0x00000020
P9_GETATTR_MTIME       = 0x00000040
P9_GETATTR_CTIME       = 0x00000080
P9_GETATTR_INO         = 0x00000100
P9_GETATTR_SIZE        = 0x00000200
P9_GETATTR_BLOCKS      = 0x00000400
P9_GETATTR_BTIME       = 0x00000800
P9_GETATTR_GEN         = 0x00001000
P9_GETATTR_DATA_VERSION = 0x00002000
P9_GETATTR_BASIC       = 0x000007ff  // All of the above minus btime, gen, data_version
P9_GETATTR_ALL         = 0x00003fff
```

The `valid` field in `Rgetattr` echoes which fields are actually populated in the response. Always return at least `P9_GETATTR_BASIC`.

### A.8 Setattr Valid Mask

```
P9_SETATTR_MODE        = 0x00000001
P9_SETATTR_UID         = 0x00000002
P9_SETATTR_GID         = 0x00000004
P9_SETATTR_SIZE        = 0x00000008
P9_SETATTR_ATIME       = 0x00000010
P9_SETATTR_MTIME       = 0x00000020
P9_SETATTR_CTIME       = 0x00000040  // set ctime to current time
P9_SETATTR_ATIME_SET   = 0x00000100  // use provided atime value
P9_SETATTR_MTIME_SET   = 0x00000200  // use provided mtime value
```

### A.9 Testing Strategy

1. **Unit tests for wire format:** Round-trip every message type through serialize → deserialize. Use known byte sequences from packet captures or crosvm test fixtures.
2. **Integration test with real mount:** Start the server, `mount -t 9p`, run filesystem operations, verify results. Automate with a script that mounts, operates, unmounts.
3. **Cross-platform metadata:** On Windows test hosts, verify that synthesized permissions, ownership, and symlinks appear correct inside the VM.
4. **Stress test:** `git clone` a large repo, `npm install` a project with deep `node_modules`, run a build. Compare results with a native filesystem.
5. **Crash test:** Kill the agent mid-write, restart, verify WAL recovery.

### A.10 Performance Notes

- **`msize` matters.** The default in many 9P clients is 8KB, which is catastrophically slow. Negotiate the largest `msize` both sides support (4MB+ is reasonable). Each `Tread`/`Twrite` can carry `msize - 24` bytes of payload.
- **Readdir batching.** Pack as many entries as fit in the response buffer. Small `Rreaddir` responses cause excessive round-trips on large directories.
- **Avoid unnecessary stat calls.** The `Tgetattr` request mask tells you exactly which fields the client needs. Skip expensive operations for fields not requested.
- **File handle caching.** Keep host file descriptors open between `Tlopen` and `Tclunk` rather than reopening on every `Tread`/`Twrite`.

---

## Appendix B: Unresolved Design Points

The following items need to be resolved before a coding agent can fully implement the system. They are listed roughly in order of how early they block implementation.

### B.1 Cargo Workspace Structure

The project needs a defined workspace layout: root `Cargo.toml`, member crates, edition, feature flags, and dependency versions. A recommended starting point:

```
sandbox-agent/
  Cargo.toml              -- workspace root
  crates/
    agent/                 -- host-side agent binary (depends on all below)
    interceptor/           -- WriteInterceptor trait + UndoInterceptor impl
    virtiofsd-fork/        -- forked virtiofsd with interception hooks + macOS compat layer (Linux + macOS)
    p9/                    -- 9P2000.L protocol + server (Windows only)
    control/               -- control channel protocol types and handler
    shim/                  -- VM-side shim (separate binary, minimal deps, built for x86_64 + aarch64)
    common/                -- shared types (undo log types, step types)
```

Decisions needed: Rust edition (2021 vs 2024), MSRV (minimum supported Rust version), whether crates should be publishable independently, how the virtiofsd fork is vendored (git subtree, submodule, or full copy).

### B.2 Undo Log Storage Format

Section 4.4 describes the behavior (preimage capture, compression, WAL). Preimage capture is the chosen approach for MVP — each step stores full copies of affected files before mutation.

Decisions needed:
- **Directory structure:** Where does the undo log live relative to the working folder? Recommended: `.sandbox/undo/` adjacent to the working folder, with `steps/{step_id}/` subdirectories.
- **Naming scheme:** One file per affected path within each step directory, with a manifest file listing all affected paths and their `existed_before` status? Or a single archive per step?
- **WAL layout:** The WAL uses a `wal/in_progress/` directory that is promoted to `steps/{step_id}/` on completion. What metadata file format for the step manifest (JSON, CBOR, bincode)?
- **Compression configuration:** zstd compression level (default 3 is a good speed/ratio trade-off). Per-file compression or archive-level?
- **Metadata capture format:** How to serialize mode bits, timestamps, and xattrs alongside file contents? A sidecar `.meta` file per preimage? A single manifest?
- **Reflink detection:** How to detect at runtime whether the filesystem supports `FICLONE` / `clonefile()` and fall back to copy + compression?
- **Evaluate SQLite:** Consider migrating from flat files to SQLite if query patterns (listing history, searching affected paths) become bottlenecks post-MVP.

### B.3 QEMU Configuration

The VM runtime is now decided: QEMU with `q35` machine type on Linux/Windows (KVM/WHPX), `virt` machine type on macOS Apple Silicon (HVF). All platforms use direct kernel boot. Configuration details that still need specification:

- **QEMU command-line templates per platform:** Exact flags for q35/virt machine types, virtio-fs or virtio-9p device, virtio-serial device, memory, CPUs, network, direct kernel boot. See Appendix C §C.7 for the Linux and macOS templates; Windows template needs derivation.
- **VM image:** What base image? Alpine (minimal, fast boot)? Debian (more packages available)? Custom minimal image? Must be built for both x86_64 (Linux/Windows hosts) and aarch64 (macOS hosts).
- **Kernel builds:** Two kernel configs needed — x86_64 and aarch64. Both should be minimal, enabling only virtio-fs, v9fs, virtio-serial, virtio-net, and essential subsystems.
- **How is the VM-side shim installed?** Baked into the image? Copied in at session start via a second virtio-fs share?
- **`session.start` implementation:** Exact QEMU invocation per platform. How are the vhost-user socket and virtio-serial socket paths managed?
- **Network policy enforcement:** QEMU user-mode networking + iptables in guest? QEMU tap device with host-side firewall rules?
- **Memory backend per platform:** `memory-backend-memfd` on Linux, `memory-backend-shm` on macOS. Windows uses QEMU's built-in 9P device (no vhost-user, so no shared memory backend needed for the filesystem channel).
- **HVF specifics:** `qemu-system-aarch64 -machine virt -accel hvf -cpu host` — verify virtio-fs-pci and virtio-serial-pci work with the `virt` machine's ecam PCI.
- **WHPX specifics:** `qemu-system-x86_64 -machine q35 -accel whpx` — verify virtio-9p-pci device works.

### B.4 STDIO API and MCP Server Transport (Resolved)

The STDIO API and MCP server use **separate transports**:

- **STDIO API** stays on the agent's stdin/stdout (good for IDE/frontend integration — the frontend spawns the agent as a child process).
- **MCP server** listens on a separate local transport: Unix domain socket (Linux/macOS) or named pipe (Windows).

This avoids protocol multiplexing complexity and makes debugging easier. The MCP socket path is printed to stderr on startup and configurable via `--mcp-socket`. The frontend can pass this path to the LLM client.

Remaining decisions:
- **Socket path convention:** Predictable paths (e.g., `/tmp/sandbox-{session_id}-mcp.sock`) or random? Predictable paths are easier for tooling but risk conflicts.
- **Authentication:** Should the MCP socket require a shared token for connection? Without it, any local process can connect.

### B.5 STDIO API and MCP Message Schemas

The tables in §4.5 and §4.6 list operations and fields, but there are no formal JSON schemas. A coding agent implementing these would need to invent field names, nesting, and types. Decisions needed:

- Formal JSON schema or TypeScript type definitions for every request, response, and event message.
- **Stable error taxonomy:** A structured error response format with numeric error `code`, human-readable `message`, and optional `data` field. Error codes should be organized by category (session errors, undo errors, filesystem errors, safeguard errors) and documented as a stable contract.
- **Protocol version negotiation:** The first message exchange between frontend and agent (and between LLM client and MCP server) should include version negotiation. The agent advertises its protocol version; the client can reject incompatible versions. This is critical for evolving the protocol without breaking existing integrations.
- **Event vs response ordering guarantees:** Define whether events can arrive interleaved with responses to requests, and whether the agent guarantees ordered delivery within each stream.

### B.6 Error Propagation Strategy

How errors flow through the system is not fully specified:

- **Host FS errors → filesystem backend:** For 9P: which `errno` values map to which host platform errors? Windows `GetLastError` codes don't map 1:1 to POSIX errnos. For virtiofsd: errors propagate naturally since it's Linux-to-Linux.
- **Filesystem errors → STDIO API:** When a write fails (e.g., permission denied), how is this surfaced to the frontend? As an `event.warning`? As part of `event.step_completed`?
- **Rust error types:** What is the internal error hierarchy? `thiserror` enums? `anyhow`?

### B.7 Platform Normalization Trait (9P Backend, Windows Only)

Section 5 describes *what* to normalize, and Appendix A §A.4 shows a `platform/` module, but the trait itself is not defined:

```rust
trait PlatformNormalizer {
    fn normalize_metadata(&self, meta: &std::fs::Metadata, path: &Path) -> Result<PosixAttrs>;
    fn supports_symlinks(&self) -> bool;
    fn is_case_sensitive(&self) -> bool;
    fn validate_filename(&self, name: &str) -> Result<()>;
    // ... what else?
}
```

Decisions needed: exact trait shape, what `PosixAttrs` contains. Since the 9P backend is now Windows-only, this trait can be a concrete struct rather than a polymorphic trait — there's only one implementation (Windows). However, keeping it as a trait may be useful if the 9P backend is ever used as a fallback on other platforms.

### B.8 VM-Side Shim Language and Packaging

The shim is described as "a few hundred lines" but:

- **Language:** Shell script (zero dependencies, but limited error handling)? Python (widely available in Linux VMs, better structured)? Compiled Rust/Go (fastest, but requires cross-compilation)?
- **Packaging:** Baked into the VM image? Copied in at session start? Mounted via a second read-only share?
- **Startup:** How does the shim start? systemd service? init script? Launched by the hypervisor?

---

## Appendix C: Forked virtiofsd Implementation Guide

This appendix provides enough detail for an LLM coding agent to create and maintain the forked virtiofsd used on Linux and macOS hosts. The fork adds write interception hooks to the official virtiofsd while preserving all existing functionality, and includes a macOS portability layer (§C.8) for the Linux-specific fd management APIs.

### C.1 Upstream Project

- **Repository:** `gitlab.com/virtio-fs/virtiofsd`
- **Language:** Rust
- **License:** Apache 2.0 (compatible with our project)
- **Key dependency:** `fuse-backend-rs` — provides the `FileSystem` trait and FUSE/vhost-user transport layers
- **Architecture:** virtiofsd implements the `FileSystem` trait via `PassthroughFs`, which is a FUSE passthrough filesystem that maps guest operations directly to host syscalls using `/proc/self/fd` and `O_PATH` file descriptors for security.

### C.2 Fork Strategy

**Approach: Thin wrapper, not deep modification.**

Rather than modifying `PassthroughFs` directly (which would create merge conflicts with upstream), create a **wrapper struct** that delegates to the original `PassthroughFs` and adds interception:

```rust
use fuse_backend_rs::api::filesystem::FileSystem;

/// Wraps the upstream PassthroughFs, adding write interception hooks.
pub struct InterceptedFs<F: FileSystem> {
    inner: F,
    interceptor: Arc<dyn WriteInterceptor>,
    inode_map: InodePathMap,  // Maps inode numbers to host paths
}

impl<F: FileSystem> FileSystem for InterceptedFs<F> {
    // Read operations: delegate directly
    fn read(&self, ctx, inode, handle, w, size, offset, ...) -> io::Result<usize> {
        self.inner.read(ctx, inode, handle, w, size, offset, ...)
    }

    fn getattr(&self, ctx, inode, handle) -> io::Result<(stat64, Duration)> {
        self.inner.getattr(ctx, inode, handle)
    }

    // Write operations: intercept, then delegate
    fn write(&self, ctx, inode, handle, r, size, offset, ...) -> io::Result<usize> {
        let path = self.inode_map.get(inode)?;
        self.interceptor.pre_write(&path)?;
        self.inner.write(ctx, inode, handle, r, size, offset, ...)
    }

    fn unlink(&self, ctx, parent, name) -> io::Result<()> {
        let path = self.inode_map.resolve(parent, name)?;
        self.interceptor.pre_unlink(&path, false)?;
        self.inner.unlink(ctx, parent, name)
    }

    fn rmdir(&self, ctx, parent, name) -> io::Result<()> {
        let path = self.inode_map.resolve(parent, name)?;
        self.interceptor.pre_unlink(&path, true)?;
        self.inner.rmdir(ctx, parent, name)
    }

    fn rename(&self, ctx, olddir, oldname, newdir, newname, flags) -> io::Result<()> {
        let old_path = self.inode_map.resolve(olddir, oldname)?;
        let new_path = self.inode_map.resolve(newdir, newname)?;
        self.interceptor.pre_rename(&old_path, &new_path)?;
        self.inner.rename(ctx, olddir, oldname, newdir, newname, flags)
    }

    fn create(&self, ctx, parent, name, args) -> io::Result<(Entry, Option<Handle>, ...)> {
        let result = self.inner.create(ctx, parent, name, args)?;
        let path = self.inode_map.resolve(parent, name)?;
        self.interceptor.post_create(&path)?;
        Ok(result)
    }

    fn mkdir(&self, ctx, parent, name, mode, umask) -> io::Result<Entry> {
        let result = self.inner.mkdir(ctx, parent, name, mode, umask)?;
        let path = self.inode_map.resolve(parent, name)?;
        self.interceptor.post_mkdir(&path)?;
        Ok(result)
    }

    fn setattr(&self, ctx, inode, attr, handle, valid) -> io::Result<(stat64, Duration)> {
        let path = self.inode_map.get(inode)?;
        self.interceptor.pre_setattr(&path)?;
        self.inner.setattr(ctx, inode, attr, handle, valid)
    }

    fn symlink(&self, ctx, linkname, parent, name) -> io::Result<Entry> {
        let result = self.inner.symlink(ctx, linkname, parent, name)?;
        let link_path = self.inode_map.resolve(parent, name)?;
        self.interceptor.post_symlink(&Path::new(linkname), &link_path)?;
        Ok(result)
    }

    fn link(&self, ctx, inode, newparent, newname) -> io::Result<Entry> {
        let target = self.inode_map.get(inode)?;
        let link_path = self.inode_map.resolve(newparent, newname)?;
        self.interceptor.pre_link(&target, &link_path)?;
        self.inner.link(ctx, inode, newparent, newname)
    }

    // ... delegate all other methods unchanged
}
```

### C.3 Inode-to-Path Mapping

The upstream `PassthroughFs` identifies files by inode number internally. The `WriteInterceptor` needs host filesystem paths. The `InodePathMap` bridges this gap:

```rust
/// Maps virtiofsd inode numbers to host filesystem paths.
/// Updated on lookup, create, mkdir, rename, unlink.
struct InodePathMap {
    map: RwLock<HashMap<Inode, PathBuf>>,
    root: PathBuf,
}

impl InodePathMap {
    /// Get the host path for an inode.
    fn get(&self, inode: Inode) -> io::Result<PathBuf> { ... }

    /// Resolve parent inode + child name to a host path.
    fn resolve(&self, parent: Inode, name: &CStr) -> io::Result<PathBuf> { ... }

    /// Update the map when a new entry is created or looked up.
    fn insert(&self, inode: Inode, path: PathBuf) { ... }

    /// Remove an entry on unlink/rmdir.
    fn remove(&self, inode: Inode) { ... }

    /// Update on rename.
    fn rename(&self, inode: Inode, new_path: PathBuf) { ... }
}
```

The map is populated lazily via `lookup` calls (which are the first thing the guest kernel does when accessing a path) and maintained through create/rename/unlink events.

**Alternative approach:** If maintaining a parallel inode map proves fragile, read the path from `/proc/self/fd/{fd}` via `readlink` on the file descriptors that `PassthroughFs` already holds. This is slower but always accurate. Profile before deciding.

### C.4 Methods Requiring Interception

The `FileSystem` trait has many methods. Only mutating operations need interception:

| Method | Interception | Hook |
|---|---|---|
| `write` | Pre-write | `interceptor.pre_write(path)` |
| `create` | Post-create | `interceptor.post_create(path)` |
| `mkdir` | Post-create | `interceptor.post_mkdir(path)` |
| `unlink` | Pre-delete | `interceptor.pre_unlink(path, false)` |
| `rmdir` | Pre-delete | `interceptor.pre_unlink(path, true)` |
| `rename` | Pre-rename | `interceptor.pre_rename(old, new)` |
| `setattr` | Pre-modify (truncate, chmod, etc.) | `interceptor.pre_setattr(path)` |
| `symlink` | Post-create | `interceptor.post_symlink(target, link_path)` |
| `link` | Pre-link | `interceptor.pre_link(target, link_path)` |
| `fallocate` | Pre-write (if modifying) | `interceptor.pre_fallocate(path)` |
| `setxattr` | Pre-modify | `interceptor.pre_xattr(path)` |
| `removexattr` | Pre-modify | `interceptor.pre_xattr(path)` |
| `open`/`create` with `O_TRUNC` | Pre-truncate | `interceptor.pre_open_trunc(path)` |
| `copy_file_range` | Pre-write (destination) | `interceptor.pre_copy_file_range(dst_path)` |

All read-only methods (`read`, `readdir`, `getattr`, `lookup`, `open`, `release`, `statfs`, `getxattr`, `listxattr`, etc.) delegate directly with no interception.

### C.5 Fork Maintenance Strategy

**Recommended: Pin to release tags, maintain as a patch series.**

1. Pin to a specific upstream release tag (e.g., `v1.12.0`).
2. Maintain the fork as a small set of well-documented patches on top.
3. The changes are structurally isolated: one new file (`intercepted_fs.rs`), one new file (`inode_map.rs`), one new directory (`compat/` for the macOS portability layer), and a small modification to `main.rs` to wrap `PassthroughFs` in `InterceptedFs`.
4. When upstream releases a new version, rebase the patches. Since the changes don't modify existing files heavily (the macOS layer wraps rather than replaces Linux calls), conflicts should be rare.
5. Subscribe to upstream security advisories. Cherry-pick security fixes between release updates if needed.

**Alternatively**, if the `fuse-backend-rs` `FileSystem` trait is stable enough, the `InterceptedFs` wrapper could live entirely in our workspace (in `crates/virtiofsd-fork/`) and depend on `virtiofsd` as a library rather than forking the binary. This depends on whether virtiofsd exposes `PassthroughFs` as a public API — check the crate's public interface before deciding.

### C.6 Build and Test

**Build (Linux):**
```bash
# From workspace root
cargo build --release -p virtiofsd-fork --target x86_64-unknown-linux-gnu
```

**Build (macOS):**
```bash
cargo build --release -p virtiofsd-fork --target aarch64-apple-darwin
```

**Test sequence (both platforms):**
1. **Smoke test:** Launch the forked virtiofsd, start QEMU with virtio-fs, mount inside guest, run `ls`, `cat`, `echo "test" > file`, `rm file`.
2. **Interception test:** Verify that `pre_write`, `pre_unlink`, `post_create` hooks fire correctly by logging to a test file and checking the log.
3. **Undo integration test:** Write a file via the guest, verify it appears in the undo log, rollback, verify the file is restored.
4. **Passthrough correctness:** Run the same stress tests as the 9P backend: `git clone`, `npm install`, `cargo build`, `pytest`. Compare results with a native filesystem.
5. **Performance comparison:** Benchmark against direct virtiofsd (no interception) to measure overhead of the wrapper. Target < 5% overhead for read-heavy workloads, < 15% for write-heavy workloads.

### C.7 virtiofsd Launch Configuration

The host-side agent launches the forked virtiofsd with:

```bash
virtiofsd-fork \
  --socket-path=/tmp/sandbox-virtiofs.sock \
  --shared-dir=/path/to/working-folder \
  --cache=never \
  --announce-submounts \
  --sandbox=chroot \
  --interceptor-socket=/tmp/sandbox-interceptor.sock  # custom flag for WriteInterceptor IPC
```

If the `InterceptedFs` wrapper is compiled into the same binary, the `--interceptor-socket` flag connects to the host-side agent's undo log system. If it's linked as a library, the interceptor is passed directly via the Rust API.

**Linux host (x86_64, KVM, q35):**

```bash
qemu-system-x86_64 \
  -machine q35 \
  -cpu host -accel kvm \
  -m 2G -smp 2 \
  -object memory-backend-memfd,id=mem,size=2G,share=on \
  -numa node,memdev=mem \
  -chardev socket,id=vfs,path=/tmp/sandbox-virtiofs.sock \
  -device vhost-user-fs-pci,chardev=vfs,tag=working \
  -chardev socket,id=ctrl,path=/tmp/sandbox-control.sock,server=on,wait=off \
  -device virtio-serial-pci \
  -device virtconsole,chardev=ctrl,name=control \
  -kernel /path/to/vmlinuz \
  -initrd /path/to/initrd.img \
  -append "console=hvc0 root=/dev/vda" \
  -drive file=/path/to/rootfs.img,format=raw,if=virtio \
  -netdev user,id=net0 \
  -device virtio-net-pci,netdev=net0 \
  -nographic
```

**macOS host (Apple Silicon, HVF, virt):**

```bash
qemu-system-aarch64 \
  -machine virt \
  -cpu host -accel hvf \
  -m 2G -smp 2 \
  -object memory-backend-shm,id=mem,size=2G,share=on \
  -numa node,memdev=mem \
  -chardev socket,id=vfs,path=/tmp/sandbox-virtiofs.sock \
  -device vhost-user-fs-pci,chardev=vfs,tag=working \
  -chardev socket,id=ctrl,path=/tmp/sandbox-control.sock,server=on,wait=off \
  -device virtio-serial-pci \
  -device virtconsole,chardev=ctrl,name=control \
  -kernel /path/to/Image \
  -initrd /path/to/initrd.img \
  -append "console=hvc0 root=/dev/vda" \
  -drive file=/path/to/rootfs-aarch64.img,format=raw,if=virtio \
  -netdev user,id=net0 \
  -device virtio-net-pci,netdev=net0 \
  -nographic
```

Note: macOS uses `memory-backend-shm` (POSIX `shm_open()`) instead of `memory-backend-memfd` (Linux-only). The kernel `Image` and `rootfs-aarch64.img` are aarch64 builds. The `virt` machine type provides PCI via its built-in ecam controller — `vhost-user-fs-pci` and `virtio-serial-pci` work without additional configuration.

Guest boot script or cloud-init mounts (identical on both platforms):
```bash
mount -t virtiofs -o cache=none working /mnt/working
```

Note: The exact QEMU flags will vary. The above are starting templates — see Appendix B §B.3 for the full list of configuration decisions. The Windows QEMU template (q35, WHPX, virtio-9p-pci instead of vhost-user-fs-pci) will be specified during Phase 3.

### C.8 macOS Portability Layer

The upstream virtiofsd depends on several Linux-specific APIs. Since we're already forking virtiofsd for the `InterceptedFs` wrapper, the macOS portability patches land in the same fork. The changes are isolated behind `#[cfg(target_os)]` attributes.

**Linux-specific APIs and their macOS equivalents:**

| Linux API | Where Used | macOS Equivalent | Notes |
|---|---|---|---|
| `/proc/self/fd/{n}` | Reopening fds by path for safe traversal | `fcntl(fd, F_GETPATH, buf)` to get path, then `open()` | macOS has no procfs; `F_GETPATH` returns the filesystem path for an open fd |
| `O_PATH` | Open-without-access fd handles for inodes | `open()` with `O_RDONLY \| O_NOFOLLOW` | macOS lacks `O_PATH`; `O_NOFOLLOW` prevents symlink traversal. Slightly different semantics (fd is readable, not just a handle) but safe within our sandboxed context |
| `renameat2(RENAME_EXCHANGE)` | Atomic swap of two paths | `renameatx_np(RENAME_SWAP)` | macOS provides equivalent via non-portable `renameatx_np()` |
| `renameat2(RENAME_NOREPLACE)` | Atomic rename-if-not-exists | `renameatx_np(RENAME_EXCL)` | macOS equivalent |
| `statx()` | Extended stat with birth time, mount id | `fstatat()` + `getattrlist()` for birth time | macOS `fstatat()` covers most fields; `getattrlist()` for `btime` |
| `mount_setattr()` / mount namespacing | `--sandbox=chroot` in virtiofsd | Not needed — the VM is our sandbox | virtiofsd's chroot/namespace sandboxing protects the host from the FUSE client (the guest). In our case, the guest is already isolated by the VM boundary. On macOS, skip the mount namespace setup entirely. |
| `memfd_create()` | Memory backend for vhost-user | `shm_open()` via QEMU's `memory-backend-shm` | This is a QEMU-side change, not a virtiofsd change. QEMU's Stefano Garzarella POSIX patches (merged ~9.1) already handle this. |
| `epoll` | Event loop for vhost-user socket | `kqueue` via the `mio` or `polling` Rust crates | virtiofsd uses Rust async/event-loop crates that already abstract over epoll/kqueue. Verify at fork time. |

**Implementation approach:**

1. **Create a `compat` module** in the fork with platform-abstracted wrappers:
   ```rust
   // src/compat/fd_ops.rs
   
   /// Get the filesystem path for an open file descriptor.
   #[cfg(target_os = "linux")]
   pub fn fd_to_path(fd: RawFd) -> io::Result<PathBuf> {
       std::fs::read_link(format!("/proc/self/fd/{}", fd))
   }
   
   #[cfg(target_os = "macos")]
   pub fn fd_to_path(fd: RawFd) -> io::Result<PathBuf> {
       let mut buf = vec![0u8; libc::PATH_MAX as usize];
       let ret = unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) };
       if ret == -1 {
           return Err(io::Error::last_os_error());
       }
       let nul_pos = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
       Ok(PathBuf::from(OsString::from_vec(buf[..nul_pos].to_vec())))
   }
   ```

2. **Replace direct syscalls** in `PassthroughFs` with calls to the `compat` module. This is a mechanical find-and-replace for each Linux API in the table above.

3. **Disable sandbox features on macOS:** The `--sandbox=chroot` mode uses Linux mount namespaces and pivot_root. On macOS, the sandbox flag is ignored (or set to `--sandbox=none`) since the VM boundary provides equivalent isolation.

4. **Build and test:** The fork's CI builds for both `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin`. The test suite runs the same POSIX compliance tests on both platforms (xfstests subset, pjdfstest).

**Estimated scope:** ~200-400 lines of platform abstraction code in the `compat` module, plus mechanical replacements in `PassthroughFs` call sites. The `InterceptedFs` wrapper (§C.2) is platform-independent and requires no macOS changes.

#### C.8.1 Symlink Escape Security Analysis

The VM is the primary security boundary, but the filesystem bridge is a deliberate hole in that boundary — it gives the guest direct read/write access to a host directory. A compromised guest could attempt symlink traversal attacks: creating a symlink inside `/mnt/working` that points to `../../etc/passwd` or other paths outside the shared directory.

**Linux hosts:** virtiofsd's `--sandbox=chroot` mode prevents symlink escape by chroot-ing into the shared directory. All path resolution happens within the chroot, so symlinks cannot reference paths outside it. Additionally, `PassthroughFs` uses `O_PATH` file descriptors and `/proc/self/fd` for safe traversal — it never follows symlinks naively.

**macOS hosts: additional risk.** The macOS portability layer (above) replaces `O_PATH` with `O_RDONLY|O_NOFOLLOW` and `/proc/self/fd` with `fcntl(F_GETPATH)`. This introduces subtly different semantics that require careful security analysis:

1. **`O_RDONLY|O_NOFOLLOW` vs `O_PATH`:** On Linux, `O_PATH` opens a handle to the filesystem object itself (symlink, file, or directory) without following it and without granting read/write access — it's purely a reference. On macOS, `O_RDONLY|O_NOFOLLOW` on a regular file opens it for reading (not just as a handle). On a symlink, `O_NOFOLLOW` causes `open()` to fail with `ELOOP`, which is the desired behavior — but this means the macOS code path cannot open a handle *to* a symlink the way `O_PATH` can on Linux. Any virtiofsd code path that opens a symlink via `O_PATH` to inspect it must be adapted on macOS to use `lstat()` or `readlink()` instead.

2. **`fcntl(F_GETPATH)` race condition:** On Linux, `/proc/self/fd/N` is an atomic kernel-level path that always reflects the current name of the open fd. On macOS, `fcntl(F_GETPATH)` returns the path at the time of the call, but the path could change between the `F_GETPATH` call and a subsequent operation. In the virtiofsd context, this race is mitigated because the fd is already open — operations use the fd directly, not the returned path. However, any code that uses `F_GETPATH` to *re-open* a file by path is vulnerable to TOCTOU attacks and must be avoided.

3. **No chroot on macOS:** §C.8 disables virtiofsd's sandbox mode on macOS because the VM boundary provides equivalent isolation. However, the filesystem bridge *is* the hole in the VM boundary. To prevent symlink escape without chroot, the macOS code path must enforce **path containment** — every resolved path must be verified to be within the shared directory. The recommended approach:
   - Before every operation, resolve the target path to a canonical absolute path (using the open fd, not a string path).
   - Verify the canonical path starts with the shared directory's canonical path.
   - Reject any operation where the resolved path escapes the shared directory.
   - Use `openat()` relative to the shared directory's fd wherever possible, which confines resolution to the directory subtree.

**Required before Phase 2 (macOS support):**
- A dedicated security review of every `PassthroughFs` code path that resolves paths, following the analysis above.
- A test suite of symlink escape attempts: symlinks pointing outside the shared directory, symlink chains, symlinks created by the guest pointing to absolute host paths, race conditions during path resolution.
- Document any residual risks that cannot be fully mitigated without chroot.
- **Additional containment layer:** Since macOS lacks chroot-equivalent containment for virtiofsd, add a second defense layer:
  - Run virtiofsd under a **macOS sandbox profile** (`sandbox-exec` / `sandbox_init`) that restricts file access to the shared directory and essential system paths.
  - Run virtiofsd as a **dedicated unprivileged user** with no access to user data outside the working folder.
  - Treat "guest can exploit virtiofsd" as a **real threat** — virtiofsd is exposed to untrusted input from the VM and must be hardened accordingly.
- **Case-collision detection:** Consider optional case-collision detection on macOS. A Linux guest (case-sensitive) can create `Foo` and `foo` as separate files; APFS (case-insensitive by default) treats them as the same file. This mismatch is more dangerous when the guest is case-sensitive than in native macOS development, and can cause subtle build failures or data loss. Detection can be added as an opt-in warning without requiring architectural changes.
