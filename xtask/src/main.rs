use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Development tasks for codeagent-sandbox")]
struct Cli {
    #[command(subcommand)]
    command: XtaskCommand,
}

#[derive(Subcommand)]
enum XtaskCommand {
    /// Build the guest VM image (vmlinuz + initrd.img)
    BuildGuest {
        /// Target architecture: x86_64 or aarch64
        #[arg(long, default_value_t = default_arch())]
        arch: String,

        /// Output directory for built artifacts
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Disable Docker build cache
        #[arg(long)]
        no_cache: bool,
    },
}

fn default_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64".to_string(),
        "aarch64" => "aarch64".to_string(),
        other => {
            eprintln!("warning: unknown host architecture '{other}', defaulting to x86_64");
            "x86_64".to_string()
        }
    }
}

fn project_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("xtask should be in project root")
        .to_path_buf()
}

fn check_docker() -> Result<(), String> {
    let output = Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map_err(|e| format!("failed to run 'docker': {e}\n\nDocker is required for building guest images.\nInstall Docker from https://docs.docker.com/get-docker/"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Docker is not running or not accessible:\n{stderr}\n\nEnsure Docker Desktop is running or the Docker daemon is started."
        ));
    }

    Ok(())
}

fn build_guest(arch: &str, output_dir: PathBuf, no_cache: bool) -> Result<(), String> {
    if arch != "x86_64" && arch != "aarch64" {
        return Err(format!(
            "unsupported architecture '{arch}': must be 'x86_64' or 'aarch64'"
        ));
    }

    check_docker()?;

    let root = project_root();
    let dockerfile = root.join("guest").join("Dockerfile");

    if !dockerfile.exists() {
        return Err(format!(
            "Dockerfile not found at {}\nRun this command from the project root.",
            dockerfile.display()
        ));
    }

    // Create output directory
    std::fs::create_dir_all(&output_dir).map_err(|e| {
        format!(
            "failed to create output directory {}: {e}",
            output_dir.display()
        )
    })?;

    let output_dir_abs = output_dir
        .canonicalize()
        .unwrap_or_else(|_| output_dir.clone());

    println!("Building guest image for {arch}...");
    println!("  Dockerfile: {}", dockerfile.display());
    println!("  Output:     {}", output_dir_abs.display());
    println!();

    let docker_platform = match arch {
        "aarch64" => "linux/arm64",
        _ => "linux/amd64",
    };

    let mut cmd = Command::new("docker");
    cmd.current_dir(&root)
        .env("DOCKER_BUILDKIT", "1")
        .args(["build", "--file"])
        .arg(&dockerfile)
        .args(["--platform", docker_platform])
        .args(["--build-arg", &format!("ARCH={arch}")])
        .args([
            "--output",
            &format!("type=local,dest={}", output_dir_abs.display()),
        ]);

    if no_cache {
        cmd.arg("--no-cache");
    }

    // Build context is the project root
    cmd.arg(".");

    println!("Running: docker build ...");
    println!();

    let status = cmd
        .status()
        .map_err(|e| format!("failed to run docker build: {e}"))?;

    if !status.success() {
        return Err("Docker build failed. See output above for details.".to_string());
    }

    println!();

    // Validate output
    let vmlinuz = output_dir_abs.join("vmlinuz");
    let initrd = output_dir_abs.join("initrd.img");

    if !vmlinuz.exists() {
        return Err(format!(
            "build succeeded but vmlinuz not found at {}",
            vmlinuz.display()
        ));
    }
    if !initrd.exists() {
        return Err(format!(
            "build succeeded but initrd.img not found at {}",
            initrd.display()
        ));
    }

    let vmlinuz_size = std::fs::metadata(&vmlinuz)
        .map(|m| m.len())
        .unwrap_or(0);
    let initrd_size = std::fs::metadata(&initrd).map(|m| m.len()).unwrap_or(0);

    if vmlinuz_size == 0 {
        return Err("vmlinuz is empty".to_string());
    }
    if initrd_size == 0 {
        return Err("initrd.img is empty".to_string());
    }

    println!("Guest image built successfully!");
    println!("  Architecture: {arch}");
    println!(
        "  vmlinuz:      {} ({:.1} MB)",
        vmlinuz.display(),
        vmlinuz_size as f64 / 1_048_576.0
    );
    println!(
        "  initrd.img:   {} ({:.1} MB)",
        initrd.display(),
        initrd_size as f64 / 1_048_576.0
    );

    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        XtaskCommand::BuildGuest {
            arch,
            output_dir,
            no_cache,
        } => {
            let output_dir =
                output_dir.unwrap_or_else(|| project_root().join("target/guest").join(&arch));
            build_guest(&arch, output_dir, no_cache)
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}
