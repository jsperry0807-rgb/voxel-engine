//! CPU hierarchical frustum reference traversal tests (M1-32).
//!
//! The tests pin four properties:
//!
//! 1. the plane/AABB and camera/frustum conventions are the conservative ones
//!    the traverser relies on;
//! 2. empty, offscreen and fully-inside chunks take the expected broad-phase
//!    path;
//! 3. the LOD cutoff emits exactly the nodes at `BLOCK_LEVEL - n` and their
//!    volume still adds up to one chunk;
//! 4. on **100 camera poses**, the hierarchical output set equals a
//!    brute-force per-block plane test over an independently dense-scanned
//!    block list.
//!
//! The fixture mixes shared solid chunks, a terrain surface crossing a vertical
//! chunk boundary, a sparse chunk and an empty chunk, so the parity test sees
//! both pruned octants and fully-inside subtrees. The final line prints the
//! pose/block totals so `cargo test -p voxel-storage --test reference_cull --
//! --nocapture` reports what the oracle covered.

use core::f64::consts::{PI, TAU};

use voxel_core::constants::{
    BLOCK_BITS, BLOCK_DIM, BLOCK_LEVEL, BLOCKS_PER_CHUNK, CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME,
    VOXEL_SIZE,
};
use voxel_storage::dag::Svdag;
use voxel_storage::reference::cull::{
    Aabb, Camera, ChunkView, Containment, Frustum, Plane, Vec3, VisibleNode, brute_force_blocks,
    nonempty_blocks, traverse,
};
use voxel_storage::{ChunkPos, local_index};

/// Number of poses required by M1-32.
const POSES: usize = 100;

/// One fixture chunk, keeping the dense source so the oracle can rescan it
/// independently of the SVDAG.
struct SceneChunk {
    pos: ChunkPos,
    dense: Vec<u16>,
    dag: Svdag,
}

#[test]
fn plane_test_classifies_boxes_and_keeps_tangent_boxes_visible() {
    let plane = Plane::new(Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO);
    assert!((plane.normal().x - 1.0).abs() < 1e-15);
    assert!(plane.offset().abs() < 1e-15);

    assert_eq!(
        plane.test_aabb(box_at(Vec3::new(1.5, 0.0, 0.0), 0.5)),
        Containment::Inside
    );
    assert_eq!(
        plane.test_aabb(box_at(Vec3::new(-1.5, 0.0, 0.0), 0.5)),
        Containment::Outside
    );
    assert_eq!(
        plane.test_aabb(box_at(Vec3::ZERO, 0.5)),
        Containment::Intersecting
    );
    // A box whose maximum face lies exactly on the plane is not culled; a
    // culler may over-include, never drop a touching block.
    assert_eq!(
        plane.test_aabb(box_at(Vec3::new(-0.5, 0.0, 0.0), 0.5)),
        Containment::Intersecting
    );
}

#[test]
fn camera_frustum_matches_hand_computed_regions() {
    let camera = Camera {
        eye: Vec3::ZERO,
        target: Vec3::new(0.0, 0.0, 1.0),
        up: Vec3::new(0.0, 1.0, 0.0),
        fov_y: PI / 2.0,
        aspect: 1.0,
        near: 1.0,
        far: 10.0,
    };
    let frustum = Frustum::from_camera(&camera);

    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(0.0, 0.0, 5.0), 0.5)),
        Containment::Inside
    );
    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(0.0, 0.0, -5.0), 0.5)),
        Containment::Outside,
        "behind the near plane"
    );
    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(0.0, 0.0, 11.0), 0.5)),
        Containment::Outside,
        "beyond the far plane"
    );
    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(-7.0, 0.0, 5.0), 0.5)),
        Containment::Outside,
        "left of the 90-degree side plane"
    );
    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(7.0, 0.0, 5.0), 0.5)),
        Containment::Outside,
        "right of the 90-degree side plane"
    );
    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(0.0, 7.0, 5.0), 0.5)),
        Containment::Outside,
        "above the 90-degree top plane"
    );
    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(0.0, -7.0, 5.0), 0.5)),
        Containment::Outside,
        "below the 90-degree bottom plane"
    );
    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(0.0, 0.0, 1.0), 0.5)),
        Containment::Intersecting,
        "straddling the near plane"
    );
    assert_eq!(
        frustum.test_aabb(box_at(Vec3::new(0.0, 0.0, 0.5), 0.2)),
        Containment::Outside,
        "entirely in front of the near plane"
    );
}

