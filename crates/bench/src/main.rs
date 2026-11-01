//! Runs the software benchmark lenses and writes a report.
//!
//! ```
//! cargo run --release -p voxel-bench -- all
//! cargo run --release -p voxel-bench -- memory --out target/bench
//! cargo run --release -p voxel-bench -- cpu --print
//! ```

use std::path::PathBuf;
use std::time::Instant;

use voxel_bench::report::{ProcessReport, Report};
use voxel_bench::{CountingAllocator, history, proc, report, suite};

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    All,
    Memory,
    Cpu,
    Latency,
    Layout,
}

fn main() {
    let mut mode = Mode::All;
    let mut out_dir = PathBuf::from("target/bench");
    let mut print = false;

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
            "--print" => print = true,
            "-h" | "--help" => {
                println!(
                    "usage: voxel-bench [all|memory|cpu|latency|layout] [--out DIR] [--print]\n\
                     \n\
                     all      run the memory, layout, CPU and latency lenses (default)\n\
                     memory   heap bytes, allocation churn and peak heap\n\
                     layout   bytes/node, runs, dedup and COW cost per fixture\n\
                     cpu      process CPU time, ns/op, Mops/s and allocations/op\n\
                     latency  p50/p95/p99/max for the interactive paths\n\
                     --out    report directory (default: target/bench)\n\
                     --print  also print the Markdown report\n\
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
    };

    if let Err(error) = result.write(&out_dir) {
        eprintln!("failed to write report to {}: {error}", out_dir.display());
        std::process::exit(1);
    }

    println!(
        "wrote {} and {} ({} memory, {} layout, {} CPU, {} latency cases) in {}",
        out_dir.join("report.md").display(),
        out_dir.join("report.json").display(),
        result.memory.len(),
        result.layout.len(),
        result.cpu.len(),
        result.latency.len(),
        human_ns(wall_ns),
    );

    if print {
        print!("{}", result.markdown());
    }
}

fn record_mode(args: &[String]) {
    let mut from = PathBuf::from("target/bench");
    let mut archive = PathBuf::from("benchmarks");
    let mut label = String::from("run");
    let mut note: Option<String> = None;
    let mut attachments = Vec::new();

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--from" => {
                index += 1;
                from = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--from needs a directory")),
                );
            }
            "--history" => {
                index += 1;
                archive = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--history needs a directory")),
                );
            }
            "--label" => {
                index += 1;
                label = args
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| usage("--label needs a name"));
            }
            "--note" => {
                index += 1;
                note = Some(
                    args.get(index)
                        .cloned()
                        .unwrap_or_else(|| usage("--note needs text")),
                );
            }
            "--attach" => {
                index += 1;
                attachments.push(PathBuf::from(
                    args.get(index)
                        .cloned()
                        .unwrap_or_else(|| usage("--attach needs a file")),
                ));
            }
            other => usage(&format!("unknown record argument `{other}`")),
        }
        index += 1;
    }

    match history::record(&from, &archive, &label, note.as_deref(), &attachments) {
        Ok(recorded) => {
            println!(
                "recorded `{}` -> {} ({} metrics)",
                recorded.summary.id,
                recorded.dir.display(),
                recorded.summary.metrics.len(),
            );
            println!("history -> {}", archive.join("HISTORY.md").display());
        }
        Err(error) => {
            eprintln!("failed to record run: {error}");
            std::process::exit(1);
        }
    }
}

fn refresh_mode(args: &[String]) {
    let mut archive = PathBuf::from("benchmarks");
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--history" => {
                index += 1;
                archive = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--history needs a directory")),
                );
            }
            other => usage(&format!("unknown refresh argument `{other}`")),
        }
        index += 1;
    }

    match history::refresh(&archive) {
        Ok(path) => println!("refreshed {}", path.display()),
        Err(error) => {
            eprintln!("failed to refresh history: {error}");
            std::process::exit(1);
        }
    }
}

