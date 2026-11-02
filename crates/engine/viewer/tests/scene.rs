//! M3-08 scene tests: determinism, border agreement, the material set and the
//! u16 index-stream budget.
//!
//! The scene is pure in `(seed, world position)`, so these tests can compare
//! whole volumes byte for byte. The GPU path is exercised by the viewer run
//! and the committed screenshot.

use std::collections::HashSet;

use voxel_core::Face;
use voxel_core::constants::CHUNK_DIM;
use voxel_storage::apron::{Face as ApronFace, SIDE};
use voxel_storage::{ChunkPos, local_index};
use voxel_viewer::chunk_content;
use voxel_viewer::scene::{
    BEDROCK, COBBLESTONE, DIRT, GRASS, GRAVEL, OAK_LEAVES, OAK_LOG, SAND, SANDSTONE, SNOW, STONE,
    Scene, atlas_slot, parse_seed,
};
use voxel_viewer::streaming::ring_chunks;

/// The M1.5-14 atlas slots, in the committed `faces.ron` order.
const M1_5_14_SLOTS: std::ops::RangeInclusive<u16> = 1..=23;

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The committed face table, in slot order.
fn committed_faces() -> Vec<String> {
    let text = std::fs::read_to_string(root().join("crates/engine/block/data/faces.ron"))
        .expect("faces.ron is committed");
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix('"')?;
            Some(rest.split('"').next().expect("a closing quote").to_owned())
        })
        .collect()
}

fn slot_of(key: &str) -> u16 {
    let faces = committed_faces();
    let index = faces
        .iter()
        .position(|face| face == key)
        .unwrap_or_else(|| panic!("{key} is not in faces.ron"));
    (index + 1) as u16
}

#[test]
fn the_seed_is_the_only_input() {
    let position = ChunkPos::new(2, 0, -3);
    let first = Scene::new(7);
    let second = Scene::new(7);
    assert_eq!(first.volume(position), second.volume(position));
    assert_eq!(first.apron(position), second.apron(position));
    assert_eq!(first.height(37, -12), second.height(37, -12));

    let other = Scene::new(8);
    assert_ne!(first.volume(position), other.volume(position));
    // A single column can collide after rounding and clamping; the field as a
    // whole must move.
    let moved = (0..64)
        .filter(|step| first.height(step * 7, -12) != other.height(step * 7, -12))
        .count();
    assert!(moved > 16, "only {moved} of 64 columns moved with the seed");
}

#[test]
fn seeds_parse_as_decimal_or_hex() {
    assert_eq!(parse_seed("42"), Some(42));
    assert_eq!(parse_seed(" 0x2A "), Some(42));
    assert_eq!(parse_seed("0X2a"), Some(42));
    assert_eq!(parse_seed("0x5EED1234"), Some(0x5EED_1234));
    assert_eq!(parse_seed(""), None);
    assert_eq!(parse_seed("0x"), None);
    assert_eq!(parse_seed("-1"), None);
    assert_eq!(parse_seed("scene"), None);
}

/// The seed selects both the terrain and the canopy, not just one of them.
#[test]
fn a_seed_change_moves_terrain_and_trees() {
    let a = Scene::new(11);
    let b = Scene::new(12);
    let mut heights_differ = 0;
    let mut trees_differ = 0;
    for chunk in ring_chunks(ChunkPos::new(0, 0, 0)).into_iter().take(16) {
        let x = chunk.x * CHUNK_DIM as i32 + 3;
        let z = chunk.z * CHUNK_DIM as i32 + 5;
        if a.height(x, z) != b.height(x, z) {
            heights_differ += 1;
        }
        if a.trees_touching(chunk) != b.trees_touching(chunk) {
            trees_differ += 1;
        }
    }
    assert!(
        heights_differ > 8,
        "the height field should move with the seed"
    );
    assert!(trees_differ > 8, "tree placement should move with the seed");
}

