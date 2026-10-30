//! Generate the M1.5-15 visual acceptance set.
//!
//! Run from the repository root:
//!
//! ```sh
//! cargo run -p voxel-viewer --example m1_5_screenshots
//! ```
//!
//! The tool writes into `docs/images/` by default; pass `--out DIR` to change
//! that. It uses the committed atlas and the same 16-byte vertex layout as the
//! viewer, with a local screenshot shader that preserves cutout and translucent
//! alpha so leaves, water and ice can be judged. It does not change the runtime
//! shader.
//!
//! It also writes the M1.5-17 texel-density pair: the same chunk rendered from
//! the 256² M1.5-14 sources (the historical per-face density) and from the
//! committed 16×16-tile atlas. Both sides sample level 0 with nearest filtering,
//! so the pair isolates density from the runtime mip path. Both filenames carry
//! the first eight hex characters of `atlas_png_blake3`, as generator.spec §11
//! asks.

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use glam::Vec3;
use serde::Deserialize;
use voxel_core::Face;
use voxel_core::constants::{CHUNK_BITS, CHUNK_VOLUME};
use voxel_viewer::atlas::Atlas;
use voxel_viewer::atlas_texture::AtlasTexture;
use voxel_viewer::camera::{Camera, CameraUniform};
use voxel_viewer::chunk_mesh::ChunkMesh;
use voxel_viewer::depth::{DEPTH_FORMAT, DepthTarget};
use voxel_viewer::naive_mesh::{Mesh, Quad, Vertex, mesh_face, naive_mesh};

const SHADER_TEMPLATE: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    position: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var atlas: texture_2d<f32>;

@group(1) @binding(1)
var atlas_sampler: sampler;

// Width of one atlas tile in texels: 16 for the runtime atlas, 256 for the
// legacy per-face sources.
const TILE: f32 = {{TILE}};
// Tiles per atlas edge: 16 for the 256² atlas, 8 for the 2048² source sheet.
const EDGE: u32 = {{EDGE}};
// Atlas edge in texels.
const ATLAS: f32 = {{ATLAS}};

struct VertexInput {
    @location(0) local_material: vec4<u32>,
    @location(1) face_ao0: vec4<u32>,
    @location(2) ao3_reserved: vec4<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local: vec3<f32>,
    @location(1) @interpolate(flat) material: u32,
    @location(2) @interpolate(flat) face: u32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let local = vec3<f32>(
        f32(input.local_material.x),
        f32(input.local_material.y),
        f32(input.local_material.z),
    );
    var output: VertexOutput;
    output.clip_position = camera.view_proj * vec4<f32>(local * 0.0625, 1.0);
    output.local = local;
    output.material = input.local_material.w;
    output.face = input.face_ao0.x;
    return output;
}

const FACE_SHADE: array<f32, 6> = array<f32, 6>(0.62, 0.62, 1.0, 0.5, 0.82, 0.72);

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let block = floor(input.local / 16.0) * 16.0;
    let rel = input.local - block;
    // A 1 m face is 16 voxels wide; the full tile spans that whole face.
    let scale = TILE / 16.0;

    var face_uv = vec2<f32>(rel.x, rel.y) * scale;
    if input.face == 0u {
        face_uv = vec2<f32>(16.0 - rel.z, 16.0 - rel.y) * scale;
    } else if input.face == 1u {
        face_uv = vec2<f32>(rel.z, 16.0 - rel.y) * scale;
    } else if input.face == 2u {
        face_uv = vec2<f32>(rel.x, 16.0 - rel.z) * scale;
    } else if input.face == 3u {
        face_uv = vec2<f32>(rel.x, rel.z) * scale;
    } else if input.face == 4u {
        face_uv = vec2<f32>(rel.x, 16.0 - rel.y) * scale;
    } else {
        face_uv = vec2<f32>(16.0 - rel.x, 16.0 - rel.y) * scale;
    }

    let texel = clamp(face_uv, vec2<f32>(0.5), vec2<f32>(TILE - 0.5));
    let tile_origin =
        vec2<f32>(f32(input.material % EDGE), f32(input.material / EDGE)) * TILE;
    let uv = (tile_origin + texel) / ATLAS;
    let color = textureSample(atlas, atlas_sampler, uv);
    if color.a < 0.5 {
        discard;
    }
    return vec4<f32>(color.rgb * FACE_SHADE[input.face], color.a);
}
"#;

