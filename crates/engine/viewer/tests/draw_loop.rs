//! M3-05 draw-loop tests.
//!
//! The loop draws every accepted chunk once, with one draw per chunk that has a
//! mesh. The pure test pins the visit counts and the empty-chunk rule; the GPU
//! test renders the same 5×5 chunk patch twice — once with the M3-04 frustum
//! and once with every chunk accepted — and requires the images to match byte
//! for byte, which is the "correct image" half of the acceptance.
//!
//! The GPU test also records every visited position, so a double draw would
//! fail the uniqueness check instead of only showing up as overdraw.

use std::collections::HashSet;
use std::sync::OnceLock;

use glam::Vec3;
use voxel_storage::ChunkPos;
use voxel_viewer::atlas::{AtlasBuilder, TILE_BYTES, TILE_SIZE};
use voxel_viewer::atlas_texture::AtlasTexture;
use voxel_viewer::camera::{Camera, CameraUniform};
use voxel_viewer::chunk_content;
use voxel_viewer::chunk_lighting;
use voxel_viewer::chunk_render::ChunkPipeline;
use voxel_viewer::cull::Frustum;
use voxel_viewer::depth::DepthTarget;
use voxel_viewer::mesh_pool::{MeshFormat, MeshPool};
use voxel_viewer::streaming::{
    ChunkResources, DrawCost, DrawStats, chunk_origin, draw_visible, stream_apron, stream_volume,
};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 512;

/// A fake chunk with the geometry counters the loop sums.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fake {
    id: u32,
    blocks: u64,
    quads: u64,
    bytes: u64,
}

impl DrawCost for Fake {
    fn blocks(&self) -> u64 {
        self.blocks
    }

    fn quads(&self) -> u64 {
        self.quads
    }

    fn mesh_bytes(&self) -> u64 {
        self.bytes
    }
}

fn fake(id: u32) -> Fake {
    Fake {
        id,
        blocks: 10,
        quads: 100,
        bytes: 4_400,
    }
}

#[test]
fn the_loop_visits_each_accepted_chunk_once() {
    let chunks: Vec<(ChunkPos, Fake)> = (0..6)
        .map(|index| (ChunkPos::new(index - 3, 0, index % 2), fake(index as u32)))
        .collect();
    let mut visited = Vec::new();
    let stats = draw_visible(
        chunks.iter().map(|(position, value)| (*position, value)),
        |position| position.x >= 0,
        |position, value| {
            visited.push((position, *value));
            true
        },
    );

    let expected: Vec<(ChunkPos, Fake)> = chunks
        .iter()
        .copied()
        .filter(|(position, _)| position.x >= 0)
        .collect();
    assert_eq!(stats.visible, expected.len());
    assert_eq!(stats.drawn, expected.len());
    assert_eq!(visited, expected, "visits match the accepted set, in order");

    let unique: HashSet<ChunkPos> = visited.iter().map(|(position, _)| *position).collect();
    assert_eq!(unique.len(), visited.len(), "a chunk was drawn twice");

    // The geometry counters cover exactly the accepted set (M3-06).
    assert_eq!(stats.blocks, 10 * expected.len() as u64);
    assert_eq!(stats.quads, 100 * expected.len() as u64);
    assert_eq!(stats.bytes, 4_400 * expected.len() as u64);
}

/// A chunk with no geometry: it counts as visible but contributes nothing.
#[derive(Clone, Copy, Debug)]
struct Empty;

impl DrawCost for Empty {}

#[test]
fn an_empty_chunk_counts_as_visible_but_not_drawn() {
    let chunks = [ChunkPos::new(0, 0, 0), ChunkPos::new(1, 0, 0)];
    let mut visited = 0;
    let stats = draw_visible(
        chunks.iter().map(|position| (*position, &Empty)),
        |_| true,
        |position, _| {
            visited += 1;
            // The chunk at the origin has no mesh this frame.
            position != ChunkPos::new(0, 0, 0)
        },
    );
    assert_eq!(stats.visible, 2);
    assert_eq!(stats.drawn, 1);
    assert_eq!(stats.blocks, 0, "an empty chunk has no geometry");
    assert_eq!(visited, 2, "an empty chunk is still visited once");
}

#[test]
fn a_rejected_set_draws_nothing() {
    let chunks = [ChunkPos::new(0, 0, 0)];
    let stats = draw_visible(
        chunks.iter().map(|position| (*position, &Empty)),
        |_| false,
        |_, _| panic!("a rejected chunk must not be visited"),
    );
    assert_eq!(stats, DrawStats::default(), "nothing is counted or drawn");
}

// ---------------------------------------------------------------------------
// GPU image test
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

