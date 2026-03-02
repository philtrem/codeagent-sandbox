# WinFsp/FUSE Host Mount — Standalone Binary

## Overview

A separate binary (not part of the sandbox/VM) that exposes a working directory as a
local mount point on the host. All filesystem I/O through the mount goes through the
`WriteInterceptor` for full undo tracking — no VM required.

When used with Claude Code Desktop, the user selects the mount point as their project
folder. All native file tools (Read, Write, Edit, Glob, Grep, Bash file access) are
transparently intercepted for undo tracking.

## Architecture

```
Claude Code Desktop
    │
    ├── Read/Write/Edit/Glob/Grep  ──→  mount point (S:\project or /mnt/project)
    │                                         │
    │                                    WinFsp / FUSE driver
    │                                         │
    │                                    WriteInterceptor hooks
    │                                    (pre_write, pre_unlink, etc.)
    │                                         │
    │                                    Real host filesystem
    │                                    (C:\Projects\my-app)
    │
    └── MCP execute_command  ──→  (optional) sandbox VM for execution
```

### Components

- **WinFsp** (Windows): User-mode filesystem driver. Our binary implements the WinFsp
  callback interface, translating filesystem operations to real host paths while calling
  WriteInterceptor pre/post hooks.
- **libfuse** (Linux/macOS): FUSE low-level or high-level API. Same pattern — intercept
  mutating operations, delegate reads directly.
- **UndoInterceptor**: Reused directly from `codeagent-interceptor`. Same preimage capture,
  same manifest format, same rollback algorithm. The undo log lives in a separate directory
  (specified via CLI flag).

### What gets intercepted

Mutating operations go through WriteInterceptor hooks:
- `create` / `open(O_CREAT)` → `pre_create` / `post_create`
- `write` → `pre_write` / `post_write`
- `unlink` → `pre_unlink` / `post_unlink`
- `rename` → `pre_rename` / `post_rename`
- `mkdir` → `pre_mkdir` / `post_mkdir`
- `rmdir` → `pre_rmdir` / `post_rmdir`
- `truncate` → `pre_open_trunc` / `post_open_trunc`
- `setattr` → `pre_setattr` / `post_setattr`
- `symlink` / `link` → `pre_symlink` / `pre_link`

Read-only operations pass through directly with no overhead:
- `read`, `readdir`, `stat`, `getattr`, `readlink`

### Step boundaries

Without a VM control channel, step boundaries need a different trigger. Options:
1. **Time-based ambient steps**: Similar to the control channel's ambient step logic —
   group writes within an inactivity window into a single step (e.g., 5s idle = close step).
2. **MCP-triggered**: If the sandbox MCP server is also running, each MCP `write_file` /
   `edit_file` / `execute_command` call opens/closes an explicit step.
3. **Manual**: Expose an undo CLI or small HTTP API for the user to mark step boundaries.

## Binary design

```
crates/
  mount/                           # codeagent-mount — standalone mount binary
    Cargo.toml                     # depends on codeagent-interceptor, winfsp-rs / fuser
    src/
      main.rs                      # CLI: --source-dir, --mount-point, --undo-dir
      lib.rs                       # module declarations
      winfsp_fs.rs                 # [cfg(windows)] WinFsp filesystem implementation
      fuse_fs.rs                   # [cfg(unix)] FUSE filesystem implementation
      step_manager.rs              # ambient step management (time-based)
```

### CLI

```
codeagent-mount \
  --source-dir C:\Projects\my-app \
  --mount-point S: \
  --undo-dir C:\tmp\mount-undo
```

### Dependencies

- **Windows**: `winfsp-rs` crate (Rust bindings for WinFsp, requires WinFsp installed)
- **Linux/macOS**: `fuser` crate (Rust FUSE bindings, requires libfuse/macfuse installed)
- `codeagent-interceptor` (existing) — WriteInterceptor + UndoInterceptor
- `codeagent-common` (existing) — shared types

## Relationship to sandbox VM

This binary is **complementary** to the VM-based sandbox:

| Feature | Mount binary | Sandbox VM |
|---|---|---|
| File undo tracking | Yes (via mount intercept) | Yes (via virtiofsd/9P intercept) |
| Execution sandboxing | No (host execution) | Yes (Linux VM isolation) |
| Claude Code integration | Native tools work | MCP tools only |
| Setup complexity | Low (just mount) | High (QEMU + guest image) |

Users can run both together: mount for file tracking + sandbox MCP for sandboxed execution.
Or mount alone for lightweight undo-only usage without VM overhead.