/// One atlas layout: tile width, tiles per edge and atlas edge, in texels.
#[derive(Clone, Copy)]
struct AtlasVariant {
    tile_px: u32,
    tiles_per_edge: u32,
    atlas_px: u32,
}

impl AtlasVariant {
    /// The committed runtime atlas: 16×16 tiles in a 256² image.
    const RUNTIME: Self = Self {
        tile_px: 16,
        tiles_per_edge: 16,
        atlas_px: 256,
    };
    /// The historical per-face density: 256² sources in an 8×8 sheet (2048²).
    const LEGACY: Self = Self {
        tile_px: 256,
        tiles_per_edge: 8,
        atlas_px: 2048,
    };

    fn shader(self) -> String {
        SHADER_TEMPLATE
            .replace("{{TILE}}", &format!("{}.0", self.tile_px))
            .replace("{{EDGE}}", &format!("{}u", self.tiles_per_edge))
            .replace("{{ATLAS}}", &format!("{}.0", self.atlas_px))
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] = [
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

const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 16,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &VERTEX_ATTRIBUTES,
};

#[derive(Deserialize)]
struct Manifest {
    textures: Vec<TextureEntry>,
}

#[derive(Deserialize)]
struct TextureEntry {
    slot: u16,
    source: String,
}

#[derive(Deserialize)]
struct AtlasHash {
    atlas_png_blake3: String,
}

struct Gpu {
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    atlas: AtlasTexture,
    camera_layout: wgpu::BindGroupLayout,
}

/// The historical per-face sheet: every 256² M1.5-14 source in an 8×8 grid,
/// indexed by material slot, with no mip chain. This is the "before" side of
/// M1.5-17.
struct LegacyAtlas {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl LegacyAtlas {
    fn new(gpu: &Gpu, root: &Path) -> Result<Self, Box<dyn Error>> {
        let manifest: Manifest = ron::from_str(&std::fs::read_to_string(
            root.join("crates/engine/block/data/textures.ron"),
        )?)?;
        let variant = AtlasVariant::LEGACY;
        let size = variant.atlas_px;
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        for texture in &manifest.textures {
            let source = root.join("crates/engine/block").join(&texture.source);
            let (width, height, rgba) = load_rgba(&source)?;
            assert_eq!(
                (width, height),
                (variant.tile_px, variant.tile_px),
                "{} is not {}²",
                source.display(),
                variant.tile_px
            );
            let slot = u32::from(texture.slot);
            assert!(
                slot < variant.tiles_per_edge * variant.tiles_per_edge,
                "material slot {slot} does not fit the legacy {}×{} sheet",
                variant.tiles_per_edge,
                variant.tiles_per_edge
            );
            let x0 = (slot % variant.tiles_per_edge) * variant.tile_px;
            let y0 = (slot / variant.tiles_per_edge) * variant.tile_px;
            let row_bytes = (variant.tile_px * 4) as usize;
            for row in 0..variant.tile_px {
                let source_start = (row * variant.tile_px * 4) as usize;
                let target_start = (((y0 + row) * size + x0) * 4) as usize;
                pixels[target_start..target_start + row_bytes]
                    .copy_from_slice(&rgba[source_start..source_start + row_bytes]);
            }
        }

        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("legacy 256px face sheet"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("legacy 256px sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("legacy 256px atlas"),
            layout: &gpu.atlas.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        Ok(Self {
            _texture: texture,
            bind_group,
        })
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut out = root.join("docs/images");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--out" {
            out = PathBuf::from(args.next().ok_or("--out needs a directory")?);
        }
    }
    std::fs::create_dir_all(&out)?;

    write_face_sheet(&root, &out.join("m1.5-15-face-sheet.png"))?;

    let gpu = Gpu::new(&root)?;
    let legacy = LegacyAtlas::new(&gpu, &root)?;
    let atlas_hash: AtlasHash = ron::from_str(&std::fs::read_to_string(
        root.join("crates/engine/viewer/assets/atlas/atlas.hash.ron"),
    )?)?;
    let hash = &atlas_hash.atlas_png_blake3[..8];
    let block_size = (1024, 768);
    write_grass_sheet(
        &gpu,
        &out.join("m1.5-15-grass-block.png"),
        block_quads([0, 0, 0], 16, face_materials(3, 2, 4)),
    )?;
    render_block_scene(
        &gpu,
        &out.join("m1.5-15-sandstone-block.png"),
        block_size,
        block_quads([0, 0, 0], 16, face_materials(21, 22, 23)),
        Camera::looking_at(
            Vec3::new(1.7, 1.35, 2.0),
            Vec3::new(0.5, 0.5, 0.5),
            block_size.0 as f32 / block_size.1 as f32,
        ),
    )?;
    render_block_scene(
        &gpu,
        &out.join("m1.5-15-log-and-leaves.png"),
        block_size,
        {
            let mut quads = block_quads([0, 0, 0], 16, face_materials(9, 9, 10));
            quads.extend(block_quads([16, 0, 0], 16, face_materials(11, 11, 11)));
            quads
        },
        Camera::looking_at(
            Vec3::new(2.8, 1.8, 3.4),
            Vec3::new(1.0, 0.5, 0.5),
            block_size.0 as f32 / block_size.1 as f32,
        ),
    )?;
    render_block_scene(
        &gpu,
        &out.join("m1.5-15-ores.png"),
        (1152, 640),
        {
            let mut quads = Vec::new();
            for (index, material) in [1, 13, 14, 15, 16].into_iter().enumerate() {
                quads.extend(block_quads(
                    [(index * 16) as u16, 0, 0],
                    16,
                    face_materials(material, material, material),
                ));
            }
            quads
        },
        Camera::looking_at(
            Vec3::new(2.5, 1.8, 4.2),
            Vec3::new(2.5, 0.45, 0.5),
            1152.0 / 640.0,
        ),
    )?;
    render_block_scene(
        &gpu,
        &out.join("m1.5-15-translucent.png"),
        block_size,
        {
            let mut quads = block_quads([0, 0, 0], 16, face_materials(8, 8, 8));
            quads.extend(block_quads([16, 0, 0], 16, face_materials(20, 20, 20)));
            quads
        },
        Camera::looking_at(
            Vec3::new(2.5, 1.5, 3.0),
            Vec3::new(1.0, 0.5, 0.5),
            block_size.0 as f32 / block_size.1 as f32,
        ),
    )?;

    write_chunk_sheet(&gpu, &out.join("m1.5-15-chunk-1080p.png"))?;
    write_texel_density_pair(&gpu, &legacy, &out, hash)?;
    println!("wrote M1.5-15 and M1.5-17 screenshots to {}", out.display());
    Ok(())
}

/// Writes the M1.5-17 pair from one camera: the 256² sources at full density,
/// then the committed 16×16-tile atlas. Both sides sample level 0 with nearest
/// filtering, so the pair isolates texel density from the runtime mip path.
fn write_texel_density_pair(
    gpu: &Gpu,
    legacy: &LegacyAtlas,
    out: &Path,
    hash: &str,
) -> Result<(), Box<dyn Error>> {
    let (width, height) = (1920u32, 1080u32);
    let mesh = chunk_mesh();
    let camera = Camera::looking_at(
        Vec3::new(3.85, 1.2, 3.75),
        Vec3::new(2.55, 0.8, 2.55),
        width as f32 / height as f32,
    );
    let before = out.join(format!("m1.5-17-before-256px-{hash}.png"));
    let after = out.join(format!("m1.5-17-after-16px-{hash}.png"));
    let pixels = gpu.render_with(
        &mesh,
        &camera,
        width,
        height,
        AtlasVariant::LEGACY,
        &legacy.bind_group,
    )?;
    write_png(&before, width, height, &pixels)?;
    let pixels = gpu.render_with(
        &mesh,
        &camera,
        width,
        height,
        AtlasVariant::RUNTIME,
        &gpu.lod0_atlas(),
    )?;
    write_png(&after, width, height, &pixels)?;
    Ok(())
}

fn face_materials(top: u16, bottom: u16, side: u16) -> [u16; 6] {
    let mut materials = [side; 6];
    materials[2] = top;
    materials[3] = bottom;
    materials
}

fn block_quads(origin: [u16; 3], size: u16, materials: [u16; 6]) -> Vec<Quad> {
    let mut quads = Vec::with_capacity(6);
    for face in Face::ALL {
        let corners = block_corners(face, origin, size);
        let material = materials[mesh_face(face) as usize];
        let vertices = corners.map(|local| Vertex {
            local,
            material,
            face: mesh_face(face),
            ao: [u8::MAX; 4],
            reserved: [0; 3],
        });
        quads.push(Quad {
            vertices,
            face,
            material,
        });
    }
    quads
}

fn block_corners(face: Face, [x, y, z]: [u16; 3], size: u16) -> [[u16; 3]; 4] {
    let max = |value: u16| value + size;
    match face {
        Face::East => [
            [max(x), y, z],
            [max(x), max(y), z],
            [max(x), max(y), max(z)],
            [max(x), y, max(z)],
        ],
        Face::West => [
            [x, y, z],
            [x, y, max(z)],
            [x, max(y), max(z)],
            [x, max(y), z],
        ],
        Face::Top => [
            [x, max(y), z],
            [x, max(y), max(z)],
            [max(x), max(y), max(z)],
            [max(x), max(y), z],
        ],
        Face::Bottom => [
            [x, y, z],
            [max(x), y, z],
            [max(x), y, max(z)],
            [x, y, max(z)],
        ],
        Face::South => [
            [x, y, max(z)],
            [max(x), y, max(z)],
            [max(x), max(y), max(z)],
            [x, max(y), max(z)],
        ],
        Face::North => [
            [x, y, z],
            [x, max(y), z],
            [max(x), max(y), z],
            [max(x), y, z],
        ],
    }
}

impl Gpu {
    fn new(root: &Path) -> Result<Self, Box<dyn Error>> {
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
        })?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let atlas =
            Atlas::load_verified(&root.join("crates/engine/viewer/assets/atlas/atlas.png"))?;
        let atlas = AtlasTexture::new(&device, &queue, &atlas);
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screenshot camera"),
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
        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            atlas,
            camera_layout,
        })
    }
}

