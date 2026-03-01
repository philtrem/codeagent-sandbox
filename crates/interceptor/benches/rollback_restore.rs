use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tempfile::TempDir;

use codeagent_common::SymlinkPolicy;
use codeagent_interceptor::manifest::StepManifest;
use codeagent_interceptor::preimage::{capture_preimage, path_hash};
use codeagent_interceptor::rollback::rollback_step;

/// Generate deterministic pseudo-random bytes.
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

/// Set up a step directory with a captured preimage for a single file,
/// then overwrite the file so rollback has something to restore.
struct RollbackFixture {
    _dir: TempDir,
    working: PathBuf,
    step_dir: PathBuf,
    file_path: PathBuf,
    dirty_data: Vec<u8>,
}

impl RollbackFixture {
    fn new(size: usize) -> Self {
        let dir = TempDir::new().unwrap();
        let working = dir.path().join("working");
        let step_dir = dir.path().join("step");
        let preimage_dir = step_dir.join("preimages");
        fs::create_dir_all(&working).unwrap();
        fs::create_dir_all(&preimage_dir).unwrap();

        let file_path = working.join("bench_file.dat");
        let original_data = generate_data(size);
        fs::write(&file_path, &original_data).unwrap();

        // Capture preimage
        let hash = path_hash(Path::new("bench_file.dat"));
        let (meta, _) = capture_preimage(&file_path, &working, &preimage_dir).unwrap();

        // Build manifest
        let mut manifest = StepManifest::new(1);
        manifest.add_entry("bench_file.dat", &hash, true, meta.file_type.as_str());
        manifest.write_to(&step_dir).unwrap();

        // Create dirty data to overwrite with before each iteration
        let dirty_data = vec![0xFF; size];

        // Dirty the file initially
        fs::write(&file_path, &dirty_data).unwrap();

        Self {
            _dir: dir,
            working,
            step_dir,
            file_path,
            dirty_data,
        }
    }

    fn dirty_file(&self) {
        fs::write(&self.file_path, &self.dirty_data).unwrap();
    }
}

fn bench_rollback_restore(c: &mut Criterion) {
    let sizes: &[(&str, usize)] = &[
        ("4kb", 4 * 1024),
        ("1mb", 1024 * 1024),
        ("100mb", 100 * 1024 * 1024),
    ];

    let mut group = c.benchmark_group("rollback_restore");
    group.measurement_time(Duration::from_secs(15));

    for &(label, size) in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &size, |b, &size| {
            let fixture = RollbackFixture::new(size);
            b.iter_batched(
                || fixture.dirty_file(),
                |()| {
                    rollback_step(&fixture.step_dir, &fixture.working, SymlinkPolicy::default())
                        .unwrap()
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_rollback_restore);
criterion_main!(benches);
