//! M3-02 mesh pool tests.
//!
//! The pure tests pin the allocator contract: aligned byte ranges that never
//! cross a page, reuse before growth, coalescing, exact accounting and the
//! format-agnostic descriptor. The flight test replays a 256-chunk ring over a
//! 64-column camera flight with real greedy-mesh sizes and reports the
//! fragmentation the pool would carry.
//!
//! The GPU tests need a wgpu adapter; they skip on machines without one unless
//! `VOXEL_REQUIRE_GPU=1` is set, the same rule `tests/validation.rs` uses.

use std::sync::OnceLock;

use glam::Vec3;

use voxel_core::constants::{CHUNK_BITS, CHUNK_VOLUME};
use voxel_storage::ChunkPos;
use voxel_viewer::atlas::{AtlasBuilder, TILE_BYTES};
use voxel_viewer::atlas_texture::AtlasTexture;
use voxel_viewer::camera::{Camera, CameraUniform};
use voxel_viewer::chunk_content;
use voxel_viewer::chunk_lighting;
use voxel_viewer::chunk_mesh::ChunkMesh;
use voxel_viewer::chunk_render::ChunkPipeline;
use voxel_viewer::depth::DepthTarget;
use voxel_viewer::mesh_pool::{
    Arena, DEFAULT_PAGE_BYTES, MAX_PAGE_BYTES, MIN_PAGE_BYTES, MeshFormat, MeshPool, Range,
};
use voxel_viewer::streaming::{ring_chunks, stream_apron, stream_volume};

/// One chunk's pooled record counts.
#[derive(Clone, Copy, Debug)]
struct MeshSize {
    vertices: u32,
    indices: u32,
}

impl MeshSize {
    fn of(position: ChunkPos) -> Self {
        let volume = stream_volume(position);
        let mesh = chunk_content::mesh_with_apron(&volume, &stream_apron(position));
        assert!(!mesh.indices.is_empty(), "{position:?} meshed empty");
        Self {
            vertices: (mesh.vertices.len() / MeshFormat::PACKED_8B.vertex_stride() as usize) as u32,
            indices: mesh.indices.len() as u32,
        }
    }
}

/// The single contiguous range a packed mesh reserves: vertices then indices.
fn mesh_bytes(size: MeshSize) -> u64 {
    MeshFormat::PACKED_8B.vertex_bytes(size.vertices)
        + MeshFormat::PACKED_8B.index_bytes(size.indices)
}

#[test]
fn the_packed_format_describes_the_production_stream() {
    let format = MeshFormat::PACKED_8B;
    assert_eq!(format.vertex_stride(), 8);
    assert_eq!(format.index_stride(), 2);
    assert_eq!(format.quad_bytes(), 44, "M3-00's measured 44 B/quad");
    // A quad is four 8 B vertices plus six u16 indices.
    assert_eq!(format.vertex_bytes(4), 32);
    assert_eq!(format.index_bytes(6), 12);

    let record = MeshFormat::RECORD_8B;
    assert_eq!(record.index_stride(), 0);
    assert_eq!(record.index_bytes(6), 0);
    assert_eq!(record.quad_bytes(), 32);
}

// The instanced-record path binds a page as one storage buffer; the WebGPU
// default is 128 MiB, and `max_buffer_size` defaults to 256 MiB. These hold at
// compile time, not just on the test machine.
const _: () = assert!(DEFAULT_PAGE_BYTES >= 4 * 1024 * 1024);
const _: () = assert!(DEFAULT_PAGE_BYTES <= MAX_PAGE_BYTES);
const _: () = assert!(MAX_PAGE_BYTES <= 128 * 1024 * 1024);
const _: () = assert!(DEFAULT_PAGE_BYTES.is_multiple_of(4));

#[test]
fn a_p95_chunk_mesh_fits_one_page() {
    // 49,112 quads is the measured 128³ production p95 (M3-00 §1). A draw may
    // not cross a page, so both streams must fit side by side.
    let quads = 49_112u32;
    let mut arena = Arena::new(DEFAULT_PAGE_BYTES);
    let vertices = arena.allocate(MeshFormat::PACKED_8B.vertex_bytes(quads), 4);
    let indices = arena.allocate(MeshFormat::PACKED_8B.index_bytes(quads * 6), 4);
    assert_eq!(vertices.page, 0);
    assert_eq!(indices.page, 0);
    assert!(indices.end() <= DEFAULT_PAGE_BYTES);
    assert_eq!(arena.pages(), 1);
}

