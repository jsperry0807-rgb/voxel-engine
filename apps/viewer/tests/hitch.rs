//! M3-09 headless smoke test: the hitch loop runs, streams and reports.
//!
//! The recorded measurement is `cargo run -p voxel-bench -- hitch` on the
//! reference machine; this test only pins that the loop wires the real
//! streaming path (pump, upload, submit) and produces a coherent report. It
//! skips when no adapter is available, like the other GPU tests.

use std::sync::OnceLock;

use voxel_viewer::hitch::{self, HitchConfig, Stage};
use voxel_viewer::scene::Scene;

struct TestGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// One CPU/software adapter for all GPU tests. The instance and adapter are
/// leaked: llvmpipe crashes if they are torn down after the device.
fn gpu() -> Option<&'static TestGpu> {
    static GPU: OnceLock<Option<TestGpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        // `VOXEL_SKIP_GPU=1` opts out on hosts where the WSL/Dozen GPU
        // path is unstable; the test then reports a skip, exactly like a
        // host with no adapter.
        if std::env::var_os("VOXEL_SKIP_GPU").is_some() {
            return None;
        }
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let adapter = adapters
            .into_iter()
            .find(|adapter| adapter.get_info().device_type == wgpu::DeviceType::Cpu)
            .or_else(|| {
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: true,
                    compatible_surface: None,
                    apply_limit_buckets: false,
                }))
                .ok()
            })?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        std::mem::forget(adapter);
        std::mem::forget(instance);
        Some(TestGpu { device, queue })
    })
    .as_ref()
}

#[test]
fn a_short_run_streams_frames_and_reports_them() {
    let Some(gpu) = gpu() else {
        eprintln!("no wgpu adapter; skipping the hitch smoke test");
        return;
    };
    let config = HitchConfig {
        frames: 30,
        frame_seconds: 1.0 / 120.0,
        speed: 16.0,
        ..HitchConfig::default()
    };
    let report = hitch::run(&gpu.device, &gpu.queue, Scene::default(), &config);

    assert_eq!(report.frames(), 30);
    assert!(report.warmup_frames > 0, "the warm-up ran");
    assert!(report.resident > 0, "chunks streamed in");
    assert!(
        report.percentile(Stage::Total, 0.99) >= report.percentile(Stage::Upload, 0.99),
        "the frame total includes the upload"
    );
    assert!(report.max(Stage::Total) <= std::time::Duration::from_secs(5));
    let table = report.table();
    for stage in ["pump", "upload", "finish", "total"] {
        assert!(table.contains(stage), "{table}");
    }
}
