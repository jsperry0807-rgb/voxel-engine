//! M1-40 acceptance: the analytic projection reproduces the measured 64³ terms
//! when asked for 64³, and prints both the optimistic and pessimistic 128³
//! projections.
//!
//! The fixture is frozen at 64³, so these acceptance tests are skipped in a
//! 128³ build. The unit tests in `gate::scaleup` cover the arithmetic directly.

#![cfg(not(feature = "chunk128"))]

use std::path::PathBuf;
use std::process::Command;

use voxel_bench::gate::residency::{Profile, ResidencyConfig};
use voxel_bench::gate::scaleup::{self, ProjectionMode, SCHEMA_VERSION};
use voxel_core::constants::{CHUNK_BITS, TARGET_CHUNK_BITS};

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/fixtures/m1-representative/manifest.json")
}

#[test]
fn scaleup_report_projects_both_modes_from_the_measured_64_inputs() {
    let report = scaleup::report(&manifest_path()).expect("the fixture supports a scale-up report");

    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert!(report.is_provisional(), "the M1 fixture is still synthetic");
    assert_eq!(report.fixture.id, "m1-representative");
    assert_eq!(report.fixture.status, "provisional");
    assert_eq!(report.source.chunk_bits, CHUNK_BITS);
    assert_eq!(report.target.chunk_bits, TARGET_CHUNK_BITS);
    assert_eq!(report.source.chunk_dim, 64);
    assert_eq!(report.target.chunk_dim, 128);
    assert_eq!(report.source.levels, 7);
    assert_eq!(report.target.levels, 8);
    assert_eq!(report.equal_volume_factor, 8);
    assert_eq!(report.profile, "default");
    assert_eq!(report.n64, 8 * report.n128);
    assert_eq!(report.n64, 25_736);
    assert_eq!(report.n128, 3_217);

    assert!(report.measured.edit.total_ns > 0);
    assert!(report.measured.probes.update_ns > 0);
    assert!(report.measured.memory.total_bytes > 0);
    assert_eq!(report.measured.edit.light_cells_per_edit, 27);
    assert_eq!(report.measured.probes.probes_per_chunk, 64);
    assert_eq!(report.measured.probes.rays_per_probe, 64);

    for projection in [&report.optimistic, &report.pessimistic] {
        assert!(!projection.same_edge);
        assert_eq!(
            projection.edit.total_ns,
            projection.edit.fixed_ns + projection.edit.dirty_ns,
            "{}: total is fixed + dirty",
            projection.mode.name()
        );
        assert_eq!(
            projection.edit.fixed_ns,
            projection.edit.upload_ns + projection.edit.fence_ns + projection.edit.flip_ns,
            "{}: fixed is upload + fence + flip",
            projection.mode.name()
        );
        assert_eq!(
            projection.edit.dirty_ns,
            projection.edit.cow_ns + projection.edit.light_ns,
            "{}: dirty is COW + light",
            projection.mode.name()
        );
        assert_eq!(
            projection.edit.margin_ns,
            projection.edit.budget_ns as i64 - projection.edit.total_ns as i64
        );
        assert_eq!(
            projection.probes.update_ns,
            projection.probes.select_ns + projection.probes.rebake_ns + projection.probes.upload_ns
        );
        assert_eq!(
            projection.probes.margin_ns,
            projection.probes.budget_ns as i64 - projection.probes.update_ns as i64
        );
        assert_eq!(
            projection.memory.projected_bytes,
            projection.memory.total_bytes_per_chunk * projection.memory.loaded_chunks
        );
        assert_eq!(projection.memory.loaded_chunks, report.n128);
        assert_eq!(
            projection.mode.name(),
            match projection.mode {
                ProjectionMode::Optimistic => "optimistic",
                ProjectionMode::Pessimistic => "pessimistic",
            }
        );
    }

    assert_eq!(
        report.optimistic.edit.fixed_ns, report.pessimistic.edit.fixed_ns,
        "the dirty span cannot change the fixed term"
    );
    assert_eq!(report.optimistic.edit.light_cells, 27);
    assert_eq!(report.pessimistic.edit.light_cells, 512);
    assert_eq!(
        report.optimistic.probes.affected_probes,
        report.measured.probes.affected_probes_per_edit * 4,
        "the optimistic probe set grows with surface area"
    );
    assert_eq!(report.pessimistic.probes.affected_probes, 512);
    assert_eq!(report.pessimistic.probes.rays_baked, 512 * 64);
    assert!(
        report.pessimistic.edit.total_ns >= report.optimistic.edit.total_ns,
        "the pessimistic projection cannot be cheaper"
    );
    assert!(
        report.pessimistic.probes.update_ns >= report.optimistic.probes.update_ns,
        "the pessimistic probe update cannot be cheaper"
    );
    assert!(
        report.pessimistic.memory.projected_bytes >= report.optimistic.memory.projected_bytes,
        "the pessimistic memory projection cannot be cheaper"
    );

    // Acceptance: fed the measured 64³ inputs and asked for 64³, the model
    // returns those exact numbers rather than extrapolating.
    for mode in [ProjectionMode::Optimistic, ProjectionMode::Pessimistic] {
        let identity = scaleup::project(
            &report.measured,
            report.source,
            report.source,
            report.n64,
            mode,
        );
        assert!(identity.same_edge);
        assert_eq!(identity.edit.total_ns, report.measured.edit.total_ns);
        assert_eq!(identity.edit.fixed_ns, report.measured.edit.fixed_ns);
        assert_eq!(identity.edit.dirty_ns, report.measured.edit.dirty_ns);
        assert_eq!(identity.edit.light_cells, 27);
        assert_eq!(identity.probes.update_ns, report.measured.probes.update_ns);
        assert_eq!(
            identity.memory.total_bytes_per_chunk,
            report.measured.memory.total_bytes
        );
        assert_eq!(
            identity.memory.projected_bytes,
            report.measured.memory.total_bytes * report.n64
        );
    }

    let markdown = report.markdown();
    for needle in [
        "# M1 gate scale-up (M1-40)",
        "Measured 64³ input",
        "128³ projection",
        "Optimistic",
        "Pessimistic",
        "Edit fixed",
        "Edit dirty",
        "Edit total",
        "Probe update",
        "Memory projected",
        "DAG nodes/chunk",
        "Provisional fixture",
        "512 (the whole target chunk)",
        "32,768 rays",
    ] {
        assert!(
            markdown.contains(needle),
            "the scale-up report must contain `{needle}`:\n{markdown}"
        );
    }
}