#[test]
fn a_record_pool_pages_without_crossing_a_page() {
    let format = MeshFormat::RECORD_8B;
    let page = 4 * 1024 * 1024;
    let mut arena = Arena::new(page);
    // 1.1 pages of records: the second allocation must start a new page.
    // 0.6 pages per allocation: one fits, two do not.
    let records = (page * 3 / 5 / format.vertex_stride() as u64) as u32;
    let first = arena.allocate(format.vertex_bytes(records), 4);
    let second = arena.allocate(format.vertex_bytes(records), 4);
    assert_eq!(first.page, 0);
    assert_eq!(second.page, 1);
    assert!(first.end() <= page);
    assert!(second.end() <= page);
    assert_eq!(arena.pages(), 2);
}

/// Meshes a 4×4 block of chunks once and replays a 64-column flight with those
/// sizes in a seeded shuffle.
///
/// The sizes are the production greedy mesh of the streaming terrain, so the
/// spread (4–15 k quads) is the real one. Reusing 16 sizes with a shuffle keeps
/// the test CPU-bound while still making every rotation a different mix; a real
/// ring is 256 distinct sizes, which is a smaller, not larger, challenge for
/// best fit.
#[test]
fn a_warm_ring_reuses_its_pages_over_a_flight() {
    // Sixteen chunks spread across the M3-01 ring, nearest first. Sampling the
    // ring, not a contiguous 4×4 block, keeps the basin's flattened terrain
    // from dominating the size distribution.
    let ring = ring_chunks(ChunkPos::new(1, 0, 1));
    let sizes: Vec<MeshSize> = ring
        .iter()
        .step_by(ring.len() / 16)
        .map(|position| MeshSize::of(*position))
        .collect();
    let quad_counts: Vec<u32> = sizes.iter().map(|size| size.indices / 6).collect();
    let mean_quads = quad_counts.iter().sum::<u32>() as f64 / quad_counts.len() as f64;
    eprintln!(
        "pool: sample {mean_quads:.0} quads/chunk mean, {}..{} over {} chunks",
        quad_counts.iter().min().expect("non-empty"),
        quad_counts.iter().max().expect("non-empty"),
        quad_counts.len()
    );

    // Deterministic shuffle state.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for page in [4, 8, 16, 32] {
        let mut arena = Arena::new(page * 1024 * 1024);
        let mut live: Vec<Range> = Vec::with_capacity(256);

        // Warm-up: a 256-chunk ring, nearest-first order does not matter for
        // capacity, only the sizes do. Each mesh is one contiguous range, the
        // production layout.
        for index in 0..256 {
            let size = sizes[index % sizes.len()];
            live.push(arena.allocate(mesh_bytes(size), 4));
        }
        let warm_pages = arena.pages();
        let warm = arena.stats();

        // 64 rotations: free the trailing column, load the leading one.
        for _ in 0..64 {
            for _ in 0..16 {
                arena.free(live.remove(0));
            }
            for _ in 0..16 {
                let size = sizes[(next() as usize) % sizes.len()];
                live.push(arena.allocate(mesh_bytes(size), 4));
            }
        }
        let final_stats = arena.stats();
        let overhead = final_stats.capacity_bytes as f64 / final_stats.used_bytes as f64;
        eprintln!(
            "pool: {page} MiB pages: warm {warm_pages} pages ({:.1} MiB), after 64 columns {} pages, {:.1} MiB used, {:.1} MiB slack, {:.2} MiB holes ({:.1}% fragmented), {overhead:.3}x capacity/used",
            warm.capacity_bytes as f64 / (1024.0 * 1024.0),
            final_stats.pages,
            final_stats.used_bytes as f64 / (1024.0 * 1024.0),
            final_stats.slack_bytes as f64 / (1024.0 * 1024.0),
            final_stats.hole_bytes as f64 / (1024.0 * 1024.0),
            final_stats.fragmentation * 100.0,
        );

        assert_eq!(live.len(), 256);
        assert_eq!(
            arena.stats().used_bytes,
            live.iter().map(|range| range.len).sum::<u64>(),
            "every live range is accounted for"
        );
        // The rotation must not grow the arena forever: one extra page is the
        // tolerated cost of a column that is uniformly larger than the freed
        // one. A linear leak would blow through this immediately.
        assert!(
            final_stats.pages <= warm_pages + 2,
            "{page} MiB pages grew from {warm_pages} to {} over 64 columns",
            final_stats.pages
        );
        assert!(
            overhead < 1.5,
            "{page} MiB pages waste {overhead:.2}x capacity over used bytes"
        );
        // Holes are reported, not eliminated: a rotation whose new column is
        // uniformly larger than the freed one leaves the size difference
        // behind. Keep it to a few percent of the ring so a long flight cannot
        // quietly turn the pool into confetti.
        assert!(
            final_stats.hole_bytes * 100 <= final_stats.used_bytes * 5,
            "{page} MiB pages trap {} B in holes against {} B used",
            final_stats.hole_bytes,
            final_stats.used_bytes
        );
    }
}

