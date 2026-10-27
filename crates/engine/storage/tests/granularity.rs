//! Culling-granularity instrumentation tests (M1-34).
//!
//! The fixture is deliberately tiny and exact: two adjacent chunks along `x`.
//! The first contains only the last block layer against the boundary; the second
//! is solid. The occlusion fill treats that wall as an infinite opaque plane, so
//! the wall is visible, the second chunk is entirely occluded, and the expected
//! traversed, visible and missed counts can be written down without running the
//! traversal. That makes the ratio a hand-checked number, not a snapshot.
//!
//! The tests pin four things:
//!
//! 1. at LOD0 the report counts exactly one block per emitted node and separates
//!    the visible boundary wall from the occluded neighbour chunk;
//! 2. at a coarser LOD a node contributes its whole block volume, so the extra
//!    air inside the node is visible as granularity waste;
//! 3. a measurement is independent of chunk order and deterministic;
//! 4. over 100 poses the instrumentation emits a ratio per pose and never
//!    reports a reference-visible block as missed.

use core::f64::consts::{PI, TAU};

use voxel_core::constants::{BLOCK_DIM, BLOCK_LEVEL, CHUNK_DIM, CHUNK_VOLUME, VOXEL_SIZE};
use voxel_storage::dag::Svdag;
use voxel_storage::reference::cull::{Camera, ChunkView, Frustum, Vec3};
use voxel_storage::reference::granularity::{GRANULARITY_GATE_LIMIT, measure};
use voxel_storage::reference::occlusion::flood_fill;
use voxel_storage::{ChunkPos, local_index};

/// The number of poses that print a per-pose ratio.
const POSES: usize = 100;

/// A horizon large enough to enclose both fixture chunks at either chunk edge.
///
/// The production horizon is 128 blocks; the fixture is two chunks wide, so the
/// fill semantics are identical at a much smaller radius.
fn test_horizon() -> u32 {
    (2 * (CHUNK_DIM / BLOCK_DIM) + 4) as u32
}

#[test]
fn granularity_two_chunk_case_counts_exact_volumes() {
    let scene = two_chunk_scene();
    let views = views(&scene);
    let camera = scene_camera();
    let frustum = Frustum::from_camera(&camera);
    let fill = flood_fill(camera.eye, test_horizon(), wall_plane());

    let report = measure(&frustum, &views, &fill, BLOCK_LEVEL);

    let bpe = (CHUNK_DIM / BLOCK_DIM) as u64;
    let wall_blocks = bpe * bpe;
    let solid_blocks = bpe * bpe * bpe;

    assert_eq!(
        report.traversed_blocks(),
        wall_blocks + solid_blocks,
        "LOD0 must traverse exactly one block per emitted node"
    );
    assert_eq!(
        report.visible_blocks(),
        wall_blocks,
        "only the boundary wall is visible; the solid neighbour is occluded"
    );
    assert_eq!(report.invisible_blocks(), solid_blocks);
    assert_eq!(report.missed_blocks(), 0);
    assert!(report.is_conservative());
    assert_eq!(
        report.per_chunk().len(),
        2,
        "every input chunk gets a row, even when it emits nothing"
    );

    let first = report
        .chunk(ChunkPos::new(0, 0, 0))
        .expect("the wall chunk has a row");
    assert_eq!(first.traversed_blocks(), wall_blocks);
    assert_eq!(first.visible_blocks(), wall_blocks);
    assert_eq!(first.invisible_blocks(), 0);
    assert_eq!(first.ratio(), 0.0);

    let second = report
        .chunk(ChunkPos::new(1, 0, 0))
        .expect("the occluded chunk has a row");
    assert_eq!(second.traversed_blocks(), solid_blocks);
    assert_eq!(second.visible_blocks(), 0);
    assert_eq!(second.invisible_blocks(), solid_blocks);
    assert_eq!(second.ratio(), 1.0);

    let expected = solid_blocks as f64 / (wall_blocks + solid_blocks) as f64;
    assert!(
        (report.ratio() - expected).abs() < 1e-12,
        "ratio {} != {expected}",
        report.ratio()
    );
    assert!(
        report.ratio() > GRANULARITY_GATE_LIMIT,
        "the synthetic occlusion must be visible to the gate"
    );

    // Chunk order cannot change a set-based measurement.
    let mut reversed = views.clone();
    reversed.reverse();
    let reversed_report = measure(&frustum, &reversed, &fill, BLOCK_LEVEL);
    assert_eq!(report, reversed_report);

    println!(
        "granularity LOD0: traversed {} blocks, visible {}, invisible {}, ratio {:.2}%",
        report.traversed_blocks(),
        report.visible_blocks(),
        report.invisible_blocks(),
        report.ratio() * 100.0
    );
}

