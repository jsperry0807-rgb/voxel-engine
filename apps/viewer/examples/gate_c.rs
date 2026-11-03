//! M3-00 Gate C: full-frame timings for the indexed and instanced paths.
//!
//! Scene: the largest u16-meshlet chunk of the frozen `m2-128` fixture
//! (capped at 16,384 quads) tiled to 64 visible chunks on a fixed 300-frame
//! camera path. Both paths draw the same 64 chunks with per-draw chunk
//! offsets. Timings use GPU timestamp queries, vsync-free (no surface), with
//! 60 warm-up frames and 5 runs; the reported value is the median of the five
//! run medians.
//!
//! ```sh
//! WGPU_BACKEND=vulkan WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER=1 \
//!   cargo run -p voxel-viewer --features chunk128 --example gate_c
//! VK_LOADER_DEVICE_SELECT=1002:164e ... # integrated GPU
//! ```

use std::mem::size_of;
use std::num::NonZeroU64;
use std::sync::mpsc;

use glam::{Mat4, Vec3};
use voxel_core::constants::{CHUNK_DIM, CHUNK_VOLUME};
use voxel_mesh::{GreedyQuad, QuadRecord, for_each_block, greedy_block, mesh_block};
use voxel_storage::Apron;
use voxel_storage::scan::scan_save;
use wgpu::{
    BindGroup, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource,
    BindingType, BufferBinding, BufferBindingType, BufferDescriptor, BufferUsages,
    ColorTargetState, ColorWrites, DepthStencilState, Device, DeviceDescriptor, Extent3d, Features,
    FragmentState, Instance, InstanceDescriptor, LoadOp, MultisampleState, Operations,
    PipelineLayoutDescriptor, PollType, PrimitiveState, PrimitiveTopology, QuerySetDescriptor,
    QueryType, Queue, RenderPassColorAttachment, RenderPassDepthStencilAttachment,
    RenderPassDescriptor, RenderPassTimestampWrites, RenderPipeline, RenderPipelineDescriptor,
    RequestAdapterOptions, ShaderModuleDescriptor, ShaderSource, ShaderStages, StoreOp,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
    VertexState,
};

const CHUNKS: usize = 64;
const GRID: usize = 8;
const QUAD_LIMIT: usize = 16_384;
const TARGET_WIDTH: u32 = 1920;
const TARGET_HEIGHT: u32 = 1080;
const WARMUP: usize = 60;
const UNIFORM_STRIDE: u64 = 256;
const DEFAULT_FRAMES: usize = 300;
const DEFAULT_RUNS: usize = 5;
const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

type ChunkData = (usize, Vec<QuadRecord>, Vec<u8>, Vec<u16>);

