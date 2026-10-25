//! GPU upload round-trips. Skips on machines with no wgpu adapter (CI boxes
//! without a GPU or a software rasterizer still run the layout and shader
//! tests).

use std::sync::OnceLock;

use voxel_block::{
    BlockFlags, BlockKind, BlockProperties, BlockRegistryBuilder, FaceTextures, NamespacedKey,
    RenderType,
};
use voxel_render::{
    GpuWorld, MATERIAL_WGSL, MaterialBuffers, SVDAG_WGSL, WgpuSink, material_bind_group_layout,
};
use voxel_storage::CHUNK_VOLUME;
use voxel_storage::ChunkPos;
use voxel_storage::dag::Svdag;
use voxel_storage::gpu::GpuStore;

struct Gpu {
    // Keep the instance and adapter alive for as long as the device: some
    // software drivers do not tolerate tearing them down in the other order.
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<&'static Gpu> {
    static GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .or_else(|_| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                force_fallback_adapter: true,
                ..Default::default()
            }))
        })
        .ok()?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("voxel-render-test"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .ok()?;

        Some(Gpu {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
        })
    })
    .as_ref()
}

fn read_words(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    words: usize,
) -> Vec<u32> {
    let bytes = words as u64 * 4;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback"),
    });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, bytes);
    queue.submit(Some(encoder.finish()));

    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll");

    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll");

    let mapped = slice.get_mapped_range().expect("mapped range");
    let words = mapped
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect();
    drop(mapped);
    staging.unmap();
    words
}

fn registry() -> voxel_block::BlockRegistry {
    let mut builder = BlockRegistryBuilder::new();
    for kind in [
        BlockKind::Stone,
        BlockKind::Grass,
        BlockKind::OakLog,
        BlockKind::Water,
    ] {
        builder.register_kind(kind).expect("built-in kind");
    }
    builder.freeze().expect("freeze")
}

#[test]
fn material_tables_upload_verbatim() {
    let Some(gpu) = gpu() else {
        eprintln!("no wgpu adapter; skipping GPU upload test");
        return;
    };
    let (device, queue) = (&gpu.device, &gpu.queue);
    let registry = registry();
    let materials = MaterialBuffers::upload(device, queue, &registry);

    assert_eq!(materials.blocks() as usize, registry.len());
    assert_eq!(materials.sets() as usize, registry.texture_sets().len());

    let hot = registry.gpu_hot_words();
    assert_eq!(read_words(device, queue, materials.hot(), hot.len()), hot);

    let sets = registry.gpu_texture_set_words();
    assert_eq!(
        read_words(device, queue, materials.texture_sets(), sets.len()),
        sets,
    );
}

#[test]
fn storage_flush_reaches_the_gpu_buffers() {
    let Some(gpu) = gpu() else {
        eprintln!("no wgpu adapter; skipping GPU upload test");
        return;
    };
    let (device, queue) = (&gpu.device, &gpu.queue);

    let mut store = GpuStore::new();
    let mut sink = WgpuSink::new(device, queue);
    let mut dense = vec![0u16; CHUNK_VOLUME];
    for z in 0..8 {
        for y in 0..8 {
            for x in 0..8 {
                dense[x + y * 64 + z * 4096] = 1;
            }
        }
    }
    let dag = Svdag::from_dense(&dense);

    let pos = ChunkPos::new(1, -2, 3);
    store.set_chunk(pos, &dag);
    store.set_overlay(pos, 0, 7);
    store.set_overlay(pos, CHUNK_VOLUME as u32 - 1, 8);
    store.flush(&mut sink);

    assert!(sink.node_words() >= store.pool().len());
    assert_eq!(sink.table_words(), store.table().len());
    assert!(sink.overlay_words() >= store.overlays().len());
    assert!(sink.block_table_words() >= store.block_tables().len());
    assert_eq!(
        read_words(device, queue, sink.nodes(), store.pool().len()),
        store.pool(),
    );
    assert_eq!(
        read_words(device, queue, sink.table(), store.table().len()),
        store.table(),
    );
    assert_eq!(
        read_words(device, queue, sink.overlays(), store.overlays().len()),
        store.overlays(),
    );
    assert_eq!(
        read_words(
            device,
            queue,
            sink.block_tables(),
            store.block_tables().len()
        ),
        store.block_tables(),
    );
}

#[test]
fn world_edits_flush_and_bind() {
    let Some(gpu) = gpu() else {
        eprintln!("no wgpu adapter; skipping GPU upload test");
        return;
    };
    let (device, queue) = (&gpu.device, &gpu.queue);

    let mut world = GpuWorld::new(device, queue, &registry());
    let pos = ChunkPos::new(0, 0, 0);
    let mut dense = vec![0u16; CHUNK_VOLUME];
    dense[0] = 1;
    world.set_chunk(pos, &Svdag::from_dense(&dense));
    world.flush();

    // The bind groups describe the buffers the shaders read.
    let _terrain = world.terrain_bind_group();
    let _materials = world.material_bind_group();

    world.apply_edits(pos, [(1, 2)]);
    world.flush();
    assert_eq!(world.store().lookup(pos, 1, 0, 0), 2);
    assert_eq!(world.store().lookup(pos, 0, 0, 0), 1);

    // Overlay edits are visible without folding them into the DAG.
    world.set_overlay(pos, 2, 3);
    world.flush();
    assert_eq!(world.store().lookup(pos, 2, 0, 0), 3);
    world.clear_overlay(pos, 2);
    world.flush();
    assert_eq!(world.store().lookup(pos, 2, 0, 0), 0);
}

