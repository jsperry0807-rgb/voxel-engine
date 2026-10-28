//! Headless GPU checks for the atlas texture and sampler.
//!
//! These tests skip when no adapter is available, like the render crate's GPU
//! tests. The full-screen pass samples the centre of slot 5 and reads the
//! framebuffer back.

use std::sync::OnceLock;

use voxel_core::constants::{CHUNK_BITS, CHUNK_VOLUME};
use voxel_viewer::atlas::{AtlasBuilder, TILE_BYTES, tile_origin};
use voxel_viewer::atlas_texture::AtlasTexture;
use voxel_viewer::chunk_mesh::ChunkMesh;
use voxel_viewer::naive_mesh::naive_mesh;

struct TestGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// One CPU/software adapter for all GPU tests. The instance and adapter are
/// leaked: llvmpipe crashes if they are torn down after the device.
fn gpu() -> Option<&'static TestGpu> {
    static GPU: OnceLock<Option<TestGpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        let instance = wgpu::Instance::default();
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

const SHADER: &str = r#"
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    return output;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Centre of slot 5: tile origin (80, 0), centre texel (88, 8).
    let uv = vec2<f32>(88.5 / 256.0, 8.5 / 256.0);
    return textureSample(atlas, atlas_sampler, uv);
}
"#;

#[test]
fn atlas_samples_the_expected_tile() {
    let Some(gpu) = gpu() else {
        eprintln!("skipping GPU test: no adapter");
        return;
    };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let mut builder = AtlasBuilder::new();
    let mut red = [0u8; TILE_BYTES];
    for texel in red.as_chunks_mut::<4>().0 {
        *texel = [255, 0, 0, 255];
    }
    builder.set_tile(5, &red).expect("slot 5 is valid");
    let atlas = builder.build();
    let atlas_texture = AtlasTexture::new(device, queue, &atlas);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("atlas test shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    // The camera owns group 0; the atlas is group 1, matching the runtime.
    let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("atlas test empty group 0"),
        entries: &[],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("atlas test"),
        bind_group_layouts: &[Some(&empty_layout), Some(&atlas_texture.layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("atlas test"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    const SIZE: u32 = 64;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("atlas test target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("atlas test readback"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("atlas test"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("atlas test"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(1, &atlas_texture.bind_group, &[]);
        pass.draw(0..6, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll");

    let data = slice.get_mapped_range().expect("mapped range");
    let pixels: Vec<[u8; 4]> = data.as_chunks::<4>().0.to_vec();
    let pixel = pixels[((SIZE / 2) * SIZE + SIZE / 2) as usize];
    drop(data);
    readback.unmap();

    assert_eq!(pixel[3], 255, "alpha should be opaque");
    assert!(pixel[0] > 250, "expected red, got {pixel:?}");
    assert!(pixel[1] < 5, "expected red, got {pixel:?}");
    assert!(pixel[2] < 5, "expected red, got {pixel:?}");

    // The origin helper is part of the contract the test depends on.
    assert_eq!(tile_origin(5), (80, 0));
}

#[test]
fn chunk_mesh_buffers_match_quad_count() {
    let Some(gpu) = gpu() else {
        eprintln!("skipping GPU test: no adapter");
        return;
    };

    let mut volume = vec![0u16; CHUNK_VOLUME];
    for z in 0..8 {
        for y in 0..8 {
            for x in 0..8 {
                let index = x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS));
                volume[index] = 1;
            }
        }
    }
    let mesh = naive_mesh(&volume);
    let buffers = ChunkMesh::new(&gpu.device, &gpu.queue, &mesh).expect("non-empty mesh");

    assert_eq!(buffers.quad_count(), mesh.quad_count());
    assert_eq!(buffers.vertex_count as usize, mesh.vertices.len());
    assert_eq!(buffers.index_count as usize, mesh.indices.len());
    assert_eq!(
        buffers.vertices.size(),
        (mesh.vertices.len() * std::mem::size_of::<voxel_viewer::naive_mesh::Vertex>()) as u64
    );
    assert_eq!(
        buffers.indices.size(),
        (mesh.indices.len() * std::mem::size_of::<u16>()) as u64
    );
    assert_eq!(
        buffers.byte_len(),
        (mesh.quad_count() * 76) as u64,
        "ADR-010 format is 76 bytes per quad"
    );
}
