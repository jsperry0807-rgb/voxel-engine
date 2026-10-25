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
use voxel_bench::{CountingAllocator, proc, report, suite};

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    All,
    Memory,
    Cpu,
    Latency,
}

fn main() {
    let mut mode = Mode::All;
    let mut out_dir = PathBuf::from("target/bench");
    let mut print = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "all" => mode = Mode::All,
            "memory" => mode = Mode::Memory,
            "cpu" => mode = Mode::Cpu,
            "latency" => mode = Mode::Latency,
            "--out" => {
                out_dir = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| usage("--out needs a directory"));
            }
            "--print" => print = true,
            "-h" | "--help" => {
                println!(
                    "usage: voxel-bench [all|memory|cpu|latency] [--out DIR] [--print]\n\
                     \n\
                     all      run the memory, CPU and latency lenses (default)\n\
                     memory   heap bytes, allocation churn and peak heap\n\
                     cpu      process CPU time, ns/op, Mops/s and allocations/op\n\
                     latency  p50/p95/p99/max for the interactive paths\n\
                     --out    report directory (default: target/bench)\n\
                     --print  also print the Markdown report"
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
    };

    if let Err(error) = result.write(&out_dir) {
        eprintln!("failed to write report to {}: {error}", out_dir.display());
        std::process::exit(1);
    }

    println!(
        "wrote {} and {} ({} memory, {} CPU, {} latency cases) in {}",
        out_dir.join("report.md").display(),
        out_dir.join("report.json").display(),
        result.memory.len(),
        result.cpu.len(),
        result.latency.len(),
        human_ns(wall_ns),
    );

    if print {
        print!("{}", result.markdown());
    }
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
