use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "sandbox", about = "Sandboxed coding agent host")]
pub struct CliArgs {
    /// Host directory shared with the guest VM as the primary working directory.
    #[arg(long)]
    pub working_dir: PathBuf,

    /// Directory for storing undo logs and preimages.
    #[arg(long)]
    pub undo_dir: PathBuf,

    /// VM lifecycle mode: "ephemeral" (destroyed on stop) or "persistent" (kept alive).
    #[arg(long, default_value = "ephemeral")]
    pub vm_mode: String,

    /// Path for the MCP server socket. If omitted, no MCP server is started.
    #[arg(long)]
    pub mcp_socket: Option<PathBuf>,

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
    #[arg(long, default_value = "2048")]
    pub memory_mb: u32,

    /// Number of virtual CPUs.
    #[arg(long, default_value = "2")]
    pub cpus: u32,

    /// Path to the virtiofsd binary (overrides auto-detection).
    #[arg(long)]
    pub virtiofsd_binary: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_args_parse() {
        let args = CliArgs::try_parse_from([
            "sandbox",
            "--working-dir",
            "/tmp/work",
            "--undo-dir",
            "/tmp/undo",
        ])
        .unwrap();
        assert_eq!(args.working_dir, PathBuf::from("/tmp/work"));
        assert_eq!(args.undo_dir, PathBuf::from("/tmp/undo"));
        assert_eq!(args.vm_mode, "ephemeral");
        assert!(args.mcp_socket.is_none());
        assert_eq!(args.log_level, "info");
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
            "--mcp-socket",
            "/tmp/mcp.sock",
            "--log-level",
            "debug",
        ])
        .unwrap();
        assert_eq!(args.vm_mode, "persistent");
        assert_eq!(
            args.mcp_socket,
            Some(PathBuf::from("/tmp/mcp.sock"))
        );
        assert_eq!(args.log_level, "debug");
    }

    #[test]
    fn missing_required_args_fails() {
        let result = CliArgs::try_parse_from(["sandbox"]);
        assert!(result.is_err());
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
        assert_eq!(args.memory_mb, 2048);
        assert_eq!(args.cpus, 2);
        assert!(args.virtiofsd_binary.is_none());
    }
}
