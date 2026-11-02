//! M2.5-04: latitude, ocean distance and lapse-rate climate fields.
//!
//! The design asks for four readable properties — poles cold, coasts humid,
//! peaks cold, and a 2D map without surprises. Each is asserted here, plus a
//! strict monotonicity check on the noise-free base so the map tests do not
//! depend on the 8 km variation.

use voxel_worldgen::climate::{
    ALTITUDE_LAPSE, Climate, EQUATOR, LATITUDE_WARMTH, OCEAN_HUMIDITY, VARIATION_AMPLITUDE,
    humidity_base, latitude_warmth, temperature_base,
};
use voxel_worldgen::fixed::ONE;
use voxel_worldgen::macro_grid::WORLD_EXTENT_M;
use voxel_worldgen::splines::{PEAK, SEA_LEVEL, Spline};

const WORLD_TOP: i64 = WORLD_EXTENT_M as i64;
const X: i64 = 1_234_567 << 16;

fn metres(value: i64) -> i64 {
    value << 16
}

fn assert_monotone<const N: usize>(table: &Spline<N>, direction: i32, name: &str) {
    for index in 0..N - 1 {
        let (x0, y0) = table.point(index);
        let (x1, y1) = table.point(index + 1);
        let span = (x1 - x0) as i64;
        let mut previous = y0;
        for step in 0..=128i64 {
            let x = (x0 as i64 + span * step / 128) as i32;
            let value = table.eval(x);
            match direction {
                1 => assert!(
                    value >= previous && value >= y0 && value <= y1,
                    "{name}: not non-decreasing at segment {index}"
                ),
                -1 => assert!(
                    value <= previous && value <= y0 && value >= y1,
                    "{name}: not non-increasing at segment {index}"
                ),
                _ => assert_eq!(value, y0, "{name}: flat segment {index} moved"),
            }
            previous = value;
        }
    }
}

#[test]
fn climate_tables_are_monotone() {
    assert_monotone(&LATITUDE_WARMTH, -1, "latitude warmth");
    assert_monotone(&ALTITUDE_LAPSE, 1, "altitude lapse");
    assert_monotone(&OCEAN_HUMIDITY, -1, "ocean humidity");
}

#[test]
fn climate_latitude_is_exact_at_the_equator_and_poles() {
    assert_eq!(EQUATOR, metres(512_000));
    assert_eq!(latitude_warmth(EQUATOR), ONE);
    assert_eq!(latitude_warmth(0), 0);
    assert_eq!(latitude_warmth(metres(WORLD_TOP)), 0);
}

#[test]
fn climate_latitude_is_symmetric_about_the_equator() {
    for step in 1..=16 {
        let offset = metres(32_000 * step as i64);
        assert_eq!(
            latitude_warmth(EQUATOR - offset),
            latitude_warmth(EQUATOR + offset),
            "symmetry at step {step}"
        );
    }
}

#[test]
fn climate_latitude_cools_monotonically_toward_the_poles() {
    let mut previous = ONE;
    for step in 0..=128 {
        let offset = metres(4_000 * step as i64);
        let warmth = latitude_warmth(EQUATOR + offset);
        assert!(
            warmth <= previous,
            "warmth rose at {offset} ({} > {previous})",
            warmth
        );
        previous = warmth;
    }
}

#[test]
fn climate_poles_are_cold_and_the_equator_is_hot() {
    let climate = Climate::new(1);
    let equator = climate.temperature(X, EQUATOR, SEA_LEVEL);
    let north = climate.temperature(X, 0, SEA_LEVEL);
    let south = climate.temperature(X, metres(WORLD_TOP), SEA_LEVEL);

    assert!(equator >= 240, "equator is only {equator}");
    assert!(north <= 20, "north pole is {north}");
    assert!(south <= 20, "south pole is {south}");
    assert!(
        equator >= north + 200,
        "equator {equator} and pole {north} are too close"
    );
}

#[test]
fn climate_peaks_are_cold_at_every_latitude() {
    let climate = Climate::new(2);
    for z in [
        0,
        EQUATOR / 2,
        EQUATOR,
        EQUATOR + EQUATOR / 2,
        metres(WORLD_TOP),
    ] {
        let lowland = climate.temperature(X, z, SEA_LEVEL);
        let peak = climate.temperature(X, z, PEAK);
        assert!(
            peak <= lowland,
            "peak {peak} is warmer than lowland {lowland} at z {z}"
        );
        assert!(peak <= 80, "peak temperature {peak} is not cold at z {z}");
    }

    // At the equator the drop is unmistakable, not just a clamp.
    let equatorial_low = climate.temperature(X, EQUATOR, SEA_LEVEL);
    let equatorial_peak = climate.temperature(X, EQUATOR, PEAK);
    assert!(
        equatorial_low - equatorial_peak >= 150,
        "equatorial peak {equatorial_peak} is too close to lowland {equatorial_low}"
    );
    assert!(
        equatorial_peak <= 80,
        "equatorial peak is {equatorial_peak}, should be snow-cold"
    );
}

#[test]
fn climate_altitude_only_cools() {
    let climate = Climate::new(3);
    for z in [0, EQUATOR / 3, EQUATOR, EQUATOR + EQUATOR / 3] {
        let mut previous = climate.temperature(X, z, 0);
        for step in 1..=64 {
            let surface = (PEAK as i64 * step / 64) as i32;
            let value = climate.temperature(X, z, surface);
            assert!(
                value <= previous,
                "temperature rose at altitude step {step} (z {z}): {value} > {previous}"
            );
            previous = value;
        }
    }
}

