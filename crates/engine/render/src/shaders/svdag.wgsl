// GPU mirror of `voxel_storage::dag` and `voxel_storage::gpu`.
//
// Binding group 0:
//   @binding(0) nodes        global node pool, u32 words
//   @binding(1) chunk_table  5 u32 per slot: [x, y, z, root, block_table]
//   @binding(2) overlays     per-block edit overrides, OVERLAY_WORDS per slot
//   @binding(3) block_tables 64 u32 per chunk, one overlay slot per 16^3 block
//
// Node layout, matching the CPU pool exactly:
//   [header, child_0 .. child_{k-1}]
//   header = child_mask (bits 0..8) | level << 8
//   only present children, in ascending child-index order
//   child index c = cx | cy << 1 | cz << 2
//   level > 1 : child word is a pool offset
//   level == 1: child word is a MaterialId (0 = air = absent)
//
// Overlay slot layout (one 16^3 block):
//   [0 .. 2048)    packed u16 material ids, two voxels per word
//   [2048 .. 2176) presence bits, one per voxel
// The presence bit, not the value, marks an edited voxel: material 0 carves the
// DAG value to air and must still shadow it.
//
// Chunk position x/y/z are stored as u32 two's-complement bit patterns, so
// bitcast<u32>(p.x) is the same value the CPU wrote for a negative coordinate.

// Layout constants mirrored from Rust. They are asserted value for value by
// crates/engine/render/tests/shaders.rs, which also fails if a constant here
// has no Rust owner; change the Rust table and this block together.
const NONE: u32 = 0xFFFFFFFFu;
const EMPTY: u32 = 0xFFFFFFFFu;
const TOMB: u32 = 0xFFFFFFFEu;
const NO_OVERLAY: u32 = 0xFFFFFFFFu;
const NO_TABLE: u32 = 0xFFFFFFFFu;
const ROOT_LEVEL: u32 = 6u;
const SLOT_WORDS: u32 = 5u;
const BLOCKS_PER_CHUNK: u32 = 64u;
const OVERLAY_VALUES: u32 = 2048u;
const OVERLAY_WORDS: u32 = 2176u;
const MASK_BITS: u32 = 0xFFu;

@group(0) @binding(0) var<storage, read> nodes: array<u32>;
@group(0) @binding(1) var<storage, read> chunk_table: array<u32>;
@group(0) @binding(2) var<storage, read> overlays: array<u32>;
@group(0) @binding(3) var<storage, read> block_tables: array<u32>;

// Must match `slot_hash` in voxel_storage::gpu.
fn slot_hash(p: vec3<i32>) -> u32 {
    var h = bitcast<u32>(p.x) * 73856093u ^ bitcast<u32>(p.y) * 19349663u ^ bitcast<u32>(p.z) * 83492791u;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

// Linear probe over the chunk table. Returns the slot's base word offset, or
// NONE on a miss. A slot is occupied if it has a DAG root or a block table, so a
// chunk whose edits carved every voxel out of an empty DAG is still addressable.
fn chunk_slot(p: vec3<i32>) -> u32 {
    let cap = arrayLength(&chunk_table) / SLOT_WORDS;
    let mask = cap - 1u;
    var i = slot_hash(p) & mask;
    loop {
        let base = i * SLOT_WORDS;
        let root = chunk_table[base + 3u];
        let blocks = chunk_table[base + 4u];
        if root == EMPTY && blocks == NO_TABLE {
            return NONE;
        }
        if root != TOMB
            && chunk_table[base] == bitcast<u32>(p.x)
            && chunk_table[base + 1u] == bitcast<u32>(p.y)
            && chunk_table[base + 2u] == bitcast<u32>(p.z)
        {
            return base;
        }
        i = (i + 1u) & mask;
    }
    // Unreachable: probe() mirrors the CPU table, which always has a free slot.
    // Naga needs an explicit return after a loop body that only returns.
    return NONE;
}

// DAG root for a chunk, or NONE if it has no DAG (overlay-only or absent).
fn chunk_root(p: vec3<i32>) -> u32 {
    let base = chunk_slot(p);
    if base == NONE {
        return NONE;
    }
    let root = chunk_table[base + 3u];
    if root == EMPTY {
        return NONE;
    }
    return root;
}

// DDA-free octree descent to one voxel; `pos` is chunk-local 0..64.
// Mirrors `dag_get` in voxel_storage::dag.
fn dag_get(root: u32, pos: vec3<u32>) -> u32 {
    var node = root;
    if node == NONE {
        return 0u;
    }
    var level = ROOT_LEVEL;
    loop {
        let header = nodes[node];
        let mask = header & MASK_BITS;
        let shift = level - 1u;

        let cx = (pos.x >> shift) & 1u;
        let cy = (pos.y >> shift) & 1u;
        let cz = (pos.z >> shift) & 1u;
        let child = cx | (cy << 1u) | (cz << 2u);
        if ((mask >> child) & 1u) == 0u {
            return 0u;
        }

        // Children are stored compactly: the slot is the popcount of the
        // present children below this index.
        let index = countOneBits(mask & ((1u << child) - 1u));
        let word = nodes[node + 1u + index];
        if level == 1u {
            return word;
        }
        node = word;
        level = shift;
    }
    // Unreachable: level reaches 1 before the loop can fall through.
    return 0u;
}

// One chunk-local voxel, or 0 (air) if the chunk or voxel is absent.
// The 16^3 block's overlay is checked first, then the DAG.
fn sample_chunk(p: vec3<i32>, local: vec3<u32>) -> u32 {
    let base = chunk_slot(p);
    if base == NONE {
        return 0u;
    }

    let table_slot = chunk_table[base + 4u];
    if table_slot != NO_TABLE {
        // block = bx | by << 2 | bz << 4, with 16-voxel blocks.
        let block = (local.x >> 4u) | ((local.y >> 4u) << 2u) | ((local.z >> 4u) << 4u);
        let overlay = block_tables[table_slot * BLOCKS_PER_CHUNK + block];
        if overlay != NO_OVERLAY {
            // within = x | y << 4 | z << 8, in 0..4096.
            let idx = (local.x & 15u) | ((local.y & 15u) << 4u) | ((local.z & 15u) << 8u);
            let bit_word = overlays[overlay * OVERLAY_WORDS + OVERLAY_VALUES + (idx >> 5u)];
            if ((bit_word >> (idx & 31u)) & 1u) != 0u {
                let word = overlays[overlay * OVERLAY_WORDS + (idx >> 1u)];
                return (word >> ((idx & 1u) * 16u)) & 0xFFFFu;
            }
        }
    }

    return dag_get(chunk_table[base + 3u], local);
}
