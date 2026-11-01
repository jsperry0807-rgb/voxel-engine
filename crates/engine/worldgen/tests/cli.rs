//! M2.5-05: the `voxel-worldgen macro` command line.
//!
//! The CLI is the task's verify command, so these tests execute the real
//! binary. The full 4,096-tile run is documented rather than asserted; a
//! limited run keeps the test fast while still exercising argument parsing,
//! hashing and tile writing.

use std::process::{Command, Output};

use voxel_worldgen::MacroTile;
use voxel_worldgen::tile::TILE_BYTES;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_voxel-worldgen"))
        .args(args)
        .output()
        .expect("the test binary must be runnable")
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn hash_line(text: &str) -> String {
    text.lines()
        .find(|line| line.contains("hash="))
        .unwrap_or_else(|| panic!("no hash line in {text:?}"))
        .to_owned()
}

#[test]
fn cli_macro_reports_a_stable_hash() {
    let first = stdout(&run(&["macro", "--seed", "7", "--limit", "2"]));
    let second = stdout(&run(&["macro", "--seed", "7", "--limit", "2"]));

    assert!(first.contains("tiles=2"), "{first}");
    assert_eq!(hash_line(&first), hash_line(&second));
}

#[test]
fn cli_macro_writes_tiles_the_library_can_decode() {
    let directory =
        std::env::temp_dir().join(format!("voxel-worldgen-cli-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let directory_arg = directory.to_string_lossy().into_owned();

    let text = stdout(&run(&[
        "macro",
        "--seed",
        "0x20",
        "--limit",
        "1",
        "--out",
        &directory_arg,
    ]));
    assert!(text.contains("tiles=1"), "{text}");

    let path = directory.join("0_0.vxt");
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"));
    assert_eq!(bytes.len(), TILE_BYTES);
    let tile = MacroTile::from_bytes(&bytes).expect("the written tile must decode");
    assert_eq!(tile.header().seed, 0x20);

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn cli_macro_rejects_bad_arguments() {
    assert!(!run(&["nope"]).status.success());
    assert!(!run(&["macro", "--seed", "not-a-number"]).status.success());
    assert!(!run(&["macro", "--limit"]).status.success());
    assert!(!run(&["macro", "--unknown"]).status.success());
}

#[test]
fn cli_help_prints_usage() {
    let text = stdout(&run(&["--help"]));
    assert!(text.contains("voxel-worldgen macro"), "{text}");
    assert!(text.contains("--seed"), "{text}");
}

#[test]
fn cli_hydro_reports_a_stable_hash() {
    let first = stdout(&run(&["hydro", "--seed", "3", "--size", "32"]));
    let second = stdout(&run(&["hydro", "--seed", "3", "--size", "32"]));

    assert!(first.contains("lake_cells="), "{first}");
    assert_eq!(hash_line(&first), hash_line(&second));
}

#[test]
fn cli_hydro_rejects_a_bad_size() {
    assert!(!run(&["hydro", "--size", "0"]).status.success());
    assert!(!run(&["hydro", "--size", "9000"]).status.success());
    assert!(!run(&["hydro", "--size", "nope"]).status.success());
}

#[test]
fn cli_rivers_reports_a_stable_hash() {
    let first = stdout(&run(&[
        "rivers",
        "--seed",
        "7",
        "--size",
        "96",
        "--threshold",
        "8",
    ]));
    let second = stdout(&run(&[
        "rivers",
        "--seed",
        "7",
        "--size",
        "96",
        "--threshold",
        "8",
    ]));

    assert!(first.contains("segments="), "{first}");
    assert!(first.contains("width_m="), "{first}");
    assert_eq!(hash_line(&first), hash_line(&second));
}

#[test]
fn cli_rivers_rejects_bad_threshold() {
    assert!(!run(&["rivers", "--threshold", "0"]).status.success());
    assert!(!run(&["rivers", "--threshold", "nope"]).status.success());
    assert!(!run(&["rivers", "--size", "0"]).status.success());
}

#[test]
fn cli_lakes_reports_a_stable_hash() {
    let first = stdout(&run(&[
        "lakes",
        "--seed",
        "7",
        "--size",
        "96",
        "--min-cells",
        "2",
    ]));
    let second = stdout(&run(&[
        "lakes",
        "--seed",
        "7",
        "--size",
        "96",
        "--min-cells",
        "2",
    ]));

    assert!(first.contains("lakes="), "{first}");
    assert!(first.contains("largest_cells="), "{first}");
    assert_eq!(hash_line(&first), hash_line(&second));
}

#[test]
fn cli_lakes_rejects_a_bad_minimum() {
    assert!(!run(&["lakes", "--min-cells", "0"]).status.success());
    assert!(!run(&["lakes", "--min-cells", "nope"]).status.success());
}
