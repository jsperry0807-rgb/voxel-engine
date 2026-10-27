//! CPU hierarchical frustum reference traversal (M1-32).
//!
//! This is the slow, exact oracle for the CPU broad-phase and the GPU
//! traversal that follows it. It walks a chunk's SVDAG from the root through
//! its octants and stops at the requested **LOD depth**, emitting the non-empty
//! nodes at that depth. At LOD0 the emitted nodes are the 16³ blocks the
//! renderer draws; at LOD1/LOD2 they aggregate 2³/4³ blocks per node
//! (ADR-009). The traversal is frustum-only; block occlusion is a separate
//! reference (M1-33) and is not part of this oracle.
//!
//! # Coordinates and levels
//!
//! A node's box depends on its **placement**, never on its shared content
//! (spec §4.1). [`VisibleNode`] therefore identifies a node by chunk position,
//! pool level and chunk-local voxel origin:
//!
//! ```text
//! AABB = [chunkOrigin + origin × VOXEL_SIZE,
//!         chunkOrigin + (origin + 2^level) × VOXEL_SIZE]
//! ```
//!
//! `level` is the storage/pool numbering, where the root is
//! [`voxel_core::constants::ROOT_LEVEL`] and a block node is
//! [`voxel_core::constants::BLOCK_BITS`]. [`VisibleNode::depth`] converts it to
//! the core numbering used by ADR-009's `lodMaxDepth`: the root is depth 0 and
//! a block is [`voxel_core::constants::BLOCK_LEVEL`].
//!
//! # Traversal
//!
//! 1. Test the chunk root (broad phase). A root entirely outside one frustum
//!    plane is skipped before any descent.
//! 2. For each present octant, derive its box from the parent's origin and
//!    test it. A node entirely outside one plane is pruned; a node entirely
//!    inside all planes sets a flag so its descendants need no plane tests.
//! 3. Stop when the node's pool level equals `CHUNK_BITS - lod_max_depth` and
//!    emit it. At LOD0 (`lod_max_depth == BLOCK_LEVEL`) this is exactly the
//!    block level. The traversal never reads below that cutoff.
//!
//! # Exactness
//!
//! A plane-vs-AABB test against only the six frustum planes is conservative:
//! it can keep a box near a frustum edge that the true frustum misses, but it
//! never rejects a box the frustum touches. At the block cutoff every
//! surviving block is tested individually, so the output set equals a
//! brute-force test of every non-empty block ([`brute_force_blocks`]). The
//! 100-pose test in `tests/reference_cull.rs` pins that equality.
//!
//! # Determinism
//!
//! Output is sorted by `(chunk, level, origin)`, so chunk order, band order and
//! hash-map iteration cannot change the set. Distances are `f64`; the plane
//! epsilon biases the test toward inclusion on an exact boundary, which is the
//! conservative direction for a culler.

use core::cmp::Ordering;
use core::ops::{Add, Mul, Sub};

use voxel_core::constants::{
    BLOCK_BITS, BLOCK_DIM, BLOCK_LEVEL, CHUNK_BITS, CHUNK_DIM, VOXEL_SIZE,
};

use crate::ChunkPos;
use crate::bands::Bands;
use crate::dag::Svdag;
use crate::node_word::Node;
use crate::run::fetch_child;

/// Tolerance used when classifying an AABB against a plane.
///
/// A box is culled only when its largest signed distance is strictly below
/// `-PLANE_EPSILON`. This keeps a box touching a plane visible; a culler may
/// over-include but must never create a hole.
pub const PLANE_EPSILON: f64 = 1e-9;

/// A world-space 3-component vector.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

impl Vec3 {
    /// The zero vector.
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    /// Builds a vector from its three components.
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Dot product.
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Cross product, in a right-handed coordinate system.
    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    /// Euclidean length.
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// Unit vector in the same direction.
    ///
    /// # Panics
    ///
    /// Panics if the vector is zero or non-finite.
    pub fn normalized(self) -> Self {
        let length = self.length();
        assert!(
            length.is_finite() && length > 0.0,
            "cannot normalize a zero or non-finite vector"
        );
        self * (1.0 / length)
    }
}

impl Add for Vec3 {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Self;

    fn mul(self, scale: f64) -> Self {
        Self::new(self.x * scale, self.y * scale, self.z * scale)
    }
}

