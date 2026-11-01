//! M2.5-02: macro tile round trip, corruption rejection and world geometry.
//!
//! The tile is a stored artefact, so these tests treat the byte layout as a
//! contract: every field is checked after a round trip, every structural field
//! has its own rejection case, and a payload edit is detected by the CRC even
//! when the header is intact.

use voxel_core::checksum::Crc32;
use voxel_worldgen::macro_grid::{
    CELL_SIZE_M, CELLS_PER_TILE, Cell, HEIGHT_MM_PER_UNIT, TILE_CELLS, TILE_COUNT, TILE_EXTENT_M,
    TILES_PER_AXIS, TileFlags, TilePos, WORLD_CELLS, WORLD_EXTENT_M, height_from_metres,
    height_from_mm, height_mm,
};
use voxel_worldgen::tile::{
    CELL_BYTES, CRC_OFFSET, FLAGS_OFFSET, FORMAT_VERSION, HEADER_BYTES, MAGIC, MacroTile,
    PAYLOAD_BYTES, TILE_BYTES, TileError, TileHeader,
};

const SEED: u64 = 0xDEAD_BEEF_CAFE_BABE;

fn header(pos: TilePos) -> TileHeader {
    TileHeader::new(pos, SEED, 7, 3, 11)
}

/// A deterministic, non-uniform tile: every channel has a distinct pattern so
/// a channel swap cannot pass the round trip.
fn sample_tile() -> MacroTile {
    let mut tile = MacroTile::new(header(TilePos::new(7, 9))).expect("valid position");
    let mut state = 0x1234_5678u32;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state
    };
    for z in 0..TILE_CELLS as usize {
        for x in 0..TILE_CELLS as usize {
            let value = next();
            let cell = Cell {
                height: (value & 0xFFFF) as u16,
                temperature: (value >> 16) as u8,
                humidity: (value >> 8) as u8,
                erosion: next() as u8,
                flags: if value % 3 == 0 {
                    TileFlags::RIVER.union(TileFlags::WATER)
                } else if value % 5 == 0 {
                    TileFlags::COAST
                } else {
                    TileFlags::NONE
                },
            };
            tile.set_cell(x, z, cell);
        }
    }
    tile
}

fn reseal(bytes: &mut [u8]) {
    let mut crc = Crc32::new();
    crc.update(&bytes[..CRC_OFFSET]);
    crc.update(&bytes[HEADER_BYTES..]);
    bytes[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.finish().to_le_bytes());
}

fn set_header_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    reseal(bytes);
}

fn set_header_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    reseal(bytes);
}

#[test]
fn tile_round_trips_every_field() {
    let tile = sample_tile();
    let bytes = tile.to_bytes();
    assert_eq!(bytes.len(), TILE_BYTES);

    let decoded = MacroTile::from_bytes(&bytes).expect("the tile we just wrote must decode");
    assert_eq!(decoded.header(), tile.header());
    assert_eq!(decoded.heights(), tile.heights());
    assert_eq!(decoded.temperature(), tile.temperature());
    assert_eq!(decoded.humidity(), tile.humidity());
    assert_eq!(decoded.erosion(), tile.erosion());
    assert_eq!(decoded.flags(), tile.flags());

    for (x, z) in [(0, 0), (124, 124), (63, 7), (7, 63)] {
        assert_eq!(decoded.cell(x, z), tile.cell(x, z), "cell ({x}, {z})");
    }

    // Re-encoding a decoded tile is byte-identical.
    assert_eq!(decoded.to_bytes(), bytes);
}

#[test]
fn tile_payload_size_matches_the_design() {
    assert_eq!(TILE_CELLS, 125);
    assert_eq!(CELLS_PER_TILE, 15_625);
    assert_eq!(CELL_BYTES, 6);
    assert_eq!(PAYLOAD_BYTES, 93_750);
    assert_eq!(HEADER_BYTES, 40);
    assert_eq!(TILE_BYTES, 93_790);
    assert_eq!(core::mem::size_of::<Cell>(), CELL_BYTES);
    assert_eq!(core::mem::size_of::<TileFlags>(), 1);

    // 93.75 kB in decimal, 91.55 KiB.
    assert_eq!(PAYLOAD_BYTES / 1_000, 93);
    assert_eq!(PAYLOAD_BYTES % 1_000, 750);
}

