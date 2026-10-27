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
            other => usage(&format!("unknown fixture build argument `{other}`")),
        }
        index += 1;
    }

    match voxel_bench::fixture::build(&out) {
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

fn gate_mode(args: &[String]) {
    let Some(command) = args.first() else {
        usage("gate needs `memory`, `residency`, `edits`, `probes`, `scaleup` or `report`");
    };
    match command.as_str() {
        "memory" => gate_memory(&args[1..]),
        "residency" => gate_residency(&args[1..]),
        "edits" => gate_edits(&args[1..]),
        "probes" => gate_probes(&args[1..]),
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
