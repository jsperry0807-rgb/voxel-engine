# The Four Units, Specified

Here's a concrete, internally-consistent spec. Every number is chosen to satisfy a constraint you've already committed to (6.25 cm voxels, 1 m blocks, GPU raymarch, Minecraft-like gameplay), and every struct separates "Voxelis handles this" from "you build this."

---

## The Four Units at a Glance

| Unit         | Definition                               | Stored as                                              | Who owns it             |
| ------------ | ---------------------------------------- | ------------------------------------------------------ | ----------------------- |
| **Voxel**    | 6.25 cm cube                             | `MaterialId` (u16) in an SVDAG leaf                    | VoxTree                 |
| **Block**    | 16³ voxel region = 1 m³                  | Conceptually one depth-4 SVDAG subtree; usually 1 leaf | Game layer (conceptual) |
| **Chunk**    | 64³ voxel region = 4 m³ = 4³ = 64 blocks | One `VoxTree<u16>` at max_depth 6                      | VoxChunk + game layer   |
| **Material** | Voxel appearance + properties            | `u16` index into a global registry                     | Game layer              |

**Why 4 m chunks and not 8 m:** at 64³ voxels per chunk, per-chunk SVDAG node count stays in the 5K–20K range (~80–320 KB), which keeps VRAM bounded when you have 256+ chunks loaded. 8 m chunks (128³) are 8× the volume and 8× the node count per chunk, which is workable for streaming but hostile to the GPU raymarcher's per-chunk cost. If you later want coarser streaming, make _regions_ 8 m — don't make chunks 8 m.

---

## Materials

```rust
// Global registry, indexed by MaterialId. Built once at world load, networked once.
type MaterialId = u16;          // 65,536 materials max — plenty for any modded game

struct Material {
    name:           String,
    face_textures:  [u16; 6],   // indices into the TextureArray, per face
                                // (nx, px, ny, py, nz, pz — or repeat for uniform)
    properties:     MaterialProperties,
    template:       Option<BlockId>,  // depth-4 BlockId, only for detail materials
}

struct MaterialProperties {
    opaque:         bool,        // false = transparent (water, glass, leaves)
    emissive:       bool,        // light source (torch, glowstone)
    collision:      bool,        // blocks movement
    .. // optional: hardness, sound, liquid flag, etc.
}

// Registry: Vec<Material>, indexed by MaterialId.
// Convention: MaterialId 0 = EMPTY (air). Don't use 0 for a real material.
```

**What this gets you:**

- **u16 palette** — 65,536 materials, stored in the leaf node. Fits in the SVDAG's `T` directly with no indirection.
- **Per-face textures** — grass has a top texture (green) and side texture (dirt-with-grass-edge), like Minecraft. Doesn't require a separate "grass" material per face.
- **Templates are optional** — a plain stone material has `template: None` and is a uniform leaf. Only materials that need sub-block visual detail (wood grain, cracked stone variants, grass-dirt transition blocks) pay the template cost, and only once globally.
- **Properties are separate** — opaque vs. transparent affects meshing and ray-marching; emissive affects lighting; collision affects physics. Don't try to encode these in the material's color or template.

**Editorial note:** don't add "block shape" to the material. A slab or stair is a _template material_ (a 16³ voxel pattern that happens to be a half-block). This is why templates exist.

---

## Textures

```rust
// One global texture array. All materials share it. One texture lookup per voxel face.
struct TextureArray {
    textures:   Vec<Texture2D>,   // array of 256×256 RGBA8 slices
    format:     Format,           // BC7 compressed (or BC1 for memory-critical targets)
    mip_levels: u8,               // full mip chain
}

// Global constant: 256×256 per material face.
const TEXELS_PER_FACE: u32 = 256;
const TEXELS_PER_VOXEL: u32 = 16;   // 256 / 16 voxels per block edge = 16 texels per voxel edge
```

**Why 256², not 512² or 400²:**

- **256² is power-of-2.** Clean mipmaps, full anisotropic filtering, no padding waste.
- **16 texels per voxel at 6.25 cm = 3.9 mm per texel.** Fine enough for any Minecraft-like material. You will not see individual texels at normal viewing distance.
- **256² × 6 faces × 100 materials = 39 MB VRAM uncompressed, ~10 MB BC7 compressed.** Affordable.
- **512² (for hero materials) = 4× memory** per material. Use sparingly: player characters, signature blocks, anything shown at extreme close-up. A 512² slice costs 1 MB compressed, so 10 hero materials = 10 MB.

