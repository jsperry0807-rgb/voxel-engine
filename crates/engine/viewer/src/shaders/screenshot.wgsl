// Screenshot shader for the M1.5-15 acceptance set.
//
// Same vertex path and atlas UVs as `chunk.wgsl`, but it keeps the sampled
// alpha and discards cutout texels so leaves, water and ice can be judged in
// the committed images. The example and the naga validation test both include
// this file, so a shader edit cannot drift from its test.

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

    var face_uv = vec2<f32>(rel.x, rel.y);
    if input.face == 0u {
        face_uv = vec2<f32>(16.0 - rel.z, 16.0 - rel.y);
    } else if input.face == 1u {
        face_uv = vec2<f32>(rel.z, 16.0 - rel.y);
    } else if input.face == 2u {
        face_uv = vec2<f32>(rel.x, 16.0 - rel.z);
    } else if input.face == 3u {
        face_uv = vec2<f32>(rel.x, rel.z);
    } else if input.face == 4u {
        face_uv = vec2<f32>(rel.x, 16.0 - rel.y);
    } else {
        face_uv = vec2<f32>(16.0 - rel.x, 16.0 - rel.y);
    }

    let texel = clamp(face_uv, vec2<f32>(0.5), vec2<f32>(15.5));
    let tile_origin =
        vec2<f32>(f32(input.material % 16u), f32(input.material / 16u)) * 16.0;
    let uv = (tile_origin + texel) / 256.0;
    let color = textureSample(atlas, atlas_sampler, uv);
    if color.a < 0.5 {
        discard;
    }
    return vec4<f32>(color.rgb * FACE_SHADE[input.face], color.a);
}
