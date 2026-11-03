//! Headless validation for the viewer's chunk shader and render pipeline.
//!
//! The naga test runs everywhere. The offscreen draw needs a wgpu adapter; it
//! skips on machines without one, unless `VOXEL_REQUIRE_GPU=1` is set (CI does
//! that after installing a software Vulkan driver). The draw records with a
//! wgpu validation error scope, so a bad bind group, vertex layout or shader
//! interface fails the test instead of silently passing as a skipped GPU test.

use std::sync::OnceLock;

use glam::Vec3;

use voxel_core::constants::{CHUNK_BITS, CHUNK_VOLUME};
use voxel_mesh::PackedVertex;
use voxel_viewer::atlas::{AtlasBuilder, TILE_BYTES};
use voxel_viewer::atlas_texture::AtlasTexture;
use voxel_viewer::camera::{Camera, CameraUniform};
use voxel_viewer::chunk_content;
use voxel_viewer::chunk_lighting;
use voxel_viewer::chunk_mesh::ChunkMesh;
use voxel_viewer::chunk_render::{ChunkPipeline, VERTEX_LAYOUT};
use voxel_viewer::depth::DepthTarget;

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
        // `VOXEL_SKIP_GPU=1` opts out on hosts where the WSL/Dozen GPU
        // path is unstable; the test then reports a skip, exactly like a
        // host with no adapter.
        if std::env::var_os("VOXEL_SKIP_GPU").is_some() {
            return None;
        }
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
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
    // The chunk shader is a template: the light-cell edge is substituted from
    // `LIGHT_CELLS_PER_EDGE` so the light indexing matches the uploaded volume.
    let template = include_str!("../src/shaders/chunk.wgsl");
    assert!(
        template.contains("{{CELLS}}"),
        "chunk.wgsl no longer contains the {{CELLS}} placeholder"
    );
    let source = template.replace(
        "{{CELLS}}",
        &format!("{}u", voxel_core::content_constants::LIGHT_CELLS_PER_EDGE),
    );
    validate_wgsl("chunk.wgsl", &source);
}

#[test]
fn screenshot_shader_is_valid_wgsl() {
    // The file is a template; the example substitutes the runtime atlas
    // dimensions. Validate a concrete instance and that every placeholder the
    // example replaces still exists.
    let template = include_str!("../src/shaders/screenshot.wgsl");
    for token in ["{{TILE}}", "{{EDGE}}", "{{ATLAS}}"] {
        assert!(
            template.contains(token),
            "screenshot.wgsl no longer contains {token}, which the example substitutes"
        );
    }
    // Both instances the example builds: the 256² runtime atlas and the
    // legacy 2048² per-face source sheet.
    for (tile, edge, atlas) in [("16.0", "16u", "256.0"), ("256.0", "8u", "2048.0")] {
        let source = template
            .replace("{{TILE}}", tile)
            .replace("{{EDGE}}", edge)
            .replace("{{ATLAS}}", atlas);
        validate_wgsl("screenshot.wgsl", &source);
    }
}

fn validate_wgsl(name: &str, source: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("{name} does not parse:\n{error}"));
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator
        .validate(&module)
        .unwrap_or_else(|error| panic!("{name} does not validate: {error:?}"));
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

    // Frame the single voxel at chunk-local (1, 1, 1), i.e. world
    // (0.0625, 0.0625, 0.0625) at the ADR-010 6.25 cm scale.
    let camera = Camera::looking_at(Vec3::new(0.6, 0.5, 0.6), Vec3::splat(0.093_75), 1.0);
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
    let content = chunk_content::build(&volume, voxel_storage::ChunkPos::new(0, 0, 0));
    let chunk = ChunkMesh::from_parts(device, &content.mesh.vertices, &content.mesh.indices)
        .expect("one voxel is one mesh");

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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = DepthTarget::new(device, 64, 64);
    let data_layout = chunk_lighting::layout(device);
    let lighting = chunk_lighting::upload(
        device,
        &data_layout,
        &content.light,
        &content.probes,
        [0.0; 3],
    );
    let pipeline = ChunkPipeline::new(
        device,
        &camera_layout,
        &atlas_texture.layout,
        &data_layout,
        format,
    );

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
            &lighting.bind_group,
        );
    }
    queue.submit([encoder.finish()]);

    // A pipeline that draws nothing (empty index range, a depth regression, a
    // shader that discards every fragment) still passes validation, so read the
    // target back and require at least one lit pixel.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("validation readback"),
        size: (64 * 64 * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("validation readback"),
    });
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
                bytes_per_row: Some(64 * 4),
                rows_per_image: Some(64),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll");
    receiver
        .recv()
        .expect("map callback ran")
        .expect("map the readback");

    let pixels = slice.get_mapped_range().expect("mapped range");
    let lit = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[0] > 0 || pixel[1] > 0 || pixel[2] > 0);
    drop(pixels);
    readback.unmap();
    assert!(lit, "the chunk draw wrote no pixels");

    let error = pollster::block_on(error_scope.pop());
    assert!(
        error.is_none(),
        "chunk draw raised a validation error: {error:?}"
    );
}