#[test]
fn empty_and_offscreen_chunks_emit_nothing() {
    let edge = chunk_edge();

    let empty = Svdag::empty();
    let empty_view = ChunkView::new(ChunkPos::new(0, 0, 0), &empty);
    let origin_camera = camera_looking_at(
        Vec3::new(edge * 0.5, edge * 0.5, edge * 0.5),
        Vec3::new(edge * 0.5, edge * 0.5, -edge * 8.0),
        edge,
    );
    let frustum = Frustum::from_camera(&origin_camera);
    assert!(traverse(&frustum, &[empty_view], BLOCK_LEVEL).is_empty());
    assert!(brute_force_blocks(&frustum, &[empty_view]).is_empty());

    let solid = Svdag::from_dense(&vec![1u16; CHUNK_VOLUME]);
    let solid_view = ChunkView::new(ChunkPos::new(0, 0, 0), &solid);
    let away = Camera {
        eye: Vec3::new(edge * 0.5, edge * 0.5, -edge * 2.0),
        target: Vec3::new(edge * 0.5, edge * 0.5, -edge * 10.0),
        up: Vec3::new(0.0, 1.0, 0.0),
        fov_y: PI / 3.0,
        aspect: 16.0 / 9.0,
        near: edge * 0.01,
        far: edge * 32.0,
    };
    let frustum = Frustum::from_camera(&away);
    assert!(
        traverse(&frustum, &[solid_view], BLOCK_LEVEL).is_empty(),
        "a chunk behind the camera is pruned by the root broad phase"
    );
}

#[test]
fn lod_cutoff_emits_the_expected_node_level_and_volume() {
    let edge = chunk_edge();
    let pos = ChunkPos::new(0, 0, 0);
    let solid = Svdag::from_dense(&vec![1u16; CHUNK_VOLUME]);
    let view = ChunkView::new(pos, &solid);

    let center = Vec3::new(edge * 0.5, edge * 0.5, edge * 0.5);
    let camera = camera_looking_at(center, center - Vec3::new(0.0, 0.0, edge * 10.0), edge);
    let frustum = Frustum::from_camera(&camera);

    let root = VisibleNode::new(pos, CHUNK_BITS, [0; 3]);
    assert_eq!(
        frustum.test_aabb(root.aabb()),
        Containment::Inside,
        "the fixture camera must contain the whole chunk to exercise the inside-skip path"
    );

    for lod_max_depth in 0..=BLOCK_LEVEL {
        let nodes = traverse(&frustum, &[view], lod_max_depth);
        let expected_axis = 1usize << lod_max_depth;
        let expected_nodes = expected_axis * expected_axis * expected_axis;
        let expected_level = CHUNK_BITS - lod_max_depth;

        assert_eq!(
            nodes.len(),
            expected_nodes,
            "lod_max_depth {lod_max_depth} node count"
        );
        for node in &nodes {
            assert_eq!(node.level(), expected_level);
            assert_eq!(node.depth(), lod_max_depth);
            assert_eq!(node.extent_voxels() as usize, CHUNK_DIM >> lod_max_depth);
            assert_eq!(node.chunk(), pos);
        }

        let volume: u64 = nodes.iter().map(|node| node.volume_voxels()).sum();
        assert_eq!(
            volume, CHUNK_VOLUME as u64,
            "the cutoff must cover the chunk"
        );
        let blocks: u64 = nodes
            .iter()
            .map(|node| node.volume_voxels() / (BLOCK_DIM as u64).pow(3))
            .sum();
        assert_eq!(blocks, BLOCKS_PER_CHUNK as u64);

        if lod_max_depth == BLOCK_LEVEL {
            assert_eq!(nodes, nonempty_blocks(&solid, pos));
            assert!(nodes.iter().all(|node| node.block_index().is_some()));
        }
    }
}

#[test]
fn nonempty_blocks_match_an_independent_dense_scan() {
    for chunk in scene() {
        assert_eq!(
            nonempty_blocks(&chunk.dag, chunk.pos),
            blocks_from_dense(&chunk.dense, chunk.pos),
            "chunk {:?}",
            chunk.pos
        );
    }
}

