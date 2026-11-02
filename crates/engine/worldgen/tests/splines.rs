//! M2.5-03: the shape spline tables, monotonicity and the height channel.
//!
//! The tables are the contract between the shape fields (M2.5-04/05) and the
//! surface rules (M2.5-12). These tests assert the properties the design
//! promises: control points are exact, a monotone table stays monotone and
//! never overshoots, sea level is exact at the coast, the peak is reachable,
//! and the surface is monotone in both shape inputs.

use voxel_worldgen::fixed::ONE;
use voxel_worldgen::macro_grid::height_from_q16;
use voxel_worldgen::splines::{
    CONTINENTALNESS_HEIGHT, EROSION_RELIEF, FLOOR, PEAK, RELIEF_WEIGHT, SEA_LEVEL, Spline,
    surface_height,
};

fn metres(value: i32) -> i32 {
    value << 16
}

/// Sample every segment of a table and assert it moves only in `direction`
/// (`1` up, `-1` down, `0` flat) without passing either endpoint.
fn assert_monotone<const N: usize>(table: &Spline<N>, name: &str, direction: i32) {
    for index in 0..N - 1 {
        let (x0, y0) = table.point(index);
        let (x1, y1) = table.point(index + 1);
        assert!(
            y1 >= y0 || direction < 0,
            "{name}: point {index} descends against direction {direction}"
        );
        assert!(
            y1 <= y0 || direction > 0,
            "{name}: point {index} ascends against direction {direction}"
        );
        let span = (x1 - x0) as i64;
        let mut previous = y0;
        for step in 0..=128i64 {
            let x = (x0 as i64 + span * step / 128) as i32;
            let value = table.eval(x);
            match direction {
                1 => assert!(
                    value >= previous && value >= y0 && value <= y1,
                    "{name}: overshoot at segment {index}, x {x}: {value}"
                ),
                -1 => assert!(
                    value <= previous && value <= y0 && value >= y1,
                    "{name}: overshoot at segment {index}, x {x}: {value}"
                ),
                _ => assert_eq!(value, y0, "{name}: flat segment {index} moved"),
            }
            previous = value;
        }
    }
}

#[test]
fn splines_tables_cover_their_declared_domains() {
    assert_eq!(CONTINENTALNESS_HEIGHT.len(), 10);
    assert_eq!(EROSION_RELIEF.len(), 5);
    assert_eq!(RELIEF_WEIGHT.len(), 6);

    assert_eq!(CONTINENTALNESS_HEIGHT.point(0), (-ONE, 0));
    assert_eq!(CONTINENTALNESS_HEIGHT.point(9), (ONE, metres(10_800)));
    assert_eq!(EROSION_RELIEF.point(0), (0, metres(1_200)));
    assert_eq!(EROSION_RELIEF.point(4), (ONE, 0));
    assert_eq!(RELIEF_WEIGHT.point(0), (-ONE, 0));
    assert_eq!(RELIEF_WEIGHT.point(5), (ONE, ONE));

    assert_sorted(&CONTINENTALNESS_HEIGHT, "continentalness");
    assert_sorted(&EROSION_RELIEF, "erosion");
    assert_sorted(&RELIEF_WEIGHT, "weight");
}

fn assert_sorted<const N: usize>(table: &Spline<N>, name: &str) {
    for index in 1..N {
        assert!(
            table.point(index).0 > table.point(index - 1).0,
            "{name}: x must increase at {index}"
        );
    }
}

#[test]
fn splines_hit_every_control_point_exactly() {
    for index in 0..CONTINENTALNESS_HEIGHT.len() {
        let (x, y) = CONTINENTALNESS_HEIGHT.point(index);
        assert_eq!(CONTINENTALNESS_HEIGHT.eval(x), y, "base point {index}");
    }
    for index in 0..EROSION_RELIEF.len() {
        let (x, y) = EROSION_RELIEF.point(index);
        assert_eq!(EROSION_RELIEF.eval(x), y, "relief point {index}");
    }
    for index in 0..RELIEF_WEIGHT.len() {
        let (x, y) = RELIEF_WEIGHT.point(index);
        assert_eq!(RELIEF_WEIGHT.eval(x), y, "weight point {index}");
    }
}

#[test]
fn splines_clamp_outside_the_domain() {
    assert_eq!(CONTINENTALNESS_HEIGHT.eval(-2 * ONE), 0);
    assert_eq!(CONTINENTALNESS_HEIGHT.eval(2 * ONE), metres(10_800));
    assert_eq!(EROSION_RELIEF.eval(-ONE), metres(1_200));
    assert_eq!(EROSION_RELIEF.eval(2 * ONE), 0);
    assert_eq!(RELIEF_WEIGHT.eval(-2 * ONE), 0);
    assert_eq!(RELIEF_WEIGHT.eval(2 * ONE), ONE);
}

#[test]
fn splines_monotone_tables_do_not_overshoot() {
    assert_monotone(&CONTINENTALNESS_HEIGHT, "continentalness", 1);
    assert_monotone(&EROSION_RELIEF, "erosion", -1);
    assert_monotone(&RELIEF_WEIGHT, "weight", 1);
}