#[test]
fn packed_vertex_attributes_decode_exactly() {
    let Some(gpu) = test_gpu() else {
        return;
    };
    let device = &gpu.device;
    let queue = &gpu.queue;

    let vertices = [
        PackedVertex::from_vertex(&voxel_mesh::Vertex::new(
            [1, 2, 3],
            0x0005,
            5,
            [0, 85, 170, 255],
        )),
        PackedVertex::from_vertex(&voxel_mesh::Vertex::new(
            [4, 5, 6],
            0x000A,
            3,
            [0, 85, 170, 255],
        )),
        PackedVertex::from_vertex(&voxel_mesh::Vertex::new(
            [7, 8, 9],
            0x000F,
            1,
            [0, 85, 170, 255],
        )),
        PackedVertex::from_vertex(&voxel_mesh::Vertex::new(
            [10, 11, 12],
            0x0013,
            4,
            [0, 85, 170, 255],
        )),
    ];
    let bytes: Vec<u8> = vertices.iter().flat_map(|vertex| vertex.encode()).collect();
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("packed attribute vertices"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertex_buffer, 0, &bytes);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("packed attribute shader"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
struct In {
    @location(0) local_spare: vec4<u32>,
    @location(1) material_packed: vec2<u32>,
};
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) value: u32,
};
@vertex
fn vs(input: In, @builtin(vertex_index) vertex_index: u32) -> Out {
    let packed = input.material_packed.y;
    let face = packed & 7u;
    let level = (packed >> (3u + 2u * (vertex_index % 4u))) & 3u;
    let value = input.local_spare.x
        | (input.local_spare.y << 8u)
        | (input.local_spare.z << 16u)
        | ((input.material_packed.x & 7u) << 24u)
        | (face << 27u)
        | (level << 30u);
    var out: Out;
    out.pos = vec4<f32>(-1.0 + (f32(vertex_index) + 0.5) / 2.0, 0.0, 0.0, 1.0);
    out.value = value;
    return out;
}
@fragment
fn fs(input: Out) -> @location(0) u32 {
    return input.value;
}
"#
            .into(),
        ),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("packed attribute layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("packed attribute pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(VERTEX_LAYOUT)],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R32Uint,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::PointList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("packed attribute target"),
        size: wgpu::Extent3d {
            width: 4,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Uint,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("packed attribute pass"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("packed attribute pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.draw(0..4, 0..1);
    }
    queue.submit([encoder.finish()]);

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("packed attribute readback"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("packed attribute readback"),
    });
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
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: 4,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll");
    receiver.recv().expect("map callback").expect("map");
    let values: Vec<u32> = slice
        .get_mapped_range()
        .expect("mapped range")
        .as_chunks::<4>()
        .0
        .iter()
        .take(4)
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect();
    readback.unmap();

    let expected = [
        (1u32 | 2 << 8 | 3 << 16 | 5 << 24 | 5 << 27),
        (4u32 | 5 << 8 | 6 << 16 | 2 << 24 | 3 << 27 | 1 << 30),
        (7u32 | 8 << 8 | 9 << 16 | 7 << 24 | 1 << 27 | 2 << 30),
        (10u32 | 11 << 8 | 12 << 16 | 3 << 24 | 4 << 27 | 3 << 30),
    ];
    assert_eq!(values, expected, "packed vertex attributes did not decode");
}

