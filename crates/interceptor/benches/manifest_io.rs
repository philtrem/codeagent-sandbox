use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tempfile::TempDir;

use codeagent_interceptor::manifest::StepManifest;
use codeagent_interceptor::preimage::path_hash;

/// Build a manifest with `count` entries using realistic path strings and hashes.
fn build_manifest(count: usize) -> StepManifest {
    let mut manifest = StepManifest::new(1);
    manifest.command = Some("test command".to_string());

    for i in 0..count {
        let depth = i % 4;
        let path = match depth {
            0 => format!("file_{i}.txt"),
            1 => format!("src/file_{i}.rs"),
            2 => format!("src/components/component_{i}.tsx"),
            _ => format!("src/utils/helpers/helper_{i}.ts"),
        };
        let hash = path_hash(std::path::Path::new(&path));
        let existed = i % 3 != 0;
        let file_type = if i % 10 == 0 { "directory" } else { "regular" };
        manifest.add_entry(&path, &hash, existed, file_type);
    }

    manifest
}

fn bench_manifest_write(c: &mut Criterion) {
    let counts: &[(&str, usize)] = &[
        ("10_paths", 10),
        ("100_paths", 100),
        ("1000_paths", 1000),
    ];

    let mut group = c.benchmark_group("manifest_write");
    group.measurement_time(Duration::from_secs(10));

    for &(label, count) in counts {
        let manifest = build_manifest(count);
        group.bench_with_input(BenchmarkId::from_parameter(label), &manifest, |b, manifest| {
            let dir = TempDir::new().unwrap();
            b.iter(|| {
                manifest.write_to(dir.path()).unwrap();
            });
        });
    }
    group.finish();
}

fn bench_manifest_read(c: &mut Criterion) {
    let counts: &[(&str, usize)] = &[
        ("10_paths", 10),
        ("100_paths", 100),
        ("1000_paths", 1000),
    ];

    let mut group = c.benchmark_group("manifest_read");
    group.measurement_time(Duration::from_secs(10));

    for &(label, count) in counts {
        let manifest = build_manifest(count);
        let dir = TempDir::new().unwrap();
        manifest.write_to(dir.path()).unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(label), &count, |b, _| {
            b.iter(|| {
                StepManifest::read_from(dir.path()).unwrap()
            });
        });
    }
    group.finish();
}

fn bench_manifest_serialization(c: &mut Criterion) {
    let counts: &[(&str, usize)] = &[
        ("10_paths", 10),
        ("100_paths", 100),
        ("1000_paths", 1000),
    ];

    let mut group = c.benchmark_group("manifest_serialize");
    group.measurement_time(Duration::from_secs(10));

    for &(label, count) in counts {
        let manifest = build_manifest(count);
        group.bench_with_input(BenchmarkId::from_parameter(label), &manifest, |b, manifest| {
            b.iter(|| serde_json::to_string_pretty(manifest).unwrap());
        });
    }
    group.finish();

    let mut group = c.benchmark_group("manifest_deserialize");
    group.measurement_time(Duration::from_secs(10));

    for &(label, count) in counts {
        let manifest = build_manifest(count);
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        group.bench_with_input(BenchmarkId::from_parameter(label), &json, |b, json| {
            b.iter(|| serde_json::from_str::<StepManifest>(json).unwrap());
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_manifest_write,
    bench_manifest_read,
    bench_manifest_serialization
);
criterion_main!(benches);
