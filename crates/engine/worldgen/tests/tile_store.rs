//! M2.5-09: the macro tile store.
//!
//! Two promises are easy to get wrong and are pinned here: the 32 km view fits
//! in a 3 × 3 cache under 1 MB, and sampling across a tile border is seamless.
//! The seam test compares border-crossing samples with the exact bilinear
//! reference, including a midpoint that needs one cell from each tile.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use voxel_worldgen::fixed::{Coord, ONE};
use voxel_worldgen::macro_grid::{CELL_SIZE_M, TILE_CELLS_USIZE, TilePos, WORLD_CELLS};
use voxel_worldgen::tile::{MacroTile, TileError, TileHeader};
use voxel_worldgen::tile_store::{
    Channel, DirectorySource, TileSource, TileStore, VIEW_TILES, tile_at,
};

/// A continuous synthetic field, in 0.25 m units, with no wrap-around.
fn field_height(cell_x: u32, cell_z: u32) -> u16 {
    let x = cell_x as i64;
    let z = cell_z as i64;
    (8_000 + (x * x) / 50 + z * 7) as u16
}

fn field_temperature(cell_x: u32, cell_z: u32) -> u8 {
    ((cell_x * 3 + cell_z * 5) % 256) as u8
}

fn build_tile(pos: TilePos) -> MacroTile {
    let mut tile = MacroTile::new(TileHeader::new(pos, 1, 1, 1, 1)).expect("valid position");
    let (origin_x, origin_z) = pos.origin_cell();
    for z in 0..TILE_CELLS_USIZE {
        for x in 0..TILE_CELLS_USIZE {
            let cell_x = origin_x as u32 + x as u32;
            let cell_z = origin_z as u32 + z as u32;
            let mut cell = tile.cell(x, z);
            cell.height = field_height(cell_x, cell_z);
            cell.temperature = field_temperature(cell_x, cell_z);
            cell.humidity = (cell_x % 200) as u8;
            cell.erosion = (cell_z % 200) as u8;
            tile.set_cell(x, z, cell);
        }
    }
    tile
}

/// A source over an in-memory map that counts loads.
struct CountingSource {
    tiles: HashMap<TilePos, MacroTile>,
    loads: Rc<Cell<usize>>,
}

impl CountingSource {
    fn new(positions: &[TilePos]) -> (Self, Rc<Cell<usize>>) {
        let loads = Rc::new(Cell::new(0));
        let tiles = positions
            .iter()
            .map(|&pos| (pos, build_tile(pos)))
            .collect();
        (
            Self {
                tiles,
                loads: loads.clone(),
            },
            loads,
        )
    }
}

impl TileSource for CountingSource {
    fn load(&mut self, pos: TilePos) -> Result<MacroTile, TileError> {
        self.loads.set(self.loads.get() + 1);
        self.tiles.get(&pos).cloned().ok_or(TileError::Missing {
            x: pos.x(),
            z: pos.z(),
        })
    }
}

/// The cell-centre world coordinate of a macro cell, in Q16.16 metres.
fn centre(cell: u32) -> Coord {
    (cell as i64 * CELL_SIZE_M as i64 + CELL_SIZE_M as i64 / 2) << 16
}

#[test]
fn tile_store_evicts_least_recently_used() {
    let positions: Vec<TilePos> = (0..5).map(|x| TilePos::new(x, 0)).collect();
    let (source, loads) = CountingSource::new(&positions);
    let mut store = TileStore::with_capacity(source, 2);

    store.tile(positions[0]).expect("load");
    store.tile(positions[1]).expect("load");
    assert_eq!(store.resident(), 2);
    assert_eq!(loads.get(), 2);

    // Touch 0 so 1 becomes the least recently used.
    store.tile(positions[0]).expect("hit");
    store.tile(positions[2]).expect("load");
    assert_eq!(store.resident(), 2);
    assert_eq!(loads.get(), 3);

    // 0 survived the eviction.
    store.tile(positions[0]).expect("still resident");
    assert_eq!(loads.get(), 3);

    // 1 was evicted, so asking for it loads again.
    store.tile(positions[1]).expect("reload");
    assert_eq!(loads.get(), 4);
}