fn render_block_scene(
    gpu: &Gpu,
    path: &Path,
    (width, height): (u32, u32),
    quads: Vec<Quad>,
    camera: Camera,
) -> Result<(), Box<dyn Error>> {
    let mesh = Mesh::from_quads(&quads);
    let pixels = gpu.render(&mesh, &camera, width, height)?;
    write_png(path, width, height, &pixels)?;
    Ok(())
}

fn write_grass_sheet(gpu: &Gpu, path: &Path, quads: Vec<Quad>) -> Result<(), Box<dyn Error>> {
    let (width, height) = (768u32, 768u32);
    let mesh = Mesh::from_quads(&quads);
    let top = Camera::looking_at(Vec3::new(1.7, 1.35, 2.0), Vec3::new(0.5, 0.5, 0.5), 1.0);
    let bottom = Camera::looking_at(Vec3::new(1.7, -0.35, 2.0), Vec3::new(0.5, 0.5, 0.5), 1.0);
    let top_pixels = gpu.render(&mesh, &top, width, height)?;
    let bottom_pixels = gpu.render(&mesh, &bottom, width, height)?;
    let total_width = width * 2;
    let stride = (total_width * 4) as usize;
    let mut sheet = vec![0u8; (total_width * height * 4) as usize];
    for row in 0..height as usize {
        let source = row * width as usize * 4;
        let target = row * stride;
        let panel = &top_pixels[source..source + width as usize * 4];
        sheet[target..target + panel.len()].copy_from_slice(panel);
        let panel = &bottom_pixels[source..source + width as usize * 4];
        let target = target + width as usize * 4;
        sheet[target..target + panel.len()].copy_from_slice(panel);
    }
    for row in 0..height as usize {
        let target = row * stride + width as usize * 4 - 2;
        sheet[target..target + 4].copy_from_slice(&[24, 26, 30, 255]);
    }
    write_png(path, total_width, height, &sheet)?;
    Ok(())
}

