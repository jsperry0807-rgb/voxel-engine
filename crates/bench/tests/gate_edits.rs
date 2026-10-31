//! M1-37 acceptance: the edit harness measures 1,000 edits on the frozen
//! fixture, reports the fixed and dirty-region terms per edit, and charges only
//! the bytes the COW edit actually appended.
//!
//! The fixture is frozen at 64³, so the report and these tests are skipped in a
//! 128³ build. `voxel-bench gate edits` rejects the edge mismatch explicitly.

#![cfg(not(feature = "chunk128"))]

use std::path::PathBuf;
use std::process::Command;

use voxel_bench::fixture::Manifest;
use voxel_bench::gate::edits::{
    self, BUDGET_NS, EDITS, SCHEMA_VERSION, SCRUB_BUDGET_NS, WARMUP_EDITS,
};
use voxel_core::constants::ROOT_LEVEL;

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
fn report_splits_every_fixture_edit_into_fixed_and_dirty_terms() {
    let report =
        edits::report(&manifest_path()).expect("the checked-in fixture produces an edit report");
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
    assert_eq!(report.edits.len(), EDITS);
    assert!(report.chunks_edited > 0);
    assert!(report.chunks_edited <= report.chunks_available);
    assert!(
        report.chunks_available <= report.fixture.chunks,
        "edit targets cannot outnumber live fixture chunks"
    );

    for (name, term) in [
        ("upload", &report.terms.upload),
        ("fence", &report.terms.fence),
        ("flip", &report.terms.flip),
        ("fixed", &report.terms.fixed),
        ("cow", &report.terms.cow),
        ("light", &report.terms.light),
        ("dirty", &report.terms.dirty),
        ("total", &report.terms.total),
    ] {
        assert!(term.p50_ns <= term.p95_ns, "{name}: p50 <= p95");
        assert!(term.p95_ns <= term.max_ns, "{name}: p95 <= max");
        assert!(term.total_ns > 0, "{name}: the run produced samples");
    }

    for (name, term) in [
        ("scrub_dirty", &report.scrub_terms.dirty),
        ("remesh", &report.scrub_terms.remesh),
        ("scrub_light", &report.scrub_terms.light),
        ("probe", &report.scrub_terms.probe),
        ("scrub_total", &report.scrub_terms.total),
    ] {
        assert!(term.p50_ns <= term.p95_ns, "{name}: p50 <= p95");
        assert!(term.p95_ns <= term.max_ns, "{name}: p95 <= max");
        assert!(term.total_ns > 0, "{name}: the run produced samples");
    }
    // The frozen fixture's edits are occupancy-neutral solid→solid swaps, so
    // the dirty batch is exactly the voxel's own block and no probe can change.
    assert_eq!(report.scrub_counts.dirty_blocks_per_edit, 1);
    assert_eq!(report.scrub_counts.affected_probes_per_edit, 0);
    assert!(report.scrub_counts.quads_per_edit > 0);

    assert_eq!(report.budget.budget_ns, BUDGET_NS);
    assert_eq!(report.budget.measured_p95_ns, report.terms.total.p95_ns);
    assert_eq!(
        report.budget.margin_ns,
        BUDGET_NS as i64 - report.terms.total.p95_ns as i64
    );
    // The verdict must follow the measurement, not the other way round: a
    // loaded host may exceed the budget, and that is the gate's job to report.
    assert_eq!(
        report.budget.within_budget,
        report.budget.margin_ns >= 0,
        "within_budget must agree with the measured p95"
    );

    assert_eq!(report.scrub_budget.budget_ns, SCRUB_BUDGET_NS);
    assert_eq!(
        report.scrub_budget.measured_p95_ns,
        report.scrub_terms.total.p95_ns
    );
    assert_eq!(
        report.scrub_budget.margin_ns,
        SCRUB_BUDGET_NS as i64 - report.scrub_terms.total.p95_ns as i64
    );
    assert_eq!(
        report.scrub_budget.within_budget,
        report.scrub_budget.margin_ns >= 0
    );

    for sample in &report.edits {
        assert!(
            (manifest.recipe.chunk_min[0]..=manifest.recipe.chunk_max[0])
                .contains(&sample.chunk[0])
                && (manifest.recipe.chunk_min[1]..=manifest.recipe.chunk_max[1])
                    .contains(&sample.chunk[1])
                && (manifest.recipe.chunk_min[2]..=manifest.recipe.chunk_max[2])
                    .contains(&sample.chunk[2]),
            "edit {} targets a chunk outside the frozen recipe: {:?}",
            sample.edit,
            sample.chunk,
        );
        assert_eq!(
            sample.fixed_ns,
            sample.upload_ns + sample.fence_ns + sample.flip_ns,
            "edit {}: fixed term is upload + fence + flip",
            sample.edit
        );
        assert_eq!(
            sample.dirty_ns,
            sample.cow_ns + sample.light_ns,
            "edit {}: dirty term is COW + light",
            sample.edit
        );
        assert_eq!(
            sample.total_ns,
            sample.fixed_ns + sample.dirty_ns + sample.compact_ns,
            "edit {}: total is fixed + dirty + compaction",
            sample.edit
        );
        assert_eq!(
            sample.upload_bytes,
            sample.upload_node_bytes + sample.upload_leaf_bytes + sample.upload_level_start_bytes,
            "edit {}: upload bytes are the three real ranges",
            sample.edit
        );
        assert_eq!(
            sample.scrub_total_ns,
            sample.compact_ns
                + sample.cow_ns
                + sample.scrub_dirty_ns
                + sample.remesh_ns
                + sample.scrub_light_ns
                + sample.probe_ns,
            "edit {}: scrub total is compaction + cow + dirty + remesh + light + probe",
            sample.edit
        );
        assert!(
            (1..=8).contains(&sample.dirty_blocks),
            "edit {}: the 2×2×2 apron bounds the dirty batch",
            sample.edit
        );
        assert_eq!(
            sample.affected_probes, 0,
            "edit {}: a solid→solid swap cannot change probe visibility",
            sample.edit
        );
        assert!(
            sample.remesh_ns > 0,
            "edit {}: the remesh timer produced a sample",
            sample.edit
        );
        assert!(
            sample.probe_ns > 0,
            "edit {}: the probe selection timer produced a sample",
            sample.edit
        );
        assert_eq!(
            sample.upload_node_bytes + sample.upload_leaf_bytes,
            sample.cow_bytes,
            "edit {}: upload copies the COW runs, not a padded whole chunk",
            sample.edit
        );
        assert_eq!(
            sample.cow_runs, ROOT_LEVEL as u64,
            "edit {}: one path appends one run per level",
            sample.edit
        );
        assert!(sample.cow_bytes > 0);
        assert_eq!(
            sample.light_cells, 27,
            "edit {}: one dirty cell plus a one-cell apron",
            sample.edit
        );
        assert!(
            sample.material != sample.previous_material,
            "edit {} must change the voxel, not repeat a no-op write",
            sample.edit
        );
        assert!(sample.version <= 1);
    }

    assert_eq!(
        report.bytes.upload_level_start_bytes,
        report.edit_count as u64 * 56,
        "one 7-word levelStart per edit"
    );
    assert_eq!(report.bytes.light_cells_per_edit, 27);
}

