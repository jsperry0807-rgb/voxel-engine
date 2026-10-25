// GPU mirror of `voxel_storage::dag` and `voxel_storage::gpu`.
//
// Binding group 0:
//   @binding(0) nodes        global node pool, u32 words
//   @binding(1) chunk_table  4 u32 per slot: [x, y, z, root]
//
// Node layout, matching the CPU pool exactly:
//   [header, child_0 .. child_{k-1}]
//   header = child_mask (bits 0..8) | level << 8
//   only present children, in ascending child-index order
//   child index c = cx | cy << 1 | cz << 2
//   level > 1 : child word is a pool offset
//   level == 1: child word is a MaterialId (0 = air = absent)
//
// Chunk position x/y/z are stored as u32 two's-complement bit patterns, so
// bitcast<u32>(p.x) is the same value the CPU wrote for a negative coordinate.

const NONE: u32 = 0xFFFFFFFFu;
const EMPTY: u32 = 0xFFFFFFFFu;
const TOMB: u32 = 0xFFFFFFFEu;
const ROOT_LEVEL: u32 = 6u;
const SLOT_WORDS: u32 = 4u;
const MASK_BITS: u32 = 0xFFu;
const LEVEL_SHIFT: u32 = 8u;

@group(0) @binding(0) var<storage, read> nodes: array<u32>;
@group(0) @binding(1) var<storage, read> chunk_table: array<u32>;

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

// Linear probe over the chunk table. Returns NONE on a miss.
fn chunk_root(p: vec3<i32>) -> u32 {
    let cap = arrayLength(&chunk_table) / SLOT_WORDS;
    let mask = cap - 1u;
    var i = slot_hash(p) & mask;
    loop {
        let base = i * SLOT_WORDS;
        let root = chunk_table[base + 3u];
        if root == EMPTY {
            return NONE;
        }
        if root != TOMB
            && chunk_table[base] == bitcast<u32>(p.x)
            && chunk_table[base + 1u] == bitcast<u32>(p.y)
            && chunk_table[base + 2u] == bitcast<u32>(p.z)
        {
            return root;
        }
        i = (i + 1u) & mask;
    }
    // Unreachable: probe() mirrors the CPU table, which always has a free slot.
    // Naga needs an explicit return after a loop body that only returns.
    return NONE;
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
fn sample_chunk(p: vec3<i32>, local: vec3<u32>) -> u32 {
    return dag_get(chunk_root(p), local);
}