fn write_chunk_sheet(gpu: &Gpu, path: &Path) -> Result<(), Box<dyn Error>> {
    let (width, height) = (640u32, 1080u32);
    let mesh = chunk_mesh();
    let target = Vec3::new(2.0, 0.9, 2.0);
    let views = [
        Vec3::new(7.0, 5.0, 10.0),
        Vec3::new(12.0, 8.0, 17.0),
        Vec3::new(20.0, 13.0, 28.0),
    ];
    let mut sheet = vec![0u8; (width * 3 * height * 4) as usize];
    let stride = (width * 3 * 4) as usize;
    for (index, position) in views.into_iter().enumerate() {
        let camera = Camera::looking_at(position, target, width as f32 / height as f32);
        let pixels = gpu.render(&mesh, &camera, width, height)?;
        for row in 0..height as usize {
            let source = &pixels[row * width as usize * 4..(row + 1) * width as usize * 4];
            let start = row * stride + index * width as usize * 4;
            sheet[start..start + source.len()].copy_from_slice(source);
        }
        // A thin divider keeps the three exposures distinct without adding text.
        if index < 2 {
            let x = (index + 1) * width as usize * 4 - 2;
            for row in 0..height as usize {
                let start = row * stride + x;
                sheet[start..start + 8].copy_from_slice(&[24, 26, 30, 255].repeat(2));
            }
        }
    }
    write_png(path, width * 3, height, &sheet)?;
    Ok(())
}

