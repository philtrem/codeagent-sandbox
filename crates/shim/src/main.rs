use tokio::fs::OpenOptions;

#[tokio::main]
async fn main() {
    let device_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/virtio-ports/control".to_string());

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&device_path)
        .await
        .unwrap_or_else(|error| {
            eprintln!("failed to open control device {device_path}: {error}");
            std::process::exit(1);
        });

    let file_clone = file
        .try_clone()
        .await
        .unwrap_or_else(|error| {
            eprintln!("failed to clone control device handle: {error}");
            std::process::exit(1);
        });

    if let Err(error) = codeagent_shim::run(file, file_clone).await {
        eprintln!("shim error: {error}");
        std::process::exit(1);
    }
}
