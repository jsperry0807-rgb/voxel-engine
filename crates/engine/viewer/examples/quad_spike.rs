//! M3-00 Phase 1 spike: storage-pull quad records.
//!
//! Gates in this example:
//! - **B1**: a compute pass reconstructs every corner from the 8 B record and
//!   the CPU compares positions, face, material and AO levels exactly.
//! - **B2**: real direct and indirect draws with nonzero `first_instance` fetch
//!   `records[instance_index]` and write the index to an `R32Uint` target; the
//!   CPU checks each quad's centre pixel.
//!
//! Run on the selected adapter:
//! ```sh
//! WGPU_BACKEND=vulkan WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER=1 \
//!   cargo run -p voxel-viewer --example quad_spike
//! VK_LOADER_DEVICE_SELECT=1002:164e ... # integrated GPU
//! ```

use std::mem::size_of;
use std::sync::mpsc;

use voxel_core::Face;
use voxel_mesh::{BlockPos, GreedyQuad, QuadRecord};
use wgpu::{
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingType, BufferBindingType,
    BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, ComputePassDescriptor,
    ComputePipelineDescriptor, DeviceDescriptor, Extent3d, Features, FragmentState, Instance,
    InstanceDescriptor, LoadOp, MultisampleState, Operations, PipelineLayoutDescriptor, PollType,
    PrimitiveState, PrimitiveTopology, RenderPassColorAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor, RequestAdapterOptions, ShaderModuleDescriptor, ShaderSource,
    ShaderStages, StoreOp, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    TextureViewDescriptor, VertexState,
};

const QUADS: usize = 64;
const TARGET: u32 = 256;

fn main() {
    let instance = Instance::new(InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default()))
        .expect("an adapter is available");
    let info = adapter.get_info();
    println!(
        "adapter: {} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    );
    assert!(
        adapter
            .features()
            .contains(Features::INDIRECT_FIRST_INSTANCE),
        "INDIRECT_FIRST_INSTANCE is required for the spike"
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        required_features: Features::INDIRECT_FIRST_INSTANCE,
        ..Default::default()
    }))
    .expect("device");

    let records = records();
    let records_bytes: Vec<u8> = records.iter().flat_map(|record| record.encode()).collect();
    let records_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("quad records"),
        size: records_bytes.len() as u64,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&records_buffer, 0, &records_bytes);

    gate_b1(&device, &queue, &records_buffer, &records);
    gate_b2(&device, &queue, &records_buffer);
    gate_d(&device, &queue, &records_buffer, &records);
}

/// Builds 64 full-block top-face quads on an 8 × 8 grid. Each quad covers a
/// distinct 32 × 32 pixel area, so the centre pixel is known.
fn records() -> Vec<QuadRecord> {
    let mut records = Vec::with_capacity(QUADS);
    for bz in 0..8u32 {
        for bx in 0..8u32 {
            let block = BlockPos::new(bx, 0, bz);
            let mut quad = GreedyQuad {
                face: Face::Top,
                material: (bx + bz * 8) as u16,
                ao: [0, 85, 170, 255],
                origin: [1, 1, 1],
                extent: [1, 1, 1],
            };
            // Top face: u = Z, v = X.
            quad.extent[2] = 16;
            quad.extent[0] = 16;
            records.push(QuadRecord::from_quad(&quad, block));
        }
    }
    records
}

fn map_read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    size: u64,
) -> Vec<u8> {
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("readback"),
        size,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback"),
    });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
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
    bytes
}

