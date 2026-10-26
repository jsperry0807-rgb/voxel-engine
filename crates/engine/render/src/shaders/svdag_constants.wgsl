// Layout constants mirrored from Rust. They are asserted value for value by
// crates/engine/render/tests/shaders.rs, which also fails if a constant here
// has no Rust owner; change the Rust table and this block together.
//
// The edge-dependent constants (ROOT_LEVEL, BLOCK_LEVEL and BLOCKS_PER_CHUNK)
// live in `svdag_edge_64.wgsl` / `svdag_edge_128.wgsl`; `voxel_render`'s
// `SVDAG_WGSL` prepends the one selected by the `chunk128` feature.
const NONE: u32 = 0xFFFFFFFFu;
const EMPTY: u32 = 0xFFFFFFFFu;
const TOMB: u32 = 0xFFFFFFFEu;
const NO_OVERLAY: u32 = 0xFFFFFFFFu;
const NO_TABLE: u32 = 0xFFFFFFFFu;
const SLOT_WORDS: u32 = 5u;
const OVERLAY_VALUES: u32 = 2048u;
const OVERLAY_WORDS: u32 = 2176u;
const MASK_BITS: u32 = 0xFFu;
