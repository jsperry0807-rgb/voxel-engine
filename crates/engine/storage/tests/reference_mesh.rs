//! Count-only greedy mesher reference tests (M1-31).
//!
//! Five hand-computed fixtures pin the two halves of the oracle:
//!
//! 1. an isolated voxel — six exposed faces become six quads, one per
//!    orientation;
//! 2. two adjacent same-material voxels — coplanar faces merge, so ten exposed
//!    faces become six quads;
//! 3. a solid 16³ block — each whole block face is one quad, so 1,536 exposed
//!    faces become six quads;
//! 4. two adjacent solid blocks — merging stops at the block boundary, so the
//!    ten-block-face surface is ten quads rather than six;
//! 5. a block split at x = 8 by material — the top, bottom and both z faces
//!    split at the material change while the x faces stay whole.
//!
//! For every fixture the test checks the greedy quad count against the expected
//! cover, then checks the covered face area per orientation against an
//! independent brute-force face counter. A separate test covers a full solid
//! compiled chunk: its six outer planes are split into one quad per block face
//! by the ADR-010 grain, and the dense and SVDAG entry points must agree.
//!
//! Every fixture prints its visible-face count, quad count, merge ratio and byte
//! cost, so `cargo test -p voxel-storage --test reference_mesh -- --nocapture`
//! reports the merge ratio required by M1-31.

use voxel_core::Face;
use voxel_core::constants::{BLOCK_DIM, CHUNK_DIM, CHUNK_VOLUME};
use voxel_storage::dag::Svdag;
use voxel_storage::reference::mesh::{
    MeshCounts, QUAD_BYTES, count_exposed_faces_dense, count_exposed_faces_svdag,
    count_greedy_dense, count_greedy_svdag,
};

/// Scene edge for the five fixtures: four blocks on each axis.
///
/// It is fixed at the M1 baseline independently of the compiled chunk edge, so
/// the literal counts stay stable under `chunk128`.
const EDGE: usize = 4 * BLOCK_DIM;

/// Voxels in one fixture scene.
const VOLUME: usize = EDGE * EDGE * EDGE;

/// One hand-computed scene and its expected greedy cover.
struct Fixture {
    name: &'static str,
    voxels: Vec<u16>,
    /// Expected quads, indexed by [`Face::index`].
    quads: [usize; 6],
    /// Expected covered voxel faces, indexed by [`Face::index`].
    faces: [usize; 6],
}

impl Fixture {
    fn total_quads(&self) -> usize {
        self.quads.iter().sum()
    }

    fn total_faces(&self) -> usize {
        self.faces.iter().sum()
    }
}

