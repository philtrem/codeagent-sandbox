use tokio::fs::File;

#[tokio::main]
async fn main() {
    let device_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/virtio-ports/control".to_string());

    let reader = File::open(&device_path).await.unwrap_or_else(|error| {
        eprintln!("failed to open control device {device_path}: {error}");
        std::process::exit(1);
    });

    let writer = File::create(&device_path).await.unwrap_or_else(|error| {
        eprintln!("failed to open control device for writing {device_path}: {error}");
        std::process::exit(1);
    });

    if let Err(error) = codeagent_shim::run(reader, writer).await {
        eprintln!("shim error: {error}");
        std::process::exit(1);
    }
}