#[test]
fn scaleup_minimum_profile_uses_the_smaller_loaded_count() {
    let manifest = manifest_path();
    let report = scaleup::report_for(&manifest, ResidencyConfig::from_profile(Profile::Minimum))
        .expect("the Minimum profile supports a scale-up report");
    let default = scaleup::report(&manifest).expect("the Default profile still works");

    assert_eq!(report.profile, "minimum");
    assert_eq!(report.n64, 6_440);
    assert_eq!(report.n128, 805);
    assert_eq!(report.n64, 8 * report.n128);
    assert_eq!(report.optimistic.memory.loaded_chunks, 805);
    assert_eq!(report.pessimistic.memory.loaded_chunks, 805);
    assert_eq!(
        report.optimistic.memory.total_bytes_per_chunk,
        default.optimistic.memory.total_bytes_per_chunk,
        "the per-chunk projection does not depend on the profile"
    );
    assert!(
        report.optimistic.memory.projected_bytes < default.optimistic.memory.projected_bytes,
        "the Minimum profile loads fewer chunks"
    );
    assert!(report.markdown().contains("**Profile:** `minimum`"));
}

#[test]
fn scaleup_cli_accepts_the_minimum_profile() {
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("scaleup")
        .arg("--manifest")
        .arg(manifest_path())
        .arg("--profile")
        .arg("minimum")
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate scaleup --profile minimum failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    for needle in ["**Profile:** `minimum`", "N128 = 805", "Profile"] {
        assert!(
            text.contains(needle),
            "the Minimum scale-up report must contain `{needle}`:\n{text}"
        );
    }
}

#[test]
fn scaleup_cli_prints_both_projections() {
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("scaleup")
        .arg("--manifest")
        .arg(manifest_path())
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate scaleup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    for needle in [
        "M1 gate scale-up",
        "m1-representative",
        "Provisional fixture",
        "Source edge",
        "Target edge",
        "N64",
        "N128",
        "Optimistic",
        "Pessimistic",
        "Edit fixed",
        "Edit dirty",
        "Edit total",
        "Probe update",
        "Memory projected",
    ] {
        assert!(
            text.contains(needle),
            "the gate scaleup report must contain `{needle}`:\n{text}"
        );
    }
}

#[test]
fn scaleup_cli_json_is_the_same_report() {
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("scaleup")
        .arg("--manifest")
        .arg(manifest_path())
        .arg("--json")
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate scaleup --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(json["schema_version"], SCHEMA_VERSION);
    assert_eq!(json["source"]["chunk_dim"], 64);
    assert_eq!(json["target"]["chunk_dim"], 128);
    assert_eq!(json["equal_volume_factor"], 8);
    assert_eq!(json["n64"], 25_736);
    assert_eq!(json["n128"], 3_217);
    assert_eq!(json["optimistic"]["mode"], "optimistic");
    assert_eq!(json["pessimistic"]["mode"], "pessimistic");
    assert_eq!(json["optimistic"]["same_edge"], false);
    assert_eq!(json["optimistic"]["edit"]["light_cells"], 27);
    assert_eq!(json["pessimistic"]["edit"]["light_cells"], 512);
    assert_eq!(json["pessimistic"]["probes"]["rays_baked"], 32_768);
    assert!(json["optimistic"]["edit"]["total_ns"].as_u64().unwrap() > 0);
    assert!(
        json["pessimistic"]["memory"]["projected_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
}
