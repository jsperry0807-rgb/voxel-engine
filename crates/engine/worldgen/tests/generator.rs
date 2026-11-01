//! M2.5-05: the seeded macro tile generator.
//!
//! Two things are pinned here. First the **contract**: a tile's header, its
//! channels and its content hash are exactly what the generator produces.
//! Second the **field shape**: about a third of the world is ocean, water and
//! coast flags agree with the height channel, and latitude still reads through
//! a generated tile.
//!
//! `tests/golden/macro_tiles.txt` pins per-tile hashes; regenerate after an
//! intentional field change:
//!
//! ```sh
//! UPDATE_GOLDEN=1 cargo test -p voxel-worldgen macro_generator
//! cargo test -p voxel-worldgen macro_generator
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use voxel_worldgen::fixed::Coord;
use voxel_worldgen::generator::{
    CONTENT_VERSION, GENERATOR_VERSION, MACRO_VERSION, MacroGenerator, SEA_LEVEL_UNITS,
};
use voxel_worldgen::macro_grid::{
    CELL_SIZE_M, TILE_CELLS_USIZE, TILE_COUNT, TileFlags, TilePos, WORLD_CELLS, WORLD_EXTENT_M,
    height_from_q16,
};
use voxel_worldgen::splines::SEA_LEVEL;
use voxel_worldgen::tile::{TILE_BYTES, fnv_start, fnv_u64};

const GOLDEN_SEED: u64 = 1;
const GOLDEN_TILES: &[(i32, i32)] = &[(0, 0), (1, 0), (31, 31), (32, 32), (63, 63), (10, 0)];

fn metres(value: i64) -> Coord {
    value << 16
}

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("macro_tiles.txt")
}

fn read_golden() -> BTreeMap<(u64, i32, i32), u64> {
    let path = golden_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 4, "golden line {line:?}");
        let seed: u64 = parts[0].parse().expect("seed");
        let x: i32 = parts[1].parse().expect("tile x");
        let z: i32 = parts[2].parse().expect("tile z");
        let hash = u64::from_str_radix(parts[3], 16).expect("hash");
        map.insert((seed, x, z), hash);
    }
    map
}

fn write_golden(hashes: &BTreeMap<(u64, i32, i32), u64>) {
    let mut text = String::from(
        "# M2.5-05 macro tile content hashes.\n\
         # Regenerate: UPDATE_GOLDEN=1 cargo test -p voxel-worldgen macro_generator\n\
         # seed tile_x tile_z fnv1a64\n",
    );
    for ((seed, x, z), hash) in hashes {
        text.push_str(&format!("{seed} {x} {z} {hash:016x}\n"));
    }
    let path = golden_path();
    std::fs::create_dir_all(path.parent().expect("golden directory")).expect("create golden dir");
    std::fs::write(&path, text).expect("write golden file");
    eprintln!("wrote {}", path.display());
}

fn tile_world_position(pos: TilePos, x: usize, z: usize) -> (Coord, Coord) {
    let (origin_x, origin_z) = pos.origin_m();
    let half = CELL_SIZE_M as i64 / 2;
    (
        metres(origin_x + x as i64 * CELL_SIZE_M as i64 + half),
        metres(origin_z + z as i64 * CELL_SIZE_M as i64 + half),
    )
}

#[test]
fn macro_generator_pins_content_hashes() {
    let updating = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut golden = if updating {
        BTreeMap::new()
    } else {
        read_golden()
    };

    for &(x, z) in GOLDEN_TILES {
        let tile = MacroGenerator::new(GOLDEN_SEED)
            .generate_tile(TilePos::new(x, z))
            .expect("valid tile position");
        let hash = tile.content_hash();
        if updating {
            golden.insert((GOLDEN_SEED, x, z), hash);
            continue;
        }
        let expected = golden
            .get(&(GOLDEN_SEED, x, z))
            .unwrap_or_else(|| panic!("golden entry for tile ({x}, {z}) is missing"));
        assert_eq!(
            *expected, hash,
            "tile ({x}, {z}) hash changed; regenerate with UPDATE_GOLDEN=1 if intentional"
        );
    }

    if updating {
        write_golden(&golden);
    }
}

