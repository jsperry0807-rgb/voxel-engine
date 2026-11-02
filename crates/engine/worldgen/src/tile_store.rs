//! The macro tile store: streaming, an LRU cache, and edge-safe sampling.
//!
//! The design keeps a 32 km view resident, which spans at most a 3 × 3 group of
//! 16 km tiles (one border tile of slack), under 1 MB of payload. [`TileStore`]
//! owns that cache:
//!
//! - a [`TileSource`] loads one tile on demand (memory, a directory of
//!   `.vxt` files, or anything else);
//! - the least recently used tile is evicted when the cache is full;
//! - [`TileStore::load_view`] warms the 3 × 3 neighbourhood of a tile;
//! - [`TileStore::sample`] bilinearly interpolates a channel at a world
//!   position, **crossing tile borders seamlessly** because it reads each
//!   corner from whichever tile owns it.
//!
//! # Sampling
//!
//! Cell values sit at the centres of the 128 m cells. A world position is
//! converted to a fractional cell coordinate `(x / 128 m) - 0.5` and
//! interpolated between the four surrounding centres. Positions outside the
//! world clamp to the boundary cells, so edge samples never ask for a tile
//! that does not exist.
//!
//! Returned values are Q16.16:
//!
//! | Channel | Unit |
//! |---|---|
//! | `Height` | metres (a cell value of 26,400 units is 6,600 m) |
//! | `Temperature`, `Humidity`, `Erosion` | the channel value, 0–255 |
//!
//! # Seams
//!
//! A seam would be a discontinuity where two tiles meet. The store never
//! samples a tile in isolation: the four corners of every bilinear step are
//! fetched by world cell coordinate, so the result is exactly the field the
//! tiles store, independent of which tile each corner came from. The test
//! `tile_store_has_no_seam_at_tile_borders` compares a border-crossing line
//! against the analytic bilinear reference and requires exact equality.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::fixed::{Coord, ONE};
use crate::macro_grid::{CELL_SIZE_M, TILE_CELLS, TilePos, WORLD_CELLS};
use crate::tile::{MacroTile, TileError};

/// Tiles in a 3 × 3 view: the 32 km view plus a border tile of slack.
pub const VIEW_TILES: usize = 9;

/// A source of macro tiles.
pub trait TileSource {
    /// Load one tile. The store calls this only on a cache miss.
    fn load(&mut self, pos: TilePos) -> Result<MacroTile, TileError>;
}

/// A source that reads `<directory>/<x>_<z>.vxt`, the format `macro --out`
/// writes.
#[derive(Clone, Debug)]
pub struct DirectorySource {
    directory: PathBuf,
}

impl DirectorySource {
    /// Read tiles from `directory`.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// The directory this source reads.
    pub fn directory(&self) -> &std::path::Path {
        &self.directory
    }
}

impl TileSource for DirectorySource {
    fn load(&mut self, pos: TilePos) -> Result<MacroTile, TileError> {
        let path = self.directory.join(format!("{}_{}.vxt", pos.x(), pos.z()));
        let bytes = std::fs::read(&path).map_err(|error| TileError::Io {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        MacroTile::from_bytes(&bytes)
    }
}

/// A macro channel that can be sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Surface elevation; sampled value is Q16.16 metres.
    Height,
    /// Temperature, 0 coldest; sampled value is Q16.16 channel units.
    Temperature,
    /// Humidity, 0 driest; Q16.16 channel units.
    Humidity,
    /// Erosion, 0 least eroded; Q16.16 channel units.
    Erosion,
}

struct Entry {
    tile: MacroTile,
    used: u64,
}

/// An LRU cache of macro tiles.
pub struct TileStore<S> {
    source: S,
    capacity: usize,
    clock: u64,
    tiles: HashMap<TilePos, Entry>,
}

impl<S: TileSource> TileStore<S> {
    /// A store holding one 3 × 3 view.
    pub fn new(source: S) -> Self {
        Self::with_capacity(source, VIEW_TILES)
    }

    /// A store with an explicit tile capacity.
    ///
    /// # Panics
    ///
    /// If `capacity` is zero.
    pub fn with_capacity(source: S, capacity: usize) -> Self {
        assert!(capacity > 0, "a tile store needs at least one slot");
        Self {
            source,
            capacity,
            clock: 0,
            tiles: HashMap::new(),
        }
    }

    /// Loaded tiles.
    pub fn resident(&self) -> usize {
        self.tiles.len()
    }

    /// Payload bytes held by the cache, excluding allocator overhead.
    ///
    /// A tile payload is 93,750 bytes, so a full 3 × 3 view is 843,750 bytes —
    /// under the design's 1 MB budget.
    pub fn resident_bytes(&self) -> usize {
        self.tiles.len() * crate::macro_grid::CELLS_PER_TILE * crate::tile::CELL_BYTES
    }

    /// Capacity in tiles.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The source, for inspection.
    pub fn source(&self) -> &S {
        &self.source
    }

