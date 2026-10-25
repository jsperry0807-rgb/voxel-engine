//! The tracked benchmark archive: run folders, `history.json` and `HISTORY.md`.
//!
//! A local run writes to `target/bench` (ignored). A *recorded* run is copied
//! into `benchmarks/runs/<utc-stamp>-<label>/` together with its raw Criterion,
//! iai-callgrind and perf logs, and a summary row is appended to
//! `benchmarks/history.json` and regenerated into `benchmarks/HISTORY.md`.
//!
//! Recording is explicit (`RECORD=1 scripts/bench.sh`) because most local runs
//! are experiments, not baselines.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::report::{self, Report};

/// One archived run, as summarised in `history.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSummary {
    /// Directory name, e.g. `20261025T144501Z-baseline`.
    pub id: String,
    /// Human label supplied at record time.
    pub label: String,
    pub timestamp_unix: u64,
    /// `git rev-parse HEAD`, when the run was recorded inside a repo.
    pub git_commit: Option<String>,
    /// Whether the worktree had uncommitted changes.
    pub git_dirty: Option<bool>,
    pub cpu_model: Option<String>,
    pub os: String,
    pub arch: String,
    /// Run directory relative to the archive root, e.g. `runs/<id>`.
    pub path: String,
    pub note: Option<String>,
    /// Fingerprint of the executed case set. Process-wide metrics (peak RSS,
    /// CPU time) are only comparable between runs with the same fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
    /// Stable metric keys, extracted from the report.
    pub metrics: BTreeMap<String, f64>,
}

/// The machine-readable index.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub runs: Vec<RunSummary>,
}

/// Result of recording one run.
pub struct Recorded {
    pub dir: PathBuf,
    pub summary: RunSummary,
    pub previous: Option<RunSummary>,
}