/// An axis-aligned box in world space, in metres.
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    /// Minimum corner (inclusive).
    pub min: Vec3,
    /// Maximum corner (inclusive).
    pub max: Vec3,
}

impl Aabb {
    /// Builds a box from its opposite corners.
    ///
    /// # Panics
    ///
    /// Panics if any coordinate is non-finite or `min` exceeds `max` on an
    /// axis.
    pub fn new(min: Vec3, max: Vec3) -> Self {
        assert!(
            min.x.is_finite()
                && min.y.is_finite()
                && min.z.is_finite()
                && max.x.is_finite()
                && max.y.is_finite()
                && max.z.is_finite(),
            "an AABB corner must be finite"
        );
        assert!(
            min.x <= max.x && min.y <= max.y && min.z <= max.z,
            "an AABB minimum must not exceed its maximum"
        );
        Self { min, max }
    }

    /// Builds a box from a minimum corner and an edge length.
    pub fn from_origin_size(origin: Vec3, size: f64) -> Self {
        Self::new(origin, origin + Vec3::new(size, size, size))
    }

    /// The box centre.
    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// The half-size on each axis.
    pub fn extents(self) -> Vec3 {
        (self.max - self.min) * 0.5
    }
}

/// Three-state result of a conservative plane or frustum test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Containment {
    /// Entirely on the outside of a plane; safe to cull.
    Outside,
    /// Straddles a boundary; must be descended into or emitted.
    Intersecting,
    /// Entirely on the inside of every plane; descendants can skip tests.
    Inside,
}

/// A normalized plane `normal · point + offset = 0`.
///
/// The positive half-space is inside. [`Plane::new`] normalizes the normal so
/// [`PLANE_EPSILON`] is a world-space distance.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    normal: Vec3,
    offset: f64,
}

impl Plane {
    /// Builds a plane from a normal and one point on the plane.
    ///
    /// # Panics
    ///
    /// Panics if `normal` is zero or non-finite.
    pub fn new(normal: Vec3, point: Vec3) -> Self {
        let normal = normal.normalized();
        Self {
            normal,
            offset: -normal.dot(point),
        }
    }

    /// The unit normal, pointing into the visible half-space.
    pub const fn normal(self) -> Vec3 {
        self.normal
    }

    /// The plane offset, `-normal · point`.
    pub const fn offset(self) -> f64 {
        self.offset
    }

    /// Signed distance from a point to the plane; positive is inside.
    pub fn signed_distance(self, point: Vec3) -> f64 {
        self.normal.dot(point) + self.offset
    }

    /// Classifies an AABB against this plane.
    ///
    /// Uses the centre/extents form of the plane-box test: the projected radius
    /// is `extents · |normal|`. A box is outside only when every point is
    /// strictly beyond the negative face, so an exact touch stays visible.
    pub fn test_aabb(self, aabb: Aabb) -> Containment {
        let center = aabb.center();
        let extents = aabb.extents();
        let radius = extents.x * self.normal.x.abs()
            + extents.y * self.normal.y.abs()
            + extents.z * self.normal.z.abs();
        let distance = self.signed_distance(center);

        if distance + radius < -PLANE_EPSILON {
            Containment::Outside
        } else if distance - radius >= PLANE_EPSILON {
            Containment::Inside
        } else {
            Containment::Intersecting
        }
    }
}

/// A perspective camera used to build a reference frustum.
///
/// This is intentionally a small, explicit parameter block, not the engine's
/// production camera. `eye` and `target` define the view direction; `up` is
/// projected onto the view plane, so it need not be exactly perpendicular.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// Camera position in metres.
    pub eye: Vec3,
    /// Point the camera looks at, in metres.
    pub target: Vec3,
    /// Approximate up direction; must not be parallel to the view direction.
    pub up: Vec3,
    /// Vertical field of view in radians, in `(0, π)`.
    pub fov_y: f64,
    /// Viewport width divided by height; must be positive.
    pub aspect: f64,
    /// Near clip distance in metres; must be positive.
    pub near: f64,
    /// Far clip distance in metres; must exceed `near`.
    pub far: f64,
}

/// A six-plane perspective view frustum.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [Plane; Self::PLANE_COUNT],
}

impl Frustum {
    /// Number of planes in every frustum: near, far, left, right, bottom, top.
    pub const PLANE_COUNT: usize = 6;

    /// Builds a frustum from six planes, in the order documented on
    /// [`Frustum::planes`].
    pub const fn from_planes(planes: [Plane; Self::PLANE_COUNT]) -> Self {
        Self { planes }
    }

