use std::fs;
use std::path::Path;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tempfile::TempDir;

use codeagent_interceptor::preimage::capture_preimage;

/// Generate deterministic pseudo-random bytes that resist trivial zstd compression.
/// Uses a simple LCG to produce non-compressible data with realistic compression ratios.
fn generate_data(size: usize) -> Vec<u8> {
    let mut data = vec![0u8; size];
    let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
    for chunk in data.chunks_mut(8) {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let bytes = state.to_le_bytes();
        let len = chunk.len().min(8);
        chunk[..len].copy_from_slice(&bytes[..len]);
    }
    data
}

fn create_file(dir: &Path, name: &str, size: usize) {
    let data = generate_data(size);
    fs::write(dir.join(name), data).unwrap();
}

fn bench_preimage_capture(c: &mut Criterion) {
    let sizes: &[(&str, usize)] = &[
        ("4kb", 4 * 1024),
        ("1mb", 1024 * 1024),
        ("100mb", 100 * 1024 * 1024),
    ];

    let mut group = c.benchmark_group("preimage_capture");
    group.measurement_time(Duration::from_secs(15));

    for &(label, size) in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &size, |b, &size| {
            let dir = TempDir::new().unwrap();
            let working = dir.path().join("working");
            let preimage_dir = dir.path().join("preimages");
            fs::create_dir_all(&working).unwrap();
            fs::create_dir_all(&preimage_dir).unwrap();
            create_file(&working, "bench_file.dat", size);
            let file_path = working.join("bench_file.dat");

            b.iter(|| {
                // Clean preimage output between iterations
                for entry in fs::read_dir(&preimage_dir).unwrap() {
                    let entry = entry.unwrap();
                    fs::remove_file(entry.path()).unwrap();
                }
                capture_preimage(&file_path, &working, &preimage_dir).unwrap()
            });
        });
    }
    group.finish();
}

fn bench_zstd_compress(c: &mut Criterion) {
    let sizes: &[(&str, usize)] = &[
        ("4kb", 4 * 1024),
        ("1mb", 1024 * 1024),
        ("100mb", 100 * 1024 * 1024),
    ];

    let mut group = c.benchmark_group("zstd_compress");
    group.measurement_time(Duration::from_secs(15));

    for &(label, size) in sizes {
        let data = generate_data(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &data, |b, data| {
            b.iter(|| zstd::encode_all(data.as_slice(), 3).unwrap());
        });
    }
    group.finish();
}

criterion_group!(benches, bench_preimage_capture, bench_zstd_compress);
criterion_main!(benches);