/// Records `from` (a directory containing `report.json`) into `archive`.
///
/// `attachments` are copied alongside the report with their file names
/// preserved; missing attachments are skipped.
pub fn record(
    from: &Path,
    archive: &Path,
    label: &str,
    note: Option<&str>,
    attachments: &[PathBuf],
) -> io::Result<Recorded> {
    let report_path = from.join("report.json");
    let raw = fs::read_to_string(&report_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("read {}: {error}", report_path.display()),
        )
    })?;
    let report: Report = serde_json::from_str(&raw)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let stamp = utc_stamp(now);
    let slug = slug(label);
    let runs_dir = archive.join("runs");
    fs::create_dir_all(&runs_dir)?;

    let id = unique_id(&runs_dir, &format!("{stamp}-{slug}"));
    let dir = runs_dir.join(&id);
    fs::create_dir_all(&dir)?;

    fs::write(dir.join("report.json"), &raw)?;
    fs::write(dir.join("report.md"), report.markdown())?;

    for attachment in attachments {
        if let Some(name) = attachment.file_name() {
            match fs::read(attachment) {
                Ok(bytes) => {
                    fs::write(dir.join(name), bytes)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }

    let mut history = load(archive)?;

    let summary = RunSummary {
        id: id.clone(),
        label: label.to_owned(),
        timestamp_unix: now,
        git_commit: git(&["rev-parse", "HEAD"]),
        git_dirty: git_dirty(archive),
        cpu_model: report.meta.cpu_model.clone(),
        os: report.meta.os.clone(),
        arch: report.meta.arch.clone(),
        path: format!("runs/{id}"),
        note: note.map(str::to_owned),
        suite: Some(suite_fingerprint(&report)),
        metrics: summarize(&report),
    };

    fs::write(
        dir.join("run.json"),
        serde_json::to_vec_pretty(&summary).map_err(|error| io::Error::other(error.to_string()))?,
    )?;

    history.runs.push(summary.clone());
    history.runs.sort_by(order);
    let previous = history
        .runs
        .len()
        .checked_sub(2)
        .map(|index| history.runs[index].clone());
    fs::create_dir_all(archive)?;
    fs::write(
        archive.join("history.json"),
        serde_json::to_vec_pretty(&history).map_err(|error| io::Error::other(error.to_string()))?,
    )?;
    fs::write(archive.join("HISTORY.md"), render(&history))?;

    Ok(Recorded {
        dir,
        summary,
        previous,
    })
}

/// Loads `history.json`, or an empty history when the archive is new.
///
/// Runs recorded before the suite fingerprint existed are backfilled from their
/// archived `report.json`, so a refresh can classify them without re-running.
pub fn load(archive: &Path) -> io::Result<History> {
    let path = archive.join("history.json");
    let mut history = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => History::default(),
        Err(error) => return Err(error),
    };
    for run in &mut history.runs {
        if run.suite.is_none() {
            run.suite = archived_suite(archive, run);
        }
    }
    Ok(history)
}

fn archived_suite(archive: &Path, run: &RunSummary) -> Option<String> {
    let path = archive.join(&run.path).join("report.json");
    let raw = fs::read_to_string(path).ok()?;
    let report: Report = serde_json::from_str(&raw).ok()?;
    Some(suite_fingerprint(&report))
}

/// Regenerates `HISTORY.md` from `history.json` without recording a run.
pub fn refresh(archive: &Path) -> io::Result<PathBuf> {
    let history = load(archive)?;
    fs::create_dir_all(archive)?;
    // Persist any backfilled suite fingerprints so the next load is cheap.
    fs::write(
        archive.join("history.json"),
        serde_json::to_vec_pretty(&history).map_err(|error| io::Error::other(error.to_string()))?,
    )?;
    let path = archive.join("HISTORY.md");
    fs::write(&path, render(&history))?;
    Ok(path)
}

/// FNV-1a fingerprint of the sorted case names.
///
/// Counts are part of the key so a truncated or extended suite is obvious in
/// the note, while the hash distinguishes same-size suites with renamed cases.
fn suite_fingerprint(report: &Report) -> String {
    let cases = report
        .memory
        .iter()
        .map(|record| record.case.as_str())
        .chain(report.cpu.iter().map(|record| record.case.as_str()))
        .chain(report.latency.iter().map(|record| record.case.as_str()));
    suite_key(
        cases,
        report.memory.len(),
        report.cpu.len(),
        report.latency.len(),
    )
}

fn suite_key<'a>(
    cases: impl IntoIterator<Item = &'a str>,
    memory: usize,
    cpu: usize,
    latency: usize,
) -> String {
    let mut names: Vec<&str> = cases.into_iter().collect();
    names.sort_unstable();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for name in names {
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Separator, so ["ab", "c"] and ["a", "bc"] do not collide.
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("m{memory}-c{cpu}-l{latency}-{hash:016x}")
}

/// Extracts stable metrics from a report.
pub fn summarize(report: &Report) -> BTreeMap<String, f64> {
    let mut metrics = BTreeMap::new();
    if let Some(peak) = report.process.peak_rss {
        metrics.insert("peak_rss_bytes".into(), peak as f64);
    }
    if let Some(cpu) = report.process.cpu_ns {
        metrics.insert("process_cpu_ns".into(), cpu as f64);
    }
    if let Some(percent) = report.process.cpu_percent {
        metrics.insert("process_cpu_percent".into(), percent);
    }
    metrics.insert("process_wall_ns".into(), report.process.wall_ns as f64);

    for expect in [
        ("registry_65536_mem", "memory", "registry/65536"),
        ("chunk_terrain_mem", "memory", "chunk/terrain"),
        ("world_256_mem", "memory", "world/256_chunks_gpu"),
        ("gpu_tables_mem", "memory", "gpu_tables/65536"),
        ("gpu_overlay_1_block_mem", "memory", "gpu_overlay/1"),
        ("gpu_overlay_64_blocks_mem", "memory", "gpu_overlay/64"),
    ] {
        if let Some(value) = memory(report, expect.2) {
            metrics.insert(expect.0.into(), value.resident as f64);
        }
    }

    for (key, case) in [
        ("registry_freeze_65536_ns", "registry/build_freeze/65536"),
        ("hot_lookup_ns", "lookup/hot/random"),
        ("resolved_lookup_ns", "lookup/resolved/random"),
        ("palette_resolve_ns", "palette/resolve_4096"),
        ("dag_build_terrain_ns", "dag/build/terrain"),
        ("dag_get_ns", "dag/get/random"),
        ("gpu_lookup_ns", "gpu/lookup"),
        ("gpu_lookup_overlay_ns", "gpu/lookup_overlay"),
        ("gpu_apply_edits_100_ns", "gpu/apply_edits/100"),
        ("gpu_overlay_set_100_ns", "gpu/overlay_set/100"),
        ("gpu_overlay_flush_64_ns", "gpu/overlay_flush/64_loaded"),
    ] {
        if let Some(record) = report.cpu.iter().find(|record| record.case == case) {
            metrics.insert(key.into(), record.ns_per_op);
        }
    }

    for (key, case) in [
        ("hot_lookup_p99_us", "latency/hot_lookup"),
        ("dag_get_p99_us", "latency/dag_get"),
        ("gpu_edit_p99_us", "latency/gpu_edit_1"),
        ("gpu_overlay_set_p99_us", "latency/gpu_overlay_set"),
        ("world_set_p99_us", "latency/world_set"),
    ] {
        if let Some(record) = report.latency.iter().find(|record| record.case == case) {
            metrics.insert(key.into(), record.p99_us);
        }
    }

    metrics
}

fn memory<'a>(report: &'a Report, case: &str) -> Option<&'a report::MemoryRecord> {
    report.memory.iter().find(|record| record.case == case)
}

