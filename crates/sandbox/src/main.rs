use clap::Parser;
use tokio::sync::mpsc;

use codeagent_sandbox::cli::CliArgs;
use codeagent_sandbox::orchestrator::Orchestrator;
use codeagent_stdio::{Router, StdioServer};

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();

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
