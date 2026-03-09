use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "sandbox", about = "Sandboxed coding agent host")]
pub struct CliArgs {
    /// Host directories shared with the guest VM.
    /// If not provided, falls back to `[sandbox].working_dirs` in `codeagent.toml`.
    #[arg(long = "working-dir")]
    pub working_dirs: Vec<PathBuf>,

    /// Directory for storing undo logs and preimages.
    /// If not provided, falls back to `[sandbox].undo_dir` in `codeagent.toml`.
    #[arg(long)]
    pub undo_dir: Option<PathBuf>,

    /// VM lifecycle mode: "ephemeral" (destroyed on stop) or "persistent" (kept alive).
    #[arg(long, default_value = "ephemeral")]
    pub vm_mode: String,

    /// Protocol for the stdin/stdout API: "stdio" (JSON Lines) or "mcp" (JSON-RPC 2.0).
    #[arg(long, default_value = "stdio")]
    pub protocol: String,

    /// Logging level for stderr structured logs.
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Path to the QEMU binary (overrides auto-detection).
    #[arg(long)]
    pub qemu_binary: Option<PathBuf>,

    /// Path to the guest kernel image (vmlinuz or Image).
    #[arg(long)]
    pub kernel_path: Option<PathBuf>,

    /// Path to the guest initrd image.
    #[arg(long)]
    pub initrd_path: Option<PathBuf>,

    /// Path to the guest rootfs image (optional disk image).
    #[arg(long)]
    pub rootfs_path: Option<PathBuf>,

    /// VM memory in megabytes.
    #[arg(long, default_value = "512")]
    pub memory_mb: u32,

    /// Number of virtual CPUs.
    #[arg(long, default_value = "2")]
    pub cpus: u32,

    /// Path to the virtiofsd binary (overrides auto-detection).
    #[arg(long)]
    pub virtiofsd_binary: Option<PathBuf>,

    /// Path to a TOML configuration file.
    /// If not specified, the platform default path is used
    /// (`{config_dir}/CodeAgent/codeagent.toml`).
    #[arg(long)]
    pub config_file: Option<PathBuf>,

    /// Path to a Unix domain socket for side-channel access from the desktop app.
    /// When set, the sandbox listens on this socket for additional MCP connections.
    #[arg(long)]
    pub socket_path: Option<PathBuf>,

    /// Path to a log file. When set, stderr output is also teed to this file.
    #[arg(long)]
    pub log_file: Option<PathBuf>,

    /// Block Claude Code's built-in file/command tools (Read, Edit, Write, Glob,
    /// Grep, Bash) while the sandbox is running, restoring them on exit.
    #[arg(long)]
    pub disable_builtin_tools: bool,

    /// Auto-allow MCP write tools (Bash, write_file, edit_file, undo) in
    /// Claude Code's permissions while the sandbox is running.
    #[arg(long)]
    pub auto_allow_write_tools: bool,

    /// MCP server name used for registering allowed-tool entries in Claude Code
    /// (e.g. `MCP(codeagent-sandbox:read_file)`).
    #[arg(long, default_value = "codeagent-sandbox")]
    pub server_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_args_parse() {
        let args = CliArgs::try_parse_from([
            "sandbox",
            "--working-dir",
            "/tmp/work",
            "--undo-dir",
            "/tmp/undo",
        ])
        .unwrap();
        assert_eq!(args.working_dirs, vec![PathBuf::from("/tmp/work")]);
        assert_eq!(args.undo_dir, Some(PathBuf::from("/tmp/undo")));
        assert_eq!(args.vm_mode, "ephemeral");
        assert_eq!(args.protocol, "stdio");
        assert_eq!(args.log_level, "info");
        assert!(args.socket_path.is_none());
        assert!(args.log_file.is_none());
    }

    #[test]
    fn socket_and_log_file_parse() {
        let args = CliArgs::try_parse_from([
            "sandbox",
            "--working-dir",
            "/tmp/work",
            "--undo-dir",
            "/tmp/undo",
            "--socket-path",
            "/tmp/mcp.sock",
            "--log-file",
            "/tmp/sandbox.log",
        ])
        .unwrap();
        assert_eq!(args.socket_path, Some(PathBuf::from("/tmp/mcp.sock")));
        assert_eq!(args.log_file, Some(PathBuf::from("/tmp/sandbox.log")));
    }

    #[test]
    fn multiple_working_dirs_parse() {
        let args = CliArgs::try_parse_from([
            "sandbox",
            "--working-dir",
            "/tmp/work1",
            "--working-dir",
            "/tmp/work2",
            "--undo-dir",
            "/tmp/undo",
        ])
        .unwrap();
        assert_eq!(
            args.working_dirs,
            vec![PathBuf::from("/tmp/work1"), PathBuf::from("/tmp/work2")]
        );
    }

    #[test]
    fn all_args_parse() {
        let args = CliArgs::try_parse_from([
            "sandbox",
            "--working-dir",
            "/tmp/work",
            "--undo-dir",
            "/tmp/undo",
            "--vm-mode",
            "persistent",
            "--protocol",
            "mcp",
            "--log-level",
            "debug",
        ])
        .unwrap();
        assert_eq!(args.vm_mode, "persistent");
        assert_eq!(args.protocol, "mcp");
        assert_eq!(args.log_level, "debug");
    }

    #[test]
    fn no_args_uses_defaults() {
        let args = CliArgs::try_parse_from(["sandbox"]).unwrap();
        assert!(args.working_dirs.is_empty());
        assert!(args.undo_dir.is_none());
        assert_eq!(args.vm_mode, "ephemeral");
        assert_eq!(args.protocol, "stdio");
    }

    #[test]
    fn qemu_args_parse() {
        let args = CliArgs::try_parse_from([
            "sandbox",
            "--working-dir",
            "/tmp/work",
            "--undo-dir",
            "/tmp/undo",
            "--qemu-binary",
            "/usr/bin/qemu-system-x86_64",
            "--kernel-path",
            "/boot/vmlinuz",
            "--initrd-path",
            "/boot/initrd.img",
            "--rootfs-path",
            "/boot/rootfs.img",
            "--memory-mb",
            "4096",
            "--cpus",
            "4",
            "--virtiofsd-binary",
            "/usr/libexec/virtiofsd",
        ])
        .unwrap();
        assert_eq!(
            args.qemu_binary,
            Some(PathBuf::from("/usr/bin/qemu-system-x86_64"))
        );
        assert_eq!(args.kernel_path, Some(PathBuf::from("/boot/vmlinuz")));
        assert_eq!(args.initrd_path, Some(PathBuf::from("/boot/initrd.img")));
        assert_eq!(args.rootfs_path, Some(PathBuf::from("/boot/rootfs.img")));
        assert_eq!(args.memory_mb, 4096);
        assert_eq!(args.cpus, 4);
        assert_eq!(
            args.virtiofsd_binary,
            Some(PathBuf::from("/usr/libexec/virtiofsd"))
        );
    }

    #[test]
    fn qemu_args_have_defaults() {
        let args = CliArgs::try_parse_from([
            "sandbox",
            "--working-dir",
            "/tmp/work",
            "--undo-dir",
            "/tmp/undo",
        ])
        .unwrap();
        assert!(args.qemu_binary.is_none());
        assert!(args.kernel_path.is_none());
        assert!(args.initrd_path.is_none());
        assert!(args.rootfs_path.is_none());
        assert_eq!(args.memory_mb, 512);
        assert_eq!(args.cpus, 2);
        assert!(args.virtiofsd_binary.is_none());
    }
}
