//! Frame-time lens acceptance: a viewer trace becomes a Markdown and JSON
//! report with the average, 1% low and maximum-hitch verdicts.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use voxel_bench::frame::{FrameReport, FrameTarget};
use voxel_bench::report::{ProcessReport, Report};

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("voxel-frame-{name}-{}-{nanos}", std::process::id()))
}

fn write_trace(path: &Path, frames: u64, interval_ns: u64) {
    let mut body = String::from("frame,t_ns,frame_ns\n");
    let mut t_ns = 0u64;
    for frame in 1..=frames {
        t_ns = t_ns.saturating_add(interval_ns);
        body.push_str(&format!("{frame},{t_ns},{interval_ns}\n"));
    }
    std::fs::write(path, body).expect("write trace");
}

#[test]
fn cli_writes_markdown_and_json_with_the_three_checks() {
    let dir = temp_dir("cli");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let trace = dir.join("frames.csv");
    write_trace(&trace, 120, 16_000_000);

    let output = Command::new(env!("CARGO_BIN_EXE_voxel-bench"))
        .arg("frame")
        .arg("--input")
        .arg(&trace)
        .arg("--warmup")
        .arg("20")
        .arg("--out")
        .arg(&dir)
        .output()
        .expect("voxel-bench binary runs");

    assert!(
        output.status.success(),
        "frame report failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(stdout.contains("Frame-time report"), "{stdout}");
    assert!(stdout.contains("1% low"), "{stdout}");
    assert!(stdout.contains("**Verdict: PASS**"), "{stdout}");

    let md_path = dir.join("frame.md");
    let json_path = dir.join("frame.json");
    assert!(md_path.is_file(), "frame.md was not written");
    assert!(json_path.is_file(), "frame.json was not written");

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).expect("read frame.json"))
            .expect("parse frame.json");
    assert_eq!(json["frames"], 100);
    assert_eq!(json["warmup_frames"], 20);
    assert_eq!(json["hitches"], 0);
    assert_eq!(json["passed"], true);
    assert_eq!(json["checks"].as_array().map(Vec::len), Some(3));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_main_report_and_history_carry_the_frame_section() {
    let dir = temp_dir("section");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let trace = dir.join("frames.csv");
    write_trace(&trace, 120, 16_000_000);
    let frames =
        FrameReport::from_trace(&trace, 20, FrameTarget::default()).expect("build frame report");

    let report = Report {
        meta: voxel_bench::report::meta(),
        process: ProcessReport::default(),
        memory: Vec::new(),
        cpu: Vec::new(),
        latency: Vec::new(),
        layout: Vec::new(),
        frames: Some(frames),
    };
    let markdown = report.markdown();
    assert!(
        markdown.contains("## Frame time (M3-09/M3-10)"),
        "{markdown}"
    );
    assert!(markdown.contains("| Average |"), "{markdown}");
    assert!(markdown.contains("| 1% low |"), "{markdown}");

    let metrics = voxel_bench::history::summarize(&report);
    assert_eq!(metrics.get("frame_avg_fps"), Some(&62.5));
    assert_eq!(metrics.get("frame_one_percent_low_fps"), Some(&62.5));
    assert_eq!(metrics.get("frame_max_ns"), Some(&16_000_000.0));

    std::fs::remove_dir_all(dir).ok();
}