/// Builds the five hand-computed fixtures.
fn fixtures() -> Vec<Fixture> {
    let mut isolated = vec![0u16; VOLUME];
    set(&mut isolated, 8, 8, 8, 1);

    let mut adjacent_voxels = vec![0u16; VOLUME];
    set(&mut adjacent_voxels, 8, 8, 8, 1);
    set(&mut adjacent_voxels, 9, 8, 8, 1);

    let mut solid_block = vec![0u16; VOLUME];
    fill_block(&mut solid_block, 0, 0, 0, 1);

    let mut adjacent_blocks = vec![0u16; VOLUME];
    fill_block(&mut adjacent_blocks, 0, 0, 0, 1);
    fill_block(&mut adjacent_blocks, 1, 0, 0, 1);

    let mut stripe = vec![0u16; VOLUME];
    for z in 0..BLOCK_DIM {
        for y in 0..BLOCK_DIM {
            for x in 0..BLOCK_DIM {
                let material = if x < BLOCK_DIM / 2 { 1 } else { 2 };
                set(&mut stripe, x, y, z, material);
            }
        }
    }

    vec![
        Fixture {
            name: "isolated_voxel",
            voxels: isolated,
            quads: all_faces(1),
            faces: all_faces(1),
        },
        Fixture {
            name: "two_adjacent_voxels",
            voxels: adjacent_voxels,
            quads: all_faces(1),
            faces: per_face([
                (Face::North, 2),
                (Face::South, 2),
                (Face::East, 1),
                (Face::West, 1),
                (Face::Top, 2),
                (Face::Bottom, 2),
            ]),
        },
        Fixture {
            name: "solid_block",
            voxels: solid_block,
            quads: all_faces(1),
            faces: all_faces(BLOCK_DIM * BLOCK_DIM),
        },
        Fixture {
            name: "two_adjacent_blocks",
            voxels: adjacent_blocks,
            quads: per_face([
                (Face::North, 2),
                (Face::South, 2),
                (Face::East, 1),
                (Face::West, 1),
                (Face::Top, 2),
                (Face::Bottom, 2),
            ]),
            faces: per_face([
                (Face::North, 2 * BLOCK_DIM * BLOCK_DIM),
                (Face::South, 2 * BLOCK_DIM * BLOCK_DIM),
                (Face::East, BLOCK_DIM * BLOCK_DIM),
                (Face::West, BLOCK_DIM * BLOCK_DIM),
                (Face::Top, 2 * BLOCK_DIM * BLOCK_DIM),
                (Face::Bottom, 2 * BLOCK_DIM * BLOCK_DIM),
            ]),
        },
        Fixture {
            name: "material_stripe_block",
            voxels: stripe,
            quads: per_face([
                (Face::North, 2),
                (Face::South, 2),
                (Face::East, 1),
                (Face::West, 1),
                (Face::Top, 2),
                (Face::Bottom, 2),
            ]),
            faces: all_faces(BLOCK_DIM * BLOCK_DIM),
        },
    ]
}

#[test]
fn greedy_quads_match_the_hand_counts_on_five_fixtures() {
    for fixture in fixtures() {
        let greedy = count_greedy_dense(EDGE, &fixture.voxels);
        let brute = count_exposed_faces_dense(EDGE, &fixture.voxels);
        let name = fixture.name;

        assert_eq!(
            greedy.quads(),
            fixture.total_quads(),
            "{name}: total quad count"
        );
        assert_eq!(
            greedy.covered_faces(),
            fixture.total_faces(),
            "{name}: covered face count"
        );
        assert_eq!(
            brute.total(),
            fixture.total_faces(),
            "{name}: brute-force face count"
        );

        for face in Face::ALL {
            assert_eq!(
                brute.get(face),
                fixture.faces[face.index()],
                "{name}: brute-force {face} faces"
            );
            assert_eq!(
                greedy.covered_faces_for(face),
                brute.get(face),
                "{name}: greedy {face} coverage"
            );
            assert_eq!(
                greedy.quads_for(face),
                fixture.quads[face.index()],
                "{name}: {face} quads"
            );
        }

        // The byte count is the frozen indexed-quad payload, independent of
        // how the quads are oriented or merged.
        assert_eq!(greedy.bytes(), greedy.quads() * QUAD_BYTES);
        assert_eq!(greedy.vertex_bytes() + greedy.index_bytes(), greedy.bytes());
        assert!(
            greedy.quads() <= brute.total(),
            "{name}: merging cannot add quads"
        );
        let expected_ratio = greedy.quads() as f64 / greedy.covered_faces() as f64;
        assert!((greedy.merge_ratio() - expected_ratio).abs() < 1e-12);
        assert!((greedy.merge_saving() - (1.0 - expected_ratio)).abs() < 1e-12);

        println!(
            "{:<22} faces={:5} quads={:4} merge={:6.1}% bytes={:6}",
            name,
            brute.total(),
            greedy.quads(),
            greedy.merge_saving() * 100.0,
            greedy.bytes(),
        );
    }
}

#[test]
fn isolated_voxel_has_one_outward_quad_per_face() {
    let mut voxels = vec![0u16; VOLUME];
    set(&mut voxels, 8, 8, 8, 7);

    let counts = count_greedy_dense(EDGE, &voxels);
    for face in Face::ALL {
        assert_eq!(counts.quads_for(face), 1, "{face}");
        assert_eq!(counts.covered_faces_for(face), 1, "{face}");
    }
    assert_eq!(counts.quads(), 6);
    assert_eq!(counts.bytes(), 6 * QUAD_BYTES);
    assert_eq!(counts.merge_ratio(), 1.0, "no merging is possible");
}