fn save_scan_mode(args: &[String]) {
    let mut path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--path" => {
                index += 1;
                path = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--path needs a directory")),
                ));
            }
            "--out" => {
                index += 1;
                out = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a file")),
                ));
            }
            other => usage(&format!("unknown save-scan argument `{other}`")),
        }
        index += 1;
    }

    let Some(path) = path else {
        usage("save-scan needs --path <dir>");
    };

    let report = match voxel_bench::save_scan::scan(&path) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("failed to scan {}: {error}", path.display());
            std::process::exit(1);
        }
    };
    let json = match serde_json::to_string_pretty(&report) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("failed to serialize save-scan report: {error}");
            std::process::exit(1);
        }
    };

    match out {
        Some(out) => {
            if let Some(parent) = out.parent()
                && !parent.as_os_str().is_empty()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                eprintln!("failed to create {}: {error}", parent.display());
                std::process::exit(1);
            }
            if let Err(error) = std::fs::write(&out, json + "\n") {
                eprintln!("failed to write {}: {error}", out.display());
                std::process::exit(1);
            }
            println!(
                "wrote {} ({} chunks) from {}",
                out.display(),
                report.live_chunks,
                path.display()
            );
        }
        None => println!("{json}"),
    }
}

fn fixture_mode(args: &[String]) {
    let Some(command) = args.first() else {
        usage("fixture needs `build` or `verify`");
    };
    match command.as_str() {
        "build" => fixture_build(&args[1..]),
        "verify" => fixture_verify(&args[1..]),
        other => usage(&format!("unknown fixture command `{other}`")),
    }
}

fn fixture_build(args: &[String]) {
    let mut out = PathBuf::from(voxel_bench::fixture::DEFAULT_DIR);
    let mut recipe = voxel_bench::fixture::Recipe::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                index += 1;
                out = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a directory")),
                );
            }
            "--chunk-min" => {
                index += 1;
                recipe.chunk_min = parse_point(
                    "--chunk-min",
                    args.get(index)
                        .unwrap_or_else(|| usage("--chunk-min needs x,y,z")),
                );
            }
            "--chunk-max" => {
                index += 1;
                recipe.chunk_max = parse_point(
                    "--chunk-max",
                    args.get(index)
                        .unwrap_or_else(|| usage("--chunk-max needs x,y,z")),
                );
            }
            "--seed" => {
                index += 1;
                recipe.seed = args
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| usage("--seed needs a u64"));
            }
            other => usage(&format!("unknown fixture build argument `{other}`")),
        }
        index += 1;
    }

    match voxel_bench::fixture::build_recipe(&out, &recipe) {
        Ok(built) => {
            let manifest = &built.manifest;
            println!(
                "froze {} at {} ({} chunks, {} bytes, {} files)",
                manifest.id,
                built.save_dir.display(),
                manifest.observed.live_chunks,
                manifest.integrity.total_bytes,
                manifest.integrity.files.len(),
            );
            println!("manifest  {}", built.manifest_path.display());
            println!("blake3    {}", manifest.integrity.world_hash);
            println!(
                "evidence  cave air {}, overhang roofs {}, structures {}, trees {}",
                manifest.checks.cave_air_voxels,
                manifest.checks.overhang_roof_blocks,
                manifest.checks.structure_blocks,
                manifest.checks.tree_blocks,
            );
            println!(
                "status    provisional until the first real save; do not quote its sizes as gate results"
            );
        }
        Err(error) => {
            eprintln!("failed to build fixture: {error}");
            std::process::exit(1);
        }
    }
}