#[test]
fn granularity_coarse_lod_adds_air_inside_emitted_nodes() {
    let scene = two_chunk_scene();
    let views = views(&scene);
    let camera = scene_camera();
    let frustum = Frustum::from_camera(&camera);
    let fill = flood_fill(camera.eye, test_horizon(), wall_plane());

    let report = measure(&frustum, &views, &fill, BLOCK_LEVEL - 1);

    let bpe = (CHUNK_DIM / BLOCK_DIM) as u64;
    let wall_blocks = bpe * bpe;
    let solid_blocks = bpe * bpe * bpe;
    // A LOD1 node aggregates 2 x 2 x 2 blocks. The wall occupies the last block
    // on x, so only the y/z node pairs are non-empty; each also contains one
    // air block layer on x.
    let coarse_blocks_per_node = 8u64;
    let wall_nodes = (bpe / 2) * (bpe / 2);
    let wall_chunk_volume = wall_nodes * coarse_blocks_per_node;

    assert_eq!(
        report.traversed_blocks(),
        wall_chunk_volume + solid_blocks,
        "a coarse node contributes its full block volume"
    );
    assert_eq!(
        report.visible_blocks(),
        wall_blocks,
        "the reference-visible volume stays at block resolution"
    );
    assert_eq!(
        report.invisible_blocks(),
        wall_chunk_volume + solid_blocks - wall_blocks
    );
    assert_eq!(report.missed_blocks(), 0);

    let first = report
        .chunk(ChunkPos::new(0, 0, 0))
        .expect("the wall chunk has a row");
    assert_eq!(first.traversed_blocks(), wall_chunk_volume);
    assert_eq!(first.visible_blocks(), wall_blocks);
    assert_eq!(
        first.invisible_blocks(),
        wall_chunk_volume - wall_blocks,
        "the air half of each boundary node is granularity waste"
    );

    println!(
        "granularity LOD1: traversed {} blocks, visible {}, invisible {}, ratio {:.2}%",
        report.traversed_blocks(),
        report.visible_blocks(),
        report.invisible_blocks(),
        report.ratio() * 100.0
    );
}

#[test]
fn granularity_ratio_is_emitted_per_pose() {
    let scene = two_chunk_scene();
    let views = views(&scene);
    let edge = chunk_edge();
    let center = edge * 0.5;
    let is_opaque = wall_plane();
    let mut any_over_gate = false;

    for pose in 0..POSES {
        let angle = TAU * pose as f64 / POSES as f64;
        let eye = Vec3::new(
            -edge * 0.625,
            center + edge * 0.1 * angle.sin(),
            center + edge * 0.1 * angle.cos(),
        );
        let camera = Camera {
            eye,
            target: Vec3::new(edge * 1.5, center, center),
            up: Vec3::new(0.0, 1.0, 0.0),
            fov_y: PI / 2.0,
            aspect: 1.0,
            near: edge * 0.01,
            far: edge * 16.0,
        };
        let frustum = Frustum::from_camera(&camera);
        let fill = flood_fill(eye, test_horizon(), is_opaque);
        let report = measure(&frustum, &views, &fill, BLOCK_LEVEL);

        assert!(
            report.is_conservative(),
            "pose {pose}: the traversal missed a reference-visible block"
        );
        assert!(
            report.traversed_blocks() > 0,
            "pose {pose}: the fixture must stay inside the frustum"
        );
        assert!(
            (0.0..=1.0).contains(&report.ratio()),
            "pose {pose}: ratio {} is outside [0, 1]",
            report.ratio()
        );
        any_over_gate |= report.ratio() > GRANULARITY_GATE_LIMIT;

        println!(
            "granularity pose {pose:03}: traversed {:>4} blocks, visible {:>4}, \
             invisible {:>4}, ratio {:>6.2}%",
            report.traversed_blocks(),
            report.visible_blocks(),
            report.invisible_blocks(),
            report.ratio() * 100.0
        );
    }

    assert!(
        any_over_gate,
        "at least one pose must exercise the over-gate synthetic occlusion"
    );
}

