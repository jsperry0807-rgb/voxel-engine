// First chunk shader (M1.5-12).
//
// ADR-010 vertex data is chunk-local voxel coordinates (6.25 cm), material,
// and face code. This chunk sits at the world origin, so world = local * 0.0625.
//
// UVs are derived per fragment from the face-local position inside a 16-voxel
// block face, not from a vertex attribute. A 1 m face is therefore exactly one
// 16x16 atlas tile, with one tile texel per voxel face.

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

struct VertexInput {
    // local.xyz, material
    @location(0) local_material: vec4<u32>,
    // face, ao0, ao1, ao2
    @location(1) face_ao0: vec4<u32>,
    // ao3, reserved
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
    let world = local * 0.0625;

    var output: VertexOutput;
    output.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    output.local = local;
    output.material = input.local_material.w;
    output.face = input.face_ao0.x;
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

    // Half-texel inset keeps nearest sampling off the neighbouring tile.
    let texel = clamp(face_uv, vec2<f32>(0.5), vec2<f32>(15.5));
    let tile_origin =
        vec2<f32>(f32(input.material % 16u), f32(input.material / 16u)) * 16.0;
    let uv = (tile_origin + texel) / 256.0;
    let texel_color = textureSample(atlas, atlas_sampler, uv);

    // AO placeholder: the naive mesh writes 255 in every corner. M2 replaces
    // this with the interpolated corner value.
    let ao = 1.0;
    let shade = FACE_SHADE[input.face] * ao;
    return vec4<f32>(texel_color.rgb * shade, 1.0);
}