#[test]
fn a_mesh_larger_than_a_page_is_refused() {
    // The pool is the only place that can refuse: a draw may not cross pages.
    let mut arena = Arena::new(MIN_PAGE_BYTES);
    let oversized = arena.page_bytes() + 4;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        arena.allocate(oversized, 4)
    }));
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// GPU tests
// ---------------------------------------------------------------------------

struct TestGpu {
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<&'static TestGpu> {
    static GPU: OnceLock<Option<TestGpu>> = OnceLock::new();
    GPU.get_or_init(|| {
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

/// The pool's page size must be bindable on this adapter.
#[test]
fn pool_pages_fit_the_adapter_limits() {
    let Some(gpu) = test_gpu() else {
        eprintln!("skipping GPU pool limits: no adapter");
        return;
    };
    let limits = gpu.device.limits();
    assert!(
        DEFAULT_PAGE_BYTES <= limits.max_buffer_size,
        "a {DEFAULT_PAGE_BYTES} B page exceeds max_buffer_size {}",
        limits.max_buffer_size
    );
    assert!(
        DEFAULT_PAGE_BYTES as u128 <= limits.max_storage_buffer_binding_size as u128,
        "a {DEFAULT_PAGE_BYTES} B page exceeds max_storage_buffer_binding_size {}",
        limits.max_storage_buffer_binding_size
    );

    let pool = MeshPool::new(
        gpu.device.clone(),
        MeshFormat::PACKED_8B,
        DEFAULT_PAGE_BYTES,
    );
    assert_eq!(pool.page_bytes(), DEFAULT_PAGE_BYTES);
    assert_eq!(pool.format(), MeshFormat::PACKED_8B);
    assert_eq!(pool.pages(), 0, "the pool creates pages on demand");
}

/// Writing an index count whose byte length is not a multiple of four must pad
/// the tail instead of asking `Queue::write_buffer` for an unaligned copy.
#[test]
fn an_odd_index_count_writes_an_aligned_tail() {
    let Some(gpu) = test_gpu() else {
        eprintln!("skipping GPU pool tail write: no adapter");
        return;
    };
    let device = &gpu.device;
    let queue = &gpu.queue;
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let mut pool = MeshPool::with_usage(
        device.clone(),
        MeshFormat::PACKED_8B,
        4 * 1024 * 1024,
        wgpu::BufferUsages::VERTEX
            | wgpu::BufferUsages::INDEX
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
    );
    let allocation = pool.allocate(4, 9);
    let vertices = [0xAAu8; 32];
    let mut indices = [0u16; 9];
    for (slot, index) in indices.iter_mut().enumerate() {
        *index = slot as u16;
    }
    allocation.write(queue, &vertices, bytemuck::cast_slice(&indices));

    // Read the two ranges back and compare byte for byte.
    let read = |buffer: &wgpu::Buffer, offset: u64, len: u64| -> Vec<u8> {
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pool readback"),
            size: len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pool readback"),
        });
        encoder.copy_buffer_to_buffer(buffer, offset, &readback, 0, len);
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
        let bytes = slice.get_mapped_range().expect("mapped range").to_vec();
        readback.unmap();
        bytes
    };

    let written_vertices = read(&allocation.vertices.buffer, allocation.vertices.offset, 32);
    assert_eq!(written_vertices, vertices);
    let indices_slice = allocation.indices.as_ref().expect("indexed format");
    assert_eq!(indices_slice.len, 18);
    // Read the range plus its 2 B pad: the queue requires a 4 B write.
    let written_indices = read(&indices_slice.buffer, indices_slice.offset, 20);
    assert_eq!(
        &written_indices[..18],
        bytemuck::cast_slice::<u16, u8>(&indices)
    );
    assert_eq!(
        &written_indices[18..],
        &[0, 0],
        "the padding bytes are zero"
    );

    let error = pollster::block_on(error_scope.pop());
    assert!(
        error.is_none(),
        "pool write raised a validation error: {error:?}"
    );
}

/// Renders one chunk from the pool and from a per-chunk buffer and requires the
/// same image. This pins the range offsets, slice sizes and stride.
#[test]
fn pooled_draw_matches_a_per_chunk_draw() {
    let Some(gpu) = test_gpu() else {
        eprintln!("skipping GPU pool draw: no adapter");
        return;
    };
    let device = &gpu.device;
    let queue = &gpu.queue;
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let camera = Camera::looking_at(Vec3::new(0.6, 0.5, 0.6), Vec3::splat(0.093_75), 1.0);
    let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pool camera uniform"),
        size: std::mem::size_of::<CameraUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&camera_buffer, 0, bytemuck::bytes_of(&camera.uniform()));
    let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("pool camera"),
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
        label: Some("pool camera"),
        layout: &camera_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buffer.as_entire_binding(),
        }],
    });

    let mut builder = AtlasBuilder::new();
    builder
        .set_tile(1, &[255u8; TILE_BYTES])
        .expect("slot 1 is valid");
    let atlas_texture = AtlasTexture::new(device, queue, &builder.build());

    let mut volume = vec![0u16; CHUNK_VOLUME];
    volume[1 | (1 << CHUNK_BITS) | (1 << (2 * CHUNK_BITS))] = 1;
    let content = chunk_content::build(&volume, ChunkPos::new(0, 0, 0));
    let chunk = ChunkMesh::from_parts(device, &content.mesh.vertices, &content.mesh.indices)
        .expect("one voxel is one mesh");

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
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );

    let mut pool = MeshPool::new(device.clone(), MeshFormat::PACKED_8B, 4 * 1024 * 1024);
    let pooled = pool.allocate(
        (content.mesh.vertices.len() / MeshFormat::PACKED_8B.vertex_stride() as usize) as u32,
        content.mesh.indices.len() as u32,
    );
    pooled.write(
        queue,
        &content.mesh.vertices,
        bytemuck::cast_slice(&content.mesh.indices),
    );

    let per_chunk = |device: &wgpu::Device, queue: &wgpu::Queue| -> Vec<u8> {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pool per-chunk target"),
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
            label: Some("pool per-chunk pass"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pool per-chunk pass"),
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
            pipeline.draw(
                &mut pass,
                &chunk,
                &camera_bind_group,
                &atlas_texture.bind_group,
                &lighting.bind_group,
            );
        }
        queue.submit([encoder.finish()]);
        read_pixels(device, queue, &target, 64, 64)
    };

    let pooled_pixels = |device: &wgpu::Device, queue: &wgpu::Queue| -> Vec<u8> {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pool pooled target"),
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
            label: Some("pool pooled pass"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pool pooled pass"),
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
            pipeline.draw_pooled(
                &mut pass,
                &pooled,
                &camera_bind_group,
                &atlas_texture.bind_group,
                &lighting.bind_group,
            );
        }
        queue.submit([encoder.finish()]);
        read_pixels(device, queue, &target, 64, 64)
    };

    let expected = per_chunk(device, queue);
    let actual = pooled_pixels(device, queue);
    assert!(
        expected
            .chunks(4)
            .any(|pixel| pixel[0] > 0 || pixel[1] > 0 || pixel[2] > 0),
        "the per-chunk draw wrote no pixels"
    );
    let max_diff = expected
        .iter()
        .zip(&actual)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert_eq!(max_diff, 0, "pooled draw differs from the per-chunk draw");

    let error = pollster::block_on(error_scope.pop());
    assert!(
        error.is_none(),
        "pooled draw raised a validation error: {error:?}"
    );
}

fn read_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pool readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pool readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
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
    let pixels = slice.get_mapped_range().expect("mapped range").to_vec();
    readback.unmap();
    pixels
}
