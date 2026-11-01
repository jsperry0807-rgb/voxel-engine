//! The 16 km macro tile: binary format, round trip and corruption checks.
//!
//! # Layout
//!
//! | Offset | Bytes | Field |
//! |---:|---:|---|
//! | 0 | 4 | magic `VXTL` |
//! | 4 | 2 | format version (`1`) |
//! | 6 | 2 | reserved, zero |
//! | 8 | 8 | seed |
//! | 16 | 2 | generator version |
//! | 18 | 2 | macro version |
//! | 20 | 2 | content version |
//! | 22 | 2 | reserved, zero |
//! | 24 | 4 | tile x (`i32`) |
//! | 28 | 4 | tile z (`i32`) |
//! | 32 | 4 | cell count (`15,625`) |
//! | 36 | 4 | CRC-32 over bytes 0–35 and the payload |
//! | 40 | 93,750 | payload |
//!
//! The payload is channel-major, little-endian: 15,625 `u16` heights, then
//! 15,625 bytes each of temperature, humidity, erosion and flags. A cell is
//! six bytes on disk, and the 93,750-byte payload is 93.75 kB in decimal.
//!
//! Every multi-byte field is written little-endian explicitly, so the bytes do
//! not depend on the host. There is no padding and no pointer-sized field.
//!
//! # Validation
//!
//! Decoding checks, in this order: exact length, magic, format version,
//! reserved header words, cell count, tile position, CRC, then the flag bits.
//! The order matters: a corrupted header reports its field rather than a
//! generic checksum failure, while any payload damage reports
//! [`TileError::Checksum`].
//!
//! # Why channel-major
//!
//! Height is the hot channel: the far-field mesh, the 2D map and every
//! chunk's surface lookup read heights in long runs, and a contiguous `u16`
//! plane is one cache line per 32 cells. Climate and flags are read one cell at
//! a time, where a separate byte plane costs nothing. The on-disk split is the
//! same as the in-memory split, so encoding is a copy per channel.

use crate::macro_grid::{CELLS_PER_TILE, Cell, TILE_CELLS_USIZE, TileFlags, TilePos};
use voxel_core::checksum::Crc32;

/// File magic, ASCII `VXTL`.
pub const MAGIC: [u8; 4] = *b"VXTL";

/// Current format version. A decoder rejects every other value.
pub const FORMAT_VERSION: u16 = 1;

/// Bytes per cell on disk: `u16` height plus four `u8` channels.
pub const CELL_BYTES: usize = 6;

/// Payload size: 15,625 cells × 6 B = 93,750 B.
pub const PAYLOAD_BYTES: usize = CELLS_PER_TILE * CELL_BYTES;

/// Fixed header size.
pub const HEADER_BYTES: usize = 40;

/// Offset of the CRC field inside the header.
pub const CRC_OFFSET: usize = 36;

/// Offset of the flag plane in the payload: heights (2 B), then three byte
/// channels, then flags.
pub const FLAGS_OFFSET: usize = HEADER_BYTES + CELLS_PER_TILE * (CELL_BYTES - 1);

/// Size of a complete tile file.
pub const TILE_BYTES: usize = HEADER_BYTES + PAYLOAD_BYTES;

const OFF_MAGIC: usize = 0;
const OFF_FORMAT: usize = 4;
const OFF_RESERVED_0: usize = 6;
const OFF_SEED: usize = 8;
const OFF_GENERATOR: usize = 16;
const OFF_MACRO: usize = 18;
const OFF_CONTENT: usize = 20;
const OFF_RESERVED_1: usize = 22;
const OFF_TILE_X: usize = 24;
const OFF_TILE_Z: usize = 28;
const OFF_CELL_COUNT: usize = 32;

const _: () = {
    assert!(CELL_BYTES == 6);
    assert!(PAYLOAD_BYTES == 93_750);
    assert!(TILE_BYTES == 93_790);
    assert!(OFF_RESERVED_0 + 2 <= OFF_SEED);
    assert!(OFF_RESERVED_1 + 2 <= OFF_TILE_X);
    assert!(OFF_CELL_COUNT + 4 == CRC_OFFSET);
    assert!(CRC_OFFSET + 4 == HEADER_BYTES);
};