**How texturing works on the block face:**

- Each voxel face samples a 16×16 texel region of the material's 256² texture, offset by the voxel's local block position.
- An intact 1 m block face = 16×16 voxel faces, each showing a different 16×16 slice, tiling seamlessly into the full 256².
- A damaged block = only surviving voxels' faces render, each showing its slice. The damage looks like the texture is "chipped away," exactly the intended effect.

**Texture array, not atlas** — avoids seam bleeding at mip boundaries, avoids UV wrapping rules, and makes shader sampling trivial (`texture_array[slot][uv]`).

---

## Blocks

**Blocks are conceptual, not stored.** A block is a 1 m = 16³ voxel region. What's actually in the SVDAG is voxels, and a "block" is just the depth-4 region that the player targets.

```rust
// The player-facing abstraction. Never stored directly.
enum BlockState {
    Uniform(MaterialId),                       // all 4096 voxels same material
    Damaged(MaterialId, u16),                  // mostly material, damage bitmask
    Detailed,                                  // not uniform, query voxel data
}

// Game-layer operations. All map to VoxTree batch operations.
fn place_block(chunk: &mut Chunk, block_pos: BlockCoord, material: MaterialId) {
    // Fill 16³ region with material (single batch)
    let batch = build_fill_batch(block_pos, 16, material);
    chunk.apply_batch(batch);
}

fn destroy_block(chunk: &mut Chunk, block_pos: BlockCoord) {
    // Fill 16³ region with MaterialId::EMPTY
    let batch = build_fill_batch(block_pos, 16, EMPTY);
    chunk.apply_batch(batch);
}

fn get_block(chunk: &Chunk, block_pos: BlockCoord) -> BlockState {
    // Sample 4096 voxels; if all same → Uniform; else → Damaged/Detailed
}
```

**How a block is stored in the SVDAG:**

| Block content                       | SVDAG representation                                    | Cost                                                |
| ----------------------------------- | ------------------------------------------------------- | --------------------------------------------------- |
| Uniform stone                       | One leaf node (all 4096 voxels = stone)                 | 16 bytes (shared globally)                          |
| Uniform wood with template          | One leaf node = wood material; render uses template     | 16 bytes (shared)                                   |
| Intact detailed block               | Depth-4 subtree matching template BlockId               | 1 pointer (8 bytes) in parent + template is shared  |
| Damaged block                       | Small subtree: pristine template minus destroyed voxels | 50–500 nodes × 16 B = 1–8 KB (per damaged instance) |
| Fully detailed block (player-built) | Full 16³ subtree                                        | up to 4096 nodes × 16 B = 64 KB (rare)              |

**The crucial invariant:** _uniform blocks collapse to one leaf._ This is what makes the system usable — the vast majority of blocks in a world are uniform (stone, dirt, air), and they cost 16 bytes each (shared globally via hash consing) regardless of how many of them exist. Only damaged or detailed blocks pay per-instance cost.

**Templates are how uniform blocks get detail without paying for it.** A "wood plank" material has `template: Some(wood_grain_block_id)`. When you place a wood block, you write a uniform leaf pointing at "wood plank." At _render_ time, the shader dereferences the material, sees the template, and draws the template's detail. At _storage_ time, it's 16 bytes.

When the block is damaged, the template is "realized" — the damaged version becomes an actual subtree. This is why damage costs more than uniform placement.

---

## Chunks

```rust
struct Chunk {
    // --- Voxelis-managed ---
    position:   IVec3,               // chunk coords (world_pos / 4m)
    svdag:      VoxTree<u16>,        // max_depth 6 = 64³ voxels, at 6.25 cm
    // --- Game-layer-managed ---
    overlay:    EditOverlay,         // pending edits (HashMap<u32, u16>)
    lod:        [Option<LodChunk>; 3],  // explicit LOD levels, lazily built
    dirty:      bool,                // needs re-consolidation or re-upload
    light:      Vec<u8>,             // per-voxel light (64³ × 2 channels = 32 KB)
    metadata:   ChunkMetadata,       // version, last_modified, dirty_flags, etc.
}

struct EditOverlay {
    edits: HashMap<u32, u16>,        // voxel offset (0..64³) → material
    count: u32,                      // for consolidation trigger
}

struct LodChunk {
    svdag:      VoxTree<u16>,        // coarser resolution
    level:      u8,                  // 1, 2, 3 → 32³, 16³, 8³ voxels per edge
}
```