fn render(history: &History) -> String {
    let mut runs = history.runs.clone();
    runs.sort_by(order);

    let mut out = String::from("# Benchmark history\n\n");
    out.push_str(
        "Generated by `voxel-bench record` from `benchmarks/history.json`. \
         Newest run first. Times and memory are machine-specific; compare runs \
         taken on the same host and power profile. See `benchmarks/README.md` \
         for the lenses and their caveats.\n\n",
    );

    if runs.is_empty() {
        out.push_str("_No runs recorded yet._\n");
        return out;
    }

    out.push_str(
        "| Date (UTC) | Run | Commit | Peak RSS | CPU time | Hot lookup | Resolved | DAG get | GPU lookup | Edit p99 | Set p99 |\n\
         |---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for run in runs.iter().rev() {
        let commit = run
            .git_commit
            .as_deref()
            .map(|hash| {
                format!(
                    "`{}`{}",
                    short(hash),
                    if run.git_dirty == Some(true) { "*" } else { "" }
                )
            })
            .unwrap_or_else(|| "n/a".into());
        out.push_str(&format!(
            "| {} | [{}]({}) | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            date(run.timestamp_unix),
            run.label,
            run.path,
            commit,
            metric(run, "peak_rss_bytes", "bytes"),
            metric(run, "process_cpu_ns", "ns"),
            metric(run, "hot_lookup_ns", "ns"),
            metric(run, "resolved_lookup_ns", "ns"),
            metric(run, "dag_get_ns", "ns"),
            metric(run, "gpu_lookup_ns", "ns"),
            metric(run, "gpu_edit_p99_us", "us"),
            metric(run, "world_set_p99_us", "us"),
        ));
    }

    out.push_str(
        "\nPeak RSS and CPU time cover the whole run, not one case; the comparison \
         below omits them when the suite fingerprint changes.\n",
    );

    if runs.len() >= 2 {
        let latest = runs.last().expect("at least two runs");
        let previous = &runs[runs.len() - 2];
        let same_suite = latest.suite == previous.suite;
        out.push_str(&format!(
            "\n## Latest vs previous\n\n`{}` vs `{}`\n\n| Metric | Previous | Latest | Change |\n|---|---:|---:|---:|\n",
            latest.label, previous.label,
        ));
        for (key, label, unit) in TREND {
            if !same_suite && matches!(*key, "peak_rss_bytes" | "process_cpu_ns") {
                continue;
            }
            let (Some(a), Some(b)) = (previous.metrics.get(*key), latest.metrics.get(*key)) else {
                continue;
            };
            out.push_str(&format!(
                "| {label} | {} | {} | {} |\n",
                format_unit(*a, unit),
                format_unit(*b, unit),
                change(*a, *b),
            ));
        }
        if !same_suite {
            out.push_str(&format!(
                "\n_Process metrics omitted: the case set changed (`{}` → `{}`). \
                 Adding or removing a case moves the whole-run totals._\n",
                previous.suite.as_deref().unwrap_or("unfingerprinted"),
                latest.suite.as_deref().unwrap_or("unfingerprinted"),
            ));
        }
        out.push_str(
            "\n`*` marks a run recorded with an uncommitted worktree. \
             Lower is better for every value above.\n",
        );
    }

    out
}

