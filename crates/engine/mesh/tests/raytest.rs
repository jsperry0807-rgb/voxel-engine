//! M2-25: the optional SVDAG ray-test occluder.
//!
//! The flag (`occlusion.raytest`) is off by default and the code is gated by the
//! `raytest` cargo feature. The scene-comparison tests compare the ray-test's
//! visible set with the M1-33 flood fill on the same scenes:
//!
//! - the ray-test is a **subset** of the fill (line of sight implies
//!   connectivity), so it can never open a hole the fill closed;
//! - open scenes, an enclosed room and a floor agree or show the documented
//!   delta (the fill reaches around an obstacle, the ray does not);
//! - the SVDAG-backed scene produces the same bits as a dense scene, which pins
//!   the chunk/voxel mapping, including negative chunks and multiple chunks;
//! - a material-aware scene keeps water transparent;
//! - a camera inside solid and a camera on a block boundary are both handled.

#![cfg(feature = "raytest")]

use std::collections::HashMap;

use voxel_core::constants::{BLOCK_DIM, CHUNK_DIM, CHUNK_VOLUME};
use voxel_mesh::raytest::{
    BLOCK_EDGE_M, DEFAULT_HORIZON_BLOCKS, RayTestConfig, RayTestFill, SvdagScene, VoxelScene,
    ray_test_scene,
};
use voxel_storage::dag::Svdag;
use voxel_storage::reference::cull::Vec3;
use voxel_storage::reference::occlusion;
use voxel_storage::{AIR, ChunkPos, local_index};

/// Small enough to keep the `O(horizon⁴)` ray-test quick, large enough to reach
/// around a one-block pillar.
const HORIZON: u32 = 4;

/// Blocks per chunk edge at the compiled edge.
fn blocks_per_edge() -> i32 {
    (CHUNK_DIM / BLOCK_DIM) as i32
}

/// Resident chunks, as dense materials and as SVDAGs.
struct Scene {
    dense: HashMap<ChunkPos, Vec<u16>>,
    dags: HashMap<ChunkPos, Svdag>,
}

impl Scene {
    fn empty() -> Self {
        Self {
            dense: HashMap::new(),
            dags: HashMap::new(),
        }
    }

    /// Adds one chunk; `material(bx, by, bz)` is in chunk-local block units.
    fn add(&mut self, pos: ChunkPos, material: impl Fn(i32, i32, i32) -> u16) {
        let blocks = blocks_per_edge();
        let edge = BLOCK_DIM as i32;
        let mut dense = vec![AIR; CHUNK_VOLUME];
        for bz in 0..blocks {
            for by in 0..blocks {
                for bx in 0..blocks {
                    let value = material(bx, by, bz);
                    if value == AIR {
                        continue;
                    }
                    for z in 0..edge {
                        for y in 0..edge {
                            for x in 0..edge {
                                let index = local_index(
                                    (bx * edge + x) as u32,
                                    (by * edge + y) as u32,
                                    (bz * edge + z) as u32,
                                );
                                dense[index as usize] = value;
                            }
                        }
                    }
                }
            }
        }
        self.dags.insert(pos, Svdag::from_dense(&dense));
        self.dense.insert(pos, dense);
    }

    fn material(&self, x: i32, y: i32, z: i32) -> u16 {
        let chunk = CHUNK_DIM as i32;
        let pos = ChunkPos::new(
            x.div_euclid(chunk),
            y.div_euclid(chunk),
            z.div_euclid(chunk),
        );
        let Some(dense) = self.dense.get(&pos) else {
            return AIR;
        };
        let index = local_index(
            x.rem_euclid(chunk) as u32,
            y.rem_euclid(chunk) as u32,
            z.rem_euclid(chunk) as u32,
        );
        dense[index as usize]
    }

    fn refs(&self) -> HashMap<ChunkPos, &Svdag> {
        self.dags.iter().map(|(pos, dag)| (*pos, dag)).collect()
    }
}

impl VoxelScene for Scene {
    fn opaque(&self, x: i32, y: i32, z: i32) -> bool {
        self.material(x, y, z) != AIR
    }
}

fn scene_with(pos: ChunkPos, material: impl Fn(i32, i32, i32) -> u16) -> Scene {
    let mut scene = Scene::empty();
    scene.add(pos, material);
    scene
}