**Chunk spec, exhaustively:**

| Property            | Value                             | Why                                    |
| ------------------- | --------------------------------- | -------------------------------------- |
| Size                | 4 m × 4 m × 4 m                   | 64³ voxels at 6.25 cm                  |
| Voxels per axis     | 64 (= 2⁶)                         | Aligns with VoxTree's power-of-2 depth |
| Blocks per axis     | 4                                 | 64 / 16 = 4 blocks per chunk edge      |
| Blocks per chunk    | 64                                | 4³ = 64 blocks                         |
| `max_depth`         | 6                                 | 64³ = full chunk                       |
| LOD 0               | full detail (64³)                 | Near range (0–32 chunks)               |
| LOD 1               | 32³ voxels per edge (12.5 cm)     | Mid range (32–96 chunks)               |
| LOD 2               | 16³ voxels per edge (25 cm)       | Far range (96–256 chunks)              |
| LOD 3               | 8³ voxels per edge (50 cm)        | Very far (256+ chunks)                 |
| Memory (near, full) | ~100–400 KB SVDAG                 | Depends on detail                      |
| Memory (LOD 1)      | ~1/8 of LOD 0                     | Geometric halving per axis             |
| Light               | 64³ × 2 × 1 B = 32 KB             | Separate from SVDAG, per-voxel         |
| Overlay             | up to ~4096 entries × 8 B = 32 KB | Bounded by consolidation policy        |

**Why LOD chunks are explicit, not baked into a single SVDAG:**

