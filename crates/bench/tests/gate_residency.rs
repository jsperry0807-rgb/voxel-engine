//! M1-36 acceptance: the configured view/band produces a 64³ loaded chunk
//! count, and the 128³ equivalent at equal loaded volume is exactly
//! `N64 / 8` with the volume identity also asserted.

use std::process::Command;

use voxel_bench::gate::residency::{self, Profile, ResidencyConfig};

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("residency")
        .args(args)
        .output()
        .expect("voxel-bench binary runs")
}

#[test]
fn default_config_matches_the_frozen_default_numbers() {
    let report = residency::default_report().expect("default residency report");
    assert_eq!(report.config.profile, Profile::Default);
    assert!(report.config.gated);
    assert!(!report.config.customized);
    assert_eq!(report.config.near_radius_m, 128);
    assert_eq!(report.config.vertical_shell_m, 32);
    assert_eq!(report.config.vertical_shell_half_m, 16);

    assert_eq!(report.baseline.chunk_dim, 64);
    assert_eq!(report.baseline.edge_m, 4);
    assert_eq!(report.baseline.horizontal_columns, 3_217);
    assert_eq!(report.baseline.vertical_layers, 8);
    assert_eq!(report.baseline.loaded_chunks, 25_736);

    assert_eq!(report.target.chunk_dim, 128);
    assert_eq!(report.target.edge_m, 8);
    assert_eq!(report.target.loaded_chunks, 3_217);
    assert_eq!(report.target.derivation, "equal_volume");

    assert_eq!(report.equal_volume.factor, 8);
    assert_eq!(report.equal_volume.n64, 8 * report.equal_volume.n128);
    assert_eq!(
        report.equal_volume.n64_volume_m3,
        report.equal_volume.n128_volume_m3
    );
    assert!(report.equal_volume.holds);

    let row = report
        .pre_registered
        .as_ref()
        .expect("Default has a frozen row");
    assert_eq!(row.n64, 25_736);
    assert_eq!(row.n128, 3_216);
    assert_eq!(row.n64_delta, 0);
    assert_eq!(row.n128_delta, 1, "equal volume is 25,736 / 8 = 3,217");
}

#[test]
fn minimum_config_follows_the_formula_and_reports_the_table_delta() {
    let report = residency::report(ResidencyConfig::from_profile(Profile::Minimum))
        .expect("minimum residency report");
    assert_eq!(report.config.near_radius_m, 64);
    assert_eq!(report.baseline.horizontal_columns, 805);
    assert_eq!(report.baseline.vertical_layers, 8);
    assert_eq!(report.baseline.loaded_chunks, 6_440);
    assert_eq!(report.target.loaded_chunks, 805);
    assert_eq!(report.equal_volume.n64, 8 * report.equal_volume.n128);
    let row = report.pre_registered.as_ref().unwrap();
    assert_eq!(row.n64_delta, 8);
    assert_eq!(row.n128_delta, 1);
}

#[test]
fn every_profile_holds_the_equal_volume_identity() {
    for profile in Profile::ALL {
        let report = residency::report(ResidencyConfig::from_profile(profile))
            .unwrap_or_else(|error| panic!("{}: {error}", profile.name()));
        assert_eq!(
            report.equal_volume.n64,
            8 * report.equal_volume.n128,
            "{}",
            profile.name()
        );
        assert_eq!(
            report.equal_volume.n64_volume_m3,
            report.equal_volume.n128_volume_m3,
            "{}",
            profile.name()
        );
        assert!(report.equal_volume.holds, "{}", profile.name());
    }
}

#[test]
fn custom_radius_and_band_are_measured_from_the_config() {
    let config = ResidencyConfig {
        profile: Profile::Default,
        near_radius_m: 128,
        vertical_shell_m: 64,
    };
    let report = residency::report(config).expect("custom residency report");
    assert!(report.config.customized);
    assert_eq!(report.baseline.horizontal_columns, 3_217);
    assert_eq!(report.baseline.vertical_layers, 16);
    assert_eq!(report.baseline.loaded_chunks, 51_472);

    // 51,472 is divisible by 8 even though the custom shell is 64 m.
    assert_eq!(report.target.loaded_chunks, 6_434);
    assert_eq!(report.equal_volume.n64, 8 * report.equal_volume.n128);
    assert_eq!(report.pre_registered, None);
}