fn fixture_verify(args: &[String]) {
    let mut manifest = voxel_bench::fixture::default_manifest();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--manifest needs a file")),
                );
            }
            other => usage(&format!("unknown fixture verify argument `{other}`")),
        }
        index += 1;
    }

    match voxel_bench::fixture::verify(&manifest) {
        Ok(verified) => {
            let manifest = &verified.manifest;
            println!(
                "verified {} at {} ({} chunks, {} bytes, {} files)",
                manifest.id,
                verified.save_dir.display(),
                manifest.observed.live_chunks,
                manifest.integrity.total_bytes,
                manifest.integrity.files.len(),
            );
            println!("manifest  {}", verified.manifest_path.display());
            println!("blake3    {}", manifest.integrity.world_hash);
            println!(
                "format    {} v{}, {}³ chunks, {}",
                manifest.format.chunk_format,
                manifest.format.chunk_version,
                manifest.format.chunk_dim,
                manifest.format.region_format,
            );
            println!(
                "materials {} distinct; cave air {}, overhang roofs {}, structures {}, trees {}",
                manifest.observed.materials.len(),
                manifest.checks.cave_air_voxels,
                manifest.checks.overhang_roof_blocks,
                manifest.checks.structure_blocks,
                manifest.checks.tree_blocks,
            );
            if verified.rebuilt {
                println!("rebuild   byte-identical from the manifest recipe");
            }
            println!("status    provisional until the first real save");
        }
        Err(error) => {
            eprintln!("fixture verify failed: {error}");
            std::process::exit(1);
        }
    }
}

/// `hitch`: the M3-09 streaming hitch test.
fn hitch_mode(args: &[String]) {
    let mut config = voxel_viewer::hitch::HitchConfig::default();
    let mut seed = None;
    let mut json = false;
    let mut check = true;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--frames" => {
                index += 1;
                config.frames = args
                    .get(index)
                    .and_then(|text| text.parse().ok())
                    .unwrap_or_else(|| usage("--frames needs a positive integer"));
            }
            "--speed" => {
                index += 1;
                config.speed = args
                    .get(index)
                    .and_then(|text| text.parse().ok())
                    .unwrap_or_else(|| usage("--speed needs a number"));
            }
            "--seed" => {
                index += 1;
                let text = args
                    .get(index)
                    .unwrap_or_else(|| usage("--seed needs a value"));
                seed = Some(
                    voxel_viewer::scene::parse_seed(text)
                        .unwrap_or_else(|| usage("--seed needs a u32, decimal or 0x hex")),
                );
            }
            "--upload-kib" => {
                index += 1;
                let kib: usize = args
                    .get(index)
                    .and_then(|text| text.parse().ok())
                    .unwrap_or_else(|| usage("--upload-kib needs a positive integer"));
                config.upload_budget = kib * 1024;
            }
            "--json" => json = true,
            "--no-check" => check = false,
            other => usage(&format!("unknown hitch argument `{other}`")),
        }
        index += 1;
    }
    if config.frames == 0 {
        usage("--frames must be at least 1");
    }

    let scene = seed.map_or_else(
        voxel_viewer::scene::Scene::from_env,
        voxel_viewer::scene::Scene::new,
    );
    let Some((device, queue)) = hitch_device() else {
        eprintln!("no wgpu adapter; skipping the hitch test");
        std::process::exit(0);
    };
    let report = voxel_viewer::hitch::run(&device, &queue, scene, &config);

    if json {
        let report = serde_json::json!({
            "seed": scene.seed(),
            "frames": report.frames(),
            "recenters": report.recenters,
            "upload_frames": report.upload_frames,
            "uploaded_bytes": report.uploaded_bytes,
            "overruns": report.overruns,
            "pump_hitches": report.pump_hitches,
            "warmup_frames": report.warmup_frames,
            "warmup_upload_max_ms": report.warmup_upload_max.as_secs_f64() * 1000.0,
            "warmup_total_max_ms": report.warmup_total_max.as_secs_f64() * 1000.0,
            "resident": report.resident,
            "pending": report.pending,
            "upload_p50_ms": ms(voxel_viewer::hitch::Stage::Upload, &report, 0.50),
            "upload_p95_ms": ms(voxel_viewer::hitch::Stage::Upload, &report, 0.95),
            "upload_p99_ms": ms(voxel_viewer::hitch::Stage::Upload, &report, 0.99),
            "upload_max_ms": ms(voxel_viewer::hitch::Stage::Upload, &report, 1.0),
            "total_p50_ms": ms(voxel_viewer::hitch::Stage::Total, &report, 0.50),
            "total_p95_ms": ms(voxel_viewer::hitch::Stage::Total, &report, 0.95),
            "total_p99_ms": ms(voxel_viewer::hitch::Stage::Total, &report, 0.99),
            "total_max_ms": ms(voxel_viewer::hitch::Stage::Total, &report, 1.0),
            "passed": report.passed(),
        });
        match serde_json::to_string_pretty(&report) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize the hitch report: {error}");
                std::process::exit(1);
            }
        }
    } else {
        println!(
            "hitch: seed 0x{:08X}, {} frames at {:.0} u/s ({} recenters); warm-up {} frames, worst upload {:.2} ms / frame {:.2} ms\n",
            scene.seed(),
            report.frames(),
            config.speed,
            report.recenters,
            report.warmup_frames,
            report.warmup_upload_max.as_secs_f64() * 1000.0,
            report.warmup_total_max.as_secs_f64() * 1000.0,
        );
        print!("{}", report.table());
        println!(
            "\nupload frames {} / {} ({:.1} MiB), overruns {}, pump hitches {}, resident {} / pending {}",
            report.upload_frames,
            report.frames(),
            report.uploaded_bytes as f64 / (1024.0 * 1024.0),
            report.overruns,
            report.pump_hitches,
            report.resident,
            report.pending,
        );
        for (frame, sample) in &report.slowest {
            println!(
                "slowest frame {frame}: pump {:.2} ms, upload {:.2} ms ({} KiB), finish {:.2} ms, total {:.2} ms",
                sample.pump.as_secs_f64() * 1000.0,
                sample.upload.as_secs_f64() * 1000.0,
                sample.uploaded_bytes / 1024,
                sample.finish.as_secs_f64() * 1000.0,
                sample.total.as_secs_f64() * 1000.0,
            );
        }
        println!(
            "budget: upload p99 <= {:.0} ms, frame max <= {:.0} ms: {}",
            voxel_viewer::hitch::UPLOAD_P99_BUDGET.as_secs_f64() * 1000.0,
            voxel_viewer::hitch::FRAME_MAX_BUDGET.as_secs_f64() * 1000.0,
            if report.passed() { "PASS" } else { "FAIL" },
        );
    }
    if check && !report.passed() {
        std::process::exit(1);
    }
}

