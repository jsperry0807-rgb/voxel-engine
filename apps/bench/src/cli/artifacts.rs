//! Archive, history and inspection commands.

use super::*;

pub(crate) fn record_mode(args: &[String]) {
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

pub(crate) fn refresh_mode(args: &[String]) {
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

pub(crate) fn save_scan_mode(args: &[String]) {
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

    let report = match crate::save_scan::scan(&path) {
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

/// `image-diff`: the M3-12 visual regression check.
pub(crate) fn image_diff_mode(args: &[String]) {
    let mut reference = None;
    let mut candidate = None;
    let mut threshold = crate::image_diff::Threshold::default();
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
    let reference = match crate::image_diff::Image::load(&reference_path) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("image-diff: {error}");
            std::process::exit(1);
        }
    };
    let candidate_image = match crate::image_diff::Image::load(&candidate_path) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("image-diff: {error}");
            std::process::exit(1);
        }
    };
    let diff = match crate::image_diff::compare(&reference, &candidate_image, &threshold) {
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