#[test]
fn climate_ocean_floor_is_as_warm_as_the_water_surface() {
    let climate = Climate::new(4);
    for z in [0, EQUATOR, metres(WORLD_TOP)] {
        assert_eq!(
            climate.temperature(X, z, 0),
            climate.temperature(X, z, SEA_LEVEL),
            "the lapse must not apply below sea level at z {z}"
        );
    }
}

#[test]
fn climate_coasts_are_wetter_than_interiors() {
    let climate = Climate::new(5);
    let coast = climate.humidity(X, EQUATOR, 0);
    let inland = climate.humidity(X, EQUATOR, metres(512_000));
    assert!(coast >= 220, "coast humidity is only {coast}");
    assert!(inland <= 90, "inland humidity is still {inland}");
    assert!(coast > inland);
}

#[test]
fn climate_humidity_falls_off_with_ocean_distance() {
    let climate = Climate::new(6);
    let mut previous = climate.humidity(X, EQUATOR, 0);
    for step in 1..=128 {
        let distance = metres(4_000 * step as i64);
        let value = climate.humidity(X, EQUATOR, distance);
        assert!(
            value <= previous,
            "humidity rose at {distance} m: {value} > {previous}"
        );
        previous = value;
    }
}

#[test]
fn climate_temperature_matches_the_noise_envelope() {
    let climate = Climate::new(7);
    for z_step in 0..=32 {
        let z = metres(WORLD_TOP * z_step / 32);
        for alt_step in 0..=16 {
            let surface = (PEAK as i64 * alt_step / 16) as i32;
            let base = temperature_base(z, surface);
            let low = voxel_worldgen::climate::unit_to_u8(base - VARIATION_AMPLITUDE) as i32;
            let high = voxel_worldgen::climate::unit_to_u8(base + VARIATION_AMPLITUDE) as i32;
            let actual = climate.temperature(X, z, surface) as i32;
            assert!(
                actual >= low - 1 && actual <= high + 1,
                "temperature {actual} outside [{low}, {high}] at z {z}, surface {surface}"
            );
        }
    }
}

#[test]
fn climate_2d_map_is_monotone_in_latitude_and_altitude() {
    // 33 latitude rows (equator in the middle) by 17 altitude steps.
    const ROWS: usize = 33;
    const COLS: usize = 17;
    let mut map = [[0i32; COLS]; ROWS];
    for (row, values) in map.iter_mut().enumerate() {
        let z = metres(WORLD_TOP * row as i64 / (ROWS as i64 - 1));
        for (col, value) in values.iter_mut().enumerate() {
            let surface = (PEAK as i64 * col as i64 / (COLS as i64 - 1)) as i32;
            *value = temperature_base(z, surface);
        }
    }

    for values in &map {
        for pair in values.windows(2) {
            assert!(
                pair[1] <= pair[0],
                "map warms with altitude: {} then {}",
                pair[0],
                pair[1]
            );
        }
    }
    for (col, _) in map[0].iter().enumerate() {
        let column: Vec<i32> = map.iter().map(|row| row[col]).collect();
        for pair in column.windows(2).take(ROWS / 2) {
            assert!(
                pair[0] <= pair[1],
                "map cools toward the equator at column {col}"
            );
        }
        for pair in column.windows(2).skip(ROWS / 2) {
            assert!(
                pair[0] >= pair[1],
                "map cools toward the equator at column {col}"
            );
        }
    }
}

#[test]
fn climate_2d_humidity_map_falls_off_from_the_coast() {
    const COLUMNS: usize = 513;
    for x_step in 0..8 {
        let x = metres(16_000 * x_step as i64);
        let mut previous = humidity_base(0);
        for distance in 1..COLUMNS {
            let value = humidity_base(metres(1_000 * distance as i64));
            assert!(
                value <= previous,
                "humidity rose at {distance} km for x {x}: {value} > {previous}"
            );
            previous = value;
        }
    }
}

#[test]
fn climate_is_deterministic_and_seed_dependent() {
    let first = Climate::new(11);
    let again = Climate::new(11);
    let other = Climate::new(12);
    let mut differs = false;
    for step in 0..32 {
        let x = metres(1_000 * step);
        let z = EQUATOR + metres(1_000 * step);
        let surface = (PEAK / 4) * (step % 4) as i32;
        let a = first.sample(x, z, surface, metres(5_000 * step));
        let b = again.sample(x, z, surface, metres(5_000 * step));
        let c = other.sample(x, z, surface, metres(5_000 * step));
        assert_eq!(a, b, "same seed diverged at step {step}");
        differs |= a != c;
    }
    assert!(differs, "two seeds produced identical climate samples");
}

#[test]
fn climate_sample_agrees_with_the_channels() {
    let climate = Climate::new(13);
    let x = metres(700_000);
    let z = metres(400_000);
    let surface = SEA_LEVEL + (PEAK - SEA_LEVEL) / 3;
    let distance = metres(42_000);
    let sample = climate.sample(x, z, surface, distance);
    assert_eq!(sample.temperature, climate.temperature(x, z, surface));
    assert_eq!(sample.humidity, climate.humidity(x, z, distance));
}
