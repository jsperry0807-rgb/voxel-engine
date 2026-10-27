//! M1-28 acceptance: the checked-in fixture manifest states its provenance and
//! limitation, the save is complete, and `fixture verify` reproduces it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/fixtures/m1-representative")
}

fn manifest_path() -> PathBuf {
    fixture_dir().join("manifest.json")
}

fn read_manifest() -> Value {
    let path = manifest_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn run_verify(manifest: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("fixture")
        .arg("verify")
        .arg("--manifest")
        .arg(manifest)
        .output()
        .expect("voxel-bench binary runs")
}

#[cfg(not(feature = "chunk128"))]
fn temp_dir(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "voxel-fixture-{name}-{}-{stamp}",
        std::process::id()
    ))
}

#[test]
fn manifest_records_provenance_limitation_and_files() {
    let manifest = read_manifest();
    assert_eq!(manifest["schema_version"].as_u64(), Some(1));
    assert_eq!(manifest["id"].as_str(), Some("m1-representative"));
    assert_eq!(manifest["status"].as_str(), Some("provisional"));
    assert_eq!(
        manifest["provisional_until_first_real_save"].as_bool(),
        Some(true)
    );

    let limitation = manifest["limitation"].as_str().unwrap();
    assert!(
        limitation.contains("Provisional until the first real save"),
        "the limitation must be stated verbatim: {limitation}"
    );
    assert!(limitation.contains("M1-35"));
    assert!(limitation.contains("must not be quoted as gate results"));

    let provenance = &manifest["provenance"];
    assert_eq!(
        provenance["kind"].as_str(),
        Some("synthetic-engine-generated")
    );
    assert_eq!(provenance["captured_from_gameplay"].as_bool(), Some(false));
    assert_eq!(provenance["external_data"].as_bool(), Some(false));
    assert!(provenance["source_commit"].is_string());
    assert!(
        provenance["worldgen"]
            .as_str()
            .unwrap()
            .contains("not docs/worldgen-design.md")
    );

    let recipe = &manifest["recipe"];
    let features: Vec<&str> = recipe["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|feature| feature.as_str().unwrap())
        .collect();
    assert_eq!(
        features,
        [
            "terrain",
            "caves",
            "overhangs",
            "structures",
            "mixed_materials"
        ]
    );

    let format = &manifest["format"];
    assert_eq!(format["chunk_format"].as_str(), Some("VXV2"));
    assert_eq!(format["chunk_bits"].as_u64(), Some(6));
    assert_eq!(format["chunk_dim"].as_u64(), Some(64));

    let palette = manifest["palette"].as_array().unwrap();
    assert_eq!(palette.len(), 20, "air plus 19 built-in materials");
    assert_eq!(palette[0]["key"].as_str(), Some("voxel:air"));
    assert_eq!(palette[1]["id"].as_u64(), Some(1));
    assert_eq!(palette[1]["key"].as_str(), Some("voxel:stone"));

    let observed = &manifest["observed"];
    assert_eq!(observed["live_chunks"].as_u64(), Some(72));
    assert_eq!(observed["total_overrides"].as_u64(), Some(0));
    assert_eq!(observed["superseded_bytes"].as_u64(), Some(0));
    assert_eq!(observed["warnings"].as_array().unwrap().len(), 0);
    assert_eq!(observed["materials"].as_array().unwrap().len(), 19);
    assert!(observed["node_bytes"]["p50"].is_number());
    assert!(observed["voxels"]["p95"].is_number());

    let checks = &manifest["checks"];
    assert!(checks["cave_air_voxels"].as_u64().unwrap() > 1_000);
    assert!(checks["overhang_roof_blocks"].as_u64().unwrap() > 256);
    assert!(checks["structure_blocks"].as_u64().unwrap() > 512);
    assert!(checks["tree_blocks"].as_u64().unwrap() > 512);

    let integrity = &manifest["integrity"];
    assert_eq!(integrity["algorithm"].as_str(), Some("blake3-256"));
    let files = integrity["files"].as_array().unwrap();
    assert_eq!(files.len(), 4);
    let save = fixture_dir().join(manifest["save_root"].as_str().unwrap());
    for file in files {
        let path = save.join(file["path"].as_str().unwrap());
        let bytes =
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert_eq!(bytes.len() as u64, file["bytes"].as_u64().unwrap());
        assert_eq!(
            blake3::hash(&bytes).to_hex().to_string(),
            file["blake3"].as_str().unwrap()
        );
    }
}

#[test]
fn checked_in_fixture_verifies() {
    let output = run_verify(&manifest_path());
    #[cfg(feature = "chunk128")]
    {
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("frozen at 6-bit chunks")
                && stderr.contains("this build uses 7-bit chunks"),
            "the 128³ build must reject the 64³ fixture explicitly: {stderr}"
        );
    }
    #[cfg(not(feature = "chunk128"))]
    {
        assert!(
            output.status.success(),
            "fixture verify failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("rebuild   byte-identical from the manifest recipe"));
        assert!(stdout.contains("provisional until the first real save"));
    }
}

#[cfg(not(feature = "chunk128"))]
#[test]
fn edited_fixture_is_rejected_before_rebuild() {
    let source = fixture_dir();
    let target = temp_dir("tamper");
    std::fs::create_dir_all(target.join("save")).unwrap();
    std::fs::copy(source.join("manifest.json"), target.join("manifest.json")).unwrap();
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(source.join("manifest.json")).unwrap())
            .unwrap();
    let first = manifest["integrity"]["files"][0]["path"].as_str().unwrap();
    let bytes = std::fs::read(source.join("save").join(first)).unwrap();
    std::fs::write(target.join("save").join(first), &bytes[..bytes.len() - 1]).unwrap();

    let output = run_verify(&target.join("manifest.json"));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("hash mismatch") || stderr.contains("size mismatch"),
        "a truncated region file must fail on integrity, not rebuild: {stderr}"
    );
    std::fs::remove_dir_all(target).ok();
}