    /// Builds a symmetric perspective frustum from a camera.
    ///
    /// The planes are stored as near, far, left, right, bottom and top; all
    /// normals point into the visible half-space.
    ///
    /// # Panics
    ///
    /// Panics on a degenerate camera: a zero view vector, an `up` parallel to
    /// the view direction, a field of view outside `(0, π)`, a non-positive
    /// aspect or near plane, or `far <= near`.
    pub fn from_camera(camera: &Camera) -> Self {
        assert!(
            camera.fov_y > 0.0 && camera.fov_y < core::f64::consts::PI,
            "camera field of view must be inside (0, pi)"
        );
        assert!(camera.aspect > 0.0, "camera aspect must be positive");
        assert!(camera.near > 0.0, "camera near plane must be positive");
        assert!(
            camera.far > camera.near,
            "camera far plane must be beyond the near plane"
        );

        let forward = (camera.target - camera.eye).normalized();
        let right = forward.cross(camera.up);
        assert!(
            right.length() > 1e-9,
            "camera up must not be parallel to the view direction"
        );
        let right = right.normalized();
        // `right` is perpendicular to `forward`, so this is already unit length
        // up to rounding.
        let true_up = right.cross(forward).normalized();

        let tan_y = (camera.fov_y * 0.5).tan();
        let tan_x = tan_y * camera.aspect;
        let eye = camera.eye;

        Self::from_planes([
            Plane::new(forward, eye + forward * camera.near),
            Plane::new(forward * -1.0, eye + forward * camera.far),
            Plane::new(forward * tan_x + right, eye),
            Plane::new(forward * tan_x - right, eye),
            Plane::new(forward * tan_y + true_up, eye),
            Plane::new(forward * tan_y - true_up, eye),
        ])
    }

    /// The six planes: near, far, left, right, bottom, top.
    pub const fn planes(self) -> [Plane; Self::PLANE_COUNT] {
        self.planes
    }

    /// Classifies an AABB against the frustum.
    ///
    /// Outside means at least one plane reports [`Containment::Outside`];
    /// Inside means every plane reports [`Containment::Inside`]. Anything else
    /// intersects.
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
}

/// One chunk participating in a reference traversal.
#[derive(Clone, Copy, Debug)]
pub struct ChunkView<'a> {
    /// Chunk position in chunk coordinates.
    pub pos: ChunkPos,
    /// The chunk's compiled SVDAG.
    pub dag: &'a Svdag,
}

impl<'a> ChunkView<'a> {
    /// Pairs a chunk position with its DAG.
    pub const fn new(pos: ChunkPos, dag: &'a Svdag) -> Self {
        Self { pos, dag }
    }
}

/// One non-empty node selected for emission.
///
/// The node's box is fully determined by its chunk, pool level and chunk-local
/// voxel origin; the node's content (which may be shared with another placed
/// node) never defines the box. `origin` is aligned to `2^level` on every
/// axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VisibleNode {
    chunk: ChunkPos,
    level: u32,
    origin: [u32; 3],
}

impl VisibleNode {
    /// Builds a placed node.
    ///
    /// `level` is the storage/pool level and `origin` is the chunk-local voxel
    /// origin.
    ///
    /// # Panics
    ///
    /// Panics if `level > CHUNK_BITS`, if any origin coordinate is outside the
    /// chunk, or if an origin coordinate is not aligned to the node extent.
    pub fn new(chunk: ChunkPos, level: u32, origin: [u32; 3]) -> Self {
        assert!(
            level <= CHUNK_BITS,
            "node level {level} is above the chunk root {CHUNK_BITS}"
        );
        let extent = 1u32 << level;
        for coordinate in origin {
            assert!(
                coordinate < CHUNK_DIM as u32,
                "node origin is outside the chunk"
            );
            assert!(
                coordinate % extent == 0,
                "node origin is not aligned to its {extent}-voxel extent"
            );
        }
        Self {
            chunk,
            level,
            origin,
        }
    }

    /// The chunk that contains this node.
    pub const fn chunk(self) -> ChunkPos {
        self.chunk
    }

    /// The storage/pool level: root `CHUNK_BITS`, block `BLOCK_BITS`.
    pub const fn level(self) -> u32 {
        self.level
    }