fn ms(stage: voxel_viewer::hitch::Stage, report: &voxel_viewer::hitch::HitchReport, p: f64) -> f64 {
    report.percentile(stage, p).as_secs_f64() * 1000.0
}

/// One headless device for the GPU-driven bench modes.
///
/// The instance and adapter are leaked on purpose: llvmpipe and Dozen both
/// crash when they are torn down while threads still reference the device, and
/// the process is about to exit anyway.
fn hitch_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: None,
        ..Default::default()
    }))
    .ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    std::mem::forget(adapter);
    std::mem::forget(instance);
    Some((device, queue))
}

/// `image-diff`: the M3-12 visual regression check.
fn image_diff_mode(args: &[String]) {
    let mut reference = None;
    let mut candidate = None;
    let mut threshold = voxel_bench::image_diff::Threshold::default();
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--reference" => {
                index += 1;
                reference = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--reference needs a file")),
                ));
            }
            "--candidate" => {
                index += 1;
                candidate = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--candidate needs a file")),
                ));
            }
            "--max-fraction" => {
                index += 1;
                threshold.max_fraction = args
                    .get(index)
                    .and_then(|text| text.parse().ok())
                    .unwrap_or_else(|| usage("--max-fraction needs a number"));
            }
            "--tolerance" => {
                index += 1;
                threshold.channel_tolerance = args
                    .get(index)
                    .and_then(|text| text.parse().ok())
                    .unwrap_or_else(|| usage("--tolerance needs an integer"));
            }
            "--max-delta" => {
                index += 1;
                threshold.max_delta = args
                    .get(index)
                    .and_then(|text| text.parse().ok())
                    .unwrap_or_else(|| usage("--max-delta needs an integer"));
            }
            "--max-mean-abs" => {
                index += 1;
                threshold.max_mean_abs = args
                    .get(index)
                    .and_then(|text| text.parse().ok())
                    .unwrap_or_else(|| usage("--max-mean-abs needs a number"));
            }
            "--json" => json = true,
            other => usage(&format!("unknown image-diff argument `{other}`")),
        }
        index += 1;
    }

    let Some(reference_path) = reference else {
        usage("image-diff needs --reference");
    };
    let Some(candidate_path) = candidate else {
        usage("image-diff needs --candidate");
    };
    let reference = match voxel_bench::image_diff::Image::load(&reference_path) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("image-diff: {error}");
            std::process::exit(1);
        }
    };
    let candidate_image = match voxel_bench::image_diff::Image::load(&candidate_path) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("image-diff: {error}");
            std::process::exit(1);
        }
    };
    let diff = match voxel_bench::image_diff::compare(&reference, &candidate_image, &threshold) {
        Ok(diff) => diff,
        Err(error) => {
            eprintln!("image-diff: {error}");
            std::process::exit(1);
        }
    };

    if json {
        let report = serde_json::json!({
            "reference": reference_path.display().to_string(),
            "candidate": candidate_path.display().to_string(),
            "width": reference.width,
            "height": reference.height,
            "pixels": diff.pixels,
            "differing": diff.differing,
            "differing_fraction": diff.differing_fraction,
            "max_delta": diff.max_delta,
            "mean_abs": diff.mean_abs,
            "max_fraction": threshold.max_fraction,
            "channel_tolerance": threshold.channel_tolerance,
            "max_delta_limit": threshold.max_delta,
            "max_mean_abs": threshold.max_mean_abs,
            "passed": diff.passes(&threshold),
        });
        match serde_json::to_string_pretty(&report) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize the image diff: {error}");
                std::process::exit(1);
            }
        }
    } else {
        println!("{}", diff.line(&threshold));
    }
    if !diff.passes(&threshold) {
        std::process::exit(1);
    }
}

