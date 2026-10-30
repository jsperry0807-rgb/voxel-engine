
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