#[test]
fn output_set_equals_brute_force_per_block_on_one_hundred_poses() {
    let scene = scene();
    let views: Vec<ChunkView<'_>> = scene
        .iter()
        .map(|chunk| ChunkView::new(chunk.pos, &chunk.dag))
        .collect();
    let blocks_by_chunk: Vec<Vec<VisibleNode>> = scene
        .iter()
        .map(|chunk| blocks_from_dense(&chunk.dense, chunk.pos))
        .collect();
    let total_blocks: usize = blocks_by_chunk.iter().map(Vec::len).sum();
    assert!(total_blocks > 0, "the fixture must contain blocks");

    let mut block_hits = 0usize;
    let mut hit_poses = 0usize;
    let mut empty_poses = 0usize;

    for (pose_index, camera) in poses(POSES).into_iter().enumerate() {
        let frustum = Frustum::from_camera(&camera);

        // Independent oracle: scan the dense source once, then test every
        // non-empty block box individually.
        let mut expected: Vec<VisibleNode> = blocks_by_chunk
            .iter()
            .flatten()
            .copied()
            .filter(|block| frustum.intersects(block.aabb()))
            .collect();
        expected.sort_unstable();

        let actual = traverse(&frustum, &views, BLOCK_LEVEL);
        assert_eq!(
            actual, expected,
            "pose {pose_index}: hierarchical traversal differs from the per-block plane test"
        );
        assert_eq!(
            brute_force_blocks(&frustum, &views),
            expected,
            "pose {pose_index}: the brute-force helper disagrees with the dense oracle"
        );

        for node in &actual {
            assert_eq!(node.level(), BLOCK_BITS);
            assert!(node.block_index().is_some());
            assert!(
                node.block_index().unwrap() < BLOCKS_PER_CHUNK as u32,
                "block index leaves the chunk"
            );
        }

        block_hits += actual.len();
        if actual.is_empty() {
            empty_poses += 1;
        } else {
            hit_poses += 1;
        }
    }

    assert!(hit_poses > 0, "no pose saw any block");
    assert!(empty_poses > 0, "no pose exercised a fully culled scene");
    assert!(
        block_hits < total_blocks * POSES,
        "the fixture never culled a block"
    );

    println!(
        "reference_cull: {POSES} poses, {total_blocks} non-empty blocks, \
         {block_hits} block hits, {hit_poses} hit / {empty_poses} empty poses"
    );
}

#[test]
fn chunk_order_does_not_change_the_output_set() {
    let scene = scene();
    let mut views: Vec<ChunkView<'_>> = scene
        .iter()
        .map(|chunk| ChunkView::new(chunk.pos, &chunk.dag))
        .collect();
    let camera = poses(1).remove(0);
    let frustum = Frustum::from_camera(&camera);

    let forward = traverse(&frustum, &views, BLOCK_LEVEL);
    views.reverse();
    let backward = traverse(&frustum, &views, BLOCK_LEVEL);
    assert_eq!(forward, backward);
}

/// Builds the eleven-chunk fixture: eight terrain chunks, one negative-position
/// terrain chunk, one sparse chunk and one empty chunk.
fn scene() -> Vec<SceneChunk> {
    let mut chunks = Vec::new();
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let pos = ChunkPos::new(x, y, z);
                let dense = terrain(pos);
                let dag = Svdag::from_dense(&dense);
                chunks.push(SceneChunk { pos, dense, dag });
            }
        }
    }

    let pos = ChunkPos::new(0, 2, 0);
    let dense = sparse_chunk();
    let dag = Svdag::from_dense(&dense);
    chunks.push(SceneChunk { pos, dense, dag });

    // A non-empty chunk at a negative chunk position exercises negative world
    // origins; the empty chunk next to it exercises the broad phase.
    let pos = ChunkPos::new(-1, 0, 0);
    let dense = terrain(pos);
    let dag = Svdag::from_dense(&dense);
    chunks.push(SceneChunk { pos, dense, dag });

    chunks.push(SceneChunk {
        pos: ChunkPos::new(-2, 0, 0),
        dense: vec![0u16; CHUNK_VOLUME],
        dag: Svdag::empty(),
    });
    chunks
}

/// A terrain surface around 1.25 chunk edges above y = 0.
///
/// The bottom layer is solid everywhere; the layer above it carries the wavy
/// surface, so both fully-solid and sparse blocks occur. Materials vary with
/// depth so the DAG does not collapse every bottom chunk to one leaf.
fn terrain(pos: ChunkPos) -> Vec<u16> {
    let mut dense = vec![0u16; CHUNK_VOLUME];
    let base = CHUNK_DIM as i32 + CHUNK_DIM as i32 / 4;
    let origin = [
        pos.x * CHUNK_DIM as i32,
        pos.y * CHUNK_DIM as i32,
        pos.z * CHUNK_DIM as i32,
    ];

    for z in 0..CHUNK_DIM {
        for y in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let wx = origin[0] + x as i32;
                let wy = origin[1] + y as i32;
                let wz = origin[2] + z as i32;
                let wave = (wx * 7 + wz * 11).rem_euclid(9) - 4;
                let height = base + wave;
                if wy < height {
                    let depth = height - wy;
                    let material = match depth {
                        0..=1 => 2,
                        2..=8 => 1,
                        _ => 3,
                    };
                    dense[local_index(x as u32, y as u32, z as u32) as usize] = material;
                }
            }
        }
    }
    dense
}

