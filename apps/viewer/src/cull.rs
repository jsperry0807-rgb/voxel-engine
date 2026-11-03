//! M3-04 CPU frustum cull of chunk AABBs.
//!
//! The stopgap renderer draws a 16×16 ring of chunks. Before M5's GPU broad
//! phase exists, the frame thread tests each resident chunk's world AABB
//! against six planes extracted from the camera's view-projection matrix and
//! skips the chunks entirely outside the view.
//!
//! The planes are extracted from the matrix rather than rebuilt from the
//! camera parameters: that is the path a GPU implementation will take, and it
//! is also what makes [`Frustum::from_view_projection`] an independent check of
//! the parameter-built [`voxel_storage::reference::cull::Frustum`]. The
//! 100-pose parity test in `tests/cpu_cull.rs` requires exactly the same chunk
//! set from both.
//!
//! # Conventions
//!
//! Matrices are the engine's WebGPU convention: right-handed, clip `y` up and
//! clip depth `0..=1`. A point is inside a plane when
//! `normal · point + offset >= 0`; the six planes are near, far, left, right,
//! bottom, top.
//!
//! # Conservatism
//!
//! A six-plane test is conservative: it can keep a box near a frustum edge that
//! the true frustum misses, but it never rejects a box the frustum touches. A
//! box whose face lies exactly on a plane stays visible ([`PLANE_EPSILON`] is
//! the tolerance), which is the direction that cannot open a hole.

use glam::{Mat4, Vec3};
use voxel_core::constants::{CHUNK_DIM, VOXEL_SIZE};
use voxel_storage::ChunkPos;

/// Plane tolerance in metres.
///
/// A box is culled only when its farthest corner is strictly farther out than
/// this, so a touching box survives. The value is set for `f32` world
/// coordinates in the hundreds of metres.
pub const PLANE_EPSILON: f32 = 1e-5;

/// Edge length of one chunk in world metres.
pub const CHUNK_EXTENT: f32 = CHUNK_DIM as f32 * VOXEL_SIZE;

/// A world-space axis-aligned box in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// Minimum corner (inclusive).
    pub min: Vec3,
    /// Maximum corner (inclusive).
    pub max: Vec3,
}

impl Aabb {
    /// Builds a box from opposite corners.
    ///
    /// # Panics
    ///
    /// Panics if a coordinate is non-finite or `min` exceeds `max`.
    pub fn new(min: Vec3, max: Vec3) -> Self {
        assert!(
            min.is_finite() && max.is_finite(),
            "an AABB corner must be finite"
        );
        assert!(
            min.x <= max.x && min.y <= max.y && min.z <= max.z,
            "an AABB minimum must not exceed its maximum"
        );
        Self { min, max }
    }

    /// The box centre.
    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Half the box extent on each axis.
    pub fn extents(self) -> Vec3 {
        (self.max - self.min) * 0.5
    }
}

/// The world-space AABB of the chunk at `pos`.
pub fn chunk_aabb(pos: ChunkPos) -> Aabb {
    let origin = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32) * CHUNK_EXTENT;
    Aabb::new(origin, origin + Vec3::splat(CHUNK_EXTENT))
}

/// Three-state result of a conservative plane or frustum test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Containment {
    /// Entirely outside one plane; safe to skip.
    Outside,
    /// Straddles a plane; must be drawn.
    Intersecting,
    /// Entirely inside every plane.
    Inside,
}

/// A normalized plane `normal · point + offset = 0`; the positive half-space
/// is inside.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    normal: Vec3,
    offset: f32,
}

impl Plane {
    /// Builds a normalized plane from the four matrix-row coefficients.
    fn from_coefficients(coefficients: glam::Vec4) -> Self {
        let normal = coefficients.truncate();
        let length = normal.length();
        assert!(
            length > f32::EPSILON && length.is_finite(),
            "a frustum plane needs a finite non-zero normal"
        );
        Self {
            normal: normal / length,
            offset: coefficients.w / length,
        }
    }

    /// The unit normal, pointing into the visible half-space.
    pub fn normal(self) -> Vec3 {
        self.normal
    }

    /// The plane offset.
    pub fn offset(self) -> f32 {
        self.offset
    }

    /// Signed distance from a point to the plane; positive is inside.
    pub fn signed_distance(self, point: Vec3) -> f32 {
        self.normal.dot(point) + self.offset
    }

    /// Classifies a box against this plane.
    ///
    /// Uses the centre/extents form: the projected radius is
    /// `extents · |normal|`. A box is outside only when every point is beyond
    /// the negative face by more than [`PLANE_EPSILON`].
    pub fn test_aabb(self, aabb: Aabb) -> Containment {
        let radius = aabb.extents().dot(self.normal.abs());
        let distance = self.signed_distance(aabb.center());
        if distance + radius < -PLANE_EPSILON {
            Containment::Outside
        } else if distance - radius >= PLANE_EPSILON {
            Containment::Inside
        } else {
            Containment::Intersecting
        }
    }
}

