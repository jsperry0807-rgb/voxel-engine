// Chunk shader, M3-01: packed 8 B vertex, AO and block light.
//
// The vertex carries chunk-local voxel coordinates (6.25 cm), material, face
// code and four 2-bit AO levels. Each chunk binds a world-voxel origin, so
// world = (local + chunk_origin.xyz) * 0.0625.
//
// Tile texels are fetched per fragment from the face-local position inside a
// 16-voxel block face, not from a vertex attribute. A 1 m face is therefore
// exactly one 16x16 atlas tile, with one tile texel per voxel face.
//
// The AO array carries the quad's four corner values (a greedy quad's merge key
// requires identical values across the rectangle); the vertex shader selects its
// own corner with `vertex_index % 4`, because each quad's four vertices are
// consecutive in the index stream, and the fragment interpolates it. Block
// light comes from the M2 light volume uploaded by the viewer; the fragment
// shades from its own block cell (sampling one step along the face normal is
// later polish). Probes are uploaded and bound as a placeholder; the indirect
// pass does not sample them yet.

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

// One u32 per light word: four words per cell (sky rgb/intensity, block
// rgb/intensity), chunk-local, x fastest.
@group(2) @binding(0)
var<storage, read> block_light: array<u32>;

// Near-grid SH4 records followed by the packed validity mask. Uploaded as a
// placeholder for the later indirect pass; not sampled yet.
@group(2) @binding(1)
var<storage, read> probes: array<u32>;

// Chunk origin in world voxels.
@group(2) @binding(2)
var<uniform> chunk_origin: vec4<f32>;

const CELLS: u32 = {{CELLS}};

struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    // local.xyz, spare
    @location(0) local_spare: vec4<u32>,
    // material u16, packed u16 (face 3 bits + four 2-bit AO levels)
    @location(1) material_packed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local: vec3<f32>,
    @location(1) @interpolate(flat) material: u32,
    @location(2) @interpolate(flat) face: u32,
    @location(3) ao: f32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let local = vec3<f32>(
        f32(input.local_spare.x),
        f32(input.local_spare.y),
        f32(input.local_spare.z),
    );
    let world = (local + chunk_origin.xyz) * 0.0625;

    let material = input.material_packed.x;
    let packed = input.material_packed.y;
    let face = packed & 7u;
    let level = (packed >> (3u + 2u * (input.vertex_index % 4u))) & 3u;

    var output: VertexOutput;
    output.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    output.local = local;
    output.material = material;
    output.face = face;
    output.ao = f32(level * 85u) / 255.0;
    return output;
}

// Face codes are 0..5 = +X, -X, +Y, -Y, +Z, -Z.
const FACE_SHADE: array<f32, 6> = array<f32, 6>(0.62, 0.62, 1.0, 0.5, 0.82, 0.72);

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Block origin along the face's tangent axes. Fragment centres inside a
    // quad stay below the next block boundary, so the floor is stable.
    let block = floor(input.local / 16.0) * 16.0;
    let rel = input.local - block;

    // Face-local coordinates in 0..16. Orient each side as seen from outside;
    // v is flipped for side faces because atlas row 0 is the top row.
    var face_uv = vec2<f32>(rel.x, rel.y);
    if input.face == 0u {
        face_uv = vec2<f32>(16.0 - rel.z, 16.0 - rel.y); // +X
    } else if input.face == 1u {
        face_uv = vec2<f32>(rel.z, 16.0 - rel.y); // -X
    } else if input.face == 2u {
        face_uv = vec2<f32>(rel.x, 16.0 - rel.z); // +Y
    } else if input.face == 3u {
        face_uv = vec2<f32>(rel.x, rel.z); // -Y
    } else if input.face == 4u {
        face_uv = vec2<f32>(rel.x, 16.0 - rel.y); // +Z
    } else {
        face_uv = vec2<f32>(16.0 - rel.x, 16.0 - rel.y); // -Z
    }

    // Fetch the exact tile texel. Sampling the atlas with a sampler at a tile
    // edge bleeds the neighbouring material into the last texel (white snow
    // below grass, dark stone elsewhere), which reads as a grid line.
    let texel = vec2<u32>(
        u32(clamp(face_uv.x, 0.0, 15.999)),
        u32(clamp(face_uv.y, 0.0, 15.999)),
    );
    let tile_origin =
        vec2<u32>(input.material % 16u, input.material / 16u) * 16u;
    let texel_color = textureLoad(atlas, vec2<i32>(tile_origin + texel), 0);

    // Chunk-local light cell; the far chunk face clamps onto the last cell.
    let cell = min(
        vec3<u32>(
            u32(floor(input.local.x / 16.0)),
            u32(floor(input.local.y / 16.0)),
            u32(floor(input.local.z / 16.0)),
        ),
        vec3<u32>(CELLS - 1u),
    );
    let base = (cell.x + cell.y * CELLS + cell.z * CELLS * CELLS) * 4u;
    let sky = (block_light[base + 1u] >> 16u) & 0xFFFFu;
    let block_level = (block_light[base + 3u] >> 16u) & 0xFFFFu;
    let level = max(sky, block_level) >> 12u;
    // A small ambient floor keeps unlit faces readable without changing
    // sky-lit surfaces (level 15 gives exactly 1.0).
    let light = 0.25 + 0.75 * f32(level) / 15.0;

    // Fully occluded corners keep some ambient light; without this floor,
    // one-voxel step edges read as black grid lines.
    let ao = 0.4 + 0.6 * input.ao;
    let shade = FACE_SHADE[input.face] * ao * light;
    return vec4<f32>(texel_color.rgb * shade, 1.0);
}
