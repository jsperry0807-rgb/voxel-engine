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
    let previous = history
        .runs
        .iter()
        .max_by_key(|run| run.timestamp_unix)
        .cloned();

    let summary = RunSummary {
        id: id.clone(),
        label: label.to_owned(),
        timestamp_unix: now,
        git_commit: git(&["rev-parse", "HEAD"]),
        git_dirty: git(&["status", "--porcelain"]).map(|status| !status.is_empty()),
        cpu_model: report.meta.cpu_model.clone(),
        os: report.meta.os.clone(),
        arch: report.meta.arch.clone(),
        path: format!("runs/{id}"),
        note: note.map(str::to_owned),
        metrics: summarize(&report),
    };

    fs::write(
        dir.join("run.json"),
        serde_json::to_vec_pretty(&summary).map_err(|error| io::Error::other(error.to_string()))?,
    )?;

    history.runs.push(summary.clone());
    history
        .runs
        .sort_by_key(|run: &RunSummary| run.timestamp_unix);
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
pub fn load(archive: &Path) -> io::Result<History> {
    let path = archive.join("history.json");
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(History::default()),
        Err(error) => Err(error),
    }
}

/// Regenerates `HISTORY.md` from `history.json` without recording a run.
pub fn refresh(archive: &Path) -> io::Result<PathBuf> {
    let history = load(archive)?;
    fs::create_dir_all(archive)?;
    let path = archive.join("HISTORY.md");
    fs::write(&path, render(&history))?;
    Ok(path)
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
        ("gpu_apply_edits_100_ns", "gpu/apply_edits/100"),
    ] {
        if let Some(record) = report.cpu.iter().find(|record| record.case == case) {
            metrics.insert(key.into(), record.ns_per_op);
        }
    }

    for (key, case) in [
        ("hot_lookup_p99_us", "latency/hot_lookup"),
        ("dag_get_p99_us", "latency/dag_get"),
        ("gpu_edit_p99_us", "latency/gpu_edit_1"),
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
    runs.sort_by_key(|run| run.timestamp_unix);

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

    if runs.len() >= 2 {
        let latest = runs.last().expect("at least two runs");
        let previous = &runs[runs.len() - 2];
        out.push_str(&format!(
            "\n## Latest vs previous\n\n`{}` vs `{}`\n\n| Metric | Previous | Latest | Change |\n|---|---:|---:|---:|\n",
            latest.label, previous.label,
        ));
        for (key, label, unit) in TREND {
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
        out.push_str(
            "\n`*` marks a run recorded with an uncommitted worktree. \
             Lower is better for every value above except CPU percent.\n",
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
    ("registry_freeze_65536_ns", "Registry freeze 65k", "ns"),
    ("hot_lookup_ns", "Hot lookup", "ns"),
    ("resolved_lookup_ns", "Resolved lookup", "ns"),
    ("palette_resolve_ns", "Palette resolve 4096", "ns"),
    ("dag_build_terrain_ns", "DAG build terrain", "ns"),
    ("dag_get_ns", "DAG get", "ns"),
    ("gpu_lookup_ns", "GPU lookup", "ns"),
    ("gpu_apply_edits_100_ns", "GPU edit 100", "ns"),
    ("hot_lookup_p99_us", "Hot lookup p99", "us"),
    ("dag_get_p99_us", "DAG get p99", "us"),
    ("gpu_edit_p99_us", "GPU edit p99", "us"),
    ("world_set_p99_us", "World set p99", "us"),
];

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
}
