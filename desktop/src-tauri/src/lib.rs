mod commands;
mod config;
mod paths;

use commands::{claude, config as config_cmd, system, undo, vm};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Kill any orphaned sandbox.exe left over from a previous crash (manual mode only)
    vm::kill_orphaned_sandbox();

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
            claude::generate_claude_code_cli_command,
            // System commands
            system::get_platform,
            system::get_cpu_count,
            system::get_default_undo_dir,
            system::resolve_binary,
            system::resolve_sandbox_binary,
            system::validate_directory,
            system::validate_paths_overlap,
            system::ensure_directory,
            // Undo commands
            undo::read_undo_history,
            undo::clear_undo_history,
            // VM MCP passthrough
            vm::send_mcp_request,
            // Socket connection (MCP mode)
            vm::connect_to_sandbox,
            vm::disconnect_from_sandbox,
            // Terminal + Debug console
            vm::get_debug_log,
            vm::clear_debug_log,
            vm::execute_terminal_command,
        ])
        .manage(vm::VmState::default())
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Clean up any VM process we spawned (manual mode)
                if let Some(vm_state) = app.try_state::<vm::VmState>() {
                    if let Ok(mut guard) = vm_state.process.lock() {
                        if let Some(mut child) = guard.take() {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                    }
                }
                if let Some(pid_path) = paths::pid_file_path() {
                    let _ = std::fs::remove_file(&pid_path);
                }

            }
        });
}