#[test]
fn tile_world_geometry_covers_the_world_once() {
    assert_eq!(TILES_PER_AXIS, 64);
    assert_eq!(TILE_COUNT, 4_096);
    assert_eq!(TILE_EXTENT_M, 16_000);
    assert_eq!(CELL_SIZE_M, 128);
    assert_eq!(WORLD_CELLS, 8_000);
    assert_eq!(WORLD_EXTENT_M, 1_024_000);

    let mut area = 0u64;
    for z in 0..TILES_PER_AXIS {
        for x in 0..TILES_PER_AXIS {
            area += (TILE_EXTENT_M as u64) * (TILE_EXTENT_M as u64);
            let pos = TilePos::new(x as i32, z as i32);
            assert!(pos.is_inside_world());
            assert_eq!(pos.origin_m(), (x as i64 * 16_000, z as i64 * 16_000));
        }
    }
    assert_eq!(
        area,
        (WORLD_EXTENT_M as u64) * (WORLD_EXTENT_M as u64),
        "the tile grid must cover the world exactly once"
    );
    assert_eq!(TilePos::new(63, 63).origin_m(), (1_008_000, 1_008_000));
    assert_eq!(
        TilePos::new(63, 63).origin_m().0 + TILE_EXTENT_M as i64,
        WORLD_EXTENT_M as i64
    );

    assert!(!TilePos::new(-1, 0).is_inside_world());
    assert!(!TilePos::new(64, 0).is_inside_world());
    assert!(!TilePos::new(0, 64).is_inside_world());
}

#[test]
fn tile_cell_and_tile_coordinates_agree() {
    assert_eq!(TilePos::containing_cell(0, 0), TilePos::new(0, 0));
    assert_eq!(TilePos::containing_cell(124, 124), TilePos::new(0, 0));
    assert_eq!(TilePos::containing_cell(125, 0), TilePos::new(1, 0));
    assert_eq!(TilePos::containing_cell(7_999, 7_999), TilePos::new(63, 63));
    assert_eq!(TilePos::local_cell(0), 0);
    assert_eq!(TilePos::local_cell(124), 124);
    assert_eq!(TilePos::local_cell(125), 0);
    assert_eq!(TilePos::local_cell(7_999), 124);
}

#[test]
fn tile_rejects_a_damaged_payload() {
    let mut bytes = sample_tile().to_bytes();
    let payload = bytes.len() - 10;
    bytes[payload] ^= 0x01;
    match MacroTile::from_bytes(&bytes) {
        Err(TileError::Checksum { expected, found }) => assert_ne!(expected, found),
        other => panic!("expected a checksum error, got {other:?}"),
    }
}

#[test]
fn tile_rejects_a_damaged_header_field() {
    let mut bytes = sample_tile().to_bytes();
    // Seed is covered by the CRC only; it has no structural check of its own.
    bytes[8] ^= 0xFF;
    assert!(matches!(
        MacroTile::from_bytes(&bytes),
        Err(TileError::Checksum { .. })
    ));
}

#[test]
fn tile_rejects_structural_header_damage() {
    let base = sample_tile().to_bytes();

    let mut bad_magic = base.clone();
    bad_magic[0] = b'X';
    assert!(matches!(
        MacroTile::from_bytes(&bad_magic),
        Err(TileError::Magic { .. })
    ));

    let mut bad_format = base.clone();
    set_header_u16(&mut bad_format, 4, FORMAT_VERSION + 1);
    assert!(matches!(
        MacroTile::from_bytes(&bad_format),
        Err(TileError::Format { .. })
    ));

    let mut bad_reserved = base.clone();
    set_header_u16(&mut bad_reserved, 6, 1);
    assert!(matches!(
        MacroTile::from_bytes(&bad_reserved),
        Err(TileError::Reserved { offset: 6, .. })
    ));

    let mut bad_cells = base.clone();
    set_header_u32(&mut bad_cells, 32, CELLS_PER_TILE as u32 - 1);
    assert!(matches!(
        MacroTile::from_bytes(&bad_cells),
        Err(TileError::CellCount { .. })
    ));
}

#[test]
fn tile_rejects_wrong_length() {
    let bytes = sample_tile().to_bytes();

    let short = &bytes[..TILE_BYTES - 1];
    assert_eq!(
        MacroTile::from_bytes(short).unwrap_err(),
        TileError::Length {
            expected: TILE_BYTES,
            actual: TILE_BYTES - 1,
        }
    );

    let mut long = bytes.clone();
    long.push(0);
    assert_eq!(
        MacroTile::from_bytes(&long).unwrap_err(),
        TileError::Length {
            expected: TILE_BYTES,
            actual: TILE_BYTES + 1,
        }
    );
}