/// The apron is sampled from the same scene as the neighbour volume, including
/// the canopy. A mismatch here would draw false border faces or wrong AO.
#[test]
fn the_apron_agrees_with_the_neighbour_volume() {
    for seed in [Scene::default().seed(), 3] {
        let scene = Scene::new(seed);
        for position in [
            ChunkPos::new(0, 0, 0),
            ChunkPos::new(-2, 0, 3),
            ChunkPos::new(5, 0, -4),
        ] {
            let apron = scene.apron(position);
            for face in ApronFace::ALL {
                let (dx, dz) = match face {
                    ApronFace::NegX => (-1, 0),
                    ApronFace::PosX => (1, 0),
                    ApronFace::NegZ => (0, -1),
                    ApronFace::PosZ => (0, 1),
                    ApronFace::NegY | ApronFace::PosY => continue,
                };
                let neighbour = ChunkPos::new(position.x + dx, position.y, position.z + dz);
                let volume = scene.volume(neighbour);
                for v in 0..SIDE {
                    for u in 0..SIDE {
                        // Sparse sampling keeps the test fast; the stamping path
                        // and the per-voxel path share the same tree shape.
                        if (u + v) % 17 != 0 {
                            continue;
                        }
                        let (lx, ly, lz): (u32, u32, u32) = match face {
                            ApronFace::NegX => (CHUNK_DIM as u32 - 1, v, u),
                            ApronFace::PosX => (0, v, u),
                            ApronFace::NegZ => (u, v, CHUNK_DIM as u32 - 1),
                            ApronFace::PosZ => (u, v, 0),
                            ApronFace::NegY | ApronFace::PosY => unreachable!(),
                        };
                        assert_eq!(
                            apron.get(face, u, v),
                            volume[local_index(lx, ly, lz) as usize],
                            "{face:?} ({u}, {v}) of {position:?} disagrees with {neighbour:?}"
                        );
                    }
                }
            }
        }
    }
}

/// Every voxel on the chunk border and every apron texel agrees with the
/// reference sampler, canopy included.
///
/// This is the strongest seam check: the volume stamps trees, the apron stamps
/// trees, and both must match a plain per-voxel query. The chunks are chosen so
/// a tree actually crosses their border, otherwise the check would be vacuous.
#[test]
fn the_border_and_apron_match_the_reference_canopy() {
    let scene = Scene::default();
    let mut chosen = Vec::new();
    for chunk in ring_chunks(ChunkPos::new(0, 0, 0)) {
        let base_x = chunk.x * CHUNK_DIM as i32;
        let base_z = chunk.z * CHUNK_DIM as i32;
        let crosses = scene.trees_touching(chunk).into_iter().any(|tree| {
            let left = tree.x - tree.radius;
            let right = tree.x + tree.radius;
            let back = tree.z - tree.radius;
            let front = tree.z + tree.radius;
            left < base_x
                || right >= base_x + CHUNK_DIM as i32
                || back < base_z
                || front >= base_z + CHUNK_DIM as i32
        });
        if crosses {
            chosen.push(chunk);
            if chosen.len() == 2 {
                break;
            }
        }
    }
    assert_eq!(
        chosen.len(),
        2,
        "the ring has canopy that crosses a chunk edge"
    );

    let mut canopy_checked = 0;
    for chunk in chosen {
        let volume = scene.volume(chunk);
        let apron = scene.apron(chunk);
        let base_x = chunk.x * CHUNK_DIM as i32;
        let base_y = chunk.y * CHUNK_DIM as i32;
        let base_z = chunk.z * CHUNK_DIM as i32;
        let edge = CHUNK_DIM as u32 - 1;
        let mut sampler = scene.sampler();

        for y in 0..CHUNK_DIM as u32 {
            for t in 0..=edge {
                for (x, z) in [(0, t), (edge, t), (t, 0), (t, edge)] {
                    let expected =
                        sampler.block(base_x + x as i32, base_y + y as i32, base_z + z as i32);
                    assert_eq!(
                        volume[local_index(x, y, z) as usize],
                        expected,
                        "border voxel {x},{y},{z} of {chunk:?}"
                    );
                    if expected == OAK_LEAVES || expected == OAK_LOG {
                        canopy_checked += 1;
                    }
                }
            }
        }

        for face in ApronFace::ALL {
            for v in 0..SIDE {
                for u in 0..SIDE {
                    let (dx, dy, dz) = face.offset(u, v);
                    let expected = sampler.block(base_x + dx, base_y + dy, base_z + dz);
                    assert_eq!(
                        apron.get(face, u, v),
                        expected,
                        "{face:?} ({u}, {v}) of {chunk:?}"
                    );
                    if expected == OAK_LEAVES || expected == OAK_LOG {
                        canopy_checked += 1;
                    }
                }
            }
        }
    }
    assert!(
        canopy_checked > 0,
        "the seam check must see canopy on a border or apron plane"
    );
}