/// Renders one chunk mesh to a 64×64 target and returns the RGBA pixels.
fn render_chunk_to_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &ChunkPipeline,
    chunk: &ChunkMesh,
    camera: &wgpu::BindGroup,
    atlas: &wgpu::BindGroup,
    data: &wgpu::BindGroup,
) -> Vec<u8> {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("origin target"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = DepthTarget::new(device, 64, 64);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("origin pass"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("origin pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
        pipeline.draw(&mut pass, chunk, camera, atlas, data);
    }
    queue.submit([encoder.finish()]);

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("origin readback"),
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("origin readback"),
    });
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
                bytes_per_row: Some(256),
                rows_per_image: Some(64),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll");
    receiver
        .recv()
        .expect("map callback ran")
        .expect("map the readback");
    let pixels = slice.get_mapped_range().expect("mapped range").to_vec();
    readback.unmap();
    pixels
}

#[test]
fn chunk_origin_moves_the_chunk_in_world_space() {
    let Some(gpu) = test_gpu() else {
        return;
    };
    let device = &gpu.device;
    let queue = &gpu.queue;
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("origin camera"),
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
    let camera_bind = |position: Vec3, target: Vec3| {
        let camera = Camera::looking_at(position, target, 1.0);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("origin camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&camera.uniform()));
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("origin camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    };

    let centre = Vec3::splat(0.093_75);
    let position = Vec3::new(0.6, 0.5, 0.6);
    let shift = Vec3::new(4.0, 0.0, 0.0);
    let left_camera = camera_bind(position, centre);
    let positive_camera = camera_bind(position + shift, centre + shift);
    let negative_camera = camera_bind(position - shift, centre - shift);

    let mut builder = AtlasBuilder::new();
    builder
        .set_tile(1, &[255u8; TILE_BYTES])
        .expect("slot 1 is valid");
    let atlas_texture = AtlasTexture::new(device, queue, &builder.build());

    let mut volume = vec![0u16; CHUNK_VOLUME];
    volume[1 | (1 << CHUNK_BITS) | (1 << (2 * CHUNK_BITS))] = 1;
    let content = chunk_content::build(&volume, voxel_storage::ChunkPos::new(0, 0, 0));
    let chunk = ChunkMesh::from_parts(device, &content.mesh.vertices, &content.mesh.indices)
        .expect("one voxel is one mesh");

    let data_layout = chunk_lighting::layout(device);
    let left_data = chunk_lighting::upload(
        device,
        &data_layout,
        &content.light,
        &content.probes,
        [0.0; 3],
    );
    let positive_data = chunk_lighting::upload(
        device,
        &data_layout,
        &content.light,
        &content.probes,
        [64.0, 0.0, 0.0],
    );
    let negative_data = chunk_lighting::upload(
        device,
        &data_layout,
        &content.light,
        &content.probes,
        [-64.0, 0.0, 0.0],
    );
    let pipeline = ChunkPipeline::new(
        device,
        &camera_layout,
        &atlas_texture.layout,
        &data_layout,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );

    let left = render_chunk_to_pixels(
        device,
        queue,
        &pipeline,
        &chunk,
        &left_camera,
        &atlas_texture.bind_group,
        &left_data.bind_group,
    );
    let positive = render_chunk_to_pixels(
        device,
        queue,
        &pipeline,
        &chunk,
        &positive_camera,
        &atlas_texture.bind_group,
        &positive_data.bind_group,
    );
    let negative = render_chunk_to_pixels(
        device,
        queue,
        &pipeline,
        &chunk,
        &negative_camera,
        &atlas_texture.bind_group,
        &negative_data.bind_group,
    );
    assert!(
        left.chunks(4)
            .any(|pixel| pixel[0] > 0 || pixel[1] > 0 || pixel[2] > 0),
        "the origin-0 draw wrote no pixels"
    );
    for (label, shifted) in [("positive", &positive), ("negative", &negative)] {
        let max_diff = left
            .iter()
            .zip(shifted)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            max_diff <= 2,
            "a 64-voxel {label} origin shift with a matching camera shift should reproduce the image; max channel diff {max_diff}"
        );
    }

    let error = pollster::block_on(error_scope.pop());
    assert!(
        error.is_none(),
        "origin draw raised a validation error: {error:?}"
    );
}
