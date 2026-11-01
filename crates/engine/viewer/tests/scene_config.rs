//! M3-13: the frozen scene configuration matches the code.
//!
//! `benchmarks/m3-scene.json` is the checked-in freeze for the M3 scene. This
//! test is the enforcement: changing a constant without updating the freeze (or
//! the reverse) fails here, so the tag cannot describe a build that no longer
//! exists.

use std::path::PathBuf;

use voxel_core::constants::CHUNK_DIM;
use voxel_viewer::frametime;
use voxel_viewer::memory;
use voxel_viewer::scene;
use voxel_viewer::screenshot;
use voxel_viewer::streaming::{PUMP_BUDGET, RING_CHUNKS, RING_HALF};
use voxel_viewer::upload;

fn config() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../benchmarks/m3-scene.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn the_frozen_scene_matches_the_code() {
    let config = config();
    let scene_json = &config["scene"];

    assert_eq!(
        scene_json["seed"].as_u64(),
        Some(u64::from(scene::DEFAULT_SEED))
    );
    assert_eq!(
        scene_json["seed_hex"].as_str(),
        Some(format!("0x{:08X}", scene::DEFAULT_SEED).as_str())
    );
    assert_eq!(scene_json["ring_half"].as_i64(), Some(i64::from(RING_HALF)));
    assert_eq!(scene_json["ring_chunks"].as_u64(), Some(RING_CHUNKS as u64));

    // The gate is 128³; the default build stays at the 64³ baseline.
    assert_eq!(scene_json["chunk_dim_gate"].as_u64(), Some(128));
    assert_eq!(scene_json["chunk_dim_default"].as_u64(), Some(64));
    let expected = if cfg!(feature = "chunk128") { 128 } else { 64 };
    assert_eq!(
        CHUNK_DIM, expected,
        "the build's chunk edge must be the one the config names for it"
    );
}

#[test]
fn the_frozen_streaming_budgets_match_the_code() {
    let config = config();
    let streaming = &config["streaming"];
    assert_eq!(
        streaming["pump_budget_ms"].as_u64(),
        Some(PUMP_BUDGET.as_millis() as u64)
    );
    assert_eq!(
        streaming["upload_budget_kib"].as_u64(),
        Some(upload::DEFAULT_UPLOAD_BYTES as u64 / 1024)
    );
    assert_eq!(
        streaming["staging_chunk_kib"].as_u64(),
        Some(upload::STAGING_CHUNK_BYTES / 1024)
    );
    assert_eq!(
        streaming["max_backlog_kib"].as_u64(),
        Some(upload::MAX_BACKLOG_BYTES as u64 / 1024)
    );
}

#[test]
fn the_frozen_gate_settings_match_the_code() {
    let config = config();
    let gate = &config["gate"];
    assert_eq!(
        gate["frametime_frames"].as_u64(),
        Some(frametime::DEFAULT_FRAMES)
    );
    assert_eq!(
        gate["settle_frames"].as_u64(),
        Some(u64::from(screenshot::DEFAULT_SETTLE_FRAMES))
    );
    assert_eq!(
        gate["resolution"].as_array().map(|values| values.len()),
        Some(2)
    );
    let reference = gate["reference_image"].as_str().expect("a reference path");
    assert!(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(reference)
            .is_file(),
        "the frozen reference image exists: {reference}"
    );
    assert_eq!(
        config["budgets"]["vram_margin"].as_f64(),
        Some(memory::MARGIN)
    );
}
