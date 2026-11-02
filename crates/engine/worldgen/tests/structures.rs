//! M2.5-19: structure anchors, prefab keys and terrain-fitted stamps.

use std::collections::{HashSet, VecDeque};

use voxel_block::{BlockRegistry, BlockRegistryBuilder};
use voxel_worldgen::structures::{
    PREFAB_KEYS, Prefab, STRUCTURE_CELL_M, STRUCTURE_CELLS, Stamp, Structure, StructureGrid,
    StructureKind,
};

fn registry() -> BlockRegistry {
    let blocks = include_str!("../../block/data/blocks.ron");
    let materials = include_str!("../../block/data/materials.gen.ron");
    let mut builder = BlockRegistryBuilder::new();
    builder
        .load_ron_with_materials(blocks, materials)
        .expect("committed blocks and materials load");
    builder.freeze().expect("blocks freeze")
}

fn position(block: &voxel_worldgen::structures::PlacedBlock) -> (i32, i32, i32) {
    (block.x, block.y, block.z)
}

fn assert_connected(stamp: &Stamp, start: (i32, i32, i32)) {
    let cells: HashSet<(i32, i32, i32)> = stamp.blocks.iter().map(position).collect();
    assert!(cells.contains(&start), "start block is missing");
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([start]);
    seen.insert(start);
    while let Some((x, y, z)) = queue.pop_front() {
        for (dx, dy, dz) in [
            (-1i32, 0i32, 0i32),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ] {
            let next = (x + dx, y + dy, z + dz);
            if cells.contains(&next) && seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    assert_eq!(
        seen.len(),
        cells.len(),
        "the stamp has {} disconnected blocks",
        cells.len() - seen.len()
    );
}

#[test]
fn structures_most_cells_are_empty() {
    let grid = StructureGrid::new(1);
    let total = (STRUCTURE_CELLS as usize) * (STRUCTURE_CELLS as usize);
    let structures: Vec<_> = grid.iter().collect();
    assert!(!structures.is_empty());
    assert!(structures.len() < total / 2, "not most cells empty");
    let mut seen = [false; 4];
    for structure in &structures {
        seen[structure.kind as usize] = true;
    }
    assert!(seen.iter().all(|seen| *seen), "missing a structure kind");
}

#[test]
fn structures_anchors_stay_on_the_one_to_four_km_grid() {
    let grid = StructureGrid::new(2);
    for cell_z in (0..STRUCTURE_CELLS).step_by(3) {
        for cell_x in 0..STRUCTURE_CELLS {
            let (x, _) = grid.anchor(cell_x, cell_z);
            if cell_x + 1 < STRUCTURE_CELLS {
                let (next, _) = grid.anchor(cell_x + 1, cell_z);
                let gap = (next - x) >> 16;
                assert!((1_000..=4_000).contains(&gap), "x gap {gap}");
            }
            let (_, z) = grid.anchor(cell_x, cell_z);
            if cell_z + 1 < STRUCTURE_CELLS {
                let (_, next) = grid.anchor(cell_x, cell_z + 1);
                let gap = (next - z) >> 16;
                assert!((1_000..=4_000).contains(&gap), "z gap {gap}");
            }
        }
    }
    assert_eq!(STRUCTURE_CELL_M, 2_000);
}

#[test]
fn structures_prefabs_use_the_m25_31_keys() {
    let registry = registry();
    let mut used = HashSet::new();
    for kind in StructureKind::ALL {
        let prefab = Prefab::new(kind);
        let mut positions = HashSet::new();
        let mut has_m25 = false;
        for block in &prefab.blocks {
            assert!(
                positions.insert((block.dx, block.dy, block.dz)),
                "{kind:?} repeats a block position"
            );
            let id = registry
                .id_of(block.key)
                .unwrap_or_else(|| panic!("{kind:?} uses unknown key {:?}", block.key));
            assert!(
                registry.face_materials(id).is_some(),
                "{kind:?} key {:?} has no atlas materials",
                block.key
            );
            if PREFAB_KEYS.contains(&block.key) {
                has_m25 = true;
                used.insert(block.key);
            }
        }
        assert!(has_m25, "{kind:?} has no M2.5-31 key");
    }
    for key in PREFAB_KEYS {
        assert!(used.contains(key), "{key:?} is never used by a prefab");
    }
    let cobble = registry
        .id_of("voxel:cobblestone")
        .expect("foundation key must exist");
    assert!(registry.face_materials(cobble).is_some());
}

#[test]
fn structures_stamps_fit_terrain_without_floating_foundations() {
    // An uneven surface, but not so steep that the prefab sits in a hole.
    let height = |x: i32, z: i32| 50 + (x * 7).rem_euclid(17) + (z * 5).rem_euclid(13);
    for kind in [
        StructureKind::Tower,
        StructureKind::Ruin,
        StructureKind::BridgeRoad,
    ] {
        let structure = Structure {
            cell: [0, 0],
            x: 100,
            z: 200,
            surface: 60,
            kind,
        };
        let stamp = structure.stamp(height);
        assert!(!stamp.blocks.is_empty(), "{kind:?} placed nothing");

        let prefab = Prefab::new(kind);
        for &(dx, dz) in &prefab.foundation {
            let world_x = structure.x + dx;
            let world_z = structure.z + dz;
            let ground = height(world_x, world_z);
            let lowest = stamp
                .blocks
                .iter()
                .filter(|block| block.x == world_x && block.z == world_z)
                .map(|block| block.y)
                .min()
                .unwrap_or_else(|| panic!("{kind:?} has no block at ({world_x}, {world_z})"));
            assert_eq!(
                lowest,
                ground + 1,
                "{kind:?} foundation floats at ({world_x}, {world_z})"
            );
        }

        for block in &stamp.blocks {
            assert!(
                block.y > height(block.x, block.z),
                "{kind:?} block at ({}, {}, {}) is inside the terrain",
                block.x,
                block.y,
                block.z
            );
        }

        let start = stamp
            .blocks
            .iter()
            .find(|block| block.y == stamp.blocks.iter().map(|b| b.y).min().unwrap())
            .map(position)
            .expect("a lowest block");
        assert_connected(&stamp, start);
    }
}

#[test]
fn structures_bridge_spans_the_valley_on_two_supports() {
    let structure = Structure {
        cell: [0, 0],
        x: 100,
        z: 100,
        surface: 30,
        kind: StructureKind::BridgeRoad,
    };
    let height = |x: i32, z: i32| {
        if (x - 100).abs() <= 5 {
            20
        } else {
            30 + z.rem_euclid(2)
        }
    };
    let stamp = structure.stamp(height);
    assert!(stamp.foundation_blocks > 0);

    // The deck crosses the valley above it.
    assert!(
        stamp
            .blocks
            .iter()
            .any(|block| (95..=105).contains(&block.x) && block.y >= 31),
        "no deck over the valley"
    );
    // The middle span has no block down in the valley floor.
    assert!(
        stamp
            .blocks
            .iter()
            .all(|block| !(95..=105).contains(&block.x) || block.y > 20),
        "the bridge touches the valley floor"
    );

    let prefab = Prefab::new(StructureKind::BridgeRoad);
    for &(dx, dz) in &prefab.foundation {
        let world_x = structure.x + dx;
        let world_z = structure.z + dz;
        let ground = height(world_x, world_z);
        let lowest = stamp
            .blocks
            .iter()
            .filter(|block| block.x == world_x && block.z == world_z)
            .map(|block| block.y)
            .min()
            .expect("support column");
        assert_eq!(
            lowest,
            ground + 1,
            "support floats at ({world_x}, {world_z})"
        );
    }

    let start = stamp
        .blocks
        .iter()
        .find(|block| block.y == stamp.blocks.iter().map(|b| b.y).min().unwrap())
        .map(position)
        .expect("a support block");
    assert_connected(&stamp, start);
}

#[test]
fn structures_dungeon_is_buried_and_connected() {
    let structure = Structure {
        cell: [0, 0],
        x: 10,
        z: 20,
        surface: 80,
        kind: StructureKind::Dungeon,
    };
    let stamp = structure.stamp(|_, _| 80);
    assert!(stamp.buried);
    assert_eq!(stamp.foundation_blocks, 0);
    assert!(stamp.blocks.iter().all(|block| block.y < 80 && block.y > 0));
    let start = position(&stamp.blocks[0]);
    assert_connected(&stamp, start);
}