/// `scene-memory`: the M3-11 memory report for the 256-chunk scene.
fn scene_memory_mode(args: &[String]) {
    let mut seed = None;
    let mut upload_kib = voxel_viewer::upload::DEFAULT_UPLOAD_BYTES / 1024;
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--seed" => {
                index += 1;
                let text = args
                    .get(index)
                    .unwrap_or_else(|| usage("--seed needs a value"));
                seed = Some(
                    voxel_viewer::scene::parse_seed(text)
                        .unwrap_or_else(|| usage("--seed needs a u32, decimal or 0x hex")),
                );
            }
            "--upload-kib" => {
                index += 1;
                upload_kib = args
                    .get(index)
                    .and_then(|text| text.parse().ok())
                    .unwrap_or_else(|| usage("--upload-kib needs a positive integer"));
            }
            "--json" => json = true,
            other => usage(&format!("unknown scene-memory argument `{other}`")),
        }
        index += 1;
    }

    let scene = seed.map_or_else(
        voxel_viewer::scene::Scene::from_env,
        voxel_viewer::scene::Scene::new,
    );
    let Some((device, queue)) = hitch_device() else {
        eprintln!("no wgpu adapter; skipping the scene memory report");
        std::process::exit(0);
    };

    // The heap counters include everything this process allocates after the
    // baseline, which is the device, the scene and the streaming threads.
    let baseline = voxel_bench::mem::reset();
    let (_scene, usage) =
        match voxel_viewer::memory::fill_scene(&device, &queue, scene, upload_kib * 1024) {
            Ok(filled) => filled,
            Err(error) => {
                eprintln!("scene memory: {error}");
                std::process::exit(1);
            }
        };
    // Read the heap while the scene is still resident: dropping it first would
    // report the streaming threads' buffers as already freed.
    let live = voxel_bench::mem::live_bytes();
    let peak = voxel_bench::mem::peak_since(baseline);

    let mut report = voxel_viewer::memory::Report::new(
        usage,
        scene.seed(),
        voxel_core::constants::CHUNK_DIM as u32,
        upload_kib * 1024,
    );
    report.heap_live = Some(live);
    report.heap_peak = Some(peak);
    report.rss = voxel_bench::proc::rss_bytes();
    report.peak_rss = voxel_bench::proc::peak_rss_bytes();

    if json {
        match serde_json::to_string_pretty(&report.to_json()) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize the scene memory report: {error}");
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", report.markdown());
        println!(
            "host scene buffers: {} (worker volumes) + {} (upload backlog)",
            human_bytes(usage.worker_volumes),
            human_bytes(usage.upload_backlog_bytes as u64),
        );
    }
    if !report.fits_with_margin() {
        std::process::exit(1);
    }
}