#[test]
fn tile_rejects_an_out_of_world_position() {
    // A valid CRC is not enough: 64 is past the last tile column.
    let mut bytes = sample_tile().to_bytes();
    set_header_u32(&mut bytes, 24, TILES_PER_AXIS);
    assert_eq!(
        MacroTile::from_bytes(&bytes).unwrap_err(),
        TileError::TilePosition { x: 64, z: 9 }
    );

    assert_eq!(
        MacroTile::new(header(TilePos::new(-1, 0))).unwrap_err(),
        TileError::TilePosition { x: -1, z: 0 }
    );
}

#[test]
fn tile_rejects_reserved_flag_bits() {
    assert!(TileFlags::from_bits(0x0F).is_some());
    assert_eq!(TileFlags::from_bits(0x10), None);
    assert_eq!(TileFlags::from_bits(0x80), None);

    let mut bytes = sample_tile().to_bytes();
    bytes[FLAGS_OFFSET] = 0x10;
    reseal(&mut bytes);
    assert_eq!(
        MacroTile::from_bytes(&bytes).unwrap_err(),
        TileError::UnknownFlags {
            index: 0,
            bits: 0x10,
        }
    );
}

#[test]
fn tile_height_channel_is_quarter_metres() {
    assert_eq!(HEIGHT_MM_PER_UNIT, 250);
    assert_eq!(height_from_metres(6_600), 26_400);
    assert_eq!(height_from_metres(12_000), 48_000);
    assert_eq!(height_mm(26_400), 6_600_000);
    assert_eq!(height_from_mm(6_600_000), 26_400);

    // Round half up, so a 125 mm elevation is one unit and 124 mm is zero.
    assert_eq!(height_from_mm(0), 0);
    assert_eq!(height_from_mm(124), 0);
    assert_eq!(height_from_mm(125), 1);
    assert_eq!(height_from_mm(374), 1);
    assert_eq!(height_from_mm(375), 2);

    // Above the representable range, clamp rather than wrap.
    assert_eq!(height_from_mm(u32::MAX), u16::MAX);
    assert_eq!(height_from_metres(u32::MAX / 2), u16::MAX);
}

#[test]
fn tile_bytes_are_explicitly_little_endian() {
    let mut tile = MacroTile::new(header(TilePos::new(1, 2))).expect("valid position");
    tile.set_cell(
        0,
        0,
        Cell {
            height: 0x1234,
            temperature: 0x11,
            humidity: 0x22,
            erosion: 0x33,
            flags: TileFlags::RIVER.union(TileFlags::COAST),
        },
    );
    let bytes = tile.to_bytes();

    assert_eq!(&bytes[0..4], &MAGIC);
    assert_eq!(&bytes[4..6], &FORMAT_VERSION.to_le_bytes());
    assert_eq!(&bytes[8..16], &SEED.to_le_bytes());
    assert_eq!(&bytes[16..18], &7u16.to_le_bytes());
    assert_eq!(&bytes[18..20], &3u16.to_le_bytes());
    assert_eq!(&bytes[20..22], &11u16.to_le_bytes());
    assert_eq!(&bytes[24..28], &1i32.to_le_bytes());
    assert_eq!(&bytes[28..32], &2i32.to_le_bytes());

    // Height 0x1234 is stored low byte first, then the four flag bytes.
    assert_eq!(&bytes[HEADER_BYTES..HEADER_BYTES + 2], &[0x34, 0x12]);
    assert_eq!(
        bytes[FLAGS_OFFSET],
        TileFlags::RIVER.union(TileFlags::COAST).bits()
    );
}

#[test]
fn tile_flags_are_named_bits() {
    let both = TileFlags::RIVER.union(TileFlags::WATER);
    assert!(both.contains(TileFlags::RIVER));
    assert!(both.contains(TileFlags::WATER));
    assert!(!both.contains(TileFlags::LAKE));
    assert_eq!(
        both.bits(),
        TileFlags::RIVER.bits() | TileFlags::WATER.bits()
    );
    assert_eq!(format!("{both:?}"), "TileFlags(RIVER|WATER)");
    assert_eq!(format!("{:?}", TileFlags::NONE), "TileFlags(NONE)");
}