/// The per-voxel reference agrees with the stamped volume at sampled voxels,
/// canopy included.
#[test]
fn the_reference_block_matches_the_volume() {
    let scene = Scene::new(0xABCD_1234);
    for position in [ChunkPos::new(0, 0, 0), ChunkPos::new(1, 0, -1)] {
        let volume = scene.volume(position);
        let base_x = position.x * CHUNK_DIM as i32;
        let base_z = position.z * CHUNK_DIM as i32;
        let mut checked_canopy = 0;
        for z in (0..CHUNK_DIM).step_by(3) {
            for x in (0..CHUNK_DIM).step_by(3) {
                for y in (0..CHUNK_DIM).step_by(3) {
                    let expected = scene.block(base_x + x as i32, y as i32, base_z + z as i32);
                    let index = local_index(x as u32, y as u32, z as u32) as usize;
                    assert_eq!(
                        volume[index], expected,
                        "voxel {x},{y},{z} of {position:?} disagrees with Scene::block"
                    );
                    if expected == OAK_LEAVES || expected == OAK_LOG {
                        checked_canopy += 1;
                    }
                }
            }
        }
        assert!(checked_canopy > 0, "{position:?} sampled no canopy");
    }
}

/// Stamping trees in any order is only safe because they never overlap. The
/// grid keeps trunks ≥ 12 columns apart while a canopy is at most 7 wide.
#[test]
fn trees_never_overlap() {
    let scene = Scene::new(Scene::default().seed());
    let cell = Scene::TREE_CELL;
    let mut trees = Vec::new();
    for gz in -24..24 {
        for gx in -24..24 {
            if let Some(tree) = scene.tree_at_cell(gx, gz) {
                trees.push(tree);
            }
        }
    }
    assert!(trees.len() > 20, "the sampled area has trees");
    for (index, a) in trees.iter().enumerate() {
        for b in &trees[index + 1..] {
            let ([ax0, ay0, az0], [ax1, ay1, az1]) = a.bounds();
            let ([bx0, by0, bz0], [bx1, by1, bz1]) = b.bounds();
            let overlap =
                ax0 <= bx1 && bx0 <= ax1 && ay0 <= by1 && by0 <= ay1 && az0 <= bz1 && bz0 <= az1;
            assert!(
                !overlap,
                "{a:?} and {b:?} overlap; stamping order would matter"
            );
        }
    }
    let _ = cell;
}

/// Every material the scene emits resolves inside the M1.5-14 slot range, and
/// the resolver agrees with the committed face table.
#[test]
fn materials_use_only_the_m1_5_14_set() {
    let scene = Scene::default();
    let blocks = [
        BEDROCK,
        STONE,
        DIRT,
        GRASS,
        SAND,
        SANDSTONE,
        SNOW,
        GRAVEL,
        COBBLESTONE,
        OAK_LOG,
        OAK_LEAVES,
    ];
    for block in blocks {
        for face in Face::ALL {
            let slot = atlas_slot(block, face);
            assert!(
                M1_5_14_SLOTS.contains(&slot),
                "block {block} face {face:?} resolved to slot {slot}, outside the M1.5-14 set"
            );
        }
    }

    // Per-face materials: grass and log must not be one slot for every face.
    assert_eq!(
        atlas_slot(GRASS, Face::Top),
        slot_of("voxel:block/grass_block_top")
    );
    assert_eq!(atlas_slot(GRASS, Face::Bottom), slot_of("voxel:block/dirt"));
    assert_eq!(
        atlas_slot(GRASS, Face::North),
        slot_of("voxel:block/grass_block_side")
    );
    assert_eq!(
        atlas_slot(OAK_LOG, Face::Top),
        slot_of("voxel:block/oak_log_top")
    );
    assert_eq!(
        atlas_slot(OAK_LOG, Face::East),
        slot_of("voxel:block/oak_log")
    );
    assert_eq!(
        atlas_slot(OAK_LEAVES, Face::Top),
        slot_of("voxel:block/oak_leaves")
    );
    assert_eq!(atlas_slot(SAND, Face::Top), slot_of("voxel:block/sand"));
    assert_eq!(atlas_slot(SNOW, Face::Top), slot_of("voxel:block/snow"));
    assert_eq!(
        atlas_slot(BEDROCK, Face::Top),
        slot_of("voxel:block/bedrock")
    );
    assert_eq!(
        atlas_slot(SANDSTONE, Face::Top),
        slot_of("voxel:block/sandstone_top")
    );
    assert_eq!(
        atlas_slot(SANDSTONE, Face::Bottom),
        slot_of("voxel:block/sandstone_bottom")
    );
    assert_eq!(
        atlas_slot(SANDSTONE, Face::West),
        slot_of("voxel:block/sandstone_side")
    );
    let _ = scene;
}