- The single-SVDAG-with-baked-LOD approach (Voxelis's "zero-cost LOD") is _memory-free_ but requires loading the full-depth chunk to render any LOD. For 512+ chunks of render distance, that's 400 MB–2 GB VRAM. Too much.
- Explicit LOD chunks add ~14% memory overhead (1 + 1/8 + 1/64 + 1/512) but allow you to load only the LOD level appropriate to distance. Near chunks: full. Far chunks: 1/8 or 1/64. VRAM drops by ~5×.
- LOD chunks are built lazily (on first need) and cached. Building LOD 1 from LOD 0 is a batch tree-walk with aggregation — cheap (~1 ms per chunk).

**Overlay policy (crucial):**

- Every edit (place, destroy, damage) writes to `overlay`, O(1).
- Consolidation triggers: overlay size ≥ 4096 entries, OR 30 s idle, OR chunk save/unload, OR LOD transition. On trigger, the overlay is applied as a single `apply_batch` to the SVDAG.
- **GPU sees the overlay via a small compositing buffer.** The raymarcher checks the overlay buffer for the ray's chunk before traversing the SVDAG. This is what makes mining feel instant: the DAG isn't touched, but the GPU knows about the edit.

---

## How the Units Interact

### Player action: mine a block

```txt
1. Raycast SVDAG (64³ traversal) → hit voxel, normal
2. Snap to block: (voxel_x / 16, voxel_y / 16, voxel_z / 16)
3. Owning chunk: (block_x / 4, block_y / 4, block_z / 4)   ← always one chunk (4 m alignment)
4. Overlay write: 4096 entries (voxel_offsets → EMPTY)
   - Or one fill op if overlay supports sparse fills
5. Upload overlay buffer for this chunk to GPU (small, ~32 KB)
6. GPU next frame: overlay compositing hides the block
7. On consolidation trigger: apply_batch(overlay) → SVDAG update, LOD rebuild if needed
```

**Latency to "block is gone":** one frame (~16 ms). The player never sees the DAG cost.

### Player action: place a block

```txt
1. Raycast → hit block face
2. Adjacent block position (block_pos + face_normal)
3. Overlay write: 4096 entries → material_id
4. Upload overlay buffer
5. On consolidation: batch into SVDAG
```

Same latency profile.

### Rendering a chunk

```txt
1. Chunk's LOD selected by distance: LOD 0 near, LOD 1/2/3 farther
2. SVDAG + overlay buffer uploaded to GPU (SSBOs)
3. Raymarcher traverses SVDAG from root, checking overlay first
4. At leaf: read MaterialId → dereference Material → read texture array
5. Sample texture at UV = (voxel_local * 16 + face_u * 16, voxel_local_y * 16 + face_v * 16)
6. Composite with light data
```

The shader sees one unified structure (SVDAG + overlay). The player sees the result, not the layering.

### Saving a chunk to disk

```txt
1. Consolidate overlay → SVDAG
2. Serialize SVDAG nodes (id-mapped, zstd-compressed)
3. Serialize overlay (should be empty after consolidation — if not, save it as a diff)
4. Serialize LOD 1/2/3 (lazily, if built and dirty)
5. Write to region file (game layer handles region → file grouping)
```

### Loading a chunk from disk

```txt
1. Read region file header → chunk offset/size
2. Decompress SVDAG node data (zstd)
3. For each node: compute content hash → get_or_create in interner (auto-dedup)
4. Rebuild SVDAG root, install in VoxChunk
5. Decompress overlay if present
6. Decompress or rebuild LOD levels if needed
```

**Content-addressed loading** (hash node content → `get_or_create`) means loading the same chunk twice in two sessions reuses all nodes. No ID-mapping is needed on disk — the content _is_ the ID.

---

## The Key Design Decisions, Summarized

| Decision           | Value                                | Rationale                                          |
| ------------------ | ------------------------------------ | -------------------------------------------------- |
| Voxel size         | 6.25 cm (16/m)                       | Power-of-2 alignment, Minecraft's microblock size  |
| Block size         | 1 m (16³ voxels)                     | Minecraft conventions                              |
| Chunk size         | 4 m (64³ voxels)                     | GPU-friendly per-chunk cost, streaming granularity |
| Material ID        | u16                                  | 65,536 materials, fits in SVDAG leaf directly      |
| Texture resolution | 256² per face                        | Power-of-2, 3.9 mm texels, affordable              |
| Texture storage    | Texture array                        | No bleeding, trivial sampling                      |
| Blocks             | Conceptual (16³ voxels)              | No block-level storage, just voxel regions         |
| Templates          | Optional per material                | Detail without per-instance cost                   |
| Storage            | SVDAG + overlay                      | DAG memory win + O(1) edits                        |
| LOD                | Explicit, separate chunks            | Bounded VRAM, streamable                           |
| Lighting           | Per-chunk dense array                | Not in SVDAG, separate system                      |
| Disk               | Per-region `.vtm`, content-addressed | Auto-dedup on load                                 |
| Edit path          | Overlay → batched consolidation      | Single-block edits are O(1)                        |

---

## The One Thing That Makes or Breaks This

**Templates must be optional and rare.** If every material has a template, then every block is a subtree, and you've lost the "uniform block = 1 leaf" collapse that makes VoxTree's memory argument work. Keep templates for:

- Player-signature blocks (hero materials)
- Materials with genuine sub-voxel visual detail (wood grain, cracked stone, grass-dirt transition)
- Block _shapes_ (slabs, stairs, fences)

And keep the vast majority of materials template-free. A plain stone, dirt, cobblestone, sand, gravel — these should be uniform leaves with 256² textures. That's where the memory win lives.

**Templates are an escape hatch for the 5% of materials that need detail. The other 95% should be uniform.** If you find yourself authoring templates for 50% of materials, you've drifted into a different game (Teardown-like), and the memory math changes.

---

## What This Spec Does Not Cover (and You Must)

- **Chunk generator** — procedural terrain that fills the SVDAG on load. Separate system.
- **Lighting propagation** — BFS over the per-chunk light array. Separate system.
- **GPU streaming** — moving chunks between disk/CPU/GPU based on camera. Separate system.
- **Physics** — collision and debris. Separate system.
- **Networking** — edit propagation. Separate system.
- **Editor tooling** — placing templates, authoring materials. Separate system.

This spec gives you the **storage and rendering substrate**. Every other subsystem plugs into it through the overlay (edits), the material registry (content), and the chunk manager (streaming). Keep those three interfaces clean and the rest of the engine can evolve independently.