/// A six-plane perspective frustum, in the order near, far, left, right,
/// bottom, top.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [Plane; Self::PLANE_COUNT],
}

impl Frustum {
    /// Number of planes in every frustum.
    pub const PLANE_COUNT: usize = 6;

    /// Extracts the six clip planes from a world-to-clip matrix.
    ///
    /// For `glam`'s column-major matrix, a point `p` is in front of a plane
    /// built from rows `a` and `b` when `(a ± b) · p >= 0` within clip space:
    /// `w + x`, `w - x`, `w + y`, `w - y`, `z` (near, depth `0`) and
    /// `w - z` (far, depth `1`).
    pub fn from_view_projection(view_projection: Mat4) -> Self {
        let row = |index: usize| view_projection.row(index);
        let (w, x, y, z) = (row(3), row(0), row(1), row(2));
        Self {
            planes: [
                Plane::from_coefficients(z),
                Plane::from_coefficients(w - z),
                Plane::from_coefficients(w + x),
                Plane::from_coefficients(w - x),
                Plane::from_coefficients(w + y),
                Plane::from_coefficients(w - y),
            ],
        }
    }

    /// The six planes, in near, far, left, right, bottom, top order.
    pub fn planes(self) -> [Plane; Self::PLANE_COUNT] {
        self.planes
    }

    /// Classifies a box against the frustum.
    ///
    /// Outside means at least one plane reports [`Containment::Outside`];
    /// Inside means every plane reports [`Containment::Inside`].
    pub fn test_aabb(self, aabb: Aabb) -> Containment {
        let mut result = Containment::Inside;
        for plane in self.planes {
            match plane.test_aabb(aabb) {
                Containment::Outside => return Containment::Outside,
                Containment::Intersecting => result = Containment::Intersecting,
                Containment::Inside => {}
            }
        }
        result
    }

    /// True when the box is not entirely outside the frustum.
    pub fn intersects(self, aabb: Aabb) -> bool {
        self.test_aabb(aabb) != Containment::Outside
    }

    /// True when the chunk at `pos` is not entirely outside the frustum.
    pub fn chunk_visible(self, pos: ChunkPos) -> bool {
        self.intersects(chunk_aabb(pos))
    }
}

/// The visible subset of `positions`, in the order given.
///
/// This is the M3-04 broad phase: a per-chunk AABB test, no hierarchy and no
/// occlusion. M5 replaces it with the GPU traversal, and the M1-32 reference is
/// the equality oracle for both.
pub fn visible_chunks(
    positions: impl IntoIterator<Item = ChunkPos>,
    frustum: Frustum,
) -> Vec<ChunkPos> {
    positions
        .into_iter()
        .filter(|position| frustum.chunk_visible(*position))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    fn box_at(center: Vec3, half: f32) -> Aabb {
        Aabb::new(center - Vec3::splat(half), center + Vec3::splat(half))
    }

    #[test]
    fn plane_test_keeps_tangent_boxes() {
        // `from_coefficients` normalizes; the inside half-space is +X.
        let plane = Plane::from_coefficients(glam::Vec4::new(1.0, 0.0, 0.0, 0.0));
        assert!((plane.normal().x - 1.0).abs() < 1e-6);
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
        // The maximum face lies exactly on the plane: keep it.
        assert_eq!(
            plane.test_aabb(box_at(Vec3::new(-0.5, 0.0, 0.0), 0.5)),
            Containment::Intersecting
        );
        let scaled = Plane::from_coefficients(glam::Vec4::new(0.0, 2.0, 0.0, 0.0));
        assert!(
            (scaled.normal().y - 1.0).abs() < 1e-6,
            "the plane is normalized"
        );
    }

    #[test]
    fn the_default_camera_sees_the_chunk_at_its_feet() {
        let camera = Camera::default();
        let frustum = Frustum::from_view_projection(camera.view_projection());
        let here = crate::streaming::center_for(camera.uniform().position);
        assert!(frustum.chunk_visible(here));
        // The default pose looks toward -Z, so +Z is behind the camera.
        let behind = ChunkPos::new(here.x, here.y, here.z + 40);
        assert!(!frustum.chunk_visible(behind));
    }

    #[test]
    fn chunk_aabbs_are_world_aligned() {
        let aabb = chunk_aabb(ChunkPos::new(-2, 0, 3));
        assert_eq!(aabb.min, Vec3::new(-2.0, 0.0, 3.0) * CHUNK_EXTENT);
        assert_eq!(aabb.max, Vec3::new(-1.0, 1.0, 4.0) * CHUNK_EXTENT);
        assert_eq!(aabb.max - aabb.min, Vec3::splat(CHUNK_EXTENT));
    }
}
