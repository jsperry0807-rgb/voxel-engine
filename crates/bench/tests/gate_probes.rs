//! M1-38 acceptance: the probe harness measures incremental updates on the
//! frozen fixture, reports the triggering edit and the p50/p95/max update, and
//! compares the p95 with the 4 ms per-edited-chunk budget.
//!
//! The fixture is frozen at 64³, so the report and these tests are skipped in a
//! 128³ build. `voxel-bench gate probes` rejects the edge mismatch explicitly.

#![cfg(not(feature = "chunk128"))]

use std::path::PathBuf;
use std::process::Command;

use voxel_bench::fixture::Manifest;
use voxel_bench::gate::probes::{self, BUDGET_NS, EDITS, SCHEMA_VERSION, WARMUP_EDITS};
use voxel_core::constants::BLOCK_DIM;
use voxel_core::content_constants::PROBE_BYTES;
use voxel_storage::AIR;

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
fn report_measures_an_incremental_update_for_every_fixture_edit() {
    let report =
        probes::report(&manifest_path()).expect("the checked-in fixture produces a probe report");
    let manifest = read_manifest();

    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert!(report.is_provisional(), "the M1 fixture is still synthetic");
    assert_eq!(report.fixture.id, "m1-representative");
    assert_eq!(report.fixture.status, "provisional");
    assert_eq!(report.fixture.chunks, manifest.observed.live_chunks);
    assert_eq!(report.fixture.total_voxels, manifest.observed.total_voxels);
    assert_eq!(report.fixture.world_hash, manifest.integrity.world_hash);

    assert_eq!(report.edit_count, EDITS);
    assert_eq!(report.warmup_edits, WARMUP_EDITS);
    assert_eq!(report.updates.len(), EDITS);
    assert_eq!(
        report.probes_per_chunk,
        (64 / BLOCK_DIM).pow(3),
        "one near probe per block"
    );
    assert_eq!(report.rays_per_probe, 64);
    assert!(report.chunks_edited > 0);
    assert!(report.chunks_edited <= report.chunks_available);
    assert!(
        report.chunks_available <= report.fixture.chunks,
        "edit targets cannot outnumber live fixture chunks"
    );

    for (name, term) in [
        ("select", &report.terms.select),
        ("rebake", &report.terms.rebake),
        ("upload", &report.terms.upload),
        ("update", &report.terms.update),
    ] {
        assert!(term.p50_ns <= term.p95_ns, "{name}: p50 <= p95");
        assert!(term.p95_ns <= term.max_ns, "{name}: p95 <= max");
        assert!(term.total_ns > 0, "{name}: the run produced samples");
    }

    assert_eq!(report.budget.budget_ns, BUDGET_NS);
    assert_eq!(report.budget.measured_p95_ns, report.terms.update.p95_ns);
    assert_eq!(
        report.budget.margin_ns,
        BUDGET_NS as i64 - report.terms.update.p95_ns as i64
    );
    assert!(
        report.budget.within_budget,
        "the 64³ probe update p95 must fit the 4 ms budget"
    );

    assert_eq!(
        report.counts.solid_to_air_edits + report.counts.air_to_solid_edits,
        EDITS as u64
    );
    assert!(
        report.counts.affected_total > 0,
        "the fixture workload must actually select probes"
    );
    assert!(
        report.counts.rays_baked_total > 0,
        "the fixture workload must rebake rays"
    );
    assert_eq!(
        report.bytes.probe_bytes,
        report.counts.affected_total * PROBE_BYTES as u64,
        "one 12 B record per affected probe"
    );
    assert_eq!(
        report.bytes.validity_bytes, report.counts.affected_total,
        "one validity byte per affected probe"
    );
    assert_eq!(
        report.bytes.upload_bytes,
        report.bytes.probe_bytes + report.bytes.validity_bytes
    );

    let mut affected_total = 0u64;
    let mut baked_total = 0u64;
    let mut zero_affected = 0u64;
    for sample in &report.updates {
        assert!(
            (manifest.recipe.chunk_min[0]..=manifest.recipe.chunk_max[0])
                .contains(&sample.chunk[0])
                && (manifest.recipe.chunk_min[1]..=manifest.recipe.chunk_max[1])
                    .contains(&sample.chunk[1])
                && (manifest.recipe.chunk_min[2]..=manifest.recipe.chunk_max[2])
                    .contains(&sample.chunk[2]),
            "update {} targets a chunk outside the frozen recipe: {:?}",
            sample.edit,
            sample.chunk,
        );
        assert_eq!(
            sample.update_ns,
            sample.select_ns + sample.bake_ns + sample.upload_ns,
            "edit {}: update is select + rebake + upload",
            sample.edit
        );
        assert!(
            sample.material != sample.previous_material,
            "edit {} must change the voxel",
            sample.edit
        );
        assert_eq!(
            sample.solid_to_air,
            sample.material == AIR,
            "edit {}: a mine writes air, a place writes the original material",
            sample.edit
        );
        assert!(
            sample.affected_probes as usize <= report.probes_per_chunk,
            "edit {} cannot select more probes than the grid holds",
            sample.edit
        );
        assert!(
            sample.valid_probes <= sample.affected_probes,
            "edit {}: valid probes are a subset of affected probes",
            sample.edit
        );
        assert_eq!(
            sample.rays_baked,
            sample.valid_probes * report.rays_per_probe as u32,
            "edit {}: every valid affected probe casts the full hemisphere",
            sample.edit
        );
        assert!(
            sample.rays_tested as usize <= report.probes_per_chunk * report.rays_per_probe,
            "edit {}: selection cannot test more rays than the grid holds",
            sample.edit
        );
        assert!(
            sample.rays_reaching <= sample.rays_tested,
            "edit {}: reaching rays are a subset of tested rays",
            sample.edit
        );
        assert_eq!(
            sample.probe_bytes,
            sample.affected_probes as u64 * PROBE_BYTES as u64,
            "edit {}: one 12 B record per affected probe",
            sample.edit
        );
        assert_eq!(
            sample.validity_bytes, sample.affected_probes as u64,
            "edit {}: one validity byte per affected probe",
            sample.edit
        );

        affected_total += sample.affected_probes as u64;
        baked_total += sample.rays_baked as u64;
        zero_affected += u64::from(sample.affected_probes == 0);
    }
    assert_eq!(report.counts.affected_total, affected_total);
    assert_eq!(report.counts.rays_baked_total, baked_total);
    assert_eq!(report.counts.zero_affected_edits, zero_affected);
}