    /// Borrow a tile, loading it and updating the LRU order on a miss.
    pub fn tile(&mut self, pos: TilePos) -> Result<&MacroTile, TileError> {
        if !self.tiles.contains_key(&pos) {
            let tile = self.source.load(pos)?;
            if self.tiles.len() >= self.capacity {
                let victim = self
                    .tiles
                    .iter()
                    .min_by_key(|(_, entry)| entry.used)
                    .map(|(pos, _)| *pos)
                    .expect("a full cache has a victim");
                self.tiles.remove(&victim);
            }
            self.tiles.insert(
                pos,
                Entry {
                    tile,
                    used: self.clock,
                },
            );
        }
        self.clock += 1;
        let used = self.clock;
        let entry = self.tiles.get_mut(&pos).expect("tile is resident");
        entry.used = used;
        Ok(&entry.tile)
    }

    /// Load the 3 × 3 tiles around `centre`, clamped to the world.
    ///
    /// Returns the number of tiles that were not already resident.
    pub fn load_view(&mut self, centre: TilePos) -> Result<usize, TileError> {
        let mut loaded = 0;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = TilePos::new(centre.x() + dx, centre.z() + dz);
                if !pos.is_inside_world() {
                    continue;
                }
                if !self.tiles.contains_key(&pos) {
                    loaded += 1;
                }
                self.tile(pos)?;
            }
        }
        Ok(loaded)
    }

    /// The raw channel value of one cell, without interpolation.
    fn cell_value(&mut self, x: u32, z: u32, channel: Channel) -> Result<i32, TileError> {
        let pos = TilePos::containing_cell(x, z);
        let tile = self.tile(pos)?;
        let cell = tile.cell(TilePos::local_cell(x), TilePos::local_cell(z));
        Ok(match channel {
            Channel::Height => cell.height as i32,
            Channel::Temperature => cell.temperature as i32,
            Channel::Humidity => cell.humidity as i32,
            Channel::Erosion => cell.erosion as i32,
        })
    }

    /// Bilinearly sample a channel at a Q16.16 world position.
    ///
    /// The result is Q16.16 in the channel's unit: metres for
    /// [`Channel::Height`], channel units for the rest.
    pub fn sample(&mut self, x: Coord, z: Coord, channel: Channel) -> Result<i32, TileError> {
        let (cell_x, tx) = lattice(x, WORLD_CELLS);
        let (cell_z, tz) = lattice(z, WORLD_CELLS);
        let x1 = (cell_x + 1).min(WORLD_CELLS as usize - 1);
        let z1 = (cell_z + 1).min(WORLD_CELLS as usize - 1);

        let v00 = self.cell_value(cell_x as u32, cell_z as u32, channel)? as i64;
        let v10 = self.cell_value(x1 as u32, cell_z as u32, channel)? as i64;
        let v01 = self.cell_value(cell_x as u32, z1 as u32, channel)? as i64;
        let v11 = self.cell_value(x1 as u32, z1 as u32, channel)? as i64;
        let value = bilinear(v00, v10, v01, v11, tx, tz);

        Ok(match channel {
            // A height unit is 0.25 m, so units in Q16.16 divided by four are
            // metres in Q16.16.
            Channel::Height => (value >> 2) as i32,
            _ => value as i32,
        })
    }
}

/// Split a Q16.16 world coordinate into a cell index and interpolation
/// fraction, clamping to the world.
fn lattice(coordinate: Coord, cells: u32) -> (usize, i32) {
    assert!(
        cells >= 2,
        "interpolation needs at least two cells per axis"
    );
    // Cells are 128 m, and values sit at cell centres, so the fractional cell
    // coordinate is `coordinate / 128 m - 0.5`.
    let maximum = (cells as i64 - 1) << 16;
    let position = ((coordinate >> 7) - (ONE as i64 / 2)).clamp(0, maximum);
    let mut index = (position >> 16) as usize;
    let mut fraction = (position & 0xFFFF) as i32;
    if index >= cells as usize - 1 {
        index = cells as usize - 2;
        fraction = ONE;
    }
    (index, fraction)
}

/// Q16.16 linear interpolation of two Q16.16 values.
fn lerp_q16(a: i64, b: i64, t: i32) -> i64 {
    a + (((b - a) * t as i64) >> 16)
}

/// Bilinear interpolation of integer channel values with Q16.16 weights.
///
/// Returns Q16.16: a corner value `v` contributes `v << 16`.
fn bilinear(v00: i64, v10: i64, v01: i64, v11: i64, tx: i32, tz: i32) -> i64 {
    let top = lerp_q16(v00 << 16, v10 << 16, tx);
    let bottom = lerp_q16(v01 << 16, v11 << 16, tx);
    lerp_q16(top, bottom, tz)
}

/// The tile that contains a Q16.16 world position.
pub fn tile_at(x: Coord, z: Coord) -> TilePos {
    // A 128 m cell in Q16.16 metres is `2^(16 + 7)`; derive the shift from the
    // cell size rather than spelling it out.
    let shift = crate::fixed::FRAC_BITS + CELL_SIZE_M.trailing_zeros();
    let cell_x = (x.max(0) >> shift) as u32;
    let cell_z = (z.max(0) >> shift) as u32;
    TilePos::containing_cell(cell_x.min(WORLD_CELLS - 1), cell_z.min(WORLD_CELLS - 1))
}

const _: () = {
    // 16 km / 128 m = 125 cells per tile.
    assert!(TILE_CELLS == 125);
};