fn gate_b1(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    records_buffer: &wgpu::Buffer,
    records: &[QuadRecord],
) {
    let out_bytes = (QUADS * 4 * 16) as u64;
    let out_pos = device.create_buffer(&BufferDescriptor {
        label: Some("b1 positions"),
        size: out_bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let out_mat = device.create_buffer(&BufferDescriptor {
        label: Some("b1 materials"),
        size: out_bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("b1"),
        entries: &[
            storage_entry(0, true, ShaderStages::COMPUTE),
            storage_entry(1, false, ShaderStages::COMPUTE),
            storage_entry(2, false, ShaderStages::COMPUTE),
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("b1"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: records_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: out_pos.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: out_mat.as_entire_binding(),
            },
        ],
    });
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("b1"),
        source: ShaderSource::Wgsl(B1_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("b1"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
        label: Some("b1"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("b1") });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("b1"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(((QUADS * 4).div_ceil(64)) as u32, 1, 1);
    }
    queue.submit([encoder.finish()]);

    let pos = map_read(device, queue, &out_pos, out_bytes);
    let mat = map_read(device, queue, &out_mat, out_bytes);
    let pos: Vec<u32> = pos
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect();
    let mat: Vec<u32> = mat
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect();

    for (quad, record) in records.iter().enumerate() {
        let vertices = record.vertices();
        for (corner, vertex) in vertices.iter().enumerate() {
            let vertex = *vertex;
            let base = (quad * 4 + corner) * 4;
            assert_eq!(
                pos[base] as u16, vertex.local[0],
                "B1 quad {quad} corner {corner} x"
            );
            assert_eq!(
                pos[base + 1] as u16,
                vertex.local[1],
                "B1 quad {quad} corner {corner} y"
            );
            assert_eq!(
                pos[base + 2] as u16,
                vertex.local[2],
                "B1 quad {quad} corner {corner} z"
            );
            assert_eq!((pos[base + 3] & 7) as u8, vertex.face, "B1 face");
            assert_eq!((pos[base + 3] >> 3) as u16, vertex.material, "B1 material");
            assert_eq!(mat[base] as u8, vertex.ao[0] / 85, "B1 ao0");
            assert_eq!(mat[base + 1] as u8, vertex.ao[1] / 85, "B1 ao1");
            assert_eq!(mat[base + 2] as u8, vertex.ao[2] / 85, "B1 ao2");
            assert_eq!(mat[base + 3] as u8, vertex.ao[3] / 85, "B1 ao3");
        }
    }
    println!("B1: {} quads reconstructed exactly", records.len());
}

fn gate_b2(device: &wgpu::Device, queue: &wgpu::Queue, records_buffer: &wgpu::Buffer) {
    let target = device.create_texture(&TextureDescriptor {
        label: Some("b2 target"),
        size: Extent3d {
            width: TARGET,
            height: TARGET,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::R32Uint,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("b2"),
        entries: &[storage_entry(0, true, ShaderStages::VERTEX)],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("b2"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: records_buffer.as_entire_binding(),
        }],
    });
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("b2"),
        source: ShaderSource::Wgsl(B2_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("b2"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("b2"),
        layout: Some(&pipeline_layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState {
                format: TextureFormat::R32Uint,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let indirect = device.create_buffer(&BufferDescriptor {
        label: Some("b2 indirect"),
        size: (4 * size_of::<wgpu::util::DrawIndirectArgs>()) as u64,
        usage: BufferUsages::INDIRECT | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    for meshlet in 0..4u32 {
        let args = wgpu::util::DrawIndirectArgs {
            vertex_count: 6,
            instance_count: 16,
            first_vertex: 0,
            first_instance: meshlet * 16,
        };
        queue.write_buffer(
            &indirect,
            u64::from(meshlet) * size_of::<wgpu::util::DrawIndirectArgs>() as u64,
            args.as_bytes(),
        );
    }

    for (label, indirect_draw) in [("direct", false), ("indirect", true)] {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            for meshlet in 0..4u32 {
                if indirect_draw {
                    pass.draw_indirect(
                        &indirect,
                        u64::from(meshlet) * size_of::<wgpu::util::DrawIndirectArgs>() as u64,
                    );
                } else {
                    pass.draw(0..6, meshlet * 16..(meshlet + 1) * 16);
                }
            }
        }
        queue.submit([encoder.finish()]);
        let pixels = read_texture(device, queue, &target, TARGET);
        for bz in 0..8u32 {
            for bx in 0..8u32 {
                let expected = bx + bz * 8;
                let x = bx * 32 + 16;
                let y = (TARGET - 1) - (bz * 32 + 16);
                let value = pixels[(y * TARGET + x) as usize];
                assert_eq!(value, expected, "B2 {label} quad {expected} at ({x}, {y})");
            }
        }
        println!("B2 {label}: 64 quad centres matched");
    }
}

fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: u32,
) -> Vec<u32> {
    let bytes_per_row = size * 4;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("texture readback"),
        size: u64::from(bytes_per_row) * u64::from(size),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("texture readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size),
            },
        },
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
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
    buffer.unmap();
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect()
}

fn storage_entry(binding: u32, read_only: bool, visibility: ShaderStages) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

const B1_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read> records: array<vec2<u32>>;
@group(0) @binding(1) var<storage, read_write> out_pos: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read_write> out_mat: array<vec4<u32>>;

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

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    let quad = index / 4u;
    if (quad >= arrayLength(&records)) { return; }
    let corner = index % 4u;
    let rec = records[quad];
    let p = reconstruct(rec.x, rec.y, corner);
    let face = rec.y & 7u;
    let material = (rec.x >> 24u) | ((rec.y >> 26u) << 8u);
    out_pos[index] = vec4<u32>(p[0], p[1], p[2], face | (material << 3u));
    out_mat[index] = vec4<u32>(
        (rec.y >> 17u) & 3u,
        (rec.y >> 19u) & 3u,
        (rec.y >> 21u) & 3u,
        (rec.y >> 23u) & 3u,
    );
}
"#;

const B2_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read> records: array<vec2<u32>>;

const CORNERS: array<u32, 6> = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);

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

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) quad: u32,
};

@vertex
fn vs(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance: u32) -> VertexOutput {
    let rec = records[instance];
    let p = reconstruct(rec.x, rec.y, CORNERS[vertex_index % 6u]);
    var out: VertexOutput;
    out.pos = vec4<f32>(
        f32(p[0]) / 64.0 - 1.0,
        f32(p[2]) / 64.0 - 1.0,
        0.0,
        1.0,
    );
    out.quad = instance;
    return out;
}

@fragment
fn fs(input: VertexOutput) -> @location(0) u32 {
    return input.quad;
}
"#;

fn gate_d(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    records_buffer: &wgpu::Buffer,
    records: &[QuadRecord],
) {
    // The current indexed 16 B path.
    let mut vertices = Vec::with_capacity(records.len() * 4 * 16);
    let mut indices = Vec::with_capacity(records.len() * 6);
    for (quad, record) in records.iter().enumerate() {
        for vertex in record.vertices() {
            vertices.extend_from_slice(&vertex.encode());
        }
        let base = (quad * 4) as u16;
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let vertex_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("gate d vertices"),
        size: vertices.len() as u64,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let index_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("gate d indices"),
        size: (indices.len() * 2) as u64,
        usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertex_buffer, 0, &vertices);
    queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(&indices));

    let target = |label: &str| {
        device.create_texture(&TextureDescriptor {
            label: Some(label),
            size: Extent3d {
                width: TARGET,
                height: TARGET,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let indexed_target = target("gate d indexed");
    let instanced_target = target("gate d instanced");

    let indexed = make_indexed_pipeline(device);
    let instanced = make_instanced_color_pipeline(device, records_buffer);
    for (pipeline, target, indexed_draw) in [
        (&indexed, &indexed_target, true),
        (&instanced, &instanced_target, false),
    ] {
        let view = target.create_view(&TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gate d"),
        });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("gate d"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(wgpu::Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&pipeline.pipeline);
            if indexed_draw {
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..indices.len() as u32, 0, 0..1);
            } else {
                pass.set_bind_group(
                    0,
                    pipeline.bind_group.as_ref().expect("instanced bind group"),
                    &[],
                );
                pass.draw(0..6, 0..records.len() as u32);
            }
        }
        queue.submit([encoder.finish()]);
    }

    let indexed = read_rgba8(device, queue, &indexed_target, TARGET);
    let instanced = read_rgba8(device, queue, &instanced_target, TARGET);
    let mut max_diff = 0u8;
    for (a, b) in indexed.iter().zip(instanced.iter()) {
        max_diff = max_diff.max(a.abs_diff(*b));
    }
    assert!(
        max_diff <= 1,
        "Gate D max channel difference {max_diff} > 1 LSB"
    );
    println!("Gate D: indexed vs instanced max channel diff {max_diff} (<= 1 LSB)");
}

struct ColorPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group: Option<wgpu::BindGroup>,
}

fn make_indexed_pipeline(device: &wgpu::Device) -> ColorPipeline {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("gate d indexed"),
        source: ShaderSource::Wgsl(D_INDEXED_WGSL.into()),
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("gate d indexed"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("gate d indexed"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(VERTEX_LAYOUT_16)],
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
            topology: PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    ColorPipeline {
        pipeline,
        bind_group: None,
    }
}

fn make_instanced_color_pipeline(
    device: &wgpu::Device,
    records_buffer: &wgpu::Buffer,
) -> ColorPipeline {
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("gate d instanced"),
        entries: &[storage_entry(0, true, ShaderStages::VERTEX)],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gate d instanced"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: records_buffer.as_entire_binding(),
        }],
    });
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("gate d instanced"),
        source: ShaderSource::Wgsl(D_INSTANCED_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("gate d instanced"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("gate d instanced"),
        layout: Some(&pipeline_layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
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
            topology: PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    ColorPipeline {
        pipeline,
        bind_group: Some(bind_group),
    }
}

fn read_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: u32,
) -> Vec<u8> {
    let bytes_per_row = size * 4;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("gate d readback"),
        size: u64::from(bytes_per_row) * u64::from(size),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gate d readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size),
            },
        },
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
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
    buffer.unmap();
    bytes
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