/// Provenance and placement of a macro tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileHeader {
    /// Where the tile sits in the 64 × 64 macro map.
    pub pos: TilePos,
    /// World seed. Every channel is a function of this seed.
    pub seed: u64,
    /// Generator version that produced the tile.
    pub generator_version: u16,
    /// Macro map version (the field set and their meaning).
    pub macro_version: u16,
    /// Content version (biome and surface definitions).
    pub content_version: u16,
}

impl TileHeader {
    /// Build a header. The position is validated by [`MacroTile::new`].
    pub const fn new(
        pos: TilePos,
        seed: u64,
        generator_version: u16,
        macro_version: u16,
        content_version: u16,
    ) -> Self {
        Self {
            pos,
            seed,
            generator_version,
            macro_version,
            content_version,
        }
    }
}

/// A decoded macro tile: one header and five channel planes.
///
/// The planes hold `CELLS_PER_TILE` entries each, indexed
/// `z * TILE_CELLS + x`.
#[derive(Clone)]
pub struct MacroTile {
    header: TileHeader,
    height: Box<[u16]>,
    temperature: Box<[u8]>,
    humidity: Box<[u8]>,
    erosion: Box<[u8]>,
    flags: Box<[TileFlags]>,
}

impl MacroTile {
    /// Allocate an empty tile at `header.pos`.
    pub fn new(header: TileHeader) -> Result<Self, TileError> {
        if !header.pos.is_inside_world() {
            return Err(TileError::TilePosition {
                x: header.pos.x(),
                z: header.pos.z(),
            });
        }
        Ok(Self {
            header,
            height: vec![0; CELLS_PER_TILE].into_boxed_slice(),
            temperature: vec![0; CELLS_PER_TILE].into_boxed_slice(),
            humidity: vec![0; CELLS_PER_TILE].into_boxed_slice(),
            erosion: vec![0; CELLS_PER_TILE].into_boxed_slice(),
            flags: vec![TileFlags::NONE; CELLS_PER_TILE].into_boxed_slice(),
        })
    }

    /// Header of this tile.
    pub const fn header(&self) -> &TileHeader {
        &self.header
    }

    /// Height plane in 0.25 m units.
    pub fn heights(&self) -> &[u16] {
        &self.height
    }

    /// Mutable height plane.
    pub fn heights_mut(&mut self) -> &mut [u16] {
        &mut self.height
    }

    /// Temperature plane, 0 = coldest.
    pub fn temperature(&self) -> &[u8] {
        &self.temperature
    }

    /// Temperature plane, mutable.
    pub fn temperature_mut(&mut self) -> &mut [u8] {
        &mut self.temperature
    }

    /// Humidity plane, 0 = driest.
    pub fn humidity(&self) -> &[u8] {
        &self.humidity
    }

    /// Humidity plane, mutable.
    pub fn humidity_mut(&mut self) -> &mut [u8] {
        &mut self.humidity
    }

    /// Erosion plane, 0 = least eroded.
    pub fn erosion(&self) -> &[u8] {
        &self.erosion
    }

    /// Erosion plane, mutable.
    pub fn erosion_mut(&mut self) -> &mut [u8] {
        &mut self.erosion
    }

    /// Flag plane.
    pub fn flags(&self) -> &[TileFlags] {
        &self.flags
    }

    /// Flag plane, mutable.
    pub fn flags_mut(&mut self) -> &mut [TileFlags] {
        &mut self.flags
    }

    /// One cell, copied out of the five planes.
    ///
    /// # Panics
    ///
    /// If `x` or `z` is not in `0..125`.
    pub fn cell(&self, x: usize, z: usize) -> Cell {
        let index = cell_index(x, z);
        Cell {
            height: self.height[index],
            temperature: self.temperature[index],
            humidity: self.humidity[index],
            erosion: self.erosion[index],
            flags: self.flags[index],
        }
    }

    /// Write one cell into the five planes.
    ///
    /// # Panics
    ///
    /// If `x` or `z` is not in `0..125`.
    pub fn set_cell(&mut self, x: usize, z: usize, cell: Cell) {
        let index = cell_index(x, z);
        self.height[index] = cell.height;
        self.temperature[index] = cell.temperature;
        self.humidity[index] = cell.humidity;
        self.erosion[index] = cell.erosion;
        self.flags[index] = cell.flags;
    }

