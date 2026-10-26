// Chunk-edge constants for the 64³ baseline (`CHUNK_BITS = 6`).
//
// `BLOCK_LEVEL` is the block-grid stride: blocks per chunk edge is
// `1 << BLOCK_LEVEL`, so block indices are packed `bx | by << BLOCK_LEVEL |
// bz << (2 * BLOCK_LEVEL)`.
const ROOT_LEVEL: u32 = 6u;
const BLOCK_LEVEL: u32 = 2u;
const BLOCKS_PER_CHUNK: u32 = 64u;
