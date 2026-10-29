//! M2-13: production full-pass block light equals the M1-29 reference.
//!
//! Five fixtures cover open sky, a roofed cave, an isolated torch, an opaque
//! wall, and a deterministic mixed field. Every padded cell (chunk plus apron)
//! must match in both channels.

use voxel_core::content_constants::LIGHT_CELLS_PER_EDGE;
use voxel_mesh::light::{BlockLight, full_pass};
use voxel_storage::reference::light as reference;

type BlockFn = Box<dyn Fn(i32, i32, i32) -> (u8, u8)>;
type SkyFn = Box<dyn Fn(i32, i32, i32) -> bool>;

struct Fixture {
    name: &'static str,
    edge: usize,
    block: BlockFn,
    sky: SkyFn,
}

#[test]
fn light_full_matches_reference_on_five_fixtures() {
    for fixture in fixtures() {
        let production = full_pass(
            fixture.edge,
            |x, y, z| {
                let (emission, opacity) = (fixture.block)(x, y, z);
                BlockLight::new(emission, opacity)
            },
            |x, y, z| (fixture.sky)(x, y, z),
        );
        let oracle = reference::full_pass(
            fixture.edge,
            |x, y, z| {
                let (emission, opacity) = (fixture.block)(x, y, z);
                reference::BlockLight::new(emission, opacity)
            },
            |x, y, z| (fixture.sky)(x, y, z),
        );

        let last = fixture.edge as i32;
        for z in -1..=last {
            for y in -1..=last {
                for x in -1..=last {
                    let got = production.with_apron(x, y, z);
                    let expected = oracle.with_apron(x, y, z);
                    assert_eq!(
                        got.sky_level(),
                        expected.sky_level(),
                        "{}: sky at ({x}, {y}, {z})",
                        fixture.name
                    );
                    assert_eq!(
                        got.block_level(),
                        expected.block_level(),
                        "{}: block at ({x}, {y}, {z})",
                        fixture.name
                    );
                }
            }
        }
    }
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "open_sky",
            edge: 4,
            block: Box::new(|_, y, _| if y <= 0 { (0, 15) } else { (0, 0) }),
            sky: Box::new(|_, y, _| y > 0),
        },
        Fixture {
            name: "cave",
            edge: 4,
            block: Box::new(|x, y, z| {
                let shaft = x == 0 && z == 0 && (1..=3).contains(&y);
                let arm = y == 1 && z == 0 && (1..=3).contains(&x);
                if shaft || arm { (0, 0) } else { (0, 15) }
            }),
            sky: Box::new(|x, y, z| x == 0 && z == 0 && (1..=4).contains(&y)),
        },
        Fixture {
            name: "torch",
            edge: 4,
            block: Box::new(|x, y, z| {
                if (x, y, z) == (1, 1, 1) {
                    (15, 0)
                } else {
                    (0, 0)
                }
            }),
            sky: Box::new(|_, _, _| false),
        },
        Fixture {
            name: "opaque_wall",
            edge: 4,
            block: Box::new(|x, y, z| match (x, y, z) {
                (0, 1, 1) => (15, 0),
                _ if x == 1 => (0, 15),
                _ => (0, 0),
            }),
            sky: Box::new(|_, _, _| false),
        },
        Fixture {
            name: "mixed_field",
            edge: LIGHT_CELLS_PER_EDGE,
            block: Box::new(|x, y, z| {
                let hash = hash3(x, y, z);
                let emission = if hash.is_multiple_of(11) { 15 } else { 0 };
                let opacity = (hash / 11 % 4) as u8;
                (emission, opacity)
            }),
            sky: Box::new(|x, y, z| hash3(x, y, z).is_multiple_of(7)),
        },
    ]
}

fn hash3(x: i32, y: i32, z: i32) -> u64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h
}
