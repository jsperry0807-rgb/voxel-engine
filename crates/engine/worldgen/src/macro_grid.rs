//! The macro grid: 128 m cells over a 1,024 km square, stored in 16 km tiles.
//!
//! The world is **8,000 × 8,000 cells** (1,024,000 m per side at 128 m per
//! cell). A tile is **125 × 125 cells** (16,000 m), so the macro map is
//! **64 × 64 = 4,096 tiles**. Both divisions are exact: `64 × 125 = 8,000` and
//! `8,000 × 128 m = 1,024,000 m`.
//!
//! Coordinates are cell coordinates unless a name says otherwise. `x` grows
//! east, `z` grows south, and a tile's `(0, 0)` cell is its north-west corner.
//! Values are signed where a caller may pass a neighbour outside the world
//! (streaming looks one tile past the edge), and validated only when a tile is
//! constructed.
//!
//! # Channels
//!
//! | Channel | Type | Unit | Meaning |
//! |---|---|---|---|
//! | height | `u16` | 0.25 m | terrain surface elevation, 0 = floor |
//! | temperature | `u8` | 1/255 | 0 = coldest, 255 = hottest |
//! | humidity | `u8` | 1/255 | 0 = driest, 255 = wettest |
//! | erosion | `u8` | 1/255 | 0 = least eroded, 255 = most eroded |
//! | flags | u8 | bit field | water, river, lake, coast |
//!
//! A quarter-metre height step is `u16`-cheap: 12,000 m peak = 48,000 units,
//! and the format can represent 16,383.75 m. Sea level (6,600 m) is exactly
//! 26,400 units, so a coastline is never rounded by the storage type.

/// Horizontal size of one macro cell, in metres.
pub const CELL_SIZE_M: u32 = 128;

/// Cells per tile edge: 16,000 m / 128 m.
pub const TILE_CELLS: u32 = 125;

/// [`TILE_CELLS`] as the `usize` used for indexing.
pub const TILE_CELLS_USIZE: usize = TILE_CELLS as usize;

/// Cells in one tile.
pub const CELLS_PER_TILE: usize = TILE_CELLS_USIZE * TILE_CELLS_USIZE;

/// Edge of one tile, in metres.
pub const TILE_EXTENT_M: u32 = CELL_SIZE_M * TILE_CELLS;

/// Edge of the world, in metres.
pub const WORLD_EXTENT_M: u32 = 1_024_000;

/// Cells per world edge.
pub const WORLD_CELLS: u32 = WORLD_EXTENT_M / CELL_SIZE_M;

/// Tiles per world edge: 8,000 / 125.
pub const TILES_PER_AXIS: u32 = WORLD_CELLS / TILE_CELLS;

/// Tiles in the whole world.
pub const TILE_COUNT: u32 = TILES_PER_AXIS * TILES_PER_AXIS;

/// Height units per metre: the `u16` height channel is 0.25 m units.
pub const HEIGHT_UNITS_PER_METRE: u32 = 4;

/// Millimetres per height unit.
pub const HEIGHT_MM_PER_UNIT: u32 = 1_000 / HEIGHT_UNITS_PER_METRE;

/// Largest elevation the height channel can represent, in millimetres.
pub const MAX_HEIGHT_MM: u32 = u16::MAX as u32 * HEIGHT_MM_PER_UNIT;

const _: () = {
    assert!(TILE_EXTENT_M == 16_000);
    assert!(WORLD_CELLS == 8_000);
    assert!(TILES_PER_AXIS == 64);
    assert!(TILE_COUNT == 4_096);
    assert!(TILE_CELLS * TILES_PER_AXIS == WORLD_CELLS);
    assert!(CELL_SIZE_M * WORLD_CELLS == WORLD_EXTENT_M);
};

/// A tile coordinate in the 64 × 64 macro map.
///
/// Signed so neighbour lookups can walk one past the edge; [`is_inside_world`]
/// is the only validity rule.
///
/// [`is_inside_world`]: TilePos::is_inside_world
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TilePos {
    x: i32,
    z: i32,
}

impl TilePos {
    /// Construct a tile position. Out-of-world values are allowed here and
    /// rejected by [`MacroTile::new`](crate::tile::MacroTile::new) or by
    /// [`is_inside_world`](Self::is_inside_world).
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Tile column (west to east).
    pub const fn x(self) -> i32 {
        self.x
    }

    /// Tile row (north to south).
    pub const fn z(self) -> i32 {
        self.z
    }

