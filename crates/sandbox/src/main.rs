use clap::Parser;
use tokio::sync::mpsc;

use codeagent_sandbox::cli::CliArgs;
use codeagent_sandbox::orchestrator::Orchestrator;

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();

    match args.protocol.as_str() {
        "mcp" => run_mcp(args).await,
        _ => run_stdio(args).await,
    }
}

async fn run_stdio(args: CliArgs) {
    use codeagent_stdio::{Router, StdioServer};

    let (event_sender, event_receiver) = mpsc::unbounded_channel();
    let working_dir = args.working_dir.clone();
    let orchestrator = Orchestrator::new(args, event_sender);

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

async fn run_mcp(args: CliArgs) {
    use codeagent_mcp::{McpRouter, McpServer};
    use codeagent_stdio::protocol::SessionStartPayload;
    use codeagent_stdio::RequestHandler;

    let (event_sender, _event_receiver) = mpsc::unbounded_channel();
    let working_dir = args.working_dir.clone();
    let vm_mode = args.vm_mode.clone();
    let orchestrator = Orchestrator::new(args, event_sender);

    // MCP mode auto-starts the session from CLI args since MCP has no
    // session.start concept — the client expects tools to be ready immediately.
    let payload = SessionStartPayload {
        working_directories: vec![],
        vm_mode,
        network_policy: "disabled".to_string(),
        protocol_version: None,
    };
    if let Err(e) = orchestrator.session_start(payload) {
        eprintln!("{{\"level\":\"error\",\"message\":\"session auto-start failed: {e}\"}}");
        std::process::exit(1);
    }

    let (_notification_sender, notification_receiver) = mpsc::unbounded_channel();
    let mcp_router = McpRouter::new(working_dir, Box::new(orchestrator));
    let mut server = McpServer::new(mcp_router, notification_receiver);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    if let Err(e) = server.run(stdin, stdout).await {
        eprintln!("{{\"level\":\"error\",\"message\":\"{e}\"}}");
        std::process::exit(1);
    }
}