const D_INDEXED_WGSL: &str = r#"
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
fn vs(input: In, @builtin(vertex_index) vi: u32) -> Out {
    let ao = array<u32, 4>(input.face_ao0.y, input.face_ao0.z, input.face_ao0.w, input.ao3_reserved.x);
    let p = vec2<f32>(
        f32(input.local_material.x) / 64.0 - 1.0,
        f32(input.local_material.z) / 64.0 - 1.0,
    );
    var out: Out;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.face = input.face_ao0.x;
    out.material = input.local_material.w;
    out.ao = f32(ao[vi % 4u]) / 255.0;
    return out;
}
@fragment
fn fs(input: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(f32(input.face) / 6.0, input.ao, f32(input.material % 256u) / 255.0, 1.0);
}
"#;

const D_INSTANCED_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read> records: array<vec2<u32>>;
const CORNERS: array<u32, 6> = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
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
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) instance: u32) -> Out {
    let rec = records[instance];
    let corner = CORNERS[vi % 6u];
    let p = reconstruct(rec.x, rec.y, corner);
    let level = (rec.y >> (17u + 2u * corner)) & 3u;
    var out: Out;
    out.pos = vec4<f32>(
        f32(p[0]) / 64.0 - 1.0,
        f32(p[2]) / 64.0 - 1.0,
        0.0,
        1.0,
    );
    out.face = rec.y & 7u;
    out.material = (rec.x >> 24u) | ((rec.y >> 26u) << 8u);
    out.ao = f32(level) / 3.0;
    return out;
}
@fragment
fn fs(input: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(f32(input.face) / 6.0, input.ao, f32(input.material % 256u) / 255.0, 1.0);
}
"#;