/// Formats bytes as MiB with one decimal.
fn human_bytes(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

fn gate_mode(args: &[String]) {
    let Some(command) = args.first() else {
        usage(
            "gate needs `memory`, `residency`, `edits`, `probes`, `granularity`, `scaleup` or `report`",
        );
    };
    match command.as_str() {
        "memory" => gate_memory(&args[1..]),
        "residency" => gate_residency(&args[1..]),
        "edits" => gate_edits(&args[1..]),
        "probes" => gate_probes(&args[1..]),
        "granularity" => gate_granularity(&args[1..]),
        "scaleup" => gate_scaleup(&args[1..]),
        "report" => gate_report(&args[1..]),
        other => usage(&format!("unknown gate command `{other}`")),
    }
}

fn gate_memory(args: &[String]) {
    let mut manifest = voxel_bench::fixture::default_manifest();
    let mut out: Option<PathBuf> = None;
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--manifest needs a file")),
                );
            }
            "--out" => {
                index += 1;
                out = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a file")),
                ));
            }
            "--json" => json = true,
            other => usage(&format!("unknown gate memory argument `{other}`")),
        }
        index += 1;
    }

    let report = match voxel_bench::gate::memory::report(&manifest) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("failed to measure {}: {error}", manifest.display());
            std::process::exit(1);
        }
    };

    if let Some(out) = out {
        write_json_report(&out, &report);
    }

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize gate memory report: {error}");
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", report.markdown());
    }
}

fn gate_granularity(args: &[String]) {
    let mut manifest = voxel_bench::fixture::default_manifest();
    let mut out: Option<PathBuf> = None;
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--manifest needs a file")),
                );
            }
            "--out" => {
                index += 1;
                out = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a file")),
                ));
            }
            "--json" => json = true,
            other => usage(&format!("unknown gate granularity argument `{other}`")),
        }
        index += 1;
    }

    let report = match voxel_bench::gate::granularity::report(&manifest) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("failed to measure {}: {error}", manifest.display());
            std::process::exit(1);
        }
    };

    if let Some(out) = out {
        write_json_report(&out, &report);
    }

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize gate granularity report: {error}");
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", report.markdown());
    }
}