#[test]
fn tile_store_view_loads_three_by_three() {
    let mut positions = Vec::new();
    for z in 0..3 {
        for x in 0..3 {
            positions.push(TilePos::new(x, z));
        }
    }
    let (source, loads) = CountingSource::new(&positions);
    let mut store = TileStore::new(source);

    assert_eq!(VIEW_TILES, 9);
    let loaded = store.load_view(TilePos::new(1, 1)).expect("view");
    assert_eq!(loaded, 9);
    assert_eq!(loads.get(), 9);
    assert_eq!(store.resident(), 9);
    assert_eq!(store.resident_bytes(), 9 * 93_750);

    // A second visit is all hits.
    let loaded = store.load_view(TilePos::new(1, 1)).expect("view");
    assert_eq!(loaded, 0);
    assert_eq!(loads.get(), 9);
}

#[test]
fn tile_store_32km_view_holds_at_most_nine_tiles() {
    // A 32 km square centred on a tile corner spans at most 3 × 3 tiles.
    let mut positions = Vec::new();
    for z in 0..3 {
        for x in 0..3 {
            positions.push(TilePos::new(x, z));
        }
    }
    let (source, loads) = CountingSource::new(&positions);
    let mut store = TileStore::new(source);

    // Tile (1,1) starts at 16 km; a ±16 km square around it covers tiles
    // 0..2 in both axes.
    let centre_x = 16_000i64 << 16;
    let centre_z = 16_000i64 << 16;
    for step in -32..=32i64 {
        let offset = (step * 500) << 16;
        store
            .sample(centre_x + offset, centre_z, Channel::Height)
            .expect("sample");
        store
            .sample(centre_x, centre_z + offset, Channel::Height)
            .expect("sample");
    }

    assert!(store.resident() <= 9, "resident {}", store.resident());
    assert!(loads.get() <= 9, "loads {}", loads.get());
    assert!(
        store.resident_bytes() < 1_000_000,
        "a 3x3 view must fit in 1 MB, got {} bytes",
        store.resident_bytes()
    );
}

#[test]
fn tile_store_has_no_seam_at_tile_borders() {
    // Tiles (0,0) and (1,0). Cell 124 is the last of tile 0 and cell 125 the
    // first of tile 1; the midpoint between their centres needs both tiles.
    let (source, _loads) = CountingSource::new(&[TilePos::new(0, 0), TilePos::new(1, 0)]);
    let mut store = TileStore::new(source);

    let z = centre(40);

    // Exact at the two cell centres on both sides of the border.
    for cell in [124u32, 125] {
        let expected = (field_height(cell, 40) as i32) << 14; // units -> metres Q16.16
        assert_eq!(
            store
                .sample(centre(cell), z, Channel::Height)
                .expect("sample"),
            expected,
            "cell centre {cell}"
        );
    }

    // Midway between the centres: the average of the two cell values, in
    // Q16.16 metres. This is the sample a naive per-tile store would seam.
    let midpoint = centre(124) + (ONE as i64 * CELL_SIZE_M as i64 / 2);
    let low = field_height(124, 40) as i64;
    let high = field_height(125, 40) as i64;
    let expected = ((low + high) << 13) as i32;
    assert_eq!(
        store.sample(midpoint, z, Channel::Height).expect("sample"),
        expected,
        "midpoint across the border"
    );

    // Both tiles had to be loaded to answer it.
    assert_eq!(store.resident(), 2);
}

#[test]
fn tile_store_samples_every_channel() {
    let (source, _loads) = CountingSource::new(&[TilePos::new(0, 0)]);
    let mut store = TileStore::new(source);
    let x = centre(33);
    let z = centre(70);

    assert_eq!(
        store.sample(x, z, Channel::Height).expect("height"),
        (field_height(33, 70) as i32) << 14
    );
    assert_eq!(
        store
            .sample(x, z, Channel::Temperature)
            .expect("temperature"),
        (field_temperature(33, 70) as i32) << 16
    );
    assert_eq!(
        store.sample(x, z, Channel::Humidity).expect("humidity"),
        (33i32) << 16
    );
    assert_eq!(
        store.sample(x, z, Channel::Erosion).expect("erosion"),
        (70i32) << 16
    );

    // Interpolating a linear channel halfway between cells averages it.
    let halfway = centre(33) + (ONE as i64 * CELL_SIZE_M as i64 / 2);
    let expected = (field_temperature(33, 70) as i32 + field_temperature(34, 70) as i32) << 15;
    assert_eq!(
        store
            .sample(halfway, z, Channel::Temperature)
            .expect("temperature"),
        expected
    );
}

