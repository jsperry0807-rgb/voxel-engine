//! M2.5-16: overhangs, arches and the no-floating-rock rule.
//!
//! The fixture tests build a hand-checkable arch and shelf: a span over an air
//! gap, two legs that reach the ground, and a floating block that the prune
//! step must remove. The field scans then show that the 3D density produces
//! occasional overhangs and arches, and that the pruned field has no
//! unsupported island.

use voxel_worldgen::overhangs::{
    OVERHANG_AMPLITUDE_M, OVERHANG_WAVELENGTH_XZ_M, OVERHANG_WAVELENGTH_Y_M, OverhangField,
    prune_unsupported,
};

fn metres(value: i32) -> i64 {
    (value as i64) << 16
}

const NX: usize = 64;
const NZ: usize = 64;
const SURFACE: usize = 40;
const NY: usize = SURFACE + OVERHANG_AMPLITUDE_M as usize + 2;

fn index(x: usize, y: usize, z: usize) -> usize {
    x + NX * (y + NY * z)
}

fn build_field_mask(seed: u64, surface: usize) -> Vec<bool> {
    let field = OverhangField::new(seed);
    let mut solid = vec![false; NX * NY * NZ];
    for z in 0..NZ {
        for x in 0..NX {
            for y in 0..NY {
                solid[index(x, y, z)] = field.solid(
                    metres(x as i32),
                    metres(y as i32),
                    metres(z as i32),
                    metres(surface as i32),
                );
            }
        }
    }
    solid
}

fn solid_near_x(solid: &[bool], x: usize, y: usize, z: usize, step: i32, limit: i32) -> bool {
    let mut cx = x as i32 + step;
    for _ in 0..limit {
        if cx < 0 || cx >= NX as i32 {
            return false;
        }
        if solid[index(cx as usize, y, z)] {
            return true;
        }
        cx += step;
    }
    false
}

fn solid_near_z(solid: &[bool], x: usize, y: usize, z: usize, step: i32, limit: i32) -> bool {
    let mut cz = z as i32 + step;
    for _ in 0..limit {
        if cz < 0 || cz >= NZ as i32 {
            return false;
        }
        if solid[index(x, y, cz as usize)] {
            return true;
        }
        cz += step;
    }
    false
}

/// Solid voxels, overhang voxels (solid with air below) and arch cells (air
/// under a roof with a support on both sides).
fn count_features(solid: &[bool]) -> (usize, usize, usize) {
    let mut voxels = 0;
    let mut overhangs = 0;
    let mut arches = 0;
    for z in 0..NZ {
        for y in 0..NY {
            for x in 0..NX {
                let i = index(x, y, z);
                if solid[i] {
                    voxels += 1;
                    if y > 0 && !solid[index(x, y - 1, z)] {
                        overhangs += 1;
                    }
                    continue;
                }
                if y + 1 >= NY || !solid[index(x, y + 1, z)] {
                    continue;
                }
                let legs_x =
                    solid_near_x(solid, x, y, z, -1, 16) && solid_near_x(solid, x, y, z, 1, 16);
                let legs_z =
                    solid_near_z(solid, x, y, z, -1, 16) && solid_near_z(solid, x, y, z, 1, 16);
                if legs_x || legs_z {
                    arches += 1;
                }
            }
        }
    }
    (voxels, overhangs, arches)
}

/// An arch fixture: ground, two legs and a span over an open passage.
///
/// The span is supported at both ends; the interior is air and open along `z`.
fn arch_fixture() -> (Vec<bool>, [usize; 3]) {
    let dims = [17, 12, 9];
    let mut solid = vec![false; dims[0] * dims[1] * dims[2]];
    let at = |x: usize, y: usize, z: usize| x + dims[0] * (y + dims[1] * z);

    for z in 0..dims[2] {
        for y in 0..=1 {
            for x in 0..dims[0] {
                solid[at(x, y, z)] = true;
            }
        }
    }
    for z in 2..=6 {
        for y in 2..=7 {
            solid[at(2, y, z)] = true;
            solid[at(3, y, z)] = true;
            solid[at(13, y, z)] = true;
            solid[at(14, y, z)] = true;
        }
    }
    for z in 2..=6 {
        for y in 7..=8 {
            for x in 2..=14 {
                solid[at(x, y, z)] = true;
            }
        }
    }
    (solid, dims)
}

/// An overhang shelf attached to a wall.
fn shelf_fixture() -> (Vec<bool>, [usize; 3]) {
    let dims = [12, 10, 7];
    let mut solid = vec![false; dims[0] * dims[1] * dims[2]];
    let at = |x: usize, y: usize, z: usize| x + dims[0] * (y + dims[1] * z);

    for z in 0..dims[2] {
        for y in 0..=1 {
            for x in 0..dims[0] {
                solid[at(x, y, z)] = true;
            }
        }
    }
    for z in 1..dims[2] - 1 {
        for y in 2..=7 {
            solid[at(0, y, z)] = true;
            solid[at(1, y, z)] = true;
        }
    }
    for z in 1..dims[2] - 1 {
        for y in 5..=6 {
            for x in 2..=8 {
                solid[at(x, y, z)] = true;
            }
        }
    }
    (solid, dims)
}