#[test]
fn granularity_empty_chunk_reports_zero() {
    let empty = Svdag::empty();
    let view = ChunkView::new(ChunkPos::new(0, 0, 0), &empty);
    let camera = scene_camera();
    let frustum = Frustum::from_camera(&camera);
    let fill = flood_fill(camera.eye, test_horizon(), |_, _, _| false);

    let report = measure(&frustum, &[view], &fill, BLOCK_LEVEL);

    assert_eq!(report.traversed_blocks(), 0);
    assert_eq!(report.visible_blocks(), 0);
    assert_eq!(report.invisible_blocks(), 0);
    assert_eq!(report.missed_blocks(), 0);
    assert_eq!(report.ratio(), 0.0);
    assert!(report.is_conservative());
    assert_eq!(report.per_chunk().len(), 1);
    assert_eq!(report.per_chunk()[0].traversed_blocks(), 0);
}

/// Two adjacent chunks: the first has only its last block layer on `x`, the
/// second is solid.
fn two_chunk_scene() -> Vec<(ChunkPos, Svdag)> {
    let blocks_per_edge = CHUNK_DIM / BLOCK_DIM;
    let mut wall = vec![0u16; CHUNK_VOLUME];
    for bz in 0..blocks_per_edge {
        for by in 0..blocks_per_edge {
            fill_block(&mut wall, blocks_per_edge - 1, by, bz, 3);
        }
    }

    vec![
        (ChunkPos::new(0, 0, 0), Svdag::from_dense(&wall)),
        (
            ChunkPos::new(1, 0, 0),
            Svdag::from_dense(&vec![1u16; CHUNK_VOLUME]),
        ),
    ]
}

fn views<'a>(scene: &'a [(ChunkPos, Svdag)]) -> Vec<ChunkView<'a>> {
    scene
        .iter()
        .map(|(pos, dag)| ChunkView::new(*pos, dag))
        .collect()
}

/// The wall is the last block layer of the first chunk; everything beyond it is
/// treated as opaque, so the second chunk is sealed off.
fn wall_plane() -> impl Fn(i32, i32, i32) -> bool + Copy {
    let wall_x = (CHUNK_DIM / BLOCK_DIM - 1) as i32;
    move |x, _, _| x >= wall_x
}

fn scene_camera() -> Camera {
    let edge = chunk_edge();
    let center = edge * 0.5;
    Camera {
        eye: Vec3::new(-edge * 0.625, center, center),
        target: Vec3::new(edge * 1.5, center, center),
        up: Vec3::new(0.0, 1.0, 0.0),
        fov_y: PI / 2.0,
        aspect: 1.0,
        near: edge * 0.01,
        far: edge * 16.0,
    }
}

fn chunk_edge() -> f64 {
    CHUNK_DIM as f64 * VOXEL_SIZE as f64
}

fn fill_block(dense: &mut [u16], bx: usize, by: usize, bz: usize, material: u16) {
    for z in 0..BLOCK_DIM {
        for y in 0..BLOCK_DIM {
            for x in 0..BLOCK_DIM {
                let index = local_index(
                    (bx * BLOCK_DIM + x) as u32,
                    (by * BLOCK_DIM + y) as u32,
                    (bz * BLOCK_DIM + z) as u32,
                ) as usize;
                dense[index] = material;
            }
        }
    }
}