fn gate_residency(args: &[String]) {
    let mut profile = voxel_bench::gate::residency::Profile::default();
    let mut radius: Option<u32> = None;
    let mut band: Option<u32> = None;
    let mut out: Option<PathBuf> = None;
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--profile" => {
                index += 1;
                let name = args
                    .get(index)
                    .unwrap_or_else(|| usage("--profile needs a name"));
                profile =
                    voxel_bench::gate::residency::Profile::from_name(name).unwrap_or_else(|| {
                        usage(&format!(
                            "unknown profile `{name}`; expected minimum, default or high"
                        ))
                    });
            }
            "--radius" => {
                index += 1;
                let value = args
                    .get(index)
                    .unwrap_or_else(|| usage("--radius needs metres"));
                radius = Some(parse_metres(value, "--radius"));
            }
            "--band" => {
                index += 1;
                let value = args
                    .get(index)
                    .unwrap_or_else(|| usage("--band needs metres"));
                band = Some(parse_metres(value, "--band"));
            }
            "--out" => {
                index += 1;
                out = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a file")),
                ));
            }
            "--json" => json = true,
            other => usage(&format!("unknown gate residency argument `{other}`")),
        }
        index += 1;
    }

    let base = voxel_bench::gate::residency::ResidencyConfig::from_profile(profile);
    let config = voxel_bench::gate::residency::ResidencyConfig {
        profile,
        near_radius_m: radius.unwrap_or(base.near_radius_m),
        vertical_shell_m: band.unwrap_or(base.vertical_shell_m),
    };

    let report = match voxel_bench::gate::residency::report(config) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("failed to measure the configured view/band: {error}");
            std::process::exit(1);
        }
    };

    if let Some(out) = out {
        write_json_report(&out, &report);
    }

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize gate residency report: {error}");
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", report.markdown());
    }
}

fn gate_edits(args: &[String]) {
    let mut manifest = voxel_bench::fixture::default_manifest();
    let mut out: Option<PathBuf> = None;
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--manifest needs a file")),
                );
            }
            "--out" => {
                index += 1;
                out = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a file")),
                ));
            }
            "--json" => json = true,
            other => usage(&format!("unknown gate edits argument `{other}`")),
        }
        index += 1;
    }

    let report = match voxel_bench::gate::edits::report(&manifest) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("failed to measure {}: {error}", manifest.display());
            std::process::exit(1);
        }
    };

    if let Some(out) = out {
        write_json_report(&out, &report);
    }

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize gate edits report: {error}");
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", report.markdown());
    }
}

fn gate_probes(args: &[String]) {
    let mut manifest = voxel_bench::fixture::default_manifest();
    let mut out: Option<PathBuf> = None;
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--manifest needs a file")),
                );
            }
            "--out" => {
                index += 1;
                out = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a file")),
                ));
            }
            "--json" => json = true,
            other => usage(&format!("unknown gate probes argument `{other}`")),
        }
        index += 1;
    }

    let report = match voxel_bench::gate::probes::report(&manifest) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("failed to measure {}: {error}", manifest.display());
            std::process::exit(1);
        }
    };

    if let Some(out) = out {
        write_json_report(&out, &report);
    }

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize gate probes report: {error}");
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", report.markdown());
    }
}

fn gate_scaleup(args: &[String]) {
    let mut manifest = voxel_bench::fixture::default_manifest();
    let mut profile = voxel_bench::gate::residency::Profile::default();
    let mut out: Option<PathBuf> = None;
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--manifest needs a file")),
                );
            }
            "--profile" => {
                index += 1;
                let name = args
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| usage("--profile needs a name"));
                profile =
                    voxel_bench::gate::residency::Profile::from_name(&name).unwrap_or_else(|| {
                        usage(&format!(
                            "unknown profile `{name}`; expected minimum, default or high"
                        ))
                    });
            }
            "--out" => {
                index += 1;
                out = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a file")),
                ));
            }
            "--json" => json = true,
            other => usage(&format!("unknown gate scaleup argument `{other}`")),
        }
        index += 1;
    }

    let config = voxel_bench::gate::residency::ResidencyConfig::from_profile(profile);
    let report = match voxel_bench::gate::scaleup::report_for(&manifest, config) {
        Ok(report) => report,
        Err(error) => {
            eprintln!(
                "failed to project {} to the target edge: {error}",
                manifest.display()
            );
            std::process::exit(1);
        }
    };

    if let Some(out) = out {
        write_json_report(&out, &report);
    }

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize gate scaleup report: {error}");
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", report.markdown());
    }
}

