//! M1-35 acceptance: the gate memory report covers every fixture chunk, every
//! component is present, and the per-chunk total is exactly their sum.
//!
//! The fixture is frozen at 64³, so the report and these tests are skipped in a
//! 128³ build. `voxel-bench gate memory` rejects the edge mismatch explicitly.

#![cfg(not(feature = "chunk128"))]

use std::path::PathBuf;
use std::process::Command;

use voxel_bench::fixture::Manifest;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/fixtures/m1-representative")
}

fn manifest_path() -> PathBuf {
    fixture_dir().join("manifest.json")
}

fn read_manifest() -> Manifest {
    let path = manifest_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn report_covers_every_fixture_chunk_and_components_sum_to_total() {
    let report = voxel_bench::gate::memory::report(&manifest_path())
        .expect("the checked-in fixture produces a memory report");
    let manifest = read_manifest();

    assert_eq!(
        report.schema_version,
        voxel_bench::gate::memory::SCHEMA_VERSION
    );
    assert!(report.is_provisional(), "the M1 fixture is still synthetic");
    assert_eq!(report.fixture.id, "m1-representative");
    assert_eq!(report.fixture.status, "provisional");
    assert_eq!(report.fixture.chunks, manifest.observed.live_chunks);
    assert_eq!(report.chunks.len(), manifest.observed.live_chunks);
    assert_eq!(report.fixture.total_voxels, manifest.observed.total_voxels);
    assert_eq!(
        report.fixture.overlay_entries,
        manifest.observed.total_overrides
    );
    assert_eq!(report.fixture.world_hash, manifest.integrity.world_hash);

    // No synthetic chunks: every measured chunk is inside the recipe bounds.
    for chunk in &report.chunks {
        assert!(
            (manifest.recipe.chunk_min[0]..=manifest.recipe.chunk_max[0]).contains(&chunk.x)
                && (manifest.recipe.chunk_min[1]..=manifest.recipe.chunk_max[1]).contains(&chunk.y)
                && (manifest.recipe.chunk_min[2]..=manifest.recipe.chunk_max[2]).contains(&chunk.z),
            "chunk ({}, {}, {}) is outside the frozen recipe",
            chunk.x,
            chunk.y,
            chunk.z,
        );
        assert_eq!(
            chunk.total_bytes,
            chunk.dag_bytes
                + chunk.dag_v1_bytes
                + chunk.overlay_bytes
                + chunk.light_bytes
                + chunk.probe_bytes
                + chunk.mesh_bytes,
            "chunk ({}, {}, {}) total",
            chunk.x,
            chunk.y,
            chunk.z,
        );
        assert_eq!(chunk.dag_bytes, chunk.dag_node_bytes + chunk.dag_leaf_bytes);
        assert_eq!(
            chunk.mesh_bytes,
            chunk.mesh_quads * voxel_storage::reference::mesh::QUAD_BYTES as u64
        );
        assert_eq!(chunk.light_bytes, 1_024);
        assert_eq!(chunk.probe_bytes, 768);
    }
    assert!(
        report.chunks.iter().any(|chunk| chunk.mesh_quads > 0),
        "the fixture must contain populated chunks"
    );

    for (name, distribution) in [
        ("DAG", &report.components.dag),
        ("overlay", &report.components.overlay),
        ("light", &report.components.light),
        ("probes", &report.components.probes),
        ("mesh", &report.components.mesh),
        ("total", &report.components.total),
    ] {
        let p50 = distribution.p50.unwrap_or_else(|| panic!("{name} p50"));
        let p95 = distribution.p95.unwrap_or_else(|| panic!("{name} p95"));
        let max = distribution.max.unwrap_or_else(|| panic!("{name} max"));
        assert!(p50 <= p95 && p95 <= max, "{name} percentiles are ordered");
    }

    assert_eq!(report.components.overlay.max, Some(0));
    assert_eq!(
        report.components.overlay.total, 0,
        "the fixture has no pending overlay entries"
    );
    assert_eq!(report.components.light.p50, Some(1_024));
    assert_eq!(report.components.probes.p50, Some(768));
    assert!(report.components.mesh.p95.unwrap() > 0);

    let component_sum = report.components.dag.total
        + report.components.legacy_v1.total
        + report.components.overlay.total
        + report.components.light.total
        + report.components.probes.total
        + report.components.mesh.total;
    assert_eq!(component_sum, report.components.total.total);

    // The v1 pool is resident until M6-09a and must be counted, so the mirror
    // matches the component distribution and the fixture's recorded bytes.
    assert_eq!(report.dag_v1_view, report.components.legacy_v1);
    assert_eq!(report.dag_v1_view.p50, manifest.observed.node_bytes.p50);
    assert_eq!(report.dag_v1_view.p95, manifest.observed.node_bytes.p95);
    assert_eq!(report.dag_v1_view.max, manifest.observed.node_bytes.max);
}

#[test]
fn cli_prints_every_component_and_percentile() {
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("memory")
        .arg("--manifest")
        .arg(manifest_path())
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate memory failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    for needle in [
        "M1 gate memory",
        "m1-representative",
        "Provisional fixture",
        "DAG",
        "Overlay",
        "Light",
        "Probes",
        "Mesh estimate",
        "Total",
        "p50",
        "p95",
        "max",
    ] {
        assert!(
            text.contains(needle),
            "the gate memory report must contain `{needle}`:\n{text}"
        );
    }
}