fn scene(solid: impl Fn(i32, i32, i32) -> bool) -> Scene {
    scene_with(ChunkPos::new(0, 0, 0), |bx, by, bz| {
        if solid(bx, by, bz) { 1 } else { AIR }
    })
}

/// The flood fill's rule: only a block that is fully opaque blocks the fill.
fn block_fully_opaque(scene: &Scene, block: [i32; 3], is_opaque: &impl Fn(u16) -> bool) -> bool {
    let edge = BLOCK_DIM as i32;
    for z in 0..edge {
        for y in 0..edge {
            for x in 0..edge {
                if !is_opaque(scene.material(
                    block[0] * edge + x,
                    block[1] * edge + y,
                    block[2] * edge + z,
                )) {
                    return false;
                }
            }
        }
    }
    true
}

fn block_centre(block: [i32; 3]) -> [f64; 3] {
    [
        (block[0] as f64 + 0.5) * BLOCK_EDGE_M,
        (block[1] as f64 + 0.5) * BLOCK_EDGE_M,
        (block[2] as f64 + 0.5) * BLOCK_EDGE_M,
    ]
}

fn flood_with(
    scene: &Scene,
    camera: [f64; 3],
    is_opaque: impl Fn(u16) -> bool,
) -> occlusion::OcclusionFill {
    occlusion::flood_fill(
        Vec3::new(camera[0], camera[1], camera[2]),
        HORIZON,
        |x, y, z| block_fully_opaque(scene, [x, y, z], &is_opaque),
    )
}

fn flood(scene: &Scene, camera: [f64; 3]) -> occlusion::OcclusionFill {
    flood_with(scene, camera, |material| material != AIR)
}

fn ray_svdag_with(scene: &Scene, camera: [f64; 3], is_opaque: impl Fn(u16) -> bool) -> RayTestFill {
    let chunks = scene.refs();
    SvdagScene::with_opaque_material(&chunks, is_opaque).ray_test(camera, HORIZON)
}

fn ray_svdag(scene: &Scene, camera: [f64; 3]) -> RayTestFill {
    ray_svdag_with(scene, camera, |material| material != AIR)
}

/// Blocks the fill marks visible but the ray-test occludes.
fn delta(flood: &occlusion::OcclusionFill, ray: &RayTestFill) -> Vec<[i32; 3]> {
    flood
        .visible_blocks()
        .filter(|block| ray.is_occluded(*block))
        .collect()
}

fn assert_subset(flood: &occlusion::OcclusionFill, ray: &RayTestFill) {
    for block in ray.visible_blocks() {
        assert!(
            flood.is_visible(block),
            "the ray-test saw {block:?}, which the fill could not reach"
        );
    }
}

#[test]
fn raytest_flag_is_off_by_default() {
    let config = RayTestConfig::default();
    assert!(!config.enabled, "occlusion.raytest must default to off");
    assert_eq!(config.horizon_blocks, DEFAULT_HORIZON_BLOCKS);
    assert_eq!(DEFAULT_HORIZON_BLOCKS, 128);
}

#[test]
fn raytest_svdag_scene_matches_a_dense_scene() {
    let scene = scene(|bx, by, bz| (bx, by, bz) == (2, 1, 1));
    let camera = block_centre([0, 2, 2]);
    let dense = ray_test_scene(camera, HORIZON, &scene);
    let svdag = ray_svdag(&scene, camera);
    let default_scene = {
        let chunks = scene.refs();
        SvdagScene::new(&chunks).ray_test(camera, HORIZON)
    };

    assert_eq!(dense.words(), svdag.words(), "SVDAG and dense scenes agree");
    assert_eq!(
        dense.words(),
        default_scene.words(),
        "the non-air default constructor must agree too"
    );
    assert_eq!(dense.rays_cast(), svdag.rays_cast());
    assert_eq!(dense.voxel_steps(), svdag.voxel_steps());
    assert_eq!(dense.visible_count(), svdag.visible_count());
}

#[test]
fn raytest_open_field_matches_the_flood_fill() {
    // No solids at all: every block in the horizon cube is air, so both
    // occluders keep the whole cube.
    let scene = scene(|_, _, _| false);
    let camera = block_centre([0, 2, 0]);
    let flood = flood(&scene, camera);
    let ray = ray_svdag(&scene, camera);

    assert_subset(&flood, &ray);
    assert_eq!(ray.visible_count(), flood.visible_count());
    assert_eq!(ray.visible_count(), 9usize.pow(3));
    assert!(delta(&flood, &ray).is_empty(), "open sky has no delta");
    println!(
        "raytest open: fill visible {} / ray visible {} / rays {} steps {}",
        flood.visible_count(),
        ray.visible_count(),
        ray.rays_cast(),
        ray.voxel_steps(),
    );
}

