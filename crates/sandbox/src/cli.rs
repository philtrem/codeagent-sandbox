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
}
