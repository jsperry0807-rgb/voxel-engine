//! Prints the wgpu adapters and the limits the M3-00 spike depends on.
//!
//! ```sh
//! WGPU_BACKEND=vulkan WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER=1 \
//!   cargo run -p voxel-viewer --example adapter_info
//! VK_LOADER_DEVICE_SELECT=1002:164e ... # select the integrated GPU
//! ```

use wgpu::{Backends, Features, Instance, InstanceDescriptor, RequestAdapterOptions};

fn main() {
    let instance = Instance::new(InstanceDescriptor::new_without_display_handle_from_env());
    println!("adapters:");
    for adapter in pollster::block_on(instance.enumerate_adapters(Backends::all())) {
        let info = adapter.get_info();
        println!(
            "  {} | backend {:?} | {:?} | {:?}",
            info.name, info.backend, info.device_type, info.driver
        );
    }

    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default()))
        .expect("an adapter is available");
    let info = adapter.get_info();
    let limits = adapter.limits();
    let features = adapter.features();
    println!("\nselected: {}", info.name);
    println!("  backend: {:?} / {:?}", info.backend, info.device_type);
    println!(
        "  INDIRECT_FIRST_INSTANCE: {}",
        features.contains(Features::INDIRECT_FIRST_INSTANCE)
    );
    println!(
        "  TIMESTAMP_QUERY: {} / TIMESTAMP_QUERY_INSIDE_PASSES: {}",
        features.contains(Features::TIMESTAMP_QUERY),
        features.contains(Features::TIMESTAMP_QUERY_INSIDE_PASSES)
    );
    println!(
        "  max_storage_buffers_per_shader_stage: {}",
        limits.max_storage_buffers_per_shader_stage
    );
    println!(
        "  max_storage_buffer_binding_size: {} MiB",
        limits.max_storage_buffer_binding_size / (1024 * 1024)
    );
    println!(
        "  max_buffer_size: {} MiB",
        limits.max_buffer_size / (1024 * 1024)
    );
    println!(
        "  max_compute_workgroup_size_x: {}",
        limits.max_compute_workgroup_size_x
    );
}