#[test]
fn raytest_keeps_an_enclosed_room_and_its_shell() {
    // A 3³ shell around the single interior block (2, 2, 2).
    let scene = scene(|bx, by, bz| {
        let inside = (1..=3).contains(&bx) && (1..=3).contains(&by) && (1..=3).contains(&bz);
        inside && (bx, by, bz) != (2, 2, 2)
    });
    let camera = block_centre([2, 2, 2]);
    let flood = flood(&scene, camera);
    let ray = ray_svdag(&scene, camera);

    assert_subset(&flood, &ray);
    assert!(
        ray.is_visible([2, 2, 2]),
        "the camera's own room is visible"
    );
    assert!(ray.is_visible([1, 2, 2]), "the shell is a visible leaf");
    assert!(ray.is_occluded([4, 2, 2]), "outside the shell is occluded");
    assert!(flood.is_occluded([4, 2, 2]));
    assert_eq!(ray.occluded_count(), flood.occluded_count());
    println!(
        "raytest room: fill visible {} / ray visible {} / occluded {} / rays {} steps {}",
        flood.visible_count(),
        ray.visible_count(),
        ray.occluded_count(),
        ray.rays_cast(),
        ray.voxel_steps(),
    );
}

#[test]
fn raytest_occludes_what_the_fill_reaches_around_a_pillar() {
    // A solid block at (1, 2, 0) sits between the camera (0, 2, 0) and the air
    // behind it at (2, 2, 0).
    let scene = scene(|bx, by, bz| (bx, by, bz) == (1, 2, 0));
    let camera = block_centre([0, 2, 0]);
    let flood = flood(&scene, camera);
    let ray = ray_svdag(&scene, camera);

    assert_subset(&flood, &ray);
    assert!(
        flood.is_visible([2, 2, 0]),
        "the fill reaches around the pillar"
    );
    assert!(
        ray.is_occluded([2, 2, 0]),
        "the pillar blocks the line of sight"
    );

    let delta = delta(&flood, &ray);
    assert!(!delta.is_empty(), "the fixtures must show a delta");
    let percent = 100.0 * delta.len() as f64 / flood.visible_count() as f64;
    println!(
        "raytest pillar: fill visible {} / ray visible {} / delta {} ({percent:.1}%), \
         rays {} steps {}",
        flood.visible_count(),
        ray.visible_count(),
        delta.len(),
        ray.rays_cast(),
        ray.voxel_steps(),
    );
    assert!(
        ray.visible_count() + delta.len() == flood.visible_count(),
        "the ray set is the fill set minus the delta"
    );
}

#[test]
fn raytest_floor_is_a_subset_with_a_documented_delta() {
    // A finite floor patch: the fill walks around its edge and reaches the air below,
    // while the ray to a block directly under it crosses the floor.
    let scene = scene(|_, by, _| by == 0);
    let camera = block_centre([0, 2, 0]);
    let flood = flood(&scene, camera);
    let ray = ray_svdag(&scene, camera);

    assert_subset(&flood, &ray);
    assert!(ray.is_visible([0, 0, 0]), "the floor boundary is visible");
    assert!(
        ray.is_occluded([0, -2, 0]),
        "the ray to the air below the floor is blocked"
    );
    assert!(
        flood.is_visible([0, -2, 0]),
        "the fill goes around the edge"
    );
    let delta = delta(&flood, &ray);
    assert!(!delta.is_empty());
    println!(
        "raytest floor: fill visible {} / ray visible {} / delta {} ({:.1}%) / rays {} steps {}",
        flood.visible_count(),
        ray.visible_count(),
        delta.len(),
        100.0 * delta.len() as f64 / flood.visible_count() as f64,
        ray.rays_cast(),
        ray.voxel_steps(),
    );
}

