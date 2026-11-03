//! Argument parsing, formatting helpers and the shared JSON writer.

pub(crate) fn parse_positive(value: &str, flag: &str) -> f64 {
    value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite() && *number > 0.0)
        .unwrap_or_else(|| usage(&format!("{flag} needs a positive number, got `{value}`")))
}

pub(crate) fn parse_metres(value: &str, flag: &str) -> u32 {
    value.parse::<u32>().unwrap_or_else(|_| {
        usage(&format!(
            "{flag} needs a whole number of metres, got `{value}`"
        ))
    })
}

/// Parses an `x,y,z` chunk coordinate for `fixture build`.
pub(crate) fn parse_point(flag: &str, text: &str) -> [i32; 3] {
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

/// Formats bytes as MiB with one decimal.
pub(crate) fn human_bytes(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

pub(crate) fn human_ns(ns: u64) -> String {
    let ns = ns as f64;
    if ns >= 1e9 {
        format!("{:.2} s", ns / 1e9)
    } else if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else {
        format!("{:.2} µs", ns / 1e3)
    }
}

pub(crate) fn write_json_report<T: serde::Serialize>(out: &std::path::Path, report: &T) {
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

pub(crate) fn usage(message: &str) -> ! {
    eprintln!("voxel-bench: {message}");
    eprintln!("try `voxel-bench --help`");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("voxel-bench-{name}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn human_ns_picks_seconds_milliseconds_and_microseconds() {
        assert_eq!(human_ns(0), "0.00 µs");
        assert_eq!(human_ns(1_500), "1.50 µs");
        assert_eq!(human_ns(999_999), "1000.00 µs");
        assert_eq!(human_ns(1_000_000), "1.00 ms");
        assert_eq!(human_ns(1_500_000), "1.50 ms");
        assert_eq!(human_ns(999_999_999), "1000.00 ms");
        assert_eq!(human_ns(1_000_000_000), "1.00 s");
        assert_eq!(human_ns(4_000_000_000), "4.00 s");
    }

    #[test]
    fn write_json_report_creates_parents_and_writes_pretty_json() {
        let out = temp_dir("report").join("nested").join("gate.json");
        let report = serde_json::json!({
            "schema": 1,
            "rows": [{ "id": "edit_latency", "verdict": "pass" }],
        });

        write_json_report(&out, &report);

        let text = std::fs::read_to_string(&out).expect("report is readable");
        assert_eq!(
            text,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap())
        );
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("report is valid JSON");
        assert_eq!(parsed, report);

        std::fs::remove_dir_all(out.parent().unwrap().parent().unwrap()).ok();
    }
}