#[test]
fn empty_scene_counts_nothing() {
    let voxels = vec![0u16; VOLUME];
    let greedy = count_greedy_dense(EDGE, &voxels);
    let brute = count_exposed_faces_dense(EDGE, &voxels);

    assert_eq!(greedy, MeshCounts::EMPTY);
    assert_eq!(greedy.quads(), 0);
    assert_eq!(greedy.bytes(), 0);
    assert_eq!(greedy.merge_ratio(), 1.0);
    assert_eq!(brute.total(), 0);
}

#[test]
fn compiled_chunk_svdag_matches_dense_and_respects_block_grain() {
    // Two adjacent solid blocks at the compiled chunk edge. All remaining
    // blocks are air, so the test is cheap even with `chunk128` enabled.
    let mut voxels = vec![0u16; CHUNK_VOLUME];
    fill_block_at(&mut voxels, CHUNK_DIM, 0, 0, 0, 1);
    fill_block_at(&mut voxels, CHUNK_DIM, 1, 0, 0, 1);

    let dense = count_greedy_dense(CHUNK_DIM, &voxels);
    let brute = count_exposed_faces_dense(CHUNK_DIM, &voxels);
    let block_faces = 2 * BLOCK_DIM * BLOCK_DIM;
    let side_faces = BLOCK_DIM * BLOCK_DIM;

    assert_eq!(dense.quads(), 10);
    assert_eq!(dense.covered_faces(), 2560);
    assert_eq!(brute.total(), dense.covered_faces());
    let expected_quads = per_face([
        (Face::North, 2),
        (Face::South, 2),
        (Face::East, 1),
        (Face::West, 1),
        (Face::Top, 2),
        (Face::Bottom, 2),
    ]);
    let expected_faces = per_face([
        (Face::North, block_faces),
        (Face::South, block_faces),
        (Face::East, side_faces),
        (Face::West, side_faces),
        (Face::Top, block_faces),
        (Face::Bottom, block_faces),
    ]);
    for face in Face::ALL {
        assert_eq!(
            dense.quads_for(face),
            expected_quads[face.index()],
            "{face}"
        );
        assert_eq!(
            dense.covered_faces_for(face),
            expected_faces[face.index()],
            "{face}"
        );
    }

    let dag = Svdag::from_dense(&voxels);
    assert_eq!(count_greedy_svdag(&dag), dense);
    assert_eq!(count_exposed_faces_svdag(&dag), brute);
}

fn all_faces(value: usize) -> [usize; 6] {
    [value; 6]
}

fn per_face(entries: [(Face, usize); 6]) -> [usize; 6] {
    let mut counts = [0usize; 6];
    for (face, value) in entries {
        counts[face.index()] = value;
    }
    counts
}

fn set(voxels: &mut [u16], x: usize, y: usize, z: usize, material: u16) {
    voxels[index(x, y, z)] = material;
}

fn fill_block(voxels: &mut [u16], bx: usize, by: usize, bz: usize, material: u16) {
    fill_block_at(voxels, EDGE, bx, by, bz, material);
}

fn fill_block_at(voxels: &mut [u16], edge: usize, bx: usize, by: usize, bz: usize, material: u16) {
    let origin = [bx * BLOCK_DIM, by * BLOCK_DIM, bz * BLOCK_DIM];
    for z in 0..BLOCK_DIM {
        for y in 0..BLOCK_DIM {
            for x in 0..BLOCK_DIM {
                let index = index_at(edge, origin[0] + x, origin[1] + y, origin[2] + z);
                voxels[index] = material;
            }
        }
    }
}

fn index(x: usize, y: usize, z: usize) -> usize {
    index_at(EDGE, x, y, z)
}

fn index_at(edge: usize, x: usize, y: usize, z: usize) -> usize {
    x + y * edge + z * edge * edge
}
