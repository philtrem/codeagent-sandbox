use std::sync::Arc;

use clap::Parser;
use tokio::sync::mpsc;

use codeagent_sandbox::cli::CliArgs;
use codeagent_sandbox::config::load_config;
use codeagent_sandbox::orchestrator::Orchestrator;

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();
    let config = load_config(args.config_file.as_deref());

    match args.protocol.as_str() {
        "mcp" => run_mcp(args, config).await,
        _ => run_stdio(args, config).await,
    }
}

async fn run_stdio(args: CliArgs, config: codeagent_sandbox::config::SandboxTomlConfig) {
    use codeagent_stdio::{Router, StdioServer};

    let (event_sender, event_receiver) = mpsc::unbounded_channel();
    let working_dir = args.working_dirs[0].clone();
    let orchestrator =
        Orchestrator::new(args, event_sender, config.command_classifier, config.file_watcher);

    let router = Router::new(working_dir, Box::new(orchestrator));
    let mut server = StdioServer::new(router, event_receiver);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let stderr = tokio::io::stderr();

    if let Err(e) = server.run(stdin, stdout, stderr).await {
        eprintln!("{{\"level\":\"error\",\"message\":\"{e}\"}}");
        std::process::exit(1);
    }
}

async fn run_mcp(args: CliArgs, config: codeagent_sandbox::config::SandboxTomlConfig) {
    use codeagent_mcp::{McpRouter, McpServer};
    use codeagent_stdio::protocol::SessionStartPayload;
    use codeagent_stdio::RequestHandler;

    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
    let working_dir = args.working_dirs[0].clone();
    let vm_mode = args.vm_mode.clone();
    let all_dirs: Vec<std::path::PathBuf> = args.working_dirs.clone();
    let socket_path = args.socket_path.clone();
    let log_file = args.log_file.clone();
    let builtin_tools_denied = args.disable_builtin_tools;
    let auto_allow_write = args.auto_allow_write_tools;
    let server_name = args.server_name.clone();
    let working_directories: Vec<_> = args
        .working_dirs
        .iter()
        .map(|d| codeagent_stdio::protocol::WorkingDirectoryConfig {
            path: d.display().to_string(),
            label: None,
        })
        .collect();
    let orchestrator =
        Orchestrator::new(args, event_sender, config.command_classifier, config.file_watcher);

    // MCP mode auto-starts the session from CLI args since MCP has no
    // session.start concept — the client expects tools to be ready immediately.
    let payload = SessionStartPayload {
        working_directories,
        vm_mode,
        network_policy: "disabled".to_string(),
        protocol_version: None,
    };
    if let Err(e) = orchestrator.session_start(payload) {
        eprintln!("{{\"level\":\"error\",\"message\":\"session auto-start failed: {e}\"}}");
        std::process::exit(1);
    }

    if builtin_tools_denied {
        codeagent_sandbox::claude_settings::deny_builtin_tools();
    }
    codeagent_sandbox::claude_settings::set_allowed_tools(&server_name, auto_allow_write);

    // Drain any events emitted during session start (e.g., VM launch warnings)
    // and log them to stderr so they're visible in diagnostic output.
    while let Ok(event) = event_receiver.try_recv() {
        match &event {
            codeagent_stdio::Event::Warning { code, message }
            | codeagent_stdio::Event::Error { code, message } => {
                eprintln!("{{\"level\":\"warn\",\"code\":\"{code}\",\"message\":\"{message}\"}}");
            }
            _ => {}
        }
    }

    // Wrap orchestrator in Arc for sharing between stdin/stdout and socket servers
    let orchestrator: Arc<dyn codeagent_mcp::McpHandler> = Arc::new(orchestrator);

    // If --log-file is set, open the file for tee-ing stderr output
    if let Some(ref path) = log_file {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Truncate on startup so the desktop always reads fresh output
        if let Ok(f) = std::fs::File::create(path) {
            drop(f);
        }
    }

    // If --socket-path is set, spawn the side-channel socket server
    let _socket_handle = if let Some(ref path) = socket_path {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handler = Arc::clone(&orchestrator);
        let root = working_dir.clone();
        let dirs = all_dirs.clone();
        let socket = path.clone();
        let handle = tokio::spawn(async move {
            codeagent_sandbox::socket_server::run_socket_server(
                socket,
                handler,
                root.clone(),
                dirs,
                shutdown_rx,
            )
            .await;
        });
        Some((handle, shutdown_tx))
    } else {
        None
    };

    let (_notification_sender, notification_receiver) = mpsc::unbounded_channel();
    let mcp_router =
        McpRouter::with_working_dirs(working_dir, &all_dirs, Arc::clone(&orchestrator));
    let mut server = McpServer::new(mcp_router, notification_receiver);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    if let Err(e) = server.run(stdin, stdout).await {
        eprintln!("{{\"level\":\"error\",\"message\":\"{e}\"}}");
        std::process::exit(1);
    }

    if builtin_tools_denied {
        codeagent_sandbox::claude_settings::restore_builtin_tools();
    }
    codeagent_sandbox::claude_settings::remove_allowed_tools(&server_name);

    // Shut down socket server when stdin/stdout server exits
    if let Some((handle, shutdown_tx)) = _socket_handle {
        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
    }
}
