//! The pre-registered M1 gate measurements.

use super::*;

pub(crate) fn gate_mode(args: &[String]) {
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

pub(crate) fn gate_memory(args: &[String]) {
    let mut manifest = crate::fixture::default_manifest();
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

    let report = match crate::gate::memory::report(&manifest) {
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

pub(crate) fn gate_granularity(args: &[String]) {
    let mut manifest = crate::fixture::default_manifest();
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

    let report = match crate::gate::granularity::report(&manifest) {
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

pub(crate) fn gate_residency(args: &[String]) {
    let mut profile = crate::gate::residency::Profile::default();
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
                profile = crate::gate::residency::Profile::from_name(name).unwrap_or_else(|| {
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

    let base = crate::gate::residency::ResidencyConfig::from_profile(profile);
    let config = crate::gate::residency::ResidencyConfig {
        profile,
        near_radius_m: radius.unwrap_or(base.near_radius_m),
        vertical_shell_m: band.unwrap_or(base.vertical_shell_m),
    };

    let report = match crate::gate::residency::report(config) {
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

pub(crate) fn gate_edits(args: &[String]) {
    let mut manifest = crate::fixture::default_manifest();
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

    let report = match crate::gate::edits::report(&manifest) {
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

pub(crate) fn gate_probes(args: &[String]) {
    let mut manifest = crate::fixture::default_manifest();
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

    let report = match crate::gate::probes::report(&manifest) {
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

pub(crate) fn gate_scaleup(args: &[String]) {
    let mut manifest = crate::fixture::default_manifest();
    let mut profile = crate::gate::residency::Profile::default();
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
                profile = crate::gate::residency::Profile::from_name(&name).unwrap_or_else(|| {
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

    let config = crate::gate::residency::ResidencyConfig::from_profile(profile);
    let report = match crate::gate::scaleup::report_for(&manifest, config) {
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

pub(crate) fn gate_report(args: &[String]) {
    let mut manifest = crate::fixture::default_manifest();
    let mut profile = crate::gate::residency::Profile::default();
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
                profile = crate::gate::residency::Profile::from_name(name).unwrap_or_else(|| {
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

    let report = match crate::gate::report::GateReport::generate(&manifest, profile) {
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