const TREND: &[(&str, &str, &str)] = &[
    ("peak_rss_bytes", "Peak RSS", "bytes"),
    ("process_cpu_ns", "CPU time", "ns"),
    ("registry_65536_mem", "Registry 65k resident", "bytes"),
    ("chunk_terrain_mem", "Terrain chunk resident", "bytes"),
    ("world_256_mem", "256-chunk GPU world", "bytes"),
    ("gpu_tables_mem", "Material GPU tables", "bytes"),
    ("gpu_overlay_1_block_mem", "GPU overlay, 1 block", "bytes"),
    (
        "gpu_overlay_64_blocks_mem",
        "GPU overlay, 64 blocks",
        "bytes",
    ),
    ("registry_freeze_65536_ns", "Registry freeze 65k", "ns"),
    ("hot_lookup_ns", "Hot lookup", "ns"),
    ("resolved_lookup_ns", "Resolved lookup", "ns"),
    ("palette_resolve_ns", "Palette resolve 4096", "ns"),
    ("dag_build_terrain_ns", "DAG build terrain", "ns"),
    ("dag_get_ns", "DAG get", "ns"),
    ("gpu_lookup_ns", "GPU lookup", "ns"),
    ("gpu_lookup_overlay_ns", "GPU lookup (overlay)", "ns"),
    ("gpu_apply_edits_100_ns", "GPU edit 100", "ns"),
    ("gpu_overlay_set_100_ns", "GPU overlay set 100", "ns"),
    ("gpu_overlay_flush_64_ns", "GPU overlay flush 64", "ns"),
    ("hot_lookup_p99_us", "Hot lookup p99", "us"),
    ("dag_get_p99_us", "DAG get p99", "us"),
    ("gpu_edit_p99_us", "GPU edit p99", "us"),
    ("gpu_overlay_set_p99_us", "GPU overlay set p99", "us"),
    ("world_set_p99_us", "World set p99", "us"),
];

fn order(a: &RunSummary, b: &RunSummary) -> std::cmp::Ordering {
    a.timestamp_unix
        .cmp(&b.timestamp_unix)
        .then_with(|| a.id.cmp(&b.id))
}

fn metric(run: &RunSummary, key: &str, unit: &str) -> String {
    run.metrics
        .get(key)
        .map(|value| format_unit(*value, unit))
        .unwrap_or_else(|| "n/a".into())
}

fn format_unit(value: f64, unit: &str) -> String {
    match unit {
        "bytes" => report::bytes(value.max(0.0) as u64),
        "us" => {
            if value >= 1_000.0 {
                format!("{:.2} ms", value / 1_000.0)
            } else {
                format!("{value:.2} µs")
            }
        }
        "ns" => {
            if value >= 1_000_000.0 {
                format!("{:.2} ms", value / 1_000_000.0)
            } else if value >= 1_000.0 {
                format!("{:.2} µs", value / 1_000.0)
            } else {
                format!("{value:.1} ns")
            }
        }
        _ => format!("{value:.1}"),
    }
}

fn change(previous: f64, latest: f64) -> String {
    if previous == 0.0 {
        return "n/a".into();
    }
    let percent = (latest - previous) / previous * 100.0;
    if percent.abs() < 0.05 {
        "0.0%".into()
    } else {
        format!("{percent:+.1}%")
    }
}

fn short(hash: &str) -> String {
    hash.chars().take(8).collect()
}

/// `2026-10-25 14:45 UTC` from a Unix timestamp.
fn date(timestamp: u64) -> String {
    let (year, month, day, hour, minute, _) = utc_parts(timestamp);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// `20261025T144501Z` from a Unix timestamp.
fn utc_stamp(timestamp: u64) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(timestamp);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

fn utc_parts(timestamp: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (timestamp / 86_400) as i64;
    let seconds = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    (
        year,
        month,
        day,
        (seconds / 3_600) as u32,
        ((seconds % 3_600) / 60) as u32,
        (seconds % 60) as u32,
    )
}

/// Howard Hinnant's `civil_from_days`, with day 0 = 1970-01-01.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

fn slug(label: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in label.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "run".into() } else { out }
}

