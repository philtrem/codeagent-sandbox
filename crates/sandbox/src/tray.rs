//! System tray icon for the sandbox binary (MCP mode only).
//!
//! Shows a tray icon with status info and runtime controls:
//! - Toggle "disable built-in tools"
//! - Toggle "auto-allow write tools"
//! - Open desktop app
//! - Quit

use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// Commands sent from the tray UI to the server.
pub enum TrayCommand {
    ToggleBuiltinTools(bool),
    ToggleAutoAllowWrite(bool),
    OpenDesktopApp,
}

/// Updates sent from the server to the tray UI.
pub enum TrayUpdate {
    StatusChanged(String),
    ConfigChanged {
        disable_builtin_tools: bool,
        auto_allow_write: bool,
    },
    Shutdown,
}

/// Initial configuration for the tray icon.
pub struct TrayConfig {
    pub working_dir: String,
    pub initial_disable_builtin: bool,
    pub initial_auto_allow_write: bool,
}

/// Run the tray icon event loop on the current thread.
///
/// Blocks until the server sends `TrayUpdate::Shutdown` or the user clicks
/// "Close Tray". Closing the tray does NOT stop the MCP server — it continues
/// running until stdin closes (i.e., until Claude Code disconnects).
pub fn run_tray(
    config: TrayConfig,
    command_tx: tokio::sync::mpsc::UnboundedSender<TrayCommand>,
    update_rx: std_mpsc::Receiver<TrayUpdate>,
) {
    let icon = create_icon();
    let menu = Menu::new();

    let status_item = MenuItem::new("Status: Running", false, None);
    let dir_item = MenuItem::new(&config.working_dir, false, None);
    let disable_builtin = CheckMenuItem::new(
        "Disable built-in tools",
        true,
        config.initial_disable_builtin,
        None,
    );
    let auto_allow = CheckMenuItem::new(
        "Auto-allow write tools",
        true,
        config.initial_auto_allow_write,
        None,
    );
    let open_desktop = MenuItem::new("Open Desktop App", true, None);
    let close_tray_item = MenuItem::new("Close Tray", true, None);

    let _ = menu.append_items(&[
        &status_item,
        &dir_item,
        &PredefinedMenuItem::separator(),
        &disable_builtin,
        &auto_allow,
        &PredefinedMenuItem::separator(),
        &open_desktop,
        &PredefinedMenuItem::separator(),
        &close_tray_item,
    ]);

    let _tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("CodeAgent Sandbox")
        .with_icon(icon)
        .build()
    {
        Ok(tray) => tray,
        Err(e) => {
            eprintln!(
                "{{\"level\":\"warn\",\"message\":\"failed to create tray icon: {e}\"}}"
            );
            wait_for_shutdown(&update_rx);
            return;
        }
    };

    // Cache menu item IDs for event matching
    let disable_builtin_id = disable_builtin.id().clone();
    let auto_allow_id = auto_allow.id().clone();
    let open_desktop_id = open_desktop.id().clone();
    let close_tray_id = close_tray_item.id().clone();

    let menu_rx = MenuEvent::receiver();

    loop {
        #[cfg(target_os = "windows")]
        pump_windows_messages();

        while let Ok(event) = menu_rx.try_recv() {
            if event.id == disable_builtin_id {
                let checked = disable_builtin.is_checked();
                let _ = command_tx.send(TrayCommand::ToggleBuiltinTools(checked));
            } else if event.id == auto_allow_id {
                let checked = auto_allow.is_checked();
                let _ = command_tx.send(TrayCommand::ToggleAutoAllowWrite(checked));
            } else if event.id == open_desktop_id {
                let _ = command_tx.send(TrayCommand::OpenDesktopApp);
            } else if event.id == close_tray_id {
                return;
            }
        }

        while let Ok(update) = update_rx.try_recv() {
            match update {
                TrayUpdate::Shutdown => return,
                TrayUpdate::StatusChanged(status) => {
                    status_item.set_text(format!("Status: {status}"));
                }
                TrayUpdate::ConfigChanged {
                    disable_builtin_tools,
                    auto_allow_write,
                } => {
                    disable_builtin.set_checked(disable_builtin_tools);
                    auto_allow.set_checked(auto_allow_write);
                }
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait for a shutdown signal without a tray icon (fallback when tray creation fails).
fn wait_for_shutdown(update_rx: &std_mpsc::Receiver<TrayUpdate>) {
    loop {
        match update_rx.recv() {
            Ok(TrayUpdate::Shutdown) | Err(_) => return,
            _ => {}
        }
    }
}

/// Create a 32x32 blue circle icon for the system tray.
fn create_icon() -> Icon {
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let center = size as f32 / 2.0;
    let radius = 13.0f32;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= radius {
                let idx = ((y * size + x) * 4) as usize;
                rgba[idx] = 0x3B; // R
                rgba[idx + 1] = 0x82; // G
                rgba[idx + 2] = 0xF6; // B
                rgba[idx + 3] = 0xFF; // A
            }
        }
    }

    Icon::from_rgba(rgba, size, size).expect("failed to create tray icon")
}

/// Process pending Windows messages so the tray icon's hidden window receives events.
#[cfg(target_os = "windows")]
fn pump_windows_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, PM_REMOVE,
    };
    unsafe {
        let mut msg = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Check whether a tray icon should be shown.
///
/// Returns `false` on Linux when no display server is available (headless).
pub fn should_show_tray() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// Try to launch the desktop app.
///
/// Searches for the binary in several locations: next to the sandbox binary,
/// in workspace target directories, and then on PATH.
pub fn open_desktop_app() {
    let Some(path) = find_desktop_binary() else {
        eprintln!("{{\"level\":\"warn\",\"message\":\"desktop app not found\"}}");
        return;
    };

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let _ = std::process::Command::new(&path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    #[cfg(target_os = "macos")]
    {
        // Try macOS `open -a` for .app bundles first, fall back to direct exec
        let _ = std::process::Command::new("open")
            .arg("-a")
            .arg("Code Agent Sandbox")
            .spawn()
            .or_else(|_| std::process::Command::new(&path).spawn());
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new(&path).spawn();
    }
}

/// Search for the desktop app binary in several locations.
fn find_desktop_binary() -> Option<std::path::PathBuf> {
    let exe_name = if cfg!(windows) {
        "codeagent-desktop.exe"
    } else {
        "codeagent-desktop"
    };

    // Search next to the current (sandbox) executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(exe_name);
            if candidate.exists() {
                return Some(candidate);
            }

            // Search workspace target/{debug,release} directories
            if let Some(target_dir) = dir.parent() {
                for profile in &["release", "debug"] {
                    let candidate = target_dir.join(profile).join(exe_name);
                    if candidate.exists() {
                        return Some(candidate);
                    }
                }
            }
        }
    }

    // Fall back to PATH
    which::which("codeagent-desktop").ok()
}