#[test]
fn raytest_keeps_water_transparent_with_a_material_scene() {
    // Material 2 is water-like: it must not stop the ray.
    let scene = scene_with(ChunkPos::new(0, 0, 0), |bx, by, bz| {
        if (bx, by, bz) == (1, 2, 0) { 2 } else { AIR }
    });
    let camera = block_centre([0, 2, 0]);
    let opaque = |material: u16| material != AIR && material != 2;
    let flood = flood_with(&scene, camera, opaque);
    let ray = ray_svdag_with(&scene, camera, opaque);

    assert_subset(&flood, &ray);
    assert_eq!(ray.visible_count(), flood.visible_count());
    assert!(ray.is_visible([2, 2, 0]), "water must not block the ray");

    // The non-air default would treat water as opaque and cull behind it.
    let default_ray = ray_svdag(&scene, camera);
    assert!(default_ray.is_occluded([2, 2, 0]));
}

#[test]
fn raytest_maps_negative_chunks() {
    let pos = ChunkPos::new(-1, -1, -1);
    let blocks = blocks_per_edge();
    // Chunk-local block (2, 2, 2) -> world block (-bpe + 2, ...).
    let world = -blocks + 2;
    let scene = scene_with(
        pos,
        |bx, by, bz| {
            if (bx, by, bz) == (2, 2, 2) { 1 } else { AIR }
        },
    );
    let camera = block_centre([world - 2, world, world]);
    let flood = flood(&scene, camera);
    let ray = ray_svdag(&scene, camera);

    assert_subset(&flood, &ray);
    assert!(
        ray.is_visible([world, world, world]),
        "the solid block is a visible leaf"
    );
    assert!(
        ray.is_occluded([world + 1, world, world]),
        "behind the solid block is occluded"
    );
    assert!(
        flood.is_visible([world + 1, world, world]),
        "the fill reaches around the isolated block"
    );
}

#[test]
fn raytest_camera_inside_solid_resolves_visible() {
    // Solid at the camera block and a wall directly behind it.
    let scene = scene(|bx, by, bz| matches!((bx, by, bz), (0, 2, 0) | (1, 2, 0)));
    let camera = block_centre([0, 2, 0]);
    let flood = flood(&scene, camera);
    let ray = ray_svdag(&scene, camera);

    assert_subset(&flood, &ray);
    assert!(ray.is_visible([0, 2, 0]), "the seed block is visible");
    assert!(
        ray.is_visible([0, 3, 0]),
        "the ray must skip the solid seed block, not stop in it"
    );
    assert!(
        ray.is_occluded([2, 2, 0]),
        "the wall blocks the line of sight"
    );
}

#[test]
fn raytest_camera_on_a_block_boundary_is_deterministic() {
    let scene = scene(|bx, by, bz| (bx, by, bz) == (2, 2, 0));
    // Exactly on the x boundary between blocks 0 and 1, at the block-centre
    // y and z.
    let camera = [1.0, 2.5, 0.5];
    let flood = flood(&scene, camera);
    let ray = ray_svdag(&scene, camera);
    let again = ray_svdag(&scene, camera);

    assert_subset(&flood, &ray);
    assert_eq!(ray.words(), again.words());
}

#[test]
fn raytest_spans_resident_chunks() {
    let blocks = blocks_per_edge();
    let mut scene = Scene::empty();
    scene.add(ChunkPos::new(0, 0, 0), |_, _, _| AIR);
    scene.add(ChunkPos::new(1, 0, 0), |bx, by, bz| {
        if (bx, by, bz) == (0, 2, 0) { 1 } else { AIR }
    });
    // Two blocks west of the east chunk, so the target and the block behind it
    // stay inside the horizon at both chunk edges.
    let camera = block_centre([blocks - 2, 2, 0]);
    let target = [blocks, 2, 0];
    let flood = flood(&scene, camera);
    let ray = ray_svdag(&scene, camera);

    assert_subset(&flood, &ray);
    assert!(ray.is_visible(target), "the east chunk's block is visible");
    assert!(
        ray.is_occluded([blocks + 1, 2, 0]),
        "behind it, still in the east chunk, is occluded"
    );
}

#[test]
fn raytest_is_deterministic() {
    let scene = scene(|bx, by, bz| (bx, by, bz) == (1, 2, 0));
    let camera = block_centre([0, 2, 0]);
    let first = ray_svdag(&scene, camera);
    let second = ray_svdag(&scene, camera);
    assert_eq!(first.words(), second.words());
    assert_eq!(first.voxel_steps(), second.voxel_steps());
}