#[test]
fn tile_store_clamps_at_the_world_edge() {
    let corners = [
        TilePos::new(0, 0),
        TilePos::new(63, 0),
        TilePos::new(0, 63),
        TilePos::new(63, 63),
    ];
    let (source, loads) = CountingSource::new(&corners);
    let mut store = TileStore::new(source);

    let world = (WORLD_CELLS as i64 * CELL_SIZE_M as i64) << 16;
    for (x, z) in [
        (0, 0),
        (world - 1, 0),
        (0, world - 1),
        (world - 1, world - 1),
    ] {
        store.sample(x, z, Channel::Height).expect("edge sample");
    }
    // Only in-world corner tiles were touched; no out-of-world load.
    assert_eq!(loads.get(), 4);
    assert_eq!(store.resident(), 4);

    // Negative coordinates clamp to the first cell rather than wrapping.
    let at_origin = store.sample(0, 0, Channel::Height).expect("origin");
    assert_eq!(at_origin, (field_height(0, 0) as i32) << 14);
}

#[test]
fn tile_store_reports_source_errors() {
    struct Failing;
    impl TileSource for Failing {
        fn load(&mut self, pos: TilePos) -> Result<MacroTile, TileError> {
            Err(TileError::Missing {
                x: pos.x(),
                z: pos.z(),
            })
        }
    }

    let mut store = TileStore::new(Failing);
    assert_eq!(
        store.tile(TilePos::new(2, 3)).unwrap_err(),
        TileError::Missing { x: 2, z: 3 }
    );
    assert_eq!(store.resident(), 0, "a failed load must not be cached");
    assert_eq!(
        store.sample(0, 0, Channel::Height).unwrap_err(),
        TileError::Missing { x: 0, z: 0 }
    );
}

#[test]
fn tile_store_directory_source_reads_written_tiles() {
    let directory =
        std::env::temp_dir().join(format!("voxel-worldgen-tile-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create temp dir");

    let pos = TilePos::new(4, 5);
    let tile = build_tile(pos);
    let path = directory.join("4_5.vxt");
    std::fs::write(&path, tile.to_bytes()).expect("write tile");

    let mut store = TileStore::new(DirectorySource::new(&directory));
    let loaded = store.tile(pos).expect("load from directory");
    assert_eq!(loaded.header().seed, 1);
    assert_eq!(loaded.header().pos, pos);
    assert_eq!(loaded.cell(0, 0), tile.cell(0, 0));

    // A missing neighbour is reported as an IO error, not a panic.
    let error = store.tile(TilePos::new(9, 9)).unwrap_err();
    assert!(matches!(error, TileError::Io { .. }), "{error}");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn tile_store_tile_lookup_matches_the_cell_grid() {
    let cell_size = (CELL_SIZE_M as i64) << 16;
    assert_eq!(tile_at(0, 0), TilePos::new(0, 0));
    assert_eq!(
        tile_at(cell_size * 124, cell_size * 124),
        TilePos::new(0, 0)
    );
    assert_eq!(tile_at(cell_size * 125, 0), TilePos::new(1, 0));
    assert_eq!(
        tile_at(cell_size * 7_999, cell_size * 7_999),
        TilePos::new(63, 63)
    );
    // Negative positions clamp to the first tile.
    assert_eq!(tile_at(-1, -1), TilePos::new(0, 0));
}

#[test]
fn tile_store_capacity_is_validated() {
    let (source, _loads) = CountingSource::new(&[]);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            TileStore::with_capacity(source, 0)
        }))
        .is_err()
    );
}

#[test]
fn tile_store_view_at_the_world_corner_stays_inside() {
    let mut positions = Vec::new();
    for z in 0..2 {
        for x in 0..2 {
            positions.push(TilePos::new(x, z));
        }
    }
    let (source, loads) = CountingSource::new(&positions);
    let mut store = TileStore::new(source);
    let loaded = store.load_view(TilePos::new(0, 0)).expect("corner view");
    assert_eq!(loaded, 4, "only the in-world quadrant loads");
    assert_eq!(loads.get(), 4);
}

#[test]
fn tile_store_frame_is_reported_as_a_directory_path() {
    let source = DirectorySource::new(PathBuf::from("/tmp/example"));
    assert_eq!(source.directory(), std::path::Path::new("/tmp/example"));
}
