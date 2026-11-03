//! M3-13: the bench-side constants in the frozen scene config.
//!
//! The viewer test pins the scene and streaming knobs; this one pins the
//! comparison thresholds, which live in `voxel-bench`.

use std::path::PathBuf;

use voxel_bench::image_diff::Threshold;

fn config() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/m3-scene.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn the_frozen_image_threshold_matches_the_code() {
    let config = config();
    let threshold = &config["gate"]["image_threshold"];
    let defaults = Threshold::default();
    assert_eq!(
        threshold["max_fraction"].as_f64(),
        Some(defaults.max_fraction)
    );
    assert_eq!(
        threshold["channel_tolerance"].as_u64(),
        Some(u64::from(defaults.channel_tolerance))
    );
    assert_eq!(
        threshold["max_delta"].as_u64(),
        Some(u64::from(defaults.max_delta))
    );
    assert_eq!(
        threshold["max_mean_abs"].as_f64(),
        Some(defaults.max_mean_abs)
    );
}
