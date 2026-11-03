//! M1-27 acceptance: the save-scan report runs on a generated save and on an
//! empty directory, and the CLI emits parseable JSON for both.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use voxel_bench::patterns;
use voxel_block::MaterialId;
use voxel_storage::gpu::MemSink;
use voxel_storage::world::World;
use voxel_storage::{CHUNK_DIM, ChunkPos};

fn tmp(name: &str) -> PathBuf {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("vsavescan-{name}-{}-{t}", std::process::id()))
}

/// Writes a small engine-generated save with several regions, a folded DAG and
/// one pending overlay, then drops the world so every file is closed.
fn fixture(dir: &Path) -> Vec<ChunkPos> {
    let positions = [
        ChunkPos::new(0, 0, 0),
        ChunkPos::new(1, 2, 3),
        ChunkPos::new(-1, 0, 0),
        ChunkPos::new(8, 0, -1),
    ];
    let dense = patterns::terrain(0, 0);
    let mut world = World::new(
        dir,
        Box::new(move |_pos, out| out.copy_from_slice(&dense)),
        MemSink::default(),
    )
    .unwrap();

    let edge = CHUNK_DIM as i32;
    for (i, pos) in positions.iter().enumerate() {
        world.load(*pos).unwrap();
        world
            .set(
                pos.x * edge,
                pos.y * edge,
                pos.z * edge,
                MaterialId::from_raw(4 + i as u16),
            )
            .unwrap();
    }

    // First snapshot: DAG plus overlay diff per chunk.
    world.flush_edits().unwrap();
    // Fold the overlays into new DAGs, which supersede those diffs.
    world.consolidate(16).unwrap();
    // Leave one pending overlay so the scanner also reports a live diff.
    world
        .set(
            positions[0].x * edge,
            positions[0].y * edge + 5,
            positions[0].z * edge,
            MaterialId::from_raw(9),
        )
        .unwrap();
    world.flush_edits().unwrap();

    positions.to_vec()
}

fn run_cli(path: &Path, extra: &[&str]) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_voxel-bench");
    let output = Command::new(exe)
        .arg("save-scan")
        .arg("--path")
        .arg(path)
        .args(extra)
        .output()
        .expect("voxel-bench binary runs");
    assert!(
        output.status.success(),
        "save-scan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn cli_scan(path: &Path) -> Value {
    let output = run_cli(path, &[]);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn generated_fixture_has_per_chunk_counts_and_distributions() {
    let dir = tmp("fixture");
    let positions = fixture(&dir);

    let report = voxel_bench::save_scan::scan(&dir).expect("scan a generated save");
    assert_eq!(report.live_chunks, positions.len());
    assert_eq!(report.chunks.len(), positions.len());
    assert_eq!(report.region_files, 3, "three 8x8x8-chunk regions");
    assert!(report.file_bytes > 0);
    assert!(report.record_bytes > 0);
    assert!(
        report.superseded_bytes > 0,
        "consolidation leaves the old diff records behind"
    );
    assert!(report.total_voxels > 0);
    assert_eq!(report.total_overrides, 1);
    assert_eq!(
        report.total_voxels,
        report.materials.values().copied().sum::<u64>()
    );
    assert!(
        report.materials.len() >= 3,
        "terrain plus the edited materials: {:?}",
        report.materials.keys().collect::<Vec<_>>()
    );

    for pos in &positions {
        assert!(
            report
                .chunks
                .iter()
                .any(|chunk| chunk.x == pos.x && chunk.y == pos.y && chunk.z == pos.z),
            "missing chunk {pos:?}"
        );
    }
    for chunk in &report.chunks {
        assert_eq!(
            chunk.voxels,
            chunk.materials.values().copied().sum::<u64>(),
            "chunk {:?} histogram does not add up",
            (chunk.x, chunk.y, chunk.z)
        );
        assert!(chunk.node_words > 0);
        assert_eq!(chunk.node_bytes, chunk.node_words * 4);
        assert_eq!(chunk.dag_bytes, chunk.node_bytes + 8);
        assert!(chunk.record_bytes >= chunk.dag_bytes + chunk.diff_bytes);
    }

    let distribution = &report.node_bytes;
    let (p50, p95, max) = (
        distribution.p50.expect("non-empty"),
        distribution.p95.expect("non-empty"),
        distribution.max.expect("non-empty"),
    );
    assert!(p50 <= p95 && p95 <= max);
    let voxels = &report.voxels;
    assert!(voxels.p50.unwrap() <= voxels.p95.unwrap());
    assert!(voxels.max.unwrap() >= report.total_voxels / report.live_chunks as u64);

    let json = serde_json::to_string(&report).unwrap();
    let roundtrip: voxel_bench::save_scan::SaveScanReport = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtrip, report, "the JSON report round-trips");
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["live_chunks"].as_u64(), Some(positions.len() as u64));
    assert!(value["materials"].is_object());
    assert!(value["chunks"].is_array());
    assert!(value["node_bytes"]["p50"].is_number());
}

#[test]
fn empty_directory_is_an_empty_report() {
    let dir = tmp("empty");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("not-a-region.txt"), b"ignored").unwrap();

    let report = voxel_bench::save_scan::scan(&dir).expect("scan an empty save");
    assert_eq!(report.region_files, 0);
    assert_eq!(report.live_chunks, 0);
    assert!(report.chunks.is_empty());
    assert!(report.materials.is_empty());
    assert_eq!(report.total_voxels, 0);
    assert_eq!(report.total_overrides, 0);
    assert_eq!(report.node_bytes, Default::default());
    assert_eq!(report.voxels, Default::default());
    assert_eq!(report.file_bytes, 0);
    assert_eq!(report.record_bytes, 0);
    assert_eq!(report.superseded_bytes, 0);
    assert!(report.warnings.is_empty());

    let json = serde_json::to_string(&report).unwrap();
    let roundtrip: voxel_bench::save_scan::SaveScanReport = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtrip, report);
    let value: Value = serde_json::from_str(&json).unwrap();
    assert!(value["chunks"].as_array().unwrap().is_empty());
    assert!(value["node_bytes"]["p50"].is_null());
    assert!(value["voxels"]["max"].is_null());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cli_emits_json_and_honours_out() {
    let dir = tmp("cli");
    std::fs::create_dir_all(&dir).unwrap();

    let value = cli_scan(&dir);
    assert_eq!(value["live_chunks"].as_u64(), Some(0));
    assert!(value["warnings"].as_array().unwrap().is_empty());

    let out = dir.join("nested/report.json");
    let output = run_cli(&dir, &["--out", out.to_str().unwrap()]);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("wrote"),
        "the --out path should report where it wrote"
    );
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(written["live_chunks"].as_u64(), Some(0));
    assert_eq!(written["path"], dir.display().to_string());

    let fixture_dir = tmp("cli-fixture");
    fixture(&fixture_dir);
    let value = cli_scan(&fixture_dir);
    assert_eq!(value["live_chunks"].as_u64(), Some(4));
    assert_eq!(value["chunks"].as_array().unwrap().len(), 4);

    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(fixture_dir).ok();
}
