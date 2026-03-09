use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use tokio::sync::mpsc;

use codeagent_sandbox::cli::CliArgs;
use codeagent_sandbox::config::{load_config, SandboxTomlConfig};
use codeagent_sandbox::orchestrator::Orchestrator;
use codeagent_sandbox::tray::{TrayCommand, TrayConfig, TrayUpdate};

fn main() {
    let _instance_lock = match codeagent_sandbox::singleton::try_acquire_instance_lock() {
        Ok(lock) => lock,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };

    let mut args = CliArgs::parse();
    let config = load_config(args.config_file.as_deref());

    // Merge CLI args with TOML config: CLI overrides TOML.
    if args.working_dirs.is_empty() && !config.sandbox.working_dirs.is_empty() {
        args.working_dirs = config
            .sandbox
            .working_dirs
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
    }
    if args.undo_dir.is_none() && !config.sandbox.undo_dir.is_empty() {
        args.undo_dir = Some(std::path::PathBuf::from(&config.sandbox.undo_dir));
    }
    if args.undo_dir.is_none() {
        if let Some(data_dir) = dirs::data_local_dir() {
            args.undo_dir = Some(data_dir.join("CodeAgent").join("undo"));
        }
    }

    if args.working_dirs.is_empty() {
        eprintln!("{{\"level\":\"error\",\"message\":\"No working directories specified. \
            Provide --working-dir or set [sandbox].working_dirs in codeagent.toml.\"}}");
        std::process::exit(1);
    }

    match args.protocol.as_str() {
        "mcp" => {
            if codeagent_sandbox::tray::should_show_tray() {
                run_mcp_with_tray(args, config);
            } else {
                let rt = tokio::runtime::Runtime::new()
                    .expect("failed to create tokio runtime");
                rt.block_on(run_mcp(args, config, None, None));
            }
        }
        _ => {
            let rt =
                tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            rt.block_on(run_stdio(args, config));
        }
    }
}

fn run_mcp_with_tray(args: CliArgs, config: SandboxTomlConfig) {
    let (tray_cmd_tx, tray_cmd_rx) = tokio::sync::mpsc::unbounded_channel::<TrayCommand>();
    let (tray_update_tx, tray_update_rx) = std::sync::mpsc::channel::<TrayUpdate>();

    let tray_config = TrayConfig {
        working_dir: args.working_dirs[0].display().to_string(),
        initial_disable_builtin: args.disable_builtin_tools,
        initial_auto_allow_write: args.auto_allow_write_tools,
    };

    let server_thread = std::thread::spawn(move || {
        let rt =
            tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(run_mcp(args, config, Some(tray_cmd_rx), Some(tray_update_tx)));
    });

    codeagent_sandbox::tray::run_tray(tray_config, tray_cmd_tx, tray_update_rx);

    let _ = server_thread.join();
}

async fn run_stdio(args: CliArgs, config: SandboxTomlConfig) {
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

async fn run_mcp(
    args: CliArgs,
    config: SandboxTomlConfig,
    tray_cmd_rx: Option<mpsc::UnboundedReceiver<TrayCommand>>,
    tray_update_tx: Option<std::sync::mpsc::Sender<TrayUpdate>>,
) {
    use codeagent_mcp::{McpRouter, McpServer};
    use codeagent_stdio::protocol::SessionStartPayload;
    use codeagent_stdio::RequestHandler;

    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
    let working_dir = args.working_dirs[0].clone();
    let vm_mode = args.vm_mode.clone();
    let all_dirs: Vec<std::path::PathBuf> = args.working_dirs.clone();
    let socket_path = args
        .socket_path
        .clone()
        .or_else(|| codeagent_sandbox::config::default_config_dir().map(|d| d.join("mcp.sock")));
    let log_file = args
        .log_file
        .clone()
        .or_else(|| codeagent_sandbox::config::default_config_dir().map(|d| d.join("sandbox.log")));
    let server_name = args.server_name.clone();

    // Track toggle states with atomics so the tray command handler and
    // cleanup code can share them across tasks.
    let builtin_denied = Arc::new(AtomicBool::new(args.disable_builtin_tools));
    let auto_allow_enabled = Arc::new(AtomicBool::new(args.auto_allow_write_tools));

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

    codeagent_sandbox::claude_settings::apply_startup_settings(
        &server_name,
        builtin_denied.load(Ordering::Relaxed),
        auto_allow_enabled.load(Ordering::Relaxed),
    );

    // Drain any events emitted during session start (e.g., VM launch warnings)
    // and log them to stderr so they're visible in diagnostic output.
    while let Ok(event) = event_receiver.try_recv() {
        match &event {
            codeagent_stdio::Event::Warning { code, message }
            | codeagent_stdio::Event::Error { code, message } => {
                eprintln!(
                    "{{\"level\":\"warn\",\"code\":\"{code}\",\"message\":\"{message}\"}}"
                );
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

    // Spawn tray command handler if tray is active
    if let Some(mut cmd_rx) = tray_cmd_rx {
        let denied = Arc::clone(&builtin_denied);
        let allow = Arc::clone(&auto_allow_enabled);
        let name = server_name.clone();
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    TrayCommand::ToggleBuiltinTools(enabled) => {
                        if enabled {
                            codeagent_sandbox::claude_settings::deny_builtin_tools();
                        } else {
                            codeagent_sandbox::claude_settings::restore_builtin_tools();
                        }
                        denied.store(enabled, Ordering::Relaxed);
                    }
                    TrayCommand::ToggleAutoAllowWrite(enabled) => {
                        codeagent_sandbox::claude_settings::set_allowed_tools(
                            &name, enabled,
                        );
                        allow.store(enabled, Ordering::Relaxed);
                    }
                    TrayCommand::OpenDesktopApp => {
                        codeagent_sandbox::tray::open_desktop_app();
                    }
                }
            }
        });
    }

    let (_notification_sender, notification_receiver) = mpsc::unbounded_channel();
    let mcp_router =
        McpRouter::with_working_dirs(working_dir, &all_dirs, Arc::clone(&orchestrator));
    let mut server = McpServer::new(mcp_router, notification_receiver);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let server_result = server.run(stdin, stdout).await;

    if let Err(ref e) = server_result {
        eprintln!("{{\"level\":\"error\",\"message\":\"{e}\"}}");
    }

    // Always restore Claude settings, even if the server exited with an error
    // (e.g. broken pipe when Claude Code terminates). Uses a single file write
    // to minimize Claude Code's file watcher reload overhead.
    codeagent_sandbox::claude_settings::apply_shutdown_settings(
        &server_name,
        builtin_denied.load(Ordering::Relaxed),
    );

    // Signal tray to exit
    if let Some(tx) = tray_update_tx {
        let _ = tx.send(TrayUpdate::Shutdown);
    }

    // Shut down socket server when stdin/stdout server exits
    if let Some((handle, shutdown_tx)) = _socket_handle {
        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
    }

    if server_result.is_err() {
        std::process::exit(1);
    }
}