/// The ring really contains the terrain and canopy variants the milestone asks
/// for: all five terrain variants, canopy over meadow and forest, no tree
/// rooted on sand, highland or rock, and forest chunks visibly denser than the
/// open ones.
#[test]
fn the_ring_has_every_terrain_and_canopy_variant() {
    use voxel_viewer::scene::Biome;

    let scene = Scene::default();
    let mut biomes = HashSet::new();
    let mut trees_by_biome = [0usize; Biome::ALL.len()];
    let mut forest_chunks = 0;
    let mut open_chunks = 0;

    for chunk in ring_chunks(ChunkPos::new(0, 0, 0)) {
        let centre_x = chunk.x * CHUNK_DIM as i32 + CHUNK_DIM as i32 / 2;
        let centre_z = chunk.z * CHUNK_DIM as i32 + CHUNK_DIM as i32 / 2;
        biomes.insert(scene.biome(centre_x, centre_z));

        let base_x = chunk.x * CHUNK_DIM as i32;
        let base_z = chunk.z * CHUNK_DIM as i32;
        let rooted: Vec<_> = scene
            .trees_touching(chunk)
            .into_iter()
            .filter(|tree| {
                (base_x..base_x + CHUNK_DIM as i32).contains(&tree.x)
                    && (base_z..base_z + CHUNK_DIM as i32).contains(&tree.z)
            })
            .collect();
        let mut forest_here = 0;
        for tree in &rooted {
            let biome = scene.biome(tree.x, tree.z);
            match biome {
                Biome::Forest | Biome::Meadow => {
                    let index = Biome::ALL
                        .iter()
                        .position(|candidate| *candidate == biome)
                        .expect("a known biome");
                    trees_by_biome[index] += 1;
                    if biome == Biome::Forest {
                        forest_here += 1;
                    }
                }
                other => panic!("a {other:?} column grew a tree at {tree:?}"),
            }
        }
        if forest_here >= 5 {
            forest_chunks += 1;
        } else {
            open_chunks += 1;
        }
    }

    assert_eq!(
        biomes.len(),
        Biome::ALL.len(),
        "the ring should show every terrain variant"
    );
    let forest_trees = trees_by_biome[Biome::ALL
        .iter()
        .position(|b| *b == Biome::Forest)
        .expect("forest")];
    let meadow_trees = trees_by_biome[Biome::ALL
        .iter()
        .position(|b| *b == Biome::Meadow)
        .expect("meadow")];
    assert!(forest_trees > 200, "the forest variant carries a canopy");
    assert!(meadow_trees > 20, "the meadow variant scatters oaks");
    assert!(forest_chunks > 20, "some chunks are dense forest");
    assert!(
        open_chunks > 20,
        "some chunks are open ground: the ring shows both canopy variants"
    );
}

/// One chunk mesh uses `u16` indices, so a chunk may emit at most 16,384 quads.
/// The scene has to stay inside that budget for every seed it is asked for.
#[test]
fn the_ring_stays_inside_the_u16_index_stream() {
    let default = Scene::default().seed();
    // 128³ chunks are eight times the work, so the gate configuration checks the
    // default seed's whole ring and leaves extra seeds to the 64³ run.
    let seeds: &[u32] = if cfg!(feature = "chunk128") {
        &[default]
    } else {
        &[default, 1, 2, 3]
    };
    for &seed in seeds {
        let scene = Scene::new(seed);
        let ring = ring_chunks(ChunkPos::new(0, 0, 0));
        // The default seed gets the whole ring; the others sample the ring so
        // the test stays quick while still moving the trees around. Only the
        // mesh matters here: light and probes cannot push the quad count.
        let sample: Vec<ChunkPos> = if seed == default {
            ring
        } else {
            ring.into_iter().step_by(8).collect()
        };
        for chunk in sample {
            let volume = scene.volume(chunk);
            let mesh =
                chunk_content::mesh_resolved_with_apron(&volume, &scene.apron(chunk), atlas_slot);
            assert!(
                mesh.quads <= 16_384,
                "seed {seed}: {chunk:?} emits {} quads, over the u16 index stream",
                mesh.quads
            );
        }
    }
}