/// Three filled blocks and one isolated voxel, all separated by air.
fn sparse_chunk() -> Vec<u16> {
    let mut dense = vec![0u16; CHUNK_VOLUME];
    let blocks = CHUNK_DIM / BLOCK_DIM;
    for (bx, by, bz) in [
        (0, 0, 0),
        (blocks - 1, 0, blocks - 1),
        (blocks / 2, blocks - 1, blocks / 2),
    ] {
        fill_block(&mut dense, bx, by, bz, 5);
    }

    // A single voxel makes its block non-empty without filling the block.
    let x = BLOCK_DIM as u32 + 3;
    let y = BLOCK_DIM as u32 + 5;
    let z = BLOCK_DIM as u32 + 7;
    dense[local_index(x, y, z) as usize] = 6;
    dense
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

/// Enumerates non-empty blocks from the dense source, with no SVDAG help.
fn blocks_from_dense(dense: &[u16], pos: ChunkPos) -> Vec<VisibleNode> {
    assert_eq!(dense.len(), CHUNK_VOLUME);
    let blocks = CHUNK_DIM / BLOCK_DIM;
    let mut out = Vec::new();
    for bz in 0..blocks {
        for by in 0..blocks {
            for bx in 0..blocks {
                let mut nonempty = false;
                'block: for z in 0..BLOCK_DIM {
                    for y in 0..BLOCK_DIM {
                        for x in 0..BLOCK_DIM {
                            let index = local_index(
                                (bx * BLOCK_DIM + x) as u32,
                                (by * BLOCK_DIM + y) as u32,
                                (bz * BLOCK_DIM + z) as u32,
                            ) as usize;
                            if dense[index] != 0 {
                                nonempty = true;
                                break 'block;
                            }
                        }
                    }
                }
                if nonempty {
                    out.push(VisibleNode::new(
                        pos,
                        BLOCK_BITS,
                        [
                            (bx * BLOCK_DIM) as u32,
                            (by * BLOCK_DIM) as u32,
                            (bz * BLOCK_DIM) as u32,
                        ],
                    ));
                }
            }
        }
    }
    out.sort_unstable();
    out
}

/// Deterministic camera poses around the fixture.
///
/// A quarter of the poses sit inside or just outside the scene; the rest orbit
/// it. Half the targets are deliberately far from the centre, so the set
/// contains both hit and fully-missed poses. `up` is chosen away from the view
/// direction so a steep pose cannot degenerate the basis.
fn poses(count: usize) -> Vec<Camera> {
    let edge = chunk_edge();
    let center = Vec3::new(edge, edge * 1.25, edge);
    let mut state = 0x9e37_79b9_7f4a_7c15u64;

    (0..count)
        .map(|index| {
            let azimuth = range(&mut state, 0.0, TAU);
            let elevation = range(&mut state, -0.9, 1.1);
            let radius = if index % 4 == 0 {
                range(&mut state, edge * 0.25, edge * 1.5)
            } else {
                range(&mut state, edge * 2.0, edge * 12.0)
            };
            let eye = center
                + Vec3::new(
                    radius * elevation.cos() * azimuth.cos(),
                    radius * elevation.sin(),
                    radius * elevation.cos() * azimuth.sin(),
                );

            let spread = if index % 3 == 0 {
                edge * 6.0
            } else {
                edge * 2.5
            };
            let mut target = center
                + Vec3::new(
                    range(&mut state, -spread, spread),
                    range(&mut state, -spread * 0.4, spread * 0.4),
                    range(&mut state, -spread, spread),
                );
            if (target - eye).length() < edge * 0.1 {
                target = center;
            }

            let forward = (target - eye).normalized();
            let up = if forward.y.abs() > 0.95 {
                Vec3::new(1.0, 0.0, 0.0)
            } else {
                Vec3::new(0.0, 1.0, 0.0)
            };

            Camera {
                eye,
                target,
                up,
                fov_y: 60.0_f64.to_radians(),
                aspect: 16.0 / 9.0,
                near: edge * 0.01,
                far: edge * 32.0,
            }
        })
        .collect()
}

/// World-space chunk edge in metres.
fn chunk_edge() -> f64 {
    CHUNK_DIM as f64 * VOXEL_SIZE as f64
}

fn camera_looking_at(center: Vec3, eye: Vec3, edge: f64) -> Camera {
    Camera {
        eye,
        target: center,
        up: Vec3::new(0.0, 1.0, 0.0),
        fov_y: PI / 2.0,
        aspect: 1.0,
        near: edge * 0.001,
        far: edge * 30.0,
    }
}

fn box_at(center: Vec3, half: f64) -> Aabb {
    Aabb::new(
        center - Vec3::new(half, half, half),
        center + Vec3::new(half, half, half),
    )
}

fn range(state: &mut u64, low: f64, high: f64) -> f64 {
    low + (high - low) * unit(state)
}

fn unit(state: &mut u64) -> f64 {
    (next(state) >> 11) as f64 / (1u64 << 53) as f64
}

fn next(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}
