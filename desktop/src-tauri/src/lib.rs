mod commands;
mod config;
mod paths;

use commands::{claude, config as config_cmd, system, undo, vm};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            // Config commands
            config_cmd::read_config,
            config_cmd::write_config,
            config_cmd::get_config_path,
            // VM commands
            vm::start_vm,
            vm::stop_vm,
            vm::get_vm_status,
            // Claude commands
            claude::detect_claude_code_config,
            claude::write_claude_code_config,
            claude::remove_claude_code_config,
            claude::generate_claude_code_cli_command,
            claude::set_claude_code_denied_tools,
            claude::remove_claude_code_denied_tools,
            // System commands
            system::get_platform,
            system::get_cpu_count,
            system::get_default_undo_dir,
            system::resolve_binary,
            system::resolve_sandbox_binary,
            system::validate_directory,
            system::validate_paths_overlap,
            // Undo commands
            undo::read_undo_history,
            // VM MCP passthrough
            vm::send_mcp_request,
        ])
        .manage(vm::VmState::default())
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                claude::unregister_mcp_server();
            }
        });
}
