//! M3-04 CPU chunk cull parity tests.
//!
//! The acceptance for M3-04 is that the per-chunk AABB cull equals the M1-32
//! reference traversal on **100 poses**. The reference is
//! [`voxel_storage::reference::cull::Frustum`], built from camera parameters
//! (`eye`/`target`/`fov`/`aspect`/near/far) with `f64` arithmetic. The
//! production cull extracts planes from the camera's `f32` view-projection
//! matrix, which is the path a GPU broad phase will take, so the two share no
//! plane construction code.
//!
//! A six-plane test is conservative: a box on a plane survives in both
//! implementations. The poses sweep position, yaw and pitch around the ring but
//! avoid exact plane contact, so the two sets must match exactly.

use core::f32::consts::TAU;

use glam::Vec3;
use voxel_storage::ChunkPos;
use voxel_storage::reference::cull::{
    Aabb as RefAabb, Camera as RefCamera, Frustum as RefFrustum, Vec3 as RefVec3,
};
use voxel_viewer::camera::Camera;
use voxel_viewer::cull::{Frustum, chunk_aabb, visible_chunks};
use voxel_viewer::streaming::{RING_CHUNKS, center_for, ring_chunks};

/// Poses required by the task.
const POSES: usize = 100;

/// One deterministic camera pose around the default ring.
///
/// The position circles the ring centre at 3–15 m and 0.5–3 m of height, while
/// the target sweeps a full turn at a different rate, so yaw and pitch vary
/// together and no pose looks exactly along an axis.
fn pose(index: usize) -> Camera {
    let angle = index as f32 / POSES as f32 * TAU;
    let position = Vec3::new(
        4.5 + angle.cos() * (3.0 + 12.0 * (angle * 0.5).sin().abs()),
        1.5 + (angle * 0.7).sin().abs() * 2.5,
        6.0 + angle.sin() * (3.0 + 12.0 * (angle * 0.5).sin().abs()),
    );
    let target = position
        + Vec3::new(
            (angle * 1.7).cos() * 10.0,
            (angle * 0.9).sin() * 3.0,
            (angle * 1.7).sin() * 10.0,
        );
    Camera::looking_at(position, target, 16.0 / 9.0)
}

/// Builds the M1-32 reference camera from the production camera's parameters.
fn reference_camera(camera: &Camera) -> RefCamera {
    let eye = camera.position();
    let forward = camera.forward();
    RefCamera {
        eye: RefVec3::new(eye.x as f64, eye.y as f64, eye.z as f64),
        target: RefVec3::new(
            (eye.x + forward.x) as f64,
            (eye.y + forward.y) as f64,
            (eye.z + forward.z) as f64,
        ),
        up: RefVec3::new(0.0, 1.0, 0.0),
        fov_y: camera.fov_y() as f64,
        aspect: camera.aspect() as f64,
        near: camera.near() as f64,
        far: camera.far() as f64,
    }
}

/// The reference traversal's chunk set: the same six-plane test in `f64`.
fn reference_visible(ring: &[ChunkPos], camera: &Camera) -> Vec<ChunkPos> {
    let frustum = RefFrustum::from_camera(&reference_camera(camera));
    ring.iter()
        .copied()
        .filter(|position| {
            let aabb = chunk_aabb(*position);
            let reference = RefAabb::new(
                RefVec3::new(aabb.min.x as f64, aabb.min.y as f64, aabb.min.z as f64),
                RefVec3::new(aabb.max.x as f64, aabb.max.y as f64, aabb.max.z as f64),
            );
            frustum.intersects(reference)
        })
        .collect()
}

#[test]
fn the_chunk_cull_equals_the_m1_32_reference_on_100_poses() {
    // The ring the stopgap draws, centered on the default camera's chunk.
    let center = center_for(Camera::default().uniform().position);
    let ring = ring_chunks(center);
    assert_eq!(ring.len(), RING_CHUNKS);

    let mut visible_total = 0usize;
    let mut smallest = RING_CHUNKS;
    let mut largest = 0usize;
    for index in 0..POSES {
        let camera = pose(index);
        let production: Vec<ChunkPos> = visible_chunks(
            ring.iter().copied(),
            Frustum::from_view_projection(camera.view_projection()),
        );
        let reference = reference_visible(&ring, &camera);

        let mut production_sorted = production.clone();
        let mut reference_sorted = reference.clone();
        production_sorted.sort_by_key(|position| (position.x, position.y, position.z));
        reference_sorted.sort_by_key(|position| (position.x, position.y, position.z));
        assert_eq!(
            production_sorted, reference_sorted,
            "pose {index}: the cull and the M1-32 reference disagree"
        );

        // A cull that keeps everything or nothing would also "match" a broken
        // reference, so require a real reduction over the sweep.
        assert!(!production.is_empty(), "pose {index} culled everything");
        visible_total += production.len();
        smallest = smallest.min(production.len());
        largest = largest.max(production.len());
    }

    let mean = visible_total as f64 / POSES as f64;
    assert!(
        smallest < RING_CHUNKS && largest < RING_CHUNKS,
        "the cull never removed a chunk: {smallest}..{largest} of {RING_CHUNKS}"
    );
    eprintln!(
        "cpu_cull: {POSES} poses, {smallest}..{largest} of {RING_CHUNKS} chunks kept (mean {mean:.1})"
    );
}

#[test]
fn a_camera_inside_the_ring_keeps_the_chunk_it_stands_in() {
    let camera = Camera::default();
    let here = center_for(camera.uniform().position);
    let frustum = Frustum::from_view_projection(camera.view_projection());
    assert!(
        frustum.chunk_visible(here),
        "the camera's own chunk was culled"
    );

    // The default pose looks toward -Z, so two chunk edges at +Z are behind it.
    let behind = ChunkPos::new(here.x, here.y, here.z + 2);
    assert!(!frustum.chunk_visible(behind));
}

#[test]
fn the_far_plane_culls_a_chunk_beyond_it() {
    let camera = Camera::default();
    let frustum = Frustum::from_view_projection(camera.view_projection());
    let here = center_for(camera.uniform().position);
    // The far plane is 1 024 m; a ring chunk is at most ~130 m away, so shift
    // one far beyond it.
    let far = ChunkPos::new(here.x, here.y, here.z - 20_000);
    assert!(!frustum.chunk_visible(far));
    // And a chunk well in front, inside the far plane, is kept.
    let ahead = ChunkPos::new(here.x, here.y, here.z - 1);
    assert!(frustum.chunk_visible(ahead));
}