#[test]
fn cli_prints_both_terms_and_every_percentile() {
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("edits")
        .arg("--manifest")
        .arg(manifest_path())
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate edits failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    for needle in [
        "M1 gate edits",
        "m1-representative",
        "Provisional fixture",
        "Fixed",
        "Dirty",
        "Upload",
        "Fence",
        "Flip",
        "COW",
        "Light",
        "Upload + fence + flip",
        "COW + light",
        "Fixed + dirty",
        "1,000 measured",
        "8.00 ms",
        "p50",
        "p95",
        "max",
        "27 cells/edit",
        "Scrub budget (M2-24)",
        "Dirty blocks",
        "Remesh",
        "Probe",
        "4.00 ms",
        "Compaction + edit + dirty + remesh + light + probe",
    ] {
        assert!(
            text.contains(needle),
            "the gate edits report must contain `{needle}`:\n{text}"
        );
    }
}

#[test]
fn cli_json_is_the_same_report() {
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("edits")
        .arg("--manifest")
        .arg(manifest_path())
        .arg("--json")
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate edits --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(json["schema_version"], SCHEMA_VERSION);
    assert_eq!(json["edit_count"], EDITS as u64);
    assert_eq!(json["warmup_edits"], WARMUP_EDITS as u64);
    assert_eq!(json["edits"].as_array().map(Vec::len), Some(EDITS));
    assert_eq!(json["budget"]["budget_ns"], BUDGET_NS);
    assert_eq!(
        json["budget"]["within_budget"],
        json["budget"]["margin_ns"].as_i64().unwrap() >= 0
    );
    assert!(json["terms"]["fixed"]["p95_ns"].as_u64().unwrap() > 0);
    assert!(json["terms"]["dirty"]["p95_ns"].as_u64().unwrap() > 0);
    assert_eq!(json["bytes"]["light_cells_per_edit"], 27);
}