    /// The core depth from the chunk root: `CHUNK_BITS - level`.
    ///
    /// Depth `BLOCK_LEVEL` is one 16³ block, matching ADR-009's `lodMaxDepth`.
    pub const fn depth(self) -> u32 {
        CHUNK_BITS - self.level
    }

    /// The chunk-local voxel origin on each axis.
    pub const fn origin(self) -> [u32; 3] {
        self.origin
    }

    /// Node edge length in voxels, `2^level`.
    pub const fn extent_voxels(self) -> u32 {
        1u32 << self.level
    }

    /// Node volume in voxels, `(2^level)^3`.
    pub const fn volume_voxels(self) -> u64 {
        1u64 << (3 * self.level)
    }

    /// The node's world-space box in metres.
    pub fn aabb(self) -> Aabb {
        node_aabb(self.chunk, self.level, self.origin)
    }

    /// The block-table index when this is a block-level node.
    ///
    /// Returns `None` for a coarser LOD node. The index is row-major
    /// `x + y·B + z·B²` over the chunk's blocks, matching
    /// [`voxel_core::constants::BLOCKS_PER_CHUNK`].
    pub fn block_index(self) -> Option<u32> {
        if self.level != BLOCK_BITS {
            return None;
        }
        let block = BLOCK_DIM as u32;
        let blocks_per_edge = CHUNK_DIM as u32 / block;
        let x = self.origin[0] / block;
        let y = self.origin[1] / block;
        let z = self.origin[2] / block;
        Some(x + y * blocks_per_edge + z * blocks_per_edge * blocks_per_edge)
    }
}

impl PartialOrd for VisibleNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VisibleNode {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.chunk.x,
            self.chunk.y,
            self.chunk.z,
            self.level,
            self.origin,
        )
            .cmp(&(
                other.chunk.x,
                other.chunk.y,
                other.chunk.z,
                other.level,
                other.origin,
            ))
    }
}

/// Emits every non-empty node at the LOD cutoff that survives the frustum.
///
/// `lod_max_depth` is the core depth cutoff, `0..=BLOCK_LEVEL`: `BLOCK_LEVEL`
/// is LOD0 (block nodes) and `BLOCK_LEVEL - n` is LOD `n` (ADR-009). The
/// returned nodes are sorted by chunk, level and origin.
///
/// The chunk root is tested first. If it is fully inside the frustum, child
/// plane tests are skipped and the subtree is emitted down to the cutoff;
/// otherwise each present octant is tested individually.
///
/// # Panics
///
/// Panics if `lod_max_depth > BLOCK_LEVEL`; this reference stops at block
/// resolution and never reads voxel leaves.
pub fn traverse(
    frustum: &Frustum,
    chunks: &[ChunkView<'_>],
    lod_max_depth: u32,
) -> Vec<VisibleNode> {
    assert!(
        lod_max_depth <= BLOCK_LEVEL,
        "lod_max_depth {lod_max_depth} is deeper than block level {BLOCK_LEVEL}"
    );
    let cutoff_level = CHUNK_BITS - lod_max_depth;
    let mut visible = Vec::new();

    for view in chunks {
        let Some(root) = view.dag.chunk().root() else {
            continue;
        };
        let bands = view.dag.chunk().bands();
        let root_node = bands.get(root).expect("a live root names a node");
        let root_origin = [0; 3];
        let root_containment =
            frustum.test_aabb(node_aabb(view.pos, root_node.level(), root_origin));
        if root_containment == Containment::Outside {
            continue;
        }

        let mut traversal = Traversal {
            frustum,
            bands,
            pos: view.pos,
            cutoff_level,
            visible: &mut visible,
        };
        traversal.visit(
            root_node,
            root_origin,
            root_containment == Containment::Inside,
        );
    }

    visible.sort_unstable();
    visible
}

/// Full DAG descent that lists every non-empty block in a chunk.
///
/// This is the enumeration half of the brute-force oracle: it applies no
/// frustum test and visits every present block exactly once. The result is
/// sorted by level and origin. An empty chunk yields an empty list.
pub fn nonempty_blocks(chunk: &Svdag, pos: ChunkPos) -> Vec<VisibleNode> {
    let Some(root) = chunk.chunk().root() else {
        return Vec::new();
    };
    let bands = chunk.chunk().bands();
    let root_node = bands.get(root).expect("a live root names a node");
    let mut blocks = Vec::new();
    collect_blocks(bands, pos, root_node, [0; 3], &mut blocks);
    blocks.sort_unstable();
    blocks
}

/// Brute-force per-block frustum test, the M1-32 output oracle.
///
/// Enumerates every non-empty block ([`nonempty_blocks`]) and tests its box
/// individually against the frustum. At LOD0 this must equal
/// [`traverse`]'s output set exactly.
pub fn brute_force_blocks(frustum: &Frustum, chunks: &[ChunkView<'_>]) -> Vec<VisibleNode> {
    let mut visible = Vec::new();
    for view in chunks {
        for block in nonempty_blocks(view.dag, view.pos) {
            if frustum.intersects(block.aabb()) {
                visible.push(block);
            }
        }
    }
    visible.sort_unstable();
    visible
}

/// Per-chunk state for one recursive descent.
///
/// Keeping the invariant pieces (frustum, chunk, bands, cutoff) together leaves
/// [`Traversal::visit`] with only the node's placement and the inside flag,
/// rather than an eight-argument function.
struct Traversal<'a> {
    frustum: &'a Frustum,
    bands: &'a Bands,
    pos: ChunkPos,
    cutoff_level: u32,
    visible: &'a mut Vec<VisibleNode>,
}

impl Traversal<'_> {
    /// Recursive descent from one already-accepted node to the LOD cutoff.
    fn visit(&mut self, node: Node, origin: [u32; 3], parent_inside: bool) {
        let level = node.level();
        if level == self.cutoff_level {
            self.visible.push(VisibleNode::new(self.pos, level, origin));
            return;
        }
        debug_assert!(
            level > self.cutoff_level,
            "a node below the cutoff must not be visited"
        );

        let half = 1u32 << (level - 1);
        for child_index in 0..8u32 {
            let Some(child) = fetch_child(self.bands, node, child_index) else {
                continue;
            };
            debug_assert_eq!(child.level(), level - 1);
            let child_origin = [
                origin[0] + (child_index & 1) * half,
                origin[1] + ((child_index >> 1) & 1) * half,
                origin[2] + ((child_index >> 2) & 1) * half,
            ];

            if parent_inside {
                self.visit(child, child_origin, true);
                continue;
            }

            let containment =
                self.frustum
                    .test_aabb(node_aabb(self.pos, child.level(), child_origin));
            if containment == Containment::Outside {
                continue;
            }
            self.visit(child, child_origin, containment == Containment::Inside);
        }
    }
}