/// A one-thread compute probe that reads the same buffers a renderer would.
const PROBE_WGSL: &str = r#"
@group(2) @binding(0) var<storage, read_write> probe_out: array<u32>;

@compute @workgroup_size(1)
fn probe() {
    let id = sample_chunk(vec3<i32>(0, 0, 0), vec3<u32>(0, 0, 0));
    probe_out[0] = id;
    probe_out[1] = render_type(id);
    probe_out[2] = flags(id);
    probe_out[3] = light_emission(id);
    probe_out[4] = light_opacity(id);
    for (var face: u32 = 0u; face < 6u; face = face + 1u) {
        probe_out[5u + face] = face_texture(id, face);
    }
}
"#;

#[test]
fn shader_reads_the_uploaded_tables() {
    let Some(gpu) = gpu() else {
        eprintln!("no wgpu adapter; skipping GPU shader test");
        return;
    };
    let (device, queue) = (&gpu.device, &gpu.queue);

    // A block whose fields are all distinct, so a layout mismatch cannot pass
    // by coincidence.
    let mut builder = BlockRegistryBuilder::new();
    let tex_top = NamespacedKey::builtin("block/probe_top");
    let tex_side = NamespacedKey::builtin("block/probe_side");
    let tex_bottom = NamespacedKey::builtin("block/probe_bottom");
    let probe = builder
        .register(
            NamespacedKey::builtin("gpu_probe"),
            BlockProperties::new()
                .with_render_type(RenderType::Cutout)
                .with_textures(FaceTextures::column(tex_top, tex_side, tex_bottom))
                .with_flags(BlockFlags::SOLID | BlockFlags::CULLS_SELF)
                .with_light(7, 5),
        )
        .expect("register probe block");
    let registry = builder.freeze().expect("freeze");

    let mut world = GpuWorld::new(device, queue, &registry);
    let dense = vec![probe.as_raw(); CHUNK_VOLUME];
    world.set_chunk(ChunkPos::new(0, 0, 0), &Svdag::from_dense(&dense));
    world.flush();

    let got = run_probe(gpu, &world);
    assert_eq!(got[0], probe.as_raw() as u32, "material id");
    assert_eq!(got[1], RenderType::Cutout.to_bits(), "render type");
    assert_eq!(
        got[2],
        (BlockFlags::SOLID | BlockFlags::CULLS_SELF).as_bits(),
        "hot flags",
    );
    assert_eq!(got[3], 7, "light emission");
    assert_eq!(got[4], 5, "light opacity");

    let faces = registry
        .face_textures(probe)
        .expect("probe block has textures");
    for face in voxel_block::face::Face::ALL {
        assert_eq!(
            got[5 + face.index()],
            faces.get(face).as_raw() as u32,
            "texture for {face}",
        );
    }

    // A carved voxel must shadow the solid DAG with air. The presence bit, not
    // the value, is what makes the overlay win.
    let pos = ChunkPos::new(0, 0, 0);
    world.set_overlay(pos, 0, 0);
    world.flush();
    assert_eq!(
        world.store().lookup(pos, 0, 0, 0),
        0,
        "CPU overlay shadows the DAG"
    );
    assert_eq!(world.store().block_table_slot(pos), Some(0));
    assert!(world.store().block_overlay_slot(pos, 0).is_some());
    let got = run_probe(gpu, &world);
    assert_eq!(got[0], 0, "overlay carves the DAG voxel to air");

    // An overlay-only chunk has no DAG at all; the override is still sampled.
    world.set_chunk(pos, &Svdag::empty());
    world.set_overlay(pos, 0, probe.as_raw());
    world.flush();
    let got = run_probe(gpu, &world);
    assert_eq!(got[0], probe.as_raw() as u32, "overlay without a DAG");
}

/// Runs the one-voxel compute probe and returns its 11 output words.
fn run_probe(gpu: &Gpu, world: &GpuWorld) -> Vec<u32> {
    let (device, queue) = (&gpu.device, &gpu.queue);

    // Group 2: the probe's output.
    let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("probe-output-layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe-output"),
        size: 11 * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let output_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("probe-output"),
        layout: &output_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });

    let terrain_layout = WgpuSink::bind_group_layout(device);
    let material_layout = material_bind_group_layout(device);
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("probe-pipeline-layout"),
        bind_group_layouts: &[
            Some(&terrain_layout),
            Some(&material_layout),
            Some(&output_layout),
        ],
        immediate_size: 0,
    });

    let source = format!("{MATERIAL_WGSL}\n{SVDAG_WGSL}\n{PROBE_WGSL}");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("probe"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("probe"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("probe"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("probe"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &world.terrain_bind_group(), &[]);
        pass.set_bind_group(1, &world.material_bind_group(), &[]);
        pass.set_bind_group(2, &output_bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll");

    read_words(device, queue, &output, 11)
}
