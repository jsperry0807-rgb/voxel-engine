//! Headless validation for the viewer's chunk shader and render pipeline.
//!
//! The naga test runs everywhere. The offscreen draw needs a wgpu adapter; it
//! skips on machines without one, unless `VOXEL_REQUIRE_GPU=1` is set (CI does
//! that after installing a software Vulkan driver). The draw records with a
//! wgpu validation error scope, so a bad bind group, vertex layout or shader
//! interface fails the test instead of silently passing as a skipped GPU test.

use std::sync::OnceLock;

use voxel_core::constants::{CHUNK_BITS, CHUNK_VOLUME};
use voxel_viewer::atlas::{AtlasBuilder, TILE_BYTES};
use voxel_viewer::atlas_texture::AtlasTexture;
use voxel_viewer::camera::{Camera, CameraUniform};
use voxel_viewer::chunk_mesh::ChunkMesh;
use voxel_viewer::chunk_render::ChunkPipeline;
use voxel_viewer::depth::DepthTarget;
use voxel_viewer::naive_mesh::naive_mesh;

struct TestGpu {
    // Some software drivers do not tolerate the instance or adapter being
    // dropped before the device.
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<&'static TestGpu> {
    static GPU: OnceLock<Option<TestGpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .or_else(|_| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
                apply_limit_buckets: false,
            }))
        })
        .ok()?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(TestGpu {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
        })
    })
    .as_ref()
}

fn test_gpu() -> Option<&'static TestGpu> {
    let gpu = gpu();
    if gpu.is_none() && std::env::var_os("VOXEL_REQUIRE_GPU").is_some() {
        panic!("VOXEL_REQUIRE_GPU is set but no wgpu adapter is available");
    }
    gpu
}

#[test]
fn chunk_shader_is_valid_wgsl() {
    let source = include_str!("../src/shaders/chunk.wgsl");
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("chunk.wgsl does not parse:\n{error}"));
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator
        .validate(&module)
        .unwrap_or_else(|error| panic!("chunk.wgsl does not validate: {error:?}"));
}

#[test]
fn chunk_pipeline_draws_without_validation_errors() {
    let Some(gpu) = test_gpu() else {
        eprintln!("skipping GPU validation: no adapter");
        return;
    };
    let (device, queue) = (&gpu.device, &gpu.queue);

    // Capture every validation error from pipeline creation through submit.
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let camera = Camera::default();
    let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("validation camera uniform"),
        size: std::mem::size_of::<CameraUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&camera_buffer, 0, bytemuck::bytes_of(&camera.uniform()));
    let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("validation camera"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("validation camera"),
        layout: &camera_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buffer.as_entire_binding(),
        }],
    });

    let mut builder = AtlasBuilder::new();
    let tile = [255u8; TILE_BYTES];
    builder.set_tile(1, &tile).expect("slot 1 is valid");
    let atlas = builder.build();
    let atlas_texture = AtlasTexture::new(device, queue, &atlas);

    let mut volume = vec![0u16; CHUNK_VOLUME];
    let index = 1 | (1 << CHUNK_BITS) | (1 << (2 * CHUNK_BITS));
    volume[index] = 1;
    let mesh = naive_mesh(&volume);
    let chunk = ChunkMesh::new(device, queue, &mesh).expect("one voxel is one mesh");

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("validation target"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = DepthTarget::new(device, 64, 64);
    let pipeline = ChunkPipeline::new(device, &camera_layout, &atlas_texture.layout, format);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("validation pass"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("validation pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pipeline.draw(
            &mut pass,
            &chunk,
            &camera_bind_group,
            &atlas_texture.bind_group,
        );
    }
    queue.submit([encoder.finish()]);
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });

    let error = pollster::block_on(error_scope.pop());
    assert!(
        error.is_none(),
        "chunk draw raised a validation error: {error:?}"
    );
}