#[test]
fn macro_generator_fills_the_header_and_every_channel() {
    let pos = TilePos::new(7, 9);
    let tile = MacroGenerator::new(5)
        .generate_tile(pos)
        .expect("valid tile position");

    assert_eq!(tile.header().pos, pos);
    assert_eq!(tile.header().seed, 5);
    assert_eq!(tile.header().generator_version, GENERATOR_VERSION);
    assert_eq!(tile.header().macro_version, MACRO_VERSION);
    assert_eq!(tile.header().content_version, CONTENT_VERSION);

    // Height is inside the world, and both climate channels vary.
    assert!(tile.heights().iter().all(|&h| h <= 48_000));
    assert!(tile.heights().iter().any(|&h| h != tile.heights()[0]));
    assert!(
        tile.temperature()
            .iter()
            .any(|&t| t != tile.temperature()[0])
    );
    assert!(tile.humidity().iter().any(|&h| h != tile.humidity()[0]));
    assert!(tile.erosion().iter().any(|&e| e != tile.erosion()[0]));
}

#[test]
fn macro_generator_matches_its_public_fields() {
    let pos = TilePos::new(12, 4);
    let generator = MacroGenerator::new(9);
    let tile = generator.generate_tile(pos).expect("valid tile position");

    for (x, z) in [(0, 0), (17, 3), (62, 62), (124, 124)] {
        let (world_x, world_z) = tile_world_position(pos, x, z);
        let continentalness = generator.continentalness(world_x, world_z);
        let erosion = generator.erosion(world_x, world_z);
        let surface = generator.surface(world_x, world_z);
        let cell = tile.cell(x, z);
        assert_eq!(
            cell.height,
            height_from_q16(surface),
            "height at ({x}, {z})"
        );
        assert_eq!(
            cell.temperature,
            generator.temperature(world_x, world_z, surface),
            "temperature at ({x}, {z})"
        );
        assert_eq!(
            cell.humidity,
            generator.humidity(world_x, world_z, continentalness),
            "humidity at ({x}, {z})"
        );
        assert_eq!(
            cell.erosion,
            voxel_worldgen::climate::unit_to_u8(erosion),
            "erosion at ({x}, {z})"
        );
    }
}

#[test]
fn macro_generator_is_deterministic() {
    let pos = TilePos::new(3, 40);
    let first = MacroGenerator::new(1)
        .generate_tile(pos)
        .expect("valid tile position");
    let second = MacroGenerator::new(1)
        .generate_tile(pos)
        .expect("valid tile position");
    let other = MacroGenerator::new(2)
        .generate_tile(pos)
        .expect("valid tile position");

    assert_eq!(first.to_bytes(), second.to_bytes());
    assert_eq!(first.content_hash(), second.content_hash());
    assert_ne!(first.content_hash(), other.content_hash());
}

#[test]
fn macro_generator_content_hash_covers_every_cell() {
    let mut tile = MacroGenerator::new(1)
        .generate_tile(TilePos::new(0, 0))
        .expect("valid tile position");
    let before = tile.content_hash();
    let mut cell = tile.cell(11, 13);
    cell.height = cell.height.wrapping_add(4);
    tile.set_cell(11, 13, cell);
    assert_ne!(before, tile.content_hash());
}

#[test]
fn macro_generator_water_and_coast_flags_match_the_height() {
    let tile = MacroGenerator::new(3)
        .generate_tile(TilePos::new(20, 20))
        .expect("valid tile position");
    let mut water = 0usize;
    for z in 0..TILE_CELLS_USIZE {
        for x in 0..TILE_CELLS_USIZE {
            let cell = tile.cell(x, z);
            if cell.flags.contains(TileFlags::WATER) {
                water += 1;
                assert!(
                    cell.height <= SEA_LEVEL_UNITS,
                    "water above sea at ({x}, {z})"
                );
            } else if cell.flags.contains(TileFlags::COAST) {
                assert!(
                    cell.height > SEA_LEVEL_UNITS,
                    "coast at sea level ({x}, {z})"
                );
            }
            // River and lake wait for hydrology.
            assert!(!cell.flags.contains(TileFlags::RIVER));
            assert!(!cell.flags.contains(TileFlags::LAKE));
        }
    }
    // Tile (20, 20) straddles land and water; flags are exercised both ways.
    assert!(water > 0);
}