#[test]
fn splines_sea_level_is_exact_at_the_coast() {
    assert_eq!(SEA_LEVEL, metres(6_600));
    for erosion in [0, ONE / 8, ONE / 4, ONE / 2, ONE] {
        assert_eq!(
            surface_height(0, erosion),
            SEA_LEVEL,
            "coast must stay at sea level for erosion {erosion}"
        );
    }
}

#[test]
fn splines_surface_spans_floor_to_peak() {
    assert_eq!(FLOOR, 0);
    assert_eq!(surface_height(-ONE, 0), FLOOR);
    assert_eq!(surface_height(-ONE, ONE), FLOOR);
    assert_eq!(surface_height(ONE, 0), PEAK);
    assert_eq!(surface_height(ONE, 0), metres(12_000));
    assert_eq!(surface_height(ONE, ONE), metres(10_800));
}

#[test]
fn splines_surface_is_monotone_in_continentalness() {
    for erosion in [0, ONE / 4, ONE / 2, 3 * ONE / 4, ONE] {
        let mut previous = FLOOR;
        for step in -128..=128 {
            let continentalness = step * ONE / 128;
            let height = surface_height(continentalness, erosion);
            assert!(
                height >= previous,
                "height fell with continentalness at {continentalness} (erosion {erosion})"
            );
            previous = height;
        }
    }
}

#[test]
fn splines_surface_is_monotone_in_erosion() {
    for continentalness in [-ONE, -ONE / 2, 0, ONE / 2, 3 * ONE / 4, ONE] {
        let mut previous = PEAK;
        for step in 0..=64 {
            let erosion = step * ONE / 64;
            let height = surface_height(continentalness, erosion);
            assert!(
                height <= previous,
                "height rose with erosion at {erosion} (continentalness {continentalness})"
            );
            previous = height;
        }
    }
}

#[test]
fn splines_surface_stays_inside_the_world() {
    for c_step in -64..=64 {
        let continentalness = c_step * ONE / 64;
        for e_step in 0..=32 {
            let erosion = e_step * ONE / 32;
            let height = surface_height(continentalness, erosion);
            assert!(
                (FLOOR..=PEAK).contains(&height),
                "height {height} out of range at ({continentalness}, {erosion})"
            );
        }
    }
}

#[test]
fn splines_ocean_stays_below_sea_level() {
    // Shelf edge and deeper.
    assert_eq!(surface_height(-ONE / 4, 0), metres(6_450));
    assert_eq!(surface_height(-ONE / 2, 0), metres(4_200));
    for step in -128..0 {
        let continentalness = step * ONE / 128;
        assert!(
            surface_height(continentalness, 0) < SEA_LEVEL,
            "ocean cell {continentalness} is not below sea level"
        );
    }
}

#[test]
fn splines_relief_is_flat_underwater_and_at_the_coast() {
    for continentalness in [-ONE, -3 * ONE / 4, -ONE / 2, -ONE / 4, 0] {
        let least = surface_height(continentalness, 0);
        let most = surface_height(continentalness, ONE);
        assert_eq!(
            least, most,
            "erosion must not move the sea floor at {continentalness}"
        );
    }
}

#[test]
fn splines_erosion_reduces_land_relief() {
    // At full relief weight the difference is the entire 1,200 m range.
    let ridge = surface_height(3 * ONE / 4, 0);
    let rounded = surface_height(3 * ONE / 4, ONE);
    assert_eq!(ridge - rounded, metres(1_200));

    // Halfway through the weight ramp the difference is exactly halved.
    let half = surface_height(ONE / 8, 0) - surface_height(ONE / 8, ONE);
    assert_eq!(half, metres(600));
}

#[test]
fn splines_height_converts_to_tile_units() {
    // 0.25 m units: sea 26,400, peak 48,000.
    assert_eq!(height_from_q16(SEA_LEVEL), 26_400);
    assert_eq!(height_from_q16(PEAK), 48_000);
    assert_eq!(height_from_q16(FLOOR), 0);
    assert_eq!(height_from_q16(metres(1)), 4);
    assert_eq!(height_from_q16(metres(1) + ONE / 10), 4); // 1.10 m -> 4.4 units, rounds down
    assert_eq!(height_from_q16(metres(1) + ONE / 8), 5); // 1.125 m -> 4.5 units, rounds up
    assert_eq!(height_from_q16(metres(1) + ONE / 4), 5); // 1.25 m -> 5 units

    // Outside the channel: floor at the bottom, saturate at the top.
    assert_eq!(height_from_q16(-ONE), 0);
    assert_eq!(height_from_q16(i32::MAX), u16::MAX);
}

#[test]
fn splines_surface_output_matches_the_height_channel() {
    // A handful of representative points, pinned end to end.
    for (continentalness, erosion, units) in [
        (0, 0, 26_400u16),
        (ONE, 0, 48_000),
        (-ONE, 0, 0),
        (3 * ONE / 4, ONE, 37_600), // 9,400 m
        (ONE / 4, 0, 32_800),       // 7,000 m base + full 1,200 m relief
    ] {
        assert_eq!(
            height_from_q16(surface_height(continentalness, erosion)),
            units,
            "({continentalness}, {erosion})"
        );
    }
}
