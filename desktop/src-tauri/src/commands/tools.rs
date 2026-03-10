use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

use crate::paths;

/// Status of the tools disk image.
#[derive(Debug, Clone, Serialize)]
pub struct ToolsImageStatus {
    pub exists: bool,
    pub size_bytes: u64,
    pub created_at: String,
    pub packages: Vec<String>,
}

/// Progress event emitted during a tools image build.
#[derive(Debug, Clone, Serialize)]
pub struct ToolsBuildProgress {
    pub stage: String,
    pub message: String,
}

/// Build a tools disk image with the specified Alpine packages.
///
/// Runs `docker build` using the embedded Dockerfile.tools content.
/// Emits `tools-build-progress` events for real-time UI updates.
#[tauri::command]
pub async fn build_tools_image(
    packages: Vec<String>,
    app: AppHandle,
) -> Result<String, String> {
    let pkg_str = packages.join(" ");
    if pkg_str.trim().is_empty() {
        return Err("No packages specified".into());
    }

    // Determine output path
    let output_dir = default_tools_dir()?;
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("Failed to create output directory: {e}"))?;

    // Write the Dockerfile.tools content to a temp file so Docker can use it.
    // This avoids requiring the Dockerfile to be on disk at runtime.
    let dockerfile_content = include_str!("../../../../guest/Dockerfile.tools");
    let temp_dir = tempfile::tempdir()
        .map_err(|e| format!("Failed to create temp directory: {e}"))?;
    let dockerfile_path = temp_dir.path().join("Dockerfile.tools");
    std::fs::write(&dockerfile_path, dockerfile_content)
        .map_err(|e| format!("Failed to write Dockerfile: {e}"))?;

    // We also need a build context. Use the temp dir itself (Dockerfile.tools
    // doesn't COPY anything from the context).
    let context_dir = temp_dir.path().to_path_buf();

    let emit_progress = |stage: &str, message: &str| {
        let _ = app.emit(
            "tools-build-progress",
            ToolsBuildProgress {
                stage: stage.into(),
                message: message.into(),
            },
        );
    };

    emit_progress("pulling", "Pulling Alpine base image...");

    let output_dir_str = output_dir.to_string_lossy().to_string();

    // Run docker build in a blocking thread
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("docker");
        cmd.env("DOCKER_BUILDKIT", "1")
            .args(["build", "--file"])
            .arg(&dockerfile_path)
            .args(["--build-arg", &format!("PACKAGES={pkg_str}")])
            .args([
                "--output",
                &format!("type=local,dest={output_dir_str}"),
            ])
            .arg(&context_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run docker: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Docker build failed:\n{stderr}"));
        }

        Ok(())
    })
    .await
    .map_err(|e| format!("Task failed: {e}"))?;

    result?;

    let image_path = output_dir.join("tools.img");
    if !image_path.exists() {
        emit_progress("error", "Build succeeded but tools.img not found");
        return Err("Build succeeded but tools.img not found".into());
    }

    emit_progress("done", "Tools image built successfully");

    Ok(image_path.to_string_lossy().into_owned())
}

/// Get the status of a tools disk image.
#[tauri::command]
pub fn get_tools_image_status(image_path: String) -> Result<ToolsImageStatus, String> {
    let path = PathBuf::from(&image_path);

    if !path.exists() {
        return Ok(ToolsImageStatus {
            exists: false,
            size_bytes: 0,
            created_at: String::new(),
            packages: vec![],
        });
    }

    let metadata = std::fs::metadata(&path)
        .map_err(|e| format!("Failed to read file metadata: {e}"))?;

    let created_at = metadata
        .modified()
        .ok()
        .and_then(|t| {
            let duration = t
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            let dt = chrono::DateTime::from_timestamp(
                duration.as_secs() as i64,
                duration.subsec_nanos(),
            )?;
            Some(dt.to_rfc3339())
        })
        .unwrap_or_default();

    Ok(ToolsImageStatus {
        exists: true,
        size_bytes: metadata.len(),
        created_at,
        packages: vec![],
    })
}

/// Delete the tools disk image.
#[tauri::command]
pub fn delete_tools_image(image_path: String) -> Result<(), String> {
    let path = PathBuf::from(&image_path);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete tools image: {e}"))?;
    }
    Ok(())
}

/// Get the default path for the tools disk image.
#[tauri::command]
pub fn get_default_tools_image_path() -> Result<String, String> {
    let dir = default_tools_dir()?;
    Ok(dir.join("tools.img").to_string_lossy().into_owned())
}

/// Check if Docker is available on this system.
#[tauri::command]
pub fn check_docker_available() -> bool {
    std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Returns the default directory for the tools image.
fn default_tools_dir() -> Result<PathBuf, String> {
    paths::config_dir()
        .map(|p| p.join("guest"))
        .ok_or_else(|| "Could not determine config directory".to_string())
}