fn gate_report(args: &[String]) {
    let mut manifest = voxel_bench::fixture::default_manifest();
    let mut profile = voxel_bench::gate::residency::Profile::default();
    let mut out = PathBuf::from("target/gate");
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--manifest needs a file")),
                );
            }
            "--profile" => {
                index += 1;
                let name = args
                    .get(index)
                    .unwrap_or_else(|| usage("--profile needs a name"));
                profile =
                    voxel_bench::gate::residency::Profile::from_name(name).unwrap_or_else(|| {
                        usage(&format!(
                            "unknown profile `{name}`; expected minimum, default or high"
                        ))
                    });
            }
            "--out" => {
                index += 1;
                out = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a directory")),
                );
            }
            "--json" => json = true,
            other => usage(&format!("unknown gate report argument `{other}`")),
        }
        index += 1;
    }

    let report = match voxel_bench::gate::report::GateReport::generate(&manifest, profile) {
        Ok(report) => report,
        Err(error) => {
            eprintln!(
                "failed to build the gate report from {}: {error}",
                manifest.display()
            );
            std::process::exit(1);
        }
    };

    let json_text = match serde_json::to_string_pretty(&report) {
        Ok(json_text) => json_text,
        Err(error) => {
            eprintln!("failed to serialize the gate report: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = std::fs::create_dir_all(&out) {
        eprintln!("failed to create {}: {error}", out.display());
        std::process::exit(1);
    }
    let md_path = out.join("gate.md");
    let json_path = out.join("gate.json");
    if let Err(error) = std::fs::write(&md_path, report.markdown()) {
        eprintln!("failed to write {}: {error}", md_path.display());
        std::process::exit(1);
    }
    if let Err(error) = std::fs::write(&json_path, format!("{json_text}\n")) {
        eprintln!("failed to write {}: {error}", json_path.display());
        std::process::exit(1);
    }

    if json {
        // Keep stdout parseable when the caller asked for JSON.
        eprintln!("wrote {} and {}", md_path.display(), json_path.display());
        println!("{json_text}");
    } else {
        println!("wrote {} and {}", md_path.display(), json_path.display());
        print!("{}", report.markdown());
    }
}

fn parse_metres(value: &str, flag: &str) -> u32 {
    value.parse::<u32>().unwrap_or_else(|_| {
        usage(&format!(
            "{flag} needs a whole number of metres, got `{value}`"
        ))
    })
}

fn write_json_report<T: serde::Serialize>(out: &std::path::Path, report: &T) {
    let serialized = match serde_json::to_string_pretty(report) {
        Ok(serialized) => serialized,
        Err(error) => {
            eprintln!("failed to serialize report: {error}");
            std::process::exit(1);
        }
    };
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("failed to create {}: {error}", parent.display());
        std::process::exit(1);
    }
    if let Err(error) = std::fs::write(out, serialized + "\n") {
        eprintln!("failed to write {}: {error}", out.display());
        std::process::exit(1);
    }
    println!("wrote {}", out.display());
}

fn usage(message: &str) -> ! {
    eprintln!("voxel-bench: {message}");
    eprintln!("try `voxel-bench --help`");
    std::process::exit(2);
}

/// Parses an `x,y,z` chunk coordinate for `fixture build`.
fn parse_point(flag: &str, text: &str) -> [i32; 3] {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        usage(&format!("{flag} needs three comma-separated integers"));
    }
    let mut point = [0i32; 3];
    for (axis, part) in parts.iter().enumerate() {
        point[axis] = part
            .parse()
            .unwrap_or_else(|_| usage(&format!("{flag}: `{part}` is not an integer")));
    }
    point
}

fn human_ns(ns: u64) -> String {
    let ns = ns as f64;
    if ns >= 1e9 {
        format!("{:.2} s", ns / 1e9)
    } else if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else {
        format!("{:.2} µs", ns / 1e3)
    }
}
