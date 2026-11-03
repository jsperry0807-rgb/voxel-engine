//! Command-line entry points for the benchmark suite.
//!
//! `main.rs` only installs the counting allocator and calls [`main`] here. Each
//! command lives in a sibling module; the shared imports are re-exported so the
//! modules can `use super::*` without repeating the same ten lines.

pub(crate) use std::path::PathBuf;
pub(crate) use std::time::Instant;

pub(crate) use crate::frame::{FrameReport, FrameTarget};
pub(crate) use crate::report::{ProcessReport, Report};
pub(crate) use crate::{history, proc, report, suite};

mod artifacts;
mod fixture;
mod format;
mod gate;
mod sim;

pub(crate) use artifacts::*;
pub(crate) use fixture::*;
pub(crate) use format::*;
pub(crate) use gate::*;
pub(crate) use sim::*;

enum Mode {
    All,
    Memory,
    Cpu,
    Latency,
    Layout,
}

pub fn main() {
    let mut mode = Mode::All;
    let mut out_dir = PathBuf::from("target/bench");
    let mut print = false;
    let mut frames: Option<PathBuf> = None;
    let mut frame_warmup = 0usize;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "record") {
        record_mode(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "refresh") {
        refresh_mode(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "save-scan") {
        save_scan_mode(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "fixture") {
        fixture_mode(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "frame") {
        frame_mode(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "gate") {
        gate_mode(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "hitch") {
        hitch_mode(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "scene-memory") {
        scene_memory_mode(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "image-diff") {
        image_diff_mode(&args[1..]);
        return;
    }

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "all" => mode = Mode::All,
            "memory" => mode = Mode::Memory,
            "cpu" => mode = Mode::Cpu,
            "latency" => mode = Mode::Latency,
            "layout" => mode = Mode::Layout,
            "--out" => {
                out_dir = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| usage("--out needs a directory"));
            }
            "--frames" => {
                frames = Some(PathBuf::from(
                    args.next()
                        .unwrap_or_else(|| usage("--frames needs a file")),
                ));
            }
            "--frame-warmup" => {
                let value = args
                    .next()
                    .unwrap_or_else(|| usage("--frame-warmup needs a frame count"));
                frame_warmup = value.parse().unwrap_or_else(|_| {
                    usage(&format!(
                        "--frame-warmup needs a whole number, got `{value}`"
                    ))
                });
            }
            "--print" => print = true,
            "-h" | "--help" => {
                println!(
                    "usage: voxel-bench [all|memory|cpu|latency|layout] [--out DIR] [--print]\n\
                     \x20                  [--frames FILE] [--frame-warmup N]\n\
                     \n\
                     all      run the memory, layout, CPU and latency lenses (default)\n\
                     memory   heap bytes, allocation churn and peak heap\n\
                     layout   bytes/node, runs, dedup and COW cost per fixture\n\
                     cpu      process CPU time, ns/op, Mops/s and allocations/op\n\
                     latency  p50/p95/p99/max for the interactive paths\n\
                     --out    report directory (default: target/bench)\n\
                     --print  also print the Markdown report\n\
                     --frames  attach a viewer frame trace to the report\n\
                     --frame-warmup  discard this many trace samples (default: 0)\n\
                     \n\
                     voxel-bench frame --input FILE [--warmup N] [--out DIR]\n\
                     \x20                 [--target-fps F] [--one-percent-fps F]\n\
                     \x20                 [--hitch-ms MS] [--json]\n\
                     \n\
                     frame    average, 1% low and maximum-hitch report over a\n\
                     \x20        frame trace written by VOXEL_FRAME_PROFILE\n\
                     --input  frame trace CSV (required)\n\
                     --warmup  discard this many samples from the front\n\
                     --target-fps     average frame-rate target (default: 60)\n\
                     --one-percent-fps  1% low target (default: 45)\n\
                     --hitch-ms       frame time that counts as a hitch (default: 33.3)\n\
                     --out            output directory (default: target/frame)\n\
                     --json           print JSON instead of Markdown\n\
                     \n\
                     voxel-bench record [--from DIR] [--history DIR] [--label NAME]\n\
                     \x20                  [--note TEXT] [--attach FILE]...\n\
                     \n\
                     record   archive a run under benchmarks/runs and refresh\n\
                     \x20        benchmarks/HISTORY.md\n\
                     \n\
                     voxel-bench refresh [--history DIR]\n\
                     \n\
                     refresh  regenerate HISTORY.md from history.json\n\
                     \n\
                     voxel-bench save-scan --path DIR [--out FILE]\n\
                     \n\
                     save-scan  walk a save directory without modifying it and\n\
                     \x20          emit JSON: per-chunk node bytes, voxel counts,\n\
                     \x20          material histograms and p50/p95/max\n\
                     \n\
                     --path     save directory to scan (required)\n\
                     --out      write the JSON report to a file instead of stdout\n\
                     \n\
                     voxel-bench fixture build [--out DIR]\n\
                     voxel-bench fixture verify [--manifest FILE]\n\
                     \n\
                     fixture build  freeze the M1-28 fixture world and manifest\n\
                     fixture verify check hashes, scan the save and rebuild it\n\
                     --out          output directory (default: benchmarks/fixtures/m1-representative)\n\
                     --manifest     manifest to verify (default: <out>/manifest.json)\n\
                     --chunk-min    recipe minimum chunk, x,y,z (default: -3,0,-3)\n\
                     --chunk-max    recipe maximum chunk, x,y,z (default: 2,1,2)\n\
                     --seed         recipe seed (default: the M1 fixture seed)\n\
                     \n\
                     voxel-bench gate memory [--manifest FILE] [--out FILE] [--json]\n\
                     \n\
                     gate memory  per-chunk memory distribution over the frozen\n\
                     \x20          fixture: DAG, overlay, light, probes, mesh\n\
                     \x20          estimate and total, with p50/p95/max (M1-35)\n\
                     --manifest   fixture manifest (default: benchmarks/fixtures/m1-representative/manifest.json)\n\
                     --out        also write the JSON report to FILE\n\
                     --json       print the JSON report instead of the Markdown table\n\
                     \n\
                     voxel-bench gate residency [--profile NAME] [--radius M] [--band M]\n\
                     \x20                  [--out FILE] [--json]\n\
                     \n\
                     gate residency  N64 for the configured view/band and the\n\
                     \x20          equal-volume 128³ equivalent (M1-36)\n\
                     --profile    minimum | default | high (default: default)\n\
                     --radius     near voxel radius in metres (overrides the profile)\n\
                     --band       full vertical shell in metres (overrides the profile)\n\
                     --out        also write the JSON report to FILE\n\
                     --json       print the JSON report instead of the Markdown table\n\
                     \n\
                     voxel-bench gate edits [--manifest FILE] [--out FILE] [--json]\n\
                     \n\
                     gate edits  per-edit latency over 1,000 edits on the frozen\n\
                     \x20          fixture, split into fixed (upload/fence/flip) and\n\
                     \x20          dirty-region (COW/light) terms with p50/p95/max (M1-37)\n\
                     --manifest   fixture manifest (default: benchmarks/fixtures/m1-representative/manifest.json)\n\
                     --out        also write the JSON report to FILE\n\
                     --json       print the JSON report instead of the Markdown table\n\
                     \n\
                     voxel-bench gate probes [--manifest FILE] [--out FILE] [--json]\n\
                     \n\
                     gate probes  incremental probe-update latency over 1,000 edits\n\
                     \x20          on the frozen fixture, with p50/p95/max and the\n\
                     \x20          4 ms per-edited-chunk budget (M1-38)\n\
                     --manifest   fixture manifest (default: benchmarks/fixtures/m1-representative/manifest.json)\n\
                     --out        also write the JSON report to FILE\n\
                     --json       print the JSON report instead of the Markdown table\n\
                     \n\
                     voxel-bench gate granularity [--manifest FILE] [--out FILE] [--json]\n\
                     \n\
                     gate granularity  reference traversed-vs-visible block volume\n\
                     \x20          over 100 poses on the frozen save (M1-34, row 9)\n\
                     --manifest   fixture manifest (default: benchmarks/fixtures/m1-representative/manifest.json)\n\
                     --out        also write the JSON report to FILE\n\
                     --json       print the JSON report instead of the Markdown table\n\
                     \n\
                     voxel-bench gate scaleup [--manifest FILE] [--profile NAME]\n\
                     \x20                  [--out FILE] [--json]\n\
                     \n\
                     gate scaleup  project the measured 64³ edit, probe and memory\n\
                     \x20          terms to 128³, with optimistic (one dirty cell)\n\
                     \x20          and pessimistic (dirty spans the chunk) columns;\n\
                     \x20          fixed terms are corrected rather than multiplied (M1-40)\n\
                     --manifest   fixture manifest (default: benchmarks/fixtures/m1-representative/manifest.json)\n\
                     --profile    residency profile: minimum, default or high\n\
                     --out        also write the JSON report to FILE\n\
                     --json       print the JSON report instead of the Markdown table\n\
                     \n\
                     voxel-bench image-diff --reference FILE --candidate FILE
                     \x20          [--max-fraction F] [--tolerance N] [--max-delta N]
                     \x20          [--max-mean-abs F] [--json]
                     \n\
                     image-diff   compare a capture against the committed reference
                     \x20          (M3-12); exits 1 outside the documented threshold
                     --reference  committed PNG
                     --candidate  new capture to check
                     --max-fraction  allowed differing fraction (default: 0.0001)
                     --tolerance  per-channel delta treated as noise (default: 2)
                     --max-delta  largest allowed channel delta (default: 32)
                     --max-mean-abs largest allowed mean delta (default: 0.5)
                     \n\
                     voxel-bench scene-memory [--seed S] [--upload-kib K] [--json]
                     \n\
                     scene-memory  VRAM and host for the 256-chunk scene, split by
                     \x20          DAG/mesh/overlay/light/probes (M3-11)
                     --seed       scene seed, decimal or 0x hex
                     --upload-kib per-frame upload budget (default: 1024)
                     --json       print a JSON report instead of the Markdown table
                     \n\
                     voxel-bench gate report [--manifest FILE] [--profile NAME]\n\
                     \x20                  [--out DIR] [--json]\n\
                     \n\
                     gate report  run the M1 harnesses and write one gate.md plus one\n\
                     \x20          gate.json with the twelve pre-registered rows, a\n\
                     \x20          computed verdict and margin for each, and the\n\
                     \x20          reference machine (M1-39)\n\
                     --manifest   fixture manifest (default: benchmarks/fixtures/m1-representative/manifest.json)\n\
                     --profile    minimum | default | high (default: default)\n\
                     --out        output directory (default: target/gate)\n\
                     --json       print the JSON report instead of the Markdown table"
                );
                return;
            }
            other => usage(&format!("unknown argument `{other}`")),
        }
    }

    let frames = match frames {
        Some(path) => match FrameReport::from_trace(&path, frame_warmup, FrameTarget::default()) {
            Ok(report) => Some(report),
            Err(error) => {
                eprintln!("failed to read frame trace {}: {error}", path.display());
                std::process::exit(1);
            }
        },
        None => None,
    };

    let rss_start = proc::rss_bytes();
    let cpu_start = proc::cpu_time();
    let wall_start = Instant::now();

    let memory = matches!(mode, Mode::All | Mode::Memory)
        .then(suite::memory)
        .unwrap_or_default();
    let layout = matches!(mode, Mode::All | Mode::Layout)
        .then(suite::layout)
        .unwrap_or_default();
    let cpu = matches!(mode, Mode::All | Mode::Cpu)
        .then(suite::cpu)
        .unwrap_or_default();
    let latency = matches!(mode, Mode::All | Mode::Latency)
        .then(suite::latency)
        .unwrap_or_default();

    let wall_ns = wall_start.elapsed().as_nanos() as u64;
    let cpu_ns = match (cpu_start, proc::cpu_time()) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a).as_nanos() as u64),
        _ => None,
    };
    let cpu_percent = cpu_ns.map(|cpu| cpu as f64 / wall_ns as f64 * 100.0);

    let result = Report {
        meta: report::meta(),
        process: ProcessReport {
            rss_start,
            rss_end: proc::rss_bytes(),
            peak_rss: proc::peak_rss_bytes(),
            wall_ns,
            cpu_ns,
            cpu_percent,
        },
        memory,
        cpu,
        latency,
        layout,
        frames,
    };

    if let Err(error) = result.write(&out_dir) {
        eprintln!("failed to write report to {}: {error}", out_dir.display());
        std::process::exit(1);
    }

    println!(
        "wrote {} and {} ({} memory, {} layout, {} CPU, {} latency cases{}) in {}",
        out_dir.join("report.md").display(),
        out_dir.join("report.json").display(),
        result.memory.len(),
        result.layout.len(),
        result.cpu.len(),
        result.latency.len(),
        if result.frames.is_some() {
            ", 1 frame trace"
        } else {
            ""
        },
        human_ns(wall_ns),
    );

    if print {
        print!("{}", result.markdown());
    }
}