impl Gpu {
    /// Binds the committed atlas with a nearest, level-0-only sampler. M1.5-17
    /// uses this on the after image so both sides differ only in texel density.
    fn lod0_atlas(&self) -> wgpu::BindGroup {
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lod0 atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lod0 atlas"),
            layout: &self.atlas.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        })
    }

    fn render(
        &self,
        mesh: &Mesh,
        camera: &Camera,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        self.render_with(
            mesh,
            camera,
            width,
            height,
            AtlasVariant::RUNTIME,
            &self.atlas.bind_group,
        )
    }

    fn render_with(
        &self,
        mesh: &Mesh,
        camera: &Camera,
        width: u32,
        height: u32,
        variant: AtlasVariant,
        atlas: &wgpu::BindGroup,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let chunk = ChunkMesh::new(&self.device, &self.queue, mesh).ok_or("empty mesh")?;
        let camera_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&camera_buffer, 0, bytemuck::bytes_of(&camera.uniform()));
        let camera_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("screenshot camera"),
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let pipeline = self.pipeline(variant);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot target"),
            size: wgpu::Extent3d {
                width,
                height,
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
        let depth = DepthTarget::new(&self.device, width, height);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("screenshot pass"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("screenshot pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.008,
                            g: 0.010,
                            b: 0.012,
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
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &camera_bind_group, &[]);
            pass.set_bind_group(1, atlas, &[]);
            pass.set_vertex_buffer(0, chunk.vertices.slice(..));
            pass.set_index_buffer(chunk.indices.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..chunk.index_count, 0, 0..1);
        }
        let bytes_per_row = width * 4;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
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
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("device poll");
        let mapped = slice.get_mapped_range()?;
        let pixels = mapped.to_vec();
        drop(mapped);
        readback.unmap();
        Ok(pixels)
    }

    fn pipeline(&self, variant: AtlasVariant) -> wgpu::RenderPipeline {
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("screenshot"),
                bind_group_layouts: &[Some(&self.camera_layout), Some(&self.atlas.layout)],
                immediate_size: 0,
            });
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("screenshot"),
                source: wgpu::ShaderSource::Wgsl(variant.shader().into()),
            });
        self.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("screenshot"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(VERTEX_LAYOUT)],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
    }
}