    /// Encode the tile to its on-disk bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0u8; TILE_BYTES];
        bytes[OFF_MAGIC..OFF_MAGIC + MAGIC.len()].copy_from_slice(&MAGIC);
        bytes[OFF_FORMAT..OFF_FORMAT + 2].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[OFF_SEED..OFF_SEED + 8].copy_from_slice(&self.header.seed.to_le_bytes());
        bytes[OFF_GENERATOR..OFF_GENERATOR + 2]
            .copy_from_slice(&self.header.generator_version.to_le_bytes());
        bytes[OFF_MACRO..OFF_MACRO + 2].copy_from_slice(&self.header.macro_version.to_le_bytes());
        bytes[OFF_CONTENT..OFF_CONTENT + 2]
            .copy_from_slice(&self.header.content_version.to_le_bytes());
        bytes[OFF_TILE_X..OFF_TILE_X + 4].copy_from_slice(&self.header.pos.x().to_le_bytes());
        bytes[OFF_TILE_Z..OFF_TILE_Z + 4].copy_from_slice(&self.header.pos.z().to_le_bytes());
        bytes[OFF_CELL_COUNT..OFF_CELL_COUNT + 4]
            .copy_from_slice(&(CELLS_PER_TILE as u32).to_le_bytes());

        let mut offset = HEADER_BYTES;
        for height in self.height.iter() {
            bytes[offset..offset + 2].copy_from_slice(&height.to_le_bytes());
            offset += 2;
        }
        for channel in [&self.temperature, &self.humidity, &self.erosion] {
            bytes[offset..offset + channel.len()].copy_from_slice(channel);
            offset += channel.len();
        }
        for flags in self.flags.iter() {
            bytes[offset] = flags.bits();
            offset += 1;
        }
        debug_assert_eq!(offset, TILE_BYTES);

        // The CRC covers the header up to its own field, then the payload.
        let mut crc = Crc32::new();
        crc.update(&bytes[..CRC_OFFSET]);
        crc.update(&bytes[HEADER_BYTES..]);
        bytes[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.finish().to_le_bytes());
        bytes
    }

    /// Decode and validate an on-disk tile.
    ///
    /// Errors are reported in the order documented at the module level; a
    /// truncated or padded buffer is rejected before any field is read.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TileError> {
        if bytes.len() != TILE_BYTES {
            return Err(TileError::Length {
                expected: TILE_BYTES,
                actual: bytes.len(),
            });
        }

        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[OFF_MAGIC..OFF_MAGIC + MAGIC.len()]);
        if magic != MAGIC {
            return Err(TileError::Magic { found: magic });
        }

        let format = read_u16(bytes, OFF_FORMAT);
        if format != FORMAT_VERSION {
            return Err(TileError::Format { found: format });
        }

        for offset in [OFF_RESERVED_0, OFF_RESERVED_1] {
            let found = read_u16(bytes, offset);
            if found != 0 {
                return Err(TileError::Reserved { offset, found });
            }
        }

        let cell_count = read_u32(bytes, OFF_CELL_COUNT);
        if cell_count != CELLS_PER_TILE as u32 {
            return Err(TileError::CellCount { found: cell_count });
        }

        let pos = TilePos::new(read_i32(bytes, OFF_TILE_X), read_i32(bytes, OFF_TILE_Z));
        if !pos.is_inside_world() {
            return Err(TileError::TilePosition {
                x: pos.x(),
                z: pos.z(),
            });
        }

        let found_checksum = read_u32(bytes, CRC_OFFSET);
        let mut crc = Crc32::new();
        crc.update(&bytes[..CRC_OFFSET]);
        crc.update(&bytes[HEADER_BYTES..]);
        let expected_checksum = crc.finish();
        if found_checksum != expected_checksum {
            return Err(TileError::Checksum {
                expected: expected_checksum,
                found: found_checksum,
            });
        }

        let header = TileHeader::new(
            pos,
            read_u64(bytes, OFF_SEED),
            read_u16(bytes, OFF_GENERATOR),
            read_u16(bytes, OFF_MACRO),
            read_u16(bytes, OFF_CONTENT),
        );

        let channel = CELLS_PER_TILE;
        let mut offset = HEADER_BYTES;
        let mut heights = Vec::with_capacity(channel);
        for _ in 0..channel {
            heights.push(read_u16(bytes, offset));
            offset += 2;
        }
        let temperature = bytes[offset..offset + channel].to_vec();
        offset += channel;
        let humidity = bytes[offset..offset + channel].to_vec();
        offset += channel;
        let erosion = bytes[offset..offset + channel].to_vec();
        offset += channel;
        let mut flags = Vec::with_capacity(channel);
        for (index, &bits) in bytes[offset..offset + channel].iter().enumerate() {
            let flag = TileFlags::from_bits(bits).ok_or(TileError::UnknownFlags { index, bits })?;
            flags.push(flag);
        }
        debug_assert_eq!(offset + channel, TILE_BYTES);

        Ok(Self {
            header,
            height: heights.into_boxed_slice(),
            temperature: temperature.into_boxed_slice(),
            humidity: humidity.into_boxed_slice(),
            erosion: erosion.into_boxed_slice(),
            flags: flags.into_boxed_slice(),
        })
    }
}

