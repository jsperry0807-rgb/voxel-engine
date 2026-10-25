// GPU mirror of `voxel_block::HotRecord` and `voxel_block::FaceTextureIds`.
//
// Binding group 1:
//   @binding(0) hot          2 u32 per MaterialId
//   @binding(1) texture_sets 3 u32 per TextureSetId
//
// Hot word 0 (meta):
//   bits  0..=2   render type
//   bit   3       has textures
//   bits  4..=7   light emission (0..=15)
//   bits  8..=11  light opacity (0..=15)
//   bits 12..=27  hot flag bits
//   bits 28..=31  spare
// Hot word 1:
//   bits  0..=15  texture set id (valid iff "has textures")
//   bits 16..=31  spare
//
// A texture set is six u16 texture ids in Face::ALL order (north, south,
// east, west, top, bottom), packed two per u32 word. These constants must
// match crates/engine/block/src/hot.rs and texture.rs.

const HOT_STRIDE: u32 = 2u;
const SET_STRIDE: u32 = 3u;

const RENDER_TYPE_MASK: u32 = 7u;
const HAS_TEXTURES: u32 = 8u;
const EMISSION_SHIFT: u32 = 4u;
const OPACITY_SHIFT: u32 = 8u;
const FLAGS_SHIFT: u32 = 12u;
const LIGHT_MASK: u32 = 15u;
const FLAGS_MASK: u32 = 0xFFFFu;

const RENDER_SOLID: u32 = 0u;
const RENDER_CUTOUT: u32 = 1u;
const RENDER_TRANSLUCENT: u32 = 2u;
const RENDER_INVISIBLE: u32 = 3u;

const FLAG_SOLID: u32 = 1u << 0u;
const FLAG_OPAQUE: u32 = 1u << 1u;
const FLAG_REPLACEABLE: u32 = 1u << 2u;
const FLAG_LIQUID: u32 = 1u << 3u;
const FLAG_CULLS_SELF: u32 = 1u << 4u;

@group(1) @binding(0) var<storage, read> hot: array<u32>;
@group(1) @binding(1) var<storage, read> texture_sets: array<u32>;

fn hot_meta(id: u32) -> u32 {
    return hot[id * HOT_STRIDE];
}

fn hot_set_word(id: u32) -> u32 {
    return hot[id * HOT_STRIDE + 1u];
}

fn has_textures(id: u32) -> bool {
    return (hot_meta(id) & HAS_TEXTURES) != 0u;
}

fn render_type(id: u32) -> u32 {
    return hot_meta(id) & RENDER_TYPE_MASK;
}

fn flags(id: u32) -> u32 {
    return (hot_meta(id) >> FLAGS_SHIFT) & FLAGS_MASK;
}

fn has_flag(id: u32, flag: u32) -> bool {
    return (flags(id) & flag) == flag;
}

fn light_emission(id: u32) -> u32 {
    return (hot_meta(id) >> EMISSION_SHIFT) & LIGHT_MASK;
}

fn light_opacity(id: u32) -> u32 {
    return (hot_meta(id) >> OPACITY_SHIFT) & LIGHT_MASK;
}

fn is_solid(id: u32) -> bool {
    return has_flag(id, FLAG_SOLID);
}

fn is_opaque(id: u32) -> bool {
    return has_flag(id, FLAG_OPAQUE);
}

// Texture-array slice for one face, or 0 when the block never draws.
// `face` is a `Face::ALL` index in 0..6.
fn face_texture(id: u32, face: u32) -> u32 {
    if !has_textures(id) {
        return 0u;
    }
    let set_id = hot_set_word(id) & 0xFFFFu;
    let word = texture_sets[set_id * SET_STRIDE + face / 2u];
    return (word >> (16u * (face & 1u))) & 0xFFFFu;
}