#[test]
fn cli_prints_the_update_and_every_percentile() {
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("probes")
        .arg("--manifest")
        .arg(manifest_path())
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate probes failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    for needle in [
        "M1 gate probes",
        "m1-representative",
        "Provisional fixture",
        "Update",
        "Select",
        "Rebake",
        "Upload",
        "Select + rebake + upload",
        "Affected probes",
        "Rays",
        "64 near probes/chunk",
        "1,000 measured",
        "4.00 ms",
        "PASS",
        "p50",
        "p95",
        "max",
    ] {
        assert!(
            text.contains(needle),
            "the gate probes report must contain `{needle}`:\n{text}"
        );
    }
}

#[test]
fn cli_json_is_the_same_report() {
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("probes")
        .arg("--manifest")
        .arg(manifest_path())
        .arg("--json")
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate probes --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(json["schema_version"], SCHEMA_VERSION);
    assert_eq!(json["edit_count"], EDITS as u64);
    assert_eq!(json["warmup_edits"], WARMUP_EDITS as u64);
    assert_eq!(json["updates"].as_array().map(Vec::len), Some(EDITS));
    assert_eq!(json["probes_per_chunk"], 64);
    assert_eq!(json["rays_per_probe"], 64);
    assert_eq!(json["budget"]["budget_ns"], BUDGET_NS);
    assert_eq!(json["budget"]["within_budget"], true);
    assert!(json["terms"]["select"]["p95_ns"].as_u64().unwrap() > 0);
    assert!(json["terms"]["rebake"]["p95_ns"].as_u64().unwrap() > 0);
    assert!(json["terms"]["update"]["p95_ns"].as_u64().unwrap() > 0);
    assert!(json["counts"]["affected_total"].as_u64().unwrap() > 0);
    assert!(json["counts"]["rays_baked_total"].as_u64().unwrap() > 0);
}
