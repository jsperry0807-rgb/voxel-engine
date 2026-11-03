//! Frame, hitch and scene-memory simulations.

use super::*;

/// `hitch`: the M3-09 streaming hitch test.
pub(crate) fn hitch_mode(args: &[String]) {
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

pub(crate) fn ms(
    stage: voxel_viewer::hitch::Stage,
    report: &voxel_viewer::hitch::HitchReport,
    p: f64,
) -> f64 {
    report.percentile(stage, p).as_secs_f64() * 1000.0
}

/// One headless device for the GPU-driven bench modes.
///
/// The instance and adapter are leaked on purpose: llvmpipe and Dozen both
/// crash when they are torn down while threads still reference the device, and
/// the process is about to exit anyway.
pub(crate) fn hitch_device() -> Option<(wgpu::Device, wgpu::Queue)> {
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

pub(crate) fn frame_mode(args: &[String]) {
    let mut input: Option<PathBuf> = None;
    let mut out = PathBuf::from("target/frame");
    let mut warmup = 0usize;
    let mut target = FrameTarget::default();
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--input" => {
                index += 1;
                input = Some(PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--input needs a file")),
                ));
            }
            "--warmup" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| usage("--warmup needs a frame count"));
                warmup = value.parse().unwrap_or_else(|_| {
                    usage(&format!("--warmup needs a whole number, got `{value}`"))
                });
            }
            "--target-fps" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| usage("--target-fps needs a number"));
                target.avg_fps = parse_positive(&value, "--target-fps");
            }
            "--one-percent-fps" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| usage("--one-percent-fps needs a number"));
                target.one_percent_low_fps = parse_positive(&value, "--one-percent-fps");
            }
            "--hitch-ms" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| usage("--hitch-ms needs a number"));
                let millis = parse_positive(&value, "--hitch-ms");
                target.hitch_ns = (millis * 1e6).round() as u64;
            }
            "--out" => {
                index += 1;
                out = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a directory")),
                );
            }
            "--json" => json = true,
            other => usage(&format!("unknown frame argument `{other}`")),
        }
        index += 1;
    }

    let Some(input) = input else {
        usage("frame needs --input <trace.csv>");
    };
    let report = match FrameReport::from_trace(&input, warmup, target) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("failed to read frame trace {}: {error}", input.display());
            std::process::exit(1);
        }
    };
    if let Err(error) = report.write(&out) {
        eprintln!("failed to write frame report to {}: {error}", out.display());
        std::process::exit(1);
    }
    let (md_path, json_path) = crate::frame::output_paths(&out);

    if json {
        let serialized = match serde_json::to_string_pretty(&report) {
            Ok(serialized) => serialized,
            Err(error) => {
                eprintln!("failed to serialize frame report: {error}");
                std::process::exit(1);
            }
        };
        eprintln!("wrote {} and {}", md_path.display(), json_path.display());
        println!("{serialized}");
    } else {
        println!("wrote {} and {}", md_path.display(), json_path.display());
        print!("{}", report.markdown());
    }
}

/// `scene-memory`: the M3-11 memory report for the 256-chunk scene.
pub(crate) fn scene_memory_mode(args: &[String]) {
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
    let baseline = crate::mem::reset();
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
    let live = crate::mem::live_bytes();
    let peak = crate::mem::peak_since(baseline);

    let mut report = voxel_viewer::memory::Report::new(
        usage,
        scene.seed(),
        voxel_core::constants::CHUNK_DIM as u32,
        upload_kib * 1024,
    );
    report.heap_live = Some(live);
    report.heap_peak = Some(peak);
    report.rss = crate::proc::rss_bytes();
    report.peak_rss = crate::proc::peak_rss_bytes();

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
