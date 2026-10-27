//! M1-39 acceptance: `gate report` writes one Markdown and one JSON report with
//! all twelve pre-registered rows, a computed verdict and margin for each, and
//! the reference machine.
//!
//! The fixture is frozen at 64³, so the acceptance run is skipped in a 128³
//! build (the harness rejects the edge mismatch explicitly).

#![cfg(not(feature = "chunk128"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/fixtures/m1-representative/manifest.json")
}

fn output_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("voxel-gate-report-{}-{nanos}", std::process::id()))
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn cli_writes_one_markdown_and_one_json_with_the_twelve_rows_and_machine() {
    let out = output_dir();
    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("gate")
        .arg("report")
        .arg("--manifest")
        .arg(manifest_path())
        .arg("--out")
        .arg(&out)
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "gate report failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(stdout.contains("M1 gate report"), "{stdout}");
    assert!(stdout.contains("Machine:"), "{stdout}");

    let md_path = out.join("gate.md");
    let json_path = out.join("gate.json");
    assert!(
        md_path.is_file(),
        "gate.md was not written to {}",
        out.display()
    );
    assert!(
        json_path.is_file(),
        "gate.json was not written to {}",
        out.display()
    );

    let markdown = std::fs::read_to_string(&md_path).expect("gate.md is readable");
    for needle in [
        "# M1 gate report",
        "**Machine:**",
        "**Host:**",
        "**Fixture:**",
        "| # | Pre-registered row |",
        "**PASS**",
        "**FAIL**",
        "**NOT MEASURED**",
        "## Fallback",
        "## Notes",
    ] {
        assert!(markdown.contains(needle), "gate.md must contain `{needle}`");
    }

    let json = read_json(&json_path);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["profile"], "default");
    assert_eq!(json["profile_gated"], true);

    let machine = &json["machine"];
    assert_eq!(machine["reference"]["name"], "RTX 4060 reference");
    assert!(
        machine["reference"]["cpu"]
            .as_str()
            .is_some_and(|cpu| cpu.contains("Ryzen 5 7600")),
        "reference CPU must be named: {machine}"
    );
    assert!(
        machine["reference"]["gpu"]
            .as_str()
            .is_some_and(|gpu| gpu.contains("RTX 4060")),
        "reference GPU must be named: {machine}"
    );
    assert_eq!(machine["reference"]["vram_mib"], 8_188);
    assert_eq!(machine["gpu_probed"], false);
    assert!(
        machine["host_is_reference"].is_boolean(),
        "the report must say whether the host matches the reference: {machine}"
    );

    assert_eq!(json["fixture"]["id"], "m1-representative");
    assert_eq!(json["fixture"]["provisional_until_first_real_save"], true);

    let rows = json["rows"].as_array().expect("rows is an array");
    assert_eq!(rows.len(), 12, "the report must contain all twelve rows");
    let ids: Vec<&str> = rows
        .iter()
        .map(|row| row["id"].as_str().expect("row id"))
        .collect();
    assert_eq!(
        ids,
        [
            "vram_budget",
            "loaded_chunks",
            "node_budget",
            "light_volume",
            "probe_grid",
            "mesh_buffer",
            "edit_latency",
            "probe_update",
            "culling_granularity",
            "safety_factor",
            "measurement_dataset",
            "fallback_trigger",
        ]
    );

    let allowed = ["pass", "fail", "not_measured"];
    for row in rows {
        let verdict = row["verdict"].as_str().expect("row verdict");
        assert!(allowed.contains(&verdict), "unknown verdict `{verdict}`");
        for field in [
            "parameter",
            "pre_registered",
            "measured",
            "limit",
            "margin",
            "source",
            "note",
        ] {
            assert!(
                row[field].as_str().is_some_and(|value| !value.is_empty()),
                "row `{}` is missing `{field}`",
                row["id"]
            );
        }
    }

    // The two latency rows are numeric and compare against the frozen budgets.
    assert_eq!(rows[6]["numbers"]["limit"], 8.0);
    assert_eq!(rows[6]["numbers"]["unit"], "ms");
    assert_eq!(rows[7]["numbers"]["limit"], 4.0);
    assert_eq!(rows[7]["numbers"]["unit"], "ms");

    // The synthetic fixture and the missing granularity run are not green.
    assert_eq!(rows[8]["verdict"], "not_measured");
    assert_eq!(rows[10]["verdict"], "not_measured");
    assert_eq!(rows[11]["verdict"], "pass");

    let pass = rows.iter().filter(|row| row["verdict"] == "pass").count();
    let fail = rows.iter().filter(|row| row["verdict"] == "fail").count();
    let not_measured = rows
        .iter()
        .filter(|row| row["verdict"] == "not_measured")
        .count();
    assert_eq!(json["summary"]["rows"], 12);
    assert_eq!(json["summary"]["pass"], pass);
    assert_eq!(json["summary"]["fail"], fail);
    assert_eq!(json["summary"]["not_measured"], not_measured);
    let expected_status = if fail > 0 {
        "fail"
    } else if not_measured > 0 {
        "incomplete"
    } else {
        "pass"
    };
    assert_eq!(json["summary"]["status"], expected_status);

    // The fallback is computed from the rows, not typed into the report.
    let memory_failed = [0, 2, 3, 4, 5]
        .iter()
        .any(|&index| rows[index]["verdict"] == "fail");
    let latency_failed = rows[6]["verdict"] == "fail"
        || rows[7]["verdict"] == "fail"
        || rows[8]["verdict"] == "fail";
    assert_eq!(
        json["fallback"]["triggered"],
        memory_failed || latency_failed
    );
    if latency_failed {
        assert_eq!(json["fallback"]["rule"], 1);
    } else if memory_failed {
        assert_eq!(json["fallback"]["rule"], 2);
    } else {
        assert_eq!(json["fallback"]["triggered"], false);
    }

    std::fs::remove_dir_all(&out).ok();
}