#[test]
fn overhangs_arch_fixture_passes() {
    let (mut solid, dims) = arch_fixture();
    let at = |x: usize, y: usize, z: usize| x + dims[0] * (y + dims[1] * z);

    // The span is solid, the passage under it is air, and both legs are solid.
    assert!(solid[at(8, 8, 4)], "span");
    assert!(!solid[at(8, 6, 4)], "passage");
    assert!(solid[at(2, 4, 4)], "left leg");
    assert!(solid[at(14, 4, 4)], "right leg");
    assert!(!solid[at(8, 4, 4)], "open middle");

    // Both legs reach the ground, so the fixture is fully supported.
    assert_eq!(prune_unsupported(&mut solid, dims), 0);

    // A floating block above the span is removed, and the arch survives.
    solid[at(8, 10, 4)] = true;
    assert_eq!(prune_unsupported(&mut solid, dims), 1);
    assert!(!solid[at(8, 10, 4)], "floating block");
    assert!(solid[at(8, 8, 4)], "arch span survives");
    assert_eq!(prune_unsupported(&mut solid, dims), 0);
}

#[test]
fn overhangs_shelf_fixture_is_supported() {
    let (mut solid, dims) = shelf_fixture();
    let at = |x: usize, y: usize, z: usize| x + dims[0] * (y + dims[1] * z);

    // The shelf projects from the wall over air.
    assert!(solid[at(5, 6, 3)], "shelf");
    assert!(!solid[at(5, 4, 3)], "air under the shelf");
    assert_eq!(prune_unsupported(&mut solid, dims), 0);

    // A detached shelf has no support and is removed.
    let mut detached = solid.clone();
    for z in 1..dims[2] - 1 {
        for y in 5..=6 {
            for x in 7..=10 {
                detached[at(x, y, z)] = true;
            }
        }
    }
    // Carve the bridge between the attached and detached shelves so the
    // detached shelf is genuinely separate.
    for z in 1..dims[2] - 1 {
        for y in 5..=6 {
            detached[at(5, y, z)] = false;
            detached[at(6, y, z)] = false;
        }
    }
    assert!(prune_unsupported(&mut detached, dims) > 0);
    assert!(!detached[at(8, 6, 3)], "detached shelf removed");
}

#[test]
fn overhangs_field_has_occasional_features() {
    let mut solid = build_field_mask(1, SURFACE);
    let (voxels, overhangs, arches) = count_features(&solid);
    assert!(voxels > 0);
    assert!(overhangs > 0, "no overhang in the field");
    assert!(arches > 0, "no arch in the field");
    assert!(
        overhangs * 10_000 / voxels >= 1,
        "overhangs are too rare: {overhangs}/{voxels}"
    );
    assert!(
        overhangs * 100 / voxels <= 5,
        "overhangs are too common: {overhangs}/{voxels}"
    );

    // The field is already supported, and pruning is idempotent.
    assert_eq!(prune_unsupported(&mut solid, [NX, NY, NZ]), 0);
    assert_eq!(prune_unsupported(&mut solid, [NX, NY, NZ]), 0);
    let (_, overhangs_after, arches_after) = count_features(&solid);
    assert!(overhangs_after > 0);
    assert!(arches_after > 0);
}

#[test]
fn overhangs_field_prunes_an_unsupported_lobe() {
    // Seed 2 produces a small detached lobe; the prune step is what guarantees
    // the surface never keeps it.
    let mut solid = build_field_mask(2, SURFACE);
    let removed = prune_unsupported(&mut solid, [NX, NY, NZ]);
    assert!(removed > 0, "the fixture needs a detached lobe");
    assert_eq!(
        prune_unsupported(&mut solid, [NX, NY, NZ]),
        0,
        "a second pass must be clean"
    );
    let (_, overhangs, arches) = count_features(&solid);
    assert!(overhangs > 0);
    assert!(arches > 0);
}

#[test]
fn overhangs_field_follows_the_surface() {
    let low = build_field_mask(3, 30);
    let high = build_field_mask(3, 50);
    assert_ne!(low, high, "the field must use the supplied surface");

    // The field is solid below the maximum downward perturbation and air
    // above the maximum rise.
    let field = OverhangField::new(3);
    for surface in [30, 50] {
        assert!(field.solid(
            metres(7),
            metres(surface - OVERHANG_AMPLITUDE_M - 1),
            metres(9),
            metres(surface)
        ));
        assert!(!field.solid(
            metres(7),
            metres(surface + OVERHANG_AMPLITUDE_M + 1),
            metres(9),
            metres(surface)
        ));
    }
    assert_eq!(field.seed(), 3);
    let _ = (OVERHANG_WAVELENGTH_XZ_M, OVERHANG_WAVELENGTH_Y_M);
}