/// Recursive all-child descent used by [`nonempty_blocks`].
fn collect_blocks(
    bands: &Bands,
    pos: ChunkPos,
    node: Node,
    origin: [u32; 3],
    blocks: &mut Vec<VisibleNode>,
) {
    let level = node.level();
    if level == BLOCK_BITS {
        blocks.push(VisibleNode::new(pos, level, origin));
        return;
    }
    assert!(
        level > BLOCK_BITS,
        "a block descent cannot pass below the block level"
    );

    let half = 1u32 << (level - 1);
    for child_index in 0..8u32 {
        let Some(child) = fetch_child(bands, node, child_index) else {
            continue;
        };
        let child_origin = [
            origin[0] + (child_index & 1) * half,
            origin[1] + ((child_index >> 1) & 1) * half,
            origin[2] + ((child_index >> 2) & 1) * half,
        ];
        collect_blocks(bands, pos, child, child_origin, blocks);
    }
}

/// World-space box for a placed node.
fn node_aabb(pos: ChunkPos, level: u32, origin: [u32; 3]) -> Aabb {
    let voxel = VOXEL_SIZE as f64;
    let chunk = CHUNK_DIM as f64 * voxel;
    let chunk_origin = Vec3::new(
        pos.x as f64 * chunk,
        pos.y as f64 * chunk,
        pos.z as f64 * chunk,
    );
    let min = chunk_origin
        + Vec3::new(
            origin[0] as f64 * voxel,
            origin[1] as f64 * voxel,
            origin[2] as f64 * voxel,
        );
    Aabb::from_origin_size(min, (1u32 << level) as f64 * voxel)
}

// The reference stops at block resolution, so a block node must be reachable
// from the root and the cutoff mapping must stay in range.
const _: () = {
    assert!(BLOCK_BITS <= CHUNK_BITS);
    assert!(BLOCK_LEVEL <= CHUNK_BITS);
    assert!(BLOCK_LEVEL == CHUNK_BITS - BLOCK_BITS);
};
