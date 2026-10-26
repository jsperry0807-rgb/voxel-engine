// Chunk-edge constants for the pre-registered 128³ edge (`CHUNK_BITS = 7`),
// selected by the `chunk128` feature.
//
// `BLOCK_LEVEL` is the block-grid stride: blocks per chunk edge is
// `1 << BLOCK_LEVEL`, so block indices are packed `bx | by << BLOCK_LEVEL |
// bz << (2 * BLOCK_LEVEL)`.
const ROOT_LEVEL: u32 = 7u;
const BLOCK_LEVEL: u32 = 3u;
const BLOCKS_PER_CHUNK: u32 = 512u;