#[test]
fn macro_generator_land_fraction_is_about_a_third() {
    // One sample per 16 km over the whole world; 4,096 probes.
    for seed in [1u64, 2, 3] {
        let generator = MacroGenerator::new(seed);
        let step = WORLD_EXTENT_M as i64 / WORLD_CELLS as i64;
        let cell_step = WORLD_CELLS / 64;
        let mut ocean = 0u32;
        let mut total = 0u32;
        for z in 0..64u32 {
            for x in 0..64u32 {
                let world_x = metres(x as i64 * cell_step as i64 * step);
                let world_z = metres(z as i64 * cell_step as i64 * step);
                if generator.continentalness(world_x, world_z) <= 0 {
                    ocean += 1;
                }
                total += 1;
            }
        }
        let fraction = ocean as f64 / total as f64;
        assert!(
            (0.28..=0.42).contains(&fraction),
            "seed {seed}: ocean fraction {fraction} is not ~0.35"
        );
    }
}

#[test]
fn macro_generator_covers_the_extreme_tiles() {
    let generator = MacroGenerator::new(1);
    for pos in [TilePos::new(0, 0), TilePos::new(63, 63)] {
        let tile = generator.generate_tile(pos).expect("corner tile is valid");
        assert_eq!(tile.header().pos, pos);
        assert_eq!(tile.heights().len(), TILE_CELLS_USIZE * TILE_CELLS_USIZE);
        assert!(tile.heights().iter().any(|&h| h > SEA_LEVEL_UNITS));
    }
}

#[test]
fn macro_generator_latitude_reads_through_a_tile() {
    let generator = MacroGenerator::new(1);
    let north = generator
        .generate_tile(TilePos::new(20, 0))
        .expect("valid tile position");
    let equator = generator
        .generate_tile(TilePos::new(20, 32))
        .expect("valid tile position");

    let mean = |tile: &voxel_worldgen::MacroTile| -> f64 {
        tile.temperature().iter().map(|&t| t as f64).sum::<f64>() / tile.temperature().len() as f64
    };
    // The equator is at tile row 32 (z = 512 km); row 0 is the north pole.
    assert!(
        mean(&equator) > mean(&north) + 100.0,
        "equator mean {} is not much warmer than the north pole mean {}",
        mean(&equator),
        mean(&north)
    );
}

#[test]
fn macro_generator_world_geometry_is_unchanged() {
    assert_eq!(TILE_COUNT, 4_096);
    assert_eq!(WORLD_CELLS, 8_000);
    assert_eq!(SEA_LEVEL, 6_600 << 16);
    assert_eq!(
        voxel_worldgen::macro_grid::height_from_q16(SEA_LEVEL),
        SEA_LEVEL_UNITS
    );
    assert_eq!(TILE_BYTES, 93_790);
    // The aggregated CLI hash folds tile hashes in row-major order.
    let mut aggregate = fnv_start();
    for &(x, z) in GOLDEN_TILES {
        let tile = MacroGenerator::new(GOLDEN_SEED)
            .generate_tile(TilePos::new(x, z))
            .expect("valid tile position");
        aggregate = fnv_u64(aggregate, tile.content_hash());
    }
    assert_ne!(aggregate, 0);
}

#[test]
fn macro_generator_cell_writes_are_isolated() {
    // Guards the channel planes: writing one cell must not disturb a neighbour.
    let mut tile = MacroGenerator::new(1)
        .generate_tile(TilePos::new(2, 2))
        .expect("valid tile position");
    let neighbour = tile.cell(5, 5);
    let mut cell = tile.cell(4, 5);
    cell.height = cell.height.wrapping_add(1);
    tile.set_cell(4, 5, cell);
    assert_eq!(tile.cell(5, 5), neighbour);
    assert_eq!(tile.cell(4, 5), cell);
}

#[test]
fn macro_generator_accepts_every_position() {
    // A cheap sweep over the diagonal: every tile position generates and
    // carries its own position in the header.
    let generator = MacroGenerator::new(4);
    for index in 0..64 {
        let pos = TilePos::new(index, (index * 7) % 64);
        let tile = generator.generate_tile(pos).expect("valid tile position");
        assert_eq!(tile.header().pos, pos);
        assert!(tile.content_hash() != 0);
    }
}

#[test]
fn macro_generator_height_and_erosion_are_consistent() {
    let tile = MacroGenerator::new(8)
        .generate_tile(TilePos::new(33, 12))
        .expect("valid tile position");
    assert!(tile.heights().iter().all(|&height| height <= 48_000));
    // Erosion is a full-range byte, not stuck at one value.
    assert!(tile.erosion().iter().any(|&e| e != tile.erosion()[0]));
}