#[test]
fn a_culled_frame_matches_a_draw_everything_frame() {
    let Some(gpu) = test_gpu() else {
        eprintln!("skipping GPU draw loop: no adapter");
        return;
    };
    let device = &gpu.device;
    let queue = &gpu.queue;
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    // A pose inside the patch looking along -Z: the +Z half is behind it.
    let camera = Camera::looking_at(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 1.0, -20.0), 1.0);
    let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("draw loop camera"),
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
    let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("draw loop camera uniform"),
        size: std::mem::size_of::<CameraUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&camera_buffer, 0, bytemuck::bytes_of(&camera.uniform()));
    let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("draw loop camera"),
        layout: &camera_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buffer.as_entire_binding(),
        }],
    });

    let mut builder = AtlasBuilder::new();
    for slot in 1..=40u16 {
        let mut tile = [0u8; TILE_BYTES];
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let offset = ((y * TILE_SIZE + x) * 4) as usize;
                let hue = (u32::from(slot) * 37 + x * 5 + y * 11) % 255;
                tile[offset..offset + 4].copy_from_slice(&[hue as u8, 90, 255 - hue as u8, 255]);
            }
        }
        builder.set_tile(slot, &tile).expect("slot is valid");
    }
    let atlas_texture = AtlasTexture::new(device, queue, &builder.build());

    let data_layout = chunk_lighting::layout(device);
    let pipeline = ChunkPipeline::new(
        device,
        &camera_layout,
        &atlas_texture.layout,
        &data_layout,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );

    let mut pool = MeshPool::new(device.clone(), MeshFormat::PACKED_8B, 8 * 1024 * 1024);
    let mut chunks: Vec<(ChunkPos, ChunkResources)> = Vec::new();
    for z in -2..=2 {
        for x in -2..=2 {
            let position = ChunkPos::new(x, 0, z);
            let volume = stream_volume(position);
            let content =
                chunk_content::build_with_apron(&volume, position, &stream_apron(position));
            let vertex_count = (content.mesh.vertices.len()
                / MeshFormat::PACKED_8B.vertex_stride() as usize)
                as u32;
            let mesh = pool.allocate(vertex_count, content.mesh.indices.len() as u32);
            mesh.write(
                queue,
                &content.mesh.vertices,
                bytemuck::cast_slice(&content.mesh.indices),
            );
            let lighting = chunk_lighting::upload(
                device,
                &data_layout,
                &content.light,
                &content.probes,
                chunk_origin(position),
            );
            chunks.push((
                position,
                ChunkResources {
                    mesh: Some(mesh),
                    lighting,
                    blocks: content.mesh.blocks as u32,
                },
            ));
        }
    }

    let frustum = Frustum::from_view_projection(camera.view_projection());
    let render = |accept: &dyn Fn(ChunkPos) -> bool, visited: &mut Vec<ChunkPos>| {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("draw loop target"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
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
        let depth = DepthTarget::new(device, WIDTH, HEIGHT);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("draw loop pass"),
        });
        let stats = {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("draw loop pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.2,
                            b: 0.5,
                            a: 1.0,
                        }),
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
            draw_visible(
                chunks
                    .iter()
                    .map(|(position, resources)| (*position, resources)),
                accept,
                |position, resources| {
                    visited.push(position);
                    let Some(mesh) = &resources.mesh else {
                        return false;
                    };
                    pipeline.draw_pooled(
                        &mut pass,
                        mesh,
                        &camera_bind_group,
                        &atlas_texture.bind_group,
                        &resources.lighting.bind_group,
                    );
                    true
                },
            )
        };
        queue.submit([encoder.finish()]);
        (stats, read_pixels(device, queue, &target))
    };

    let mut all_visited = Vec::new();
    let (all_stats, all_pixels) = render(&|_| true, &mut all_visited);
    let mut culled_visited = Vec::new();
    let (culled_stats, culled_pixels) = render(
        &|position| frustum.chunk_visible(position),
        &mut culled_visited,
    );

    assert_eq!(all_stats.visible, chunks.len(), "all chunks are resident");
    assert_eq!(all_stats.drawn, chunks.len(), "every chunk has a mesh");
    assert!(
        culled_stats.visible > 0 && culled_stats.visible < chunks.len(),
        "the pose must cull some chunks: {} of {}",
        culled_stats.visible,
        chunks.len()
    );
    assert_eq!(culled_stats.drawn, culled_stats.visible);

    // The geometry totals must match the scene: sum over the chunks the cull
    // accepted, not over the whole ring.
    let mut expected_blocks = 0u64;
    let mut expected_quads = 0u64;
    let mut expected_bytes = 0u64;
    for (position, resources) in &chunks {
        if !frustum.chunk_visible(*position) {
            continue;
        }
        expected_blocks += resources.blocks();
        expected_quads += resources.quads();
        expected_bytes += resources.mesh_bytes();
    }
    assert!(expected_blocks > 0 && expected_quads > 0 && expected_bytes > 0);
    assert_eq!(culled_stats.blocks, expected_blocks, "visible blocks");
    assert_eq!(culled_stats.quads, expected_quads, "visible quads");
    assert_eq!(culled_stats.bytes, expected_bytes, "visible mesh bytes");
    assert!(
        all_stats.quads > culled_stats.quads,
        "culling must reduce the drawn geometry"
    );

    for (label, visited) in [("all", &all_visited), ("culled", &culled_visited)] {
        let unique: HashSet<ChunkPos> = visited.iter().copied().collect();
        assert_eq!(
            unique.len(),
            visited.len(),
            "{label} pass drew a chunk twice: {visited:?}"
        );
    }
    assert_eq!(
        all_pixels, culled_pixels,
        "the culled frame differs from the draw-everything frame"
    );

    let error = pollster::block_on(error_scope.pop());
    assert!(
        error.is_none(),
        "the draw loop raised a validation error: {error:?}"
    );
    eprintln!(
        "draw_loop: {} chunks, culled pose kept {} ({:.0}%)",
        chunks.len(),
        culled_stats.visible,
        culled_stats.visible as f64 / chunks.len() as f64 * 100.0
    );
}

fn read_pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("draw loop readback"),
        size: u64::from(WIDTH) * u64::from(HEIGHT) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("draw loop readback"),
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
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
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