fn chunk_mesh() -> Mesh {
    let mut volume = vec![0u16; CHUNK_VOLUME];
    for z in 8..56 {
        for x in 8..56 {
            let fx = (x as f32 - 32.0) / 16.0;
            let fz = (z as f32 - 32.0) / 16.0;
            let mound = (1.0 - (fx * fx + fz * fz).sqrt()).max(0.0);
            let ripple = (x as f32 * 0.35).sin() * 0.5 + (z as f32 * 0.4).cos() * 0.5;
            let height = (14.0 + mound * 12.0 + ripple) as usize;
            for y in 0..height {
                let material = if y + 1 == height {
                    3
                } else if y + 4 >= height {
                    2
                } else {
                    1
                };
                let index = x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS));
                volume[index] = material;
            }
        }
    }
    naive_mesh(&volume)
}

fn write_face_sheet(root: &Path, path: &Path) -> Result<(), Box<dyn Error>> {
    let manifest: Manifest = ron::from_str(&std::fs::read_to_string(
        root.join("crates/engine/block/data/textures.ron"),
    )?)?;
    let mut textures = manifest.textures;
    textures.sort_by_key(|texture| texture.slot);

    let cell = 256u32;
    let gap = 8u32;
    let margin = 8u32;
    let columns = 8u32;
    let rows = (textures.len() as u32).div_ceil(columns);
    let width = margin * 2 + columns * cell + (columns - 1) * gap;
    let height = margin * 2 + rows * cell + (rows - 1) * gap;
    let mut sheet = vec![0u8; (width * height * 4) as usize];
    for pixel in sheet.as_chunks_mut::<4>().0 {
        *pixel = [24, 26, 30, 255];
    }

    for (index, texture) in textures.iter().enumerate() {
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        let source = root.join("crates/engine/block").join(&texture.source);
        let (source_width, source_height, rgba) = load_rgba(&source)?;
        assert_eq!(
            (source_width, source_height),
            (cell, cell),
            "{} is not {cell}²",
            source.display()
        );
        let x0 = margin + column * (cell + gap);
        let y0 = margin + row * (cell + gap);
        for y in 0..cell {
            for x in 0..cell {
                let source_index = ((y * cell + x) * 4) as usize;
                let target_index = (((y0 + y) * width + x0 + x) * 4) as usize;
                let alpha = rgba[source_index + 3];
                for channel in 0..3 {
                    let foreground = u16::from(rgba[source_index + channel]);
                    let background = u16::from(sheet[target_index + channel]);
                    sheet[target_index + channel] = ((foreground * u16::from(alpha)
                        + background * (255 - u16::from(alpha)))
                        / 255) as u8;
                }
            }
        }
    }
    write_png(path, width, height, &sheet)?;
    Ok(())
}

fn load_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>), Box<dyn Error>> {
    let decoder = png::Decoder::new(File::open(path)?);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer)?;
    assert_eq!(info.color_type, png::ColorType::Rgba, "{}", path.display());
    assert_eq!(info.bit_depth, png::BitDepth::Eight, "{}", path.display());
    buffer.truncate(info.buffer_size());
    Ok((info.width, info.height, buffer))
}

fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), Box<dyn Error>> {
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    Ok(())
}
