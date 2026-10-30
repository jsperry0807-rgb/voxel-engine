//! M2-14: cross-chunk light seeding through the one-cell apron.
//!
//! Two adjacent 4³-cell chunks are filled from a world source, and each result
//! is compared with the corresponding half of one 8³-cell fill over the same
//! union. The torch crosses the border in one fixture; an opaque border wall
//! stops it in another.

use voxel_mesh::light::{
    BlockLight, LightCell, LightFill, LightSource, full_pass_at, full_pass_at_seeded,
};

const EDGE: usize = 4;

struct World {
    wall_x: Option<i32>,
    torch: Option<[i32; 3]>,
    open_sky: bool,
}

impl LightSource for World {
    fn block(&self, x: i32, y: i32, z: i32) -> BlockLight {
        if self.wall_x == Some(x) {
            return BlockLight::OPAQUE;
        }
        if self.torch == Some([x, y, z]) {
            return BlockLight::emitter(15);
        }
        BlockLight::AIR
    }

    fn sky(&self, _x: i32, y: i32, _z: i32) -> bool {
        self.open_sky && y > 0
    }
}

#[test]
fn light_apron_matches_union_reference() {
    for world in [
        World {
            wall_x: None,
            torch: Some([1, 1, 1]),
            open_sky: false,
        },
        World {
            wall_x: Some(EDGE as i32),
            torch: Some([1, 1, 1]),
            open_sky: false,
        },
        World {
            wall_x: None,
            torch: None,
            open_sky: true,
        },
    ] {
        let union = full_pass_at(EDGE * 2, [0, 0, 0], &world);
        let a = seeded(&world, &union, [0, 0, 0]);
        let b = seeded(&world, &union, [EDGE as i32, 0, 0]);
        assert_chunk_matches(&a, &union, [0, 0, 0]);
        assert_chunk_matches(&b, &union, [EDGE as i32, 0, 0]);

        if world.wall_x.is_some() {
            for y in 0..EDGE {
                for z in 0..EDGE {
                    assert_eq!(b.local(0, y, z).sky_level(), 0);
                    assert_eq!(b.local(0, y, z).block_level(), 0);
                }
            }
        }
    }
}

/// `full_pass_at_seeded` shifts the world read by the chunk origin. Run the
/// same comparison with the second half along each axis so an ignored or
/// transposed origin component shows up.
#[test]
fn apron_seeding_shifts_every_axis() {
    let world = World {
        wall_x: None,
        torch: Some([2, 1, 2]),
        open_sky: true,
    };
    let union = full_pass_at(EDGE * 2, [0, 0, 0], &world);
    for axis in 0..3 {
        let mut far = [0; 3];
        far[axis] = EDGE as i32;
        for origin in [[0, 0, 0], far] {
            let chunk = seeded(&world, &union, origin);
            assert_chunk_matches(&chunk, &union, origin);
        }
    }
}

fn seeded(world: &World, union: &LightFill, origin: [i32; 3]) -> LightFill {
    full_pass_at_seeded(EDGE, origin, world, |x, y, z| {
        if is_apron(origin, x, y, z) {
            union.with_apron(x, y, z)
        } else {
            LightCell::DARK
        }
    })
}

fn is_apron(origin: [i32; 3], x: i32, y: i32, z: i32) -> bool {
    let edge = EDGE as i32;
    [x - origin[0], y - origin[1], z - origin[2]]
        .into_iter()
        .any(|local| local == -1 || local == edge)
}

fn assert_chunk_matches(chunk: &LightFill, union: &LightFill, origin: [i32; 3]) {
    let edge = EDGE as i32;
    for z in -1..=edge {
        for y in -1..=edge {
            for x in -1..=edge {
                let got = chunk.with_apron(x, y, z);
                let expected = union.with_apron(origin[0] + x, origin[1] + y, origin[2] + z);
                assert_eq!(
                    got.sky_level(),
                    expected.sky_level(),
                    "sky at chunk-local ({x}, {y}, {z}) origin {origin:?}"
                );
                assert_eq!(
                    got.block_level(),
                    expected.block_level(),
                    "block at chunk-local ({x}, {y}, {z}) origin {origin:?}"
                );
            }
        }
    }
}