#[test]
fn a_band_that_cannot_split_into_whole_target_chunks_is_rejected() {
    let config = ResidencyConfig {
        profile: Profile::Default,
        near_radius_m: 128,
        vertical_shell_m: 16,
    };
    let error = residency::report(config).expect_err("partial 128³ chunks are not equal volume");
    assert!(error.contains("not divisible by 8"), "{error}");
}

#[test]
fn cli_prints_the_config_both_counts_and_the_volume_check() {
    let output = run(&[]);
    assert!(
        output.status.success(),
        "gate residency failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");

    for needle in [
        "M1 gate residency (M1-36)",
        "`default` (gated)",
        "near voxel radius 128 m",
        "±16 m vertical shell (32 m total)",
        "64³ baseline",
        "128³ equal volume",
        "25,736",
        "3,217",
        "8 × 3,217 = 25,736",
        "N64 = 8 × N128",
        "1,647,104 m³",
        "Pre-registered prediction",
    ] {
        assert!(
            text.contains(needle),
            "the gate residency report must contain `{needle}`:\n{text}"
        );
    }
}

#[test]
fn cli_profile_selects_the_minimum_counts() {
    let output = run(&["--profile", "minimum"]);
    assert!(
        output.status.success(),
        "gate residency --profile minimum failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(text.contains("`minimum` (gated)"), "{text}");
    assert!(text.contains("near voxel radius 64 m"), "{text}");
    assert!(text.contains("6,440"), "{text}");
    assert!(text.contains("805"), "{text}");
    assert!(text.contains("8 × 805 = 6,440"), "{text}");
    assert!(text.contains("N64 = 6,432, N128 = 804"), "{text}");
    assert!(text.contains("(delta +8 / +1)"), "{text}");
}

#[test]
fn cli_overrides_the_configured_view_and_band() {
    let output = run(&["--profile", "default", "--radius", "128", "--band", "64"]);
    assert!(
        output.status.success(),
        "custom gate residency failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(text.contains("(gated, customized)"), "{text}");
    assert!(text.contains("near voxel radius 128 m"), "{text}");
    assert!(text.contains("±32 m vertical shell (64 m total)"), "{text}");
    assert!(text.contains("51,472"), "{text}");
    assert!(text.contains("6,434"), "{text}");
}

#[test]
fn cli_json_is_the_same_report() {
    let output = run(&["--json"]);
    assert!(
        output.status.success(),
        "gate residency --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(json["schema_version"], residency::SCHEMA_VERSION);
    assert_eq!(json["config"]["profile"], "default");
    assert_eq!(json["config"]["near_radius_m"], 128);
    assert_eq!(json["config"]["vertical_shell_m"], 32);
    assert_eq!(json["baseline"]["loaded_chunks"], 25_736);
    assert_eq!(json["baseline"]["chunk_dim"], 64);
    assert_eq!(json["target"]["loaded_chunks"], 3_217);
    assert_eq!(json["target"]["chunk_dim"], 128);
    assert_eq!(json["equal_volume"]["factor"], 8);
    assert_eq!(json["equal_volume"]["holds"], true);
    assert_eq!(
        json["equal_volume"]["n64_volume_m3"],
        json["equal_volume"]["n128_volume_m3"]
    );
    assert_eq!(json["pre_registered"]["n64_delta"], 0);
    assert_eq!(json["pre_registered"]["n128_delta"], 1);
}

#[test]
fn cli_rejects_an_unknown_profile() {
    let output = run(&["--profile", "ultra"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("unknown profile `ultra`"), "{stderr}");
}
