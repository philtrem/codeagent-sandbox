use std::fs;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tempfile::TempDir;

use codeagent_test_support::TreeSnapshot;

/// Create a directory tree with `file_count` files spread across subdirectories.
/// Uses a two-level directory structure so the walk has realistic depth.
fn create_tree(root: &std::path::Path, file_count: usize) {
    let files_per_dir = 50;
    let dir_count = file_count.div_ceil(files_per_dir);

    let mut created = 0;
    for d in 0..dir_count {
        let dir = root.join(format!("dir_{d:04}"));
        fs::create_dir_all(&dir).unwrap();

        let remaining = file_count - created;
        let batch = remaining.min(files_per_dir);
        for f in 0..batch {
            let content = format!("file content for dir_{d:04}/file_{f:04}.txt — padding to vary size: {}", "x".repeat(f * 10));
            fs::write(dir.join(format!("file_{f:04}.txt")), content).unwrap();
            created += 1;
        }
    }
}

fn bench_snapshot_capture(c: &mut Criterion) {
    let counts: &[(&str, usize)] = &[
        ("100_files", 100),
        ("1000_files", 1000),
        ("10000_files", 10000),
    ];

    let mut group = c.benchmark_group("snapshot_capture");
    group.measurement_time(Duration::from_secs(15));
    group.sample_size(10);

    for &(label, count) in counts {
        // Set up the tree once outside the benchmark loop
        let dir = TempDir::new().unwrap();
        create_tree(dir.path(), count);

        group.bench_with_input(BenchmarkId::from_parameter(label), &count, |b, _| {
            b.iter(|| TreeSnapshot::capture(dir.path()));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_snapshot_capture);
criterion_main!(benches);