fn main() {
    let frames = env_usize("GATE_C_FRAMES", DEFAULT_FRAMES);
    let runs = env_usize("GATE_C_RUNS", DEFAULT_RUNS);
    let instance = Instance::new(InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default()))
        .expect("an adapter is available");
    let info = adapter.get_info();
    println!(
        "gate c adapter: {} ({:?}, {:?}, driver {:?})",
        info.name, info.backend, info.device_type, info.driver
    );
    assert!(
        adapter.features().contains(Features::TIMESTAMP_QUERY),
        "TIMESTAMP_QUERY is required"
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        required_features: Features::TIMESTAMP_QUERY,
        ..Default::default()
    }))
    .expect("device");

    let scene = scene(&device, &queue, frames);
    println!(
        "scene: {} quads/chunk, {} chunks, {} frames, {} runs",
        scene.quads, CHUNKS, frames, runs
    );

    let indexed = run_path(&device, &queue, &scene, Path::Indexed, frames, runs);
    let instanced = run_path(&device, &queue, &scene, Path::Instanced, frames, runs);
    let indexed_median = median(&indexed);
    let instanced_median = median(&instanced);
    let ratio = instanced_median / indexed_median;
    println!(
        "indexed run medians: [{}] ms",
        indexed
            .iter()
            .map(|v| format!("{v:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "instanced run medians: [{}] ms",
        instanced
            .iter()
            .map(|v| format!("{v:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "indexed median {indexed_median:.3} ms, instanced median {instanced_median:.3} ms, ratio {ratio:.3}x"
    );
    assert!(
        ratio <= 1.10,
        "instanced full-frame is {:.1}% slower than indexed",
        (ratio - 1.0) * 100.0
    );
    println!("Gate C PASS: instanced full-frame within 10% of indexed");
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    Indexed,
    Instanced,
}

struct Scene {
    quads: usize,
    index_count: u32,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    camera_layout: BindGroupLayout,
    camera_bind_group: BindGroup,
    records_layout: BindGroupLayout,
    records_bind_group: BindGroup,
    target: wgpu::Texture,
    depth: wgpu::Texture,
}

fn scene(device: &Device, queue: &Queue, frames: usize) -> Scene {
    let mut best: Option<ChunkData> = None;
    scan_save("benchmarks/fixtures/m2-128/save", |chunk| {
        let mut dense = vec![0u16; CHUNK_VOLUME];
        chunk.dag.expand(&mut dense);
        for (index, material) in chunk.overlay.iter() {
            dense[index as usize] = material;
        }
        let mut records = Vec::new();
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut quads = 0usize;
        let apron = Apron::new();
        let mut scratch_quads: Vec<GreedyQuad> = Vec::new();
        for_each_block(&dense, &apron, |block, scratch| {
            if quads >= QUAD_LIMIT {
                return;
            }
            greedy_block(scratch, &mut scratch_quads);
            if quads + scratch_quads.len() > QUAD_LIMIT {
                return;
            }
            let mesh = mesh_block(&scratch_quads, block);
            let base = vertices.len() / 16;
            for vertex in &mesh.vertices {
                vertices.extend_from_slice(&vertex.encode());
            }
            indices.extend(mesh.indices.iter().map(|index| base as u16 + index));
            for quad in &scratch_quads {
                records.push(QuadRecord::from_quad(quad, block));
            }
            quads += scratch_quads.len();
        });
        if best.as_ref().is_none_or(|(best, _, _, _)| quads > *best) {
            best = Some((quads, records, vertices, indices));
        }
    })
    .expect("the frozen m2-128 save scans");
    let (quads, records, vertices, indices) = best.expect("the fixture has a chunk");
    assert!(quads > 0 && quads <= QUAD_LIMIT);

    let records_bytes: Vec<u8> = records.iter().flat_map(|record| record.encode()).collect();
    let records_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("gate c records"),
        size: records_bytes.len() as u64,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&records_buffer, 0, &records_bytes);
    let vertex_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("gate c vertices"),
        size: vertices.len() as u64,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertex_buffer, 0, &vertices);
    let index_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("gate c indices"),
        size: (indices.len() * size_of::<u16>()) as u64,
        usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(&indices));
    let shared_indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
    let shared_index_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("gate c shared quad indices"),
        size: (shared_indices.len() * size_of::<u16>()) as u64,
        usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &shared_index_buffer,
        0,
        bytemuck::cast_slice(&shared_indices),
    );

    let entries = frames * CHUNKS;
    let camera_size = entries as u64 * UNIFORM_STRIDE;
    let camera_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("gate c camera"),
        size: camera_size,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut camera_bytes = vec![0u8; camera_size as usize];
    for frame in 0..frames {
        let view_proj = camera(frame, frames);
        for chunk in 0..CHUNKS {
            let offset = (frame * CHUNKS + chunk) as u64 * UNIFORM_STRIDE;
            let start = offset as usize;
            camera_bytes[start..start + 64]
                .copy_from_slice(bytemuck::cast_slice(&view_proj.to_cols_array()));
            let cx = (chunk % GRID) as u32;
            let cz = (chunk / GRID) as u32;
            let chunk_offset = [cx * CHUNK_DIM as u32, 0, cz * CHUNK_DIM as u32, 0];
            camera_bytes[start + 64..start + 80]
                .copy_from_slice(bytemuck::cast_slice(&chunk_offset));
        }
    }
    queue.write_buffer(&camera_buffer, 0, &camera_bytes);

    let camera_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("gate c camera"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: NonZeroU64::new(80),
            },
            count: None,
        }],
    });
    let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gate c camera"),
        layout: &camera_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: &camera_buffer,
                offset: 0,
                size: NonZeroU64::new(80),
            }),
        }],
    });
    let records_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("gate c records"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let records_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gate c records"),
        layout: &records_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: records_buffer.as_entire_binding(),
        }],
    });

    let target = device.create_texture(&TextureDescriptor {
        label: Some("gate c target"),
        size: Extent3d {
            width: TARGET_WIDTH,
            height: TARGET_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth = device.create_texture(&TextureDescriptor {
        label: Some("gate c depth"),
        size: Extent3d {
            width: TARGET_WIDTH,
            height: TARGET_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    Scene {
        quads,
        index_count: indices.len() as u32,
        vertex_buffer,
        index_buffer,
        camera_layout,
        camera_bind_group,
        records_layout,
        records_bind_group,
        target,
        depth,
    }
}

fn camera(frame: usize, frames: usize) -> Mat4 {
    let t = if frames <= 1 {
        0.0
    } else {
        frame as f32 / (frames - 1) as f32
    };
    let eye = Vec3::new(128.0 + 768.0 * t, 220.0, -180.0);
    let target = Vec3::new(512.0, 40.0, 512.0);
    let view = Mat4::look_at_rh(eye, target, Vec3::Y);
    let proj = Mat4::perspective_rh(60_f32.to_radians(), 16.0 / 9.0, 1.0, 3000.0);
    proj * view
}

struct PathPipeline {
    pipeline: RenderPipeline,
}

fn pipeline(device: &Device, scene: &Scene, path: Path) -> PathPipeline {
    let (source, records) = match path {
        Path::Indexed => (INDEXED_WGSL.to_owned(), false),
        Path::Instanced => (INSTANCED_WGSL.to_owned(), true),
    };
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("gate c"),
        source: ShaderSource::Wgsl(source.into()),
    });
    let layouts: &[Option<&BindGroupLayout>] = if records {
        &[Some(&scene.camera_layout), Some(&scene.records_layout)]
    } else {
        &[Some(&scene.camera_layout)]
    };
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("gate c"),
        bind_group_layouts: layouts,
        immediate_size: 0,
    });
    let buffers = if records {
        &[][..]
    } else {
        &[Some(VERTEX_LAYOUT_16)][..]
    };
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("gate c"),
        layout: Some(&pipeline_layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers,
        },
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState {
                format: TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: if records {
                PrimitiveTopology::TriangleStrip
            } else {
                PrimitiveTopology::TriangleList
            },
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    PathPipeline { pipeline }
}

fn run_path(
    device: &Device,
    queue: &Queue,
    scene: &Scene,
    path: Path,
    frames: usize,
    runs: usize,
) -> Vec<f64> {
    let pipeline = pipeline(device, scene, path);
    let mut medians = Vec::with_capacity(runs);
    for run in 0..runs {
        let median = render_run(device, queue, scene, &pipeline, path, frames);
        println!("{} run {run}: median {median:.3} ms", path_name(path));
        medians.push(median);
    }
    medians
}

fn render_run(
    device: &Device,
    queue: &Queue,
    scene: &Scene,
    pipeline: &PathPipeline,
    path: Path,
    frames: usize,
) -> f64 {
    let target_view = scene.target.create_view(&TextureViewDescriptor::default());
    let depth_view = scene.depth.create_view(&TextureViewDescriptor::default());

    // Warm-up without timestamps.
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gate c warmup"),
    });
    for _ in 0..WARMUP {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("gate c warmup"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(Operations {
                    load: LoadOp::Clear(1.0),
                    store: StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        draw_chunks(&mut pass, scene, pipeline, path, 0);
    }
    queue.submit([encoder.finish()]);

    let query = device.create_query_set(&QuerySetDescriptor {
        label: Some("gate c timestamps"),
        ty: QueryType::Timestamp,
        count: (2 * frames) as u32,
    });
    let resolve = device.create_buffer(&BufferDescriptor {
        label: Some("gate c resolve"),
        size: query_buffer_size(frames),
        usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gate c timed"),
    });
    for frame in 0..frames {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("gate c timed"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(wgpu::Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(Operations {
                    load: LoadOp::Clear(1.0),
                    store: StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: Some(RenderPassTimestampWrites {
                query_set: &query,
                beginning_of_pass_write_index: Some((2 * frame) as u32),
                end_of_pass_write_index: Some((2 * frame + 1) as u32),
            }),
            ..Default::default()
        });
        draw_chunks(&mut pass, scene, pipeline, path, frame);
    }
    encoder.resolve_query_set(&query, 0..(2 * frames) as u32, &resolve, 0);
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("gate c timestamps readback"),
        size: query_buffer_size(frames),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_buffer_to_buffer(&resolve, 0, &staging, 0, query_buffer_size(frames));
    queue.submit([encoder.finish()]);

    let slice = staging.slice(..);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        sender.send(result).unwrap()
    });
    device
        .poll(PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
    receiver.recv().expect("map callback").expect("map");
    let bytes = slice.get_mapped_range().expect("mapped range").to_vec();
    staging.unmap();
    let ticks: Vec<u64> = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|bytes| u64::from_le_bytes(*bytes))
        .collect();
    let period = f64::from(queue.get_timestamp_period());
    let mut frame_ms = Vec::with_capacity(frames);
    for frame in 0..frames {
        let start = ticks[2 * frame];
        let end = ticks[2 * frame + 1];
        assert!(end >= start, "timestamp {frame} goes backwards");
        frame_ms.push((end - start) as f64 * period / 1_000_000.0);
    }
    median(&frame_ms)
}

fn draw_chunks(
    pass: &mut wgpu::RenderPass<'_>,
    scene: &Scene,
    pipeline: &PathPipeline,
    path: Path,
    frame: usize,
) {
    pass.set_pipeline(&pipeline.pipeline);
    if path == Path::Indexed {
        pass.set_vertex_buffer(0, scene.vertex_buffer.slice(..));
        pass.set_index_buffer(scene.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
    } else {
        pass.set_bind_group(1, &scene.records_bind_group, &[]);
        // Four-vertex strip: [1,2,0,3] produces the same (0,1,2)/(0,2,3)
        // triangles as the indexed path with four vertex invocations.
    }
    for chunk in 0..CHUNKS {
        let offset = ((frame * CHUNKS + chunk) as u64 * UNIFORM_STRIDE) as u32;
        pass.set_bind_group(0, &scene.camera_bind_group, &[offset]);
        match path {
            Path::Indexed => {
                pass.draw_indexed(0..scene.index_count, 0, 0..1);
            }
            Path::Instanced => {
                pass.draw(0..4, 0..scene.quads as u32);
            }
        }
    }
}

fn query_buffer_size(frames: usize) -> u64 {
    let raw = (2 * frames * size_of::<u64>()) as u64;
    raw.div_ceil(256) * 256
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted[sorted.len() / 2]
}

fn path_name(path: Path) -> &'static str {
    match path {
        Path::Indexed => "indexed",
        Path::Instanced => "instanced",
    }
}

const VERTEX_ATTRIBUTES_16: [wgpu::VertexAttribute; 3] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Uint16x4,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Uint8x4,
        offset: 8,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Uint8x4,
        offset: 12,
        shader_location: 2,
    },
];
const VERTEX_LAYOUT_16: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 16,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &VERTEX_ATTRIBUTES_16,
};