fn unique_id(runs_dir: &Path, base: &str) -> String {
    let mut id = base.to_owned();
    let mut suffix = 2;
    while runs_dir.join(&id).exists() {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

/// True when the worktree has changes outside the benchmark archive itself.
///
/// Recording necessarily creates `benchmarks/...` files; counting those would
/// mark every first archive of a clean tree as dirty, so the archive path is
/// excluded from the check.
fn git_dirty(archive: &Path) -> Option<bool> {
    let status = git(&["status", "--porcelain"])?;
    let root = git(&["rev-parse", "--show-toplevel"])?;
    let archive = std::path::absolute(archive).ok()?;
    let prefix = archive
        .strip_prefix(Path::new(&root))
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"));

    for line in status.lines() {
        let path = line.get(3..).unwrap_or(line);
        let path = path.split(" -> ").last().unwrap_or(path).trim_matches('"');
        let inside_archive = prefix
            .as_deref()
            .is_some_and(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")));
        if !inside_archive {
            return Some(true);
        }
    }
    Some(false)
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formatting_matches_known_dates() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(date(0), "1970-01-01 00:00");
        // 2026-10-30T06:45:01Z
        assert_eq!(utc_stamp(1_793_342_701), "20261030T064501Z");
        assert_eq!(date(1_793_342_701), "2026-10-30 06:45");
    }

    #[test]
    fn labels_become_stable_folder_names() {
        assert_eq!(slug("Baseline 2026-10"), "baseline-2026-10");
        assert_eq!(slug("  "), "run");
        assert_eq!(slug("GPU / hot path"), "gpu-hot-path");
    }

    #[test]
    fn change_reports_percentages() {
        assert_eq!(change(100.0, 110.0), "+10.0%");
        assert_eq!(change(100.0, 90.0), "-10.0%");
        assert_eq!(change(0.0, 10.0), "n/a");
    }

    #[test]
    fn suite_fingerprint_ignores_order_and_separates_names() {
        assert_eq!(
            suite_key(["b", "a"], 1, 1, 0),
            suite_key(["a", "b"], 1, 1, 0)
        );
        assert_ne!(
            suite_key(["a", "b"], 1, 1, 0),
            suite_key(["a", "b", "c"], 2, 1, 0)
        );
        assert_ne!(
            suite_key(["ab", "c"], 1, 1, 0),
            suite_key(["a", "bc"], 1, 1, 0)
        );
    }

    fn run(label: &str, timestamp: u64, suite: &str) -> RunSummary {
        let mut metrics = BTreeMap::new();
        metrics.insert("peak_rss_bytes".into(), 1_000_000.0);
        metrics.insert("process_cpu_ns".into(), 2_000_000.0);
        metrics.insert("hot_lookup_ns".into(), 1.0);
        RunSummary {
            id: label.into(),
            label: label.into(),
            timestamp_unix: timestamp,
            git_commit: None,
            git_dirty: Some(false),
            cpu_model: None,
            os: "linux".into(),
            arch: "x86_64".into(),
            path: format!("runs/{label}"),
            note: None,
            suite: Some(suite.into()),
            metrics,
        }
    }

    #[test]
    fn process_metrics_are_omitted_when_the_suite_changes() {
        let history = History {
            runs: vec![
                run("old", 1, "m1-c1-l0-aaaa"),
                run("new", 2, "m2-c1-l0-bbbb"),
            ],
        };
        let text = render(&history);
        let trend = text
            .split("## Latest vs previous")
            .nth(1)
            .expect("trend section");
        assert!(!trend.contains("| Peak RSS |"));
        assert!(!trend.contains("| CPU time |"));
        assert!(trend.contains("case set changed"));
        assert!(trend.contains("| Hot lookup |"));
    }

    #[test]
    fn process_metrics_are_kept_for_the_same_suite() {
        let history = History {
            runs: vec![
                run("old", 1, "m1-c1-l0-aaaa"),
                run("new", 2, "m1-c1-l0-aaaa"),
            ],
        };
        let trend = render(&history)
            .split("## Latest vs previous")
            .nth(1)
            .expect("trend section")
            .to_owned();
        assert!(trend.contains("| Peak RSS |"));
        assert!(trend.contains("| CPU time |"));
        assert!(!trend.contains("case set changed"));
    }
}