    /// True when the tile lies in `0..64` on both axes.
    pub const fn is_inside_world(self) -> bool {
        self.x >= 0
            && self.z >= 0
            && (self.x as u32) < TILES_PER_AXIS
            && (self.z as u32) < TILES_PER_AXIS
    }

    /// The tile that owns a world cell. The cell must be inside the world.
    pub const fn containing_cell(cell_x: u32, cell_z: u32) -> Self {
        assert!(cell_x < WORLD_CELLS && cell_z < WORLD_CELLS);
        Self::new((cell_x / TILE_CELLS) as i32, (cell_z / TILE_CELLS) as i32)
    }

    /// The tile-local cell index along one axis.
    pub const fn local_cell(cell: u32) -> usize {
        (cell % TILE_CELLS) as usize
    }

    /// Metres from the world origin to the tile's north-west corner.
    pub const fn origin_m(self) -> (i64, i64) {
        (
            self.x as i64 * TILE_EXTENT_M as i64,
            self.z as i64 * TILE_EXTENT_M as i64,
        )
    }

    /// Macro-cell coordinates of the tile's north-west corner.
    pub const fn origin_cell(self) -> (i64, i64) {
        (
            self.x as i64 * TILE_CELLS as i64,
            self.z as i64 * TILE_CELLS as i64,
        )
    }
}

/// Surface classification bits stored in a macro cell.
///
/// Bits 4–7 are reserved and must be zero; the tile decoder rejects a tile
/// that sets one, so a future flag needs a format-version bump rather than a
/// silent reinterpretation.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct TileFlags(u8);

impl TileFlags {
    /// No flags set.
    pub const NONE: Self = Self(0);
    /// A river channel crosses this cell (M2.5-07).
    pub const RIVER: Self = Self(1 << 0);
    /// A lake covers this cell (M2.5-08).
    pub const LAKE: Self = Self(1 << 1);
    /// The cell is in the beach/coast band.
    pub const COAST: Self = Self(1 << 2);
    /// The cell's surface is below the local water surface.
    pub const WATER: Self = Self(1 << 3);
    /// Every defined bit.
    pub const ALL: u8 = 0b0000_1111;

    /// Build flags from raw bits, rejecting the reserved range.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::ALL == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Raw bits, as stored.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// True when every bit of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The union of two flag sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl core::fmt::Debug for TileFlags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("TileFlags(")?;
        if self.0 == 0 {
            return f.write_str("NONE)");
        }
        let mut first = true;
        for (bit, name) in [
            (Self::RIVER, "RIVER"),
            (Self::LAKE, "LAKE"),
            (Self::COAST, "COAST"),
            (Self::WATER, "WATER"),
        ] {
            if self.contains(bit) {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        f.write_str(")")
    }
}

/// One macro cell: the five channels sampled together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Cell {
    pub height: u16,
    pub temperature: u8,
    pub humidity: u8,
    pub erosion: u8,
    pub flags: TileFlags,
}

const _: () = {
    // The tile format's 6 B/cell budget depends on this staying 6.
    assert!(core::mem::size_of::<Cell>() == 6);
};

/// Millimetres for a height channel value.
pub const fn height_mm(units: u16) -> u32 {
    units as u32 * HEIGHT_MM_PER_UNIT
}

/// Height channel value for a millimetre elevation, rounded half up and
/// clamped to the representable range.
pub const fn height_from_mm(mm: u32) -> u16 {
    height_from_mm_wide(mm as u64)
}

/// Height channel value for a whole-metre elevation, clamped.
///
/// The multiply is done in `u64` so even `u32::MAX` metres saturates instead of
/// overflowing.
pub const fn height_from_metres(metres: u32) -> u16 {
    height_from_mm_wide(metres as u64 * 1_000)
}

const fn height_from_mm_wide(mm: u64) -> u16 {
    let units = (mm + HEIGHT_MM_PER_UNIT as u64 / 2) / HEIGHT_MM_PER_UNIT as u64;
    if units > u16::MAX as u64 {
        u16::MAX
    } else {
        units as u16
    }
}

/// Q16.16 metres to a macro-tile height unit, rounded half up and clamped.
///
/// The shape splines (`crate::splines::surface_height`) produce Q16.16 metres;
/// the tile stores 0.25 m units. Negative values clamp to the floor and values
/// above the channel's range saturate at [`u16::MAX`] rather than wrapping.
pub const fn height_from_q16(value: i32) -> u16 {
    let one = crate::fixed::ONE as i64;
    let mm = ((value as i64 * 1_000) + one / 2) / one;
    if mm <= 0 {
        return 0;
    }
    if mm > u32::MAX as i64 {
        return u16::MAX;
    }
    height_from_mm(mm as u32)
}