impl core::fmt::Debug for MacroTile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MacroTile")
            .field("header", &self.header)
            .field("cells", &CELLS_PER_TILE)
            .finish_non_exhaustive()
    }
}

/// FNV-1a 64 offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64 prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Fold bytes into an FNV-1a 64 state.
pub fn fnv_bytes(mut state: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        state ^= byte as u64;
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

/// Start a new FNV-1a 64 hash.
pub const fn fnv_start() -> u64 {
    FNV_OFFSET
}

/// Fold a `u64` into an FNV-1a 64 state, little-endian.
pub fn fnv_u64(state: u64, value: u64) -> u64 {
    fnv_bytes(state, &value.to_le_bytes())
}

impl MacroTile {
    /// 64-bit content hash of the identity header and the payload.
    ///
    /// The range is the same bytes the CRC covers (header bytes `0..36` and the
    /// payload), so the hash changes when the seed, a version, the position or
    /// any cell changes. It is stable across runs, threads and platforms: FNV-1a
    /// over the little-endian encoding.
    ///
    /// [`fnv_start`] and [`fnv_u64`] let a caller aggregate tile hashes into a
    /// world hash without storing the tiles.
    pub fn content_hash(&self) -> u64 {
        let bytes = self.to_bytes();
        let mut hash = fnv_bytes(FNV_OFFSET, &bytes[..CRC_OFFSET]);
        hash = fnv_bytes(hash, &bytes[HEADER_BYTES..]);
        hash
    }
}

/// Why a tile could not be built or decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileError {
    /// The buffer is not exactly [`TILE_BYTES`] long.
    Length { expected: usize, actual: usize },
    /// The first four bytes are not [`MAGIC`].
    Magic { found: [u8; 4] },
    /// The format version is not [`FORMAT_VERSION`].
    Format { found: u16 },
    /// A reserved header word is not zero.
    Reserved { offset: usize, found: u16 },
    /// The cell-count guard does not match [`CELLS_PER_TILE`].
    CellCount { found: u32 },
    /// The tile position is outside the 64 × 64 map.
    TilePosition { x: i32, z: i32 },
    /// The stored CRC does not match the bytes.
    Checksum { expected: u32, found: u32 },
    /// A flag cell uses a reserved bit.
    UnknownFlags { index: usize, bits: u8 },
}

impl core::fmt::Display for TileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Length { expected, actual } => {
                write!(f, "tile is {actual} bytes, expected {expected}")
            }
            Self::Magic { found } => write!(f, "bad tile magic {found:?}"),
            Self::Format { found } => {
                write!(f, "tile format {found}, expected {FORMAT_VERSION}")
            }
            Self::Reserved { offset, found } => {
                write!(f, "reserved header word at {offset} is {found}")
            }
            Self::CellCount { found } => {
                write!(f, "tile cell count {found}, expected {CELLS_PER_TILE}")
            }
            Self::TilePosition { x, z } => {
                write!(f, "tile position ({x}, {z}) is outside the world")
            }
            Self::Checksum { expected, found } => {
                write!(f, "tile checksum {found:08x}, expected {expected:08x}")
            }
            Self::UnknownFlags { index, bits } => {
                write!(f, "cell {index} has reserved flag bits {bits:#04x}")
            }
        }
    }
}

impl std::error::Error for TileError {}

fn cell_index(x: usize, z: usize) -> usize {
    assert!(
        x < TILE_CELLS_USIZE && z < TILE_CELLS_USIZE,
        "cell ({x}, {z}) is outside a {}×{} tile",
        TILE_CELLS_USIZE,
        TILE_CELLS_USIZE
    );
    z * TILE_CELLS_USIZE + x
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