const INDEXED_WGSL: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    chunk_offset: vec4<u32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
struct In {
    @location(0) local_material: vec4<u32>,
    @location(1) face_ao0: vec4<u32>,
    @location(2) ao3_reserved: vec4<u32>,
};
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) face: u32,
    @location(1) @interpolate(flat) material: u32,
    @location(2) ao: f32,
};
@vertex
fn vs(input: In, @builtin(vertex_index) vertex_index: u32) -> Out {
    let local = vec3<f32>(
        f32(input.local_material.x + camera.chunk_offset.x),
        f32(input.local_material.y + camera.chunk_offset.y),
        f32(input.local_material.z + camera.chunk_offset.z),
    ) * 0.0625;
    let ao = array<u32, 4>(input.face_ao0.y, input.face_ao0.z, input.face_ao0.w, input.ao3_reserved.x);
    var out: Out;
    out.pos = camera.view_proj * vec4<f32>(local, 1.0);
    out.face = input.face_ao0.x;
    out.material = input.local_material.w;
    out.ao = f32(ao[vertex_index % 4u]) / 255.0;
    return out;
}
@fragment
fn fs(input: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(f32(input.face) / 6.0, input.ao, f32(input.material % 64u) / 63.0, 1.0);
}
"#;

const INSTANCED_WGSL: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    chunk_offset: vec4<u32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<storage, read> records: array<vec2<u32>>;
fn corner_sign(face: u32, corner: u32) -> vec2<i32> {
    let positive = face == 0u || face == 2u || face == 4u;
    if (positive) {
        switch corner {
            case 0u: { return vec2<i32>(-1, -1); }
            case 1u: { return vec2<i32>(1, -1); }
            case 2u: { return vec2<i32>(1, 1); }
            default: { return vec2<i32>(-1, 1); }
        }
    }
    switch corner {
        case 0u: { return vec2<i32>(-1, -1); }
        case 1u: { return vec2<i32>(-1, 1); }
        case 2u: { return vec2<i32>(1, 1); }
        default: { return vec2<i32>(1, -1); }
    }
}
fn reconstruct(w0: u32, w1: u32, corner: u32) -> array<u32, 3> {
    var p = array<u32, 3>(w0 & 255u, (w0 >> 8u) & 255u, (w0 >> 16u) & 255u);
    let face = w1 & 7u;
    let axis = face / 2u;
    let positive = face % 2u == 0u;
    let u_axis = (axis + 1u) % 3u;
    let v_axis = (axis + 2u) % 3u;
    let ext_u = ((w1 >> 3u) & 127u) + 1u;
    let ext_v = ((w1 >> 10u) & 127u) + 1u;
    let s = corner_sign(face, corner);
    p[axis] = p[axis] + select(0u, 1u, positive);
    p[u_axis] = p[u_axis] + select(0u, ext_u, s.x > 0);
    p[v_axis] = p[v_axis] + select(0u, ext_v, s.y > 0);
    return p;
}
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) face: u32,
    @location(1) @interpolate(flat) material: u32,
    @location(2) ao: f32,
};
@vertex
fn vs(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance: u32) -> Out {
    let rec = records[instance];
    const STRIP: array<u32, 4> = array<u32, 4>(1u, 2u, 0u, 3u);
    let corner = STRIP[vertex_index];
    let p = reconstruct(rec.x, rec.y, corner);
    let local = vec3<f32>(
        f32(p[0] + camera.chunk_offset.x),
        f32(p[1] + camera.chunk_offset.y),
        f32(p[2] + camera.chunk_offset.z),
    ) * 0.0625;
    let level = (rec.y >> (17u + 2u * corner)) & 3u;
    var out: Out;
    out.pos = camera.view_proj * vec4<f32>(local, 1.0);
    out.face = rec.y & 7u;
    out.material = (rec.x >> 24u) | ((rec.y >> 26u) << 8u);
    out.ao = f32(level) / 3.0;
    return out;
}
@fragment
fn fs(input: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(f32(input.face) / 6.0, input.ao, f32(input.material % 64u) / 63.0, 1.0);
}
"#;
