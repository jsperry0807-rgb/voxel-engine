//! M2.5-17: ore depth tiers and vein continuity.
//!
//! The integration scan sits in the all-four band (5,500 m) so one pass can
//! check the metals and the gem. A second scan above the gem ceiling and a
//! third below the coal/iron floor pin the tier boundaries. The flood fill
//! then checks that every wholly interior component is a real vein rather than
//! an isolated speck.

use std::collections::VecDeque;

use voxel_block::{BlockRegistry, BlockRegistryBuilder};
use voxel_worldgen::ores::{MIN_VEIN_BLOCKS, OreField, OreKind};
use voxel_worldgen::splines::SEA_LEVEL;

fn metres(value: i32) -> i64 {
    (value as i64) << 16
}

const N: usize = 64;

fn index(x: usize, y: usize, z: usize) -> usize {
    x + N * (y + N * z)
}

fn scan(seed: u64, y0: usize) -> Vec<Option<OreKind>> {
    let field = OreField::new(seed);
    let mut grid = vec![None; N * N * N];
    for z in 0..N {
        for y in 0..N {
            for x in 0..N {
                grid[index(x, y, z)] =
                    field.at(metres(x as i32), metres((y0 + y) as i32), metres(z as i32));
            }
        }
    }
    grid
}

fn count(grid: &[Option<OreKind>], kind: OreKind) -> usize {
    grid.iter().filter(|cell| **cell == Some(kind)).count()
}

/// 6-connected components, with a flag for a component touching the scan edge.
fn components(grid: &[Option<OreKind>], kind: OreKind) -> Vec<(usize, bool)> {
    let mut seen = vec![false; grid.len()];
    let mut out = Vec::new();
    for start in 0..grid.len() {
        if seen[start] || grid[start] != Some(kind) {
            continue;
        }
        let mut queue = VecDeque::new();
        let mut size = 0;
        let mut boundary = false;
        seen[start] = true;
        queue.push_back(start);
        while let Some(i) = queue.pop_front() {
            size += 1;
            let x = i % N;
            let y = (i / N) % N;
            let z = i / (N * N);
            if x == 0 || y == 0 || z == 0 || x + 1 == N || y + 1 == N || z + 1 == N {
                boundary = true;
            }
            for (dx, dy, dz) in [
                (-1i32, 0i32, 0i32),
                (1, 0, 0),
                (0, -1, 0),
                (0, 1, 0),
                (0, 0, -1),
                (0, 0, 1),
            ] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                let nz = z as i32 + dz;
                if nx < 0 || ny < 0 || nz < 0 || nx >= N as i32 || ny >= N as i32 || nz >= N as i32
                {
                    continue;
                }
                let ni = index(nx as usize, ny as usize, nz as usize);
                if !seen[ni] && grid[ni] == Some(kind) {
                    seen[ni] = true;
                    queue.push_back(ni);
                }
            }
        }
        out.push((size, boundary));
    }
    out
}

fn registry() -> BlockRegistry {
    let blocks = include_str!("../../block/data/blocks.ron");
    let materials = include_str!("../../block/data/materials.gen.ron");
    let mut builder = BlockRegistryBuilder::new();
    builder
        .load_ron_with_materials(blocks, materials)
        .expect("committed blocks and materials load");
    builder.freeze().expect("blocks freeze")
}

#[test]
fn ores_have_metals_and_gems() {
    let grid = scan(1, 5_500);
    let counts: Vec<usize> = OreKind::ALL
        .iter()
        .map(|kind| count(&grid, *kind))
        .collect();
    for (kind, count) in OreKind::ALL.iter().zip(&counts) {
        assert!(*count > 0, "{kind:?} is missing from the all-four band");
    }
    assert!(OreKind::Iron.is_metal() && OreKind::Gold.is_metal());
    assert!(OreKind::Diamond.is_gem());
    assert!(!OreKind::Coal.is_metal() && !OreKind::Coal.is_gem());

    let total: usize = counts.iter().sum();
    let permille = total * 1_000 / (N * N * N);
    assert!(
        (2..=50).contains(&permille),
        "ore density {permille}/1000 is outside the expected band"
    );
}

#[test]
fn ores_veins_are_continuous() {
    let grid = scan(1, 5_500);
    for kind in OreKind::ALL {
        let components = components(&grid, kind);
        let interior: Vec<usize> = components
            .iter()
            .filter(|(_, boundary)| !boundary)
            .map(|(size, _)| *size)
            .collect();
        assert!(!interior.is_empty(), "{kind:?} has no interior vein");
        for size in &interior {
            assert!(
                *size >= MIN_VEIN_BLOCKS,
                "{kind:?} has an interior fragment of {size} blocks"
            );
        }
        let largest = interior.iter().copied().max().unwrap_or(0);
        assert!(
            largest >= 2 * MIN_VEIN_BLOCKS,
            "{kind:?} has no substantial vein (largest {largest})"
        );
        // A vein is many blocks per component; isolated specks would make the
        // ratio close to one.
        let blocks: usize = components.iter().map(|(size, _)| *size).sum();
        assert!(
            blocks >= 4 * components.len(),
            "{kind:?}: {blocks} blocks in {} components",
            components.len()
        );
    }
}

#[test]
fn ores_have_no_surface_gems() {
    // The diamond ceiling is 900 m below sea; 5,800 m is above every possible
    // diamond block, including the capsule ends.
    let above_gems = scan(1, 5_800);
    assert_eq!(count(&above_gems, OreKind::Diamond), 0);

    let field = OreField::new(1);
    assert_eq!(field.at(metres(17), SEA_LEVEL as i64, metres(23)), None);
    assert_eq!(
        field.at(metres(17), (SEA_LEVEL as i64) + (100 << 16), metres(23)),
        None
    );
}

#[test]
fn ores_depth_tiers_are_bounded() {
    // Below the coal and iron floors only gold and diamond remain.
    let deep = scan(1, 4_200);
    assert_eq!(count(&deep, OreKind::Coal), 0);
    assert_eq!(count(&deep, OreKind::Iron), 0);
    assert!(count(&deep, OreKind::Gold) + count(&deep, OreKind::Diamond) > 0);

    // Above the gold ceiling and below the gem ceiling only coal, iron and
    // gold can appear.
    let shallow = scan(1, 6_000);
    assert_eq!(count(&shallow, OreKind::Diamond), 0);
    assert!(count(&shallow, OreKind::Coal) + count(&shallow, OreKind::Iron) > 0);
}

#[test]
fn ores_every_key_resolves() {
    let registry = registry();
    for kind in OreKind::ALL {
        let id = registry
            .id_of(kind.key())
            .unwrap_or_else(|| panic!("{kind:?} key {:?} is not in the manifest", kind.key()));
        assert!(
            registry.face_materials(id).is_some(),
            "{kind:?} has no atlas materials"
        );
    }
}
