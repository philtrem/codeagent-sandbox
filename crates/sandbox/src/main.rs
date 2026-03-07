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
    let orchestrator = Orchestrator::new(args, event_sender, config.command_classifier, config.file_watcher);

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
    let working_directories: Vec<_> = args
        .working_dirs
        .iter()
        .map(|d| codeagent_stdio::protocol::WorkingDirectoryConfig {
            path: d.display().to_string(),
            label: None,
        })
        .collect();
    let orchestrator = Orchestrator::new(args, event_sender, config.command_classifier, config.file_watcher);

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

    let (_notification_sender, notification_receiver) = mpsc::unbounded_channel();
    let mcp_router = McpRouter::with_working_dirs(working_dir, &all_dirs, Box::new(orchestrator));
    let mut server = McpServer::new(mcp_router, notification_receiver);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    if let Err(e) = server.run(stdin, stdout).await {
        eprintln!("{{\"level\":\"error\",\"message\":\"{e}\"}}");
        std::process::exit(1);
    }
}
