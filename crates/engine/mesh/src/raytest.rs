//! Optional SVDAG ray-test occluder (M2-25).
//!
//! The primary occluder is the camera-seeded block flood fill (ADR-006): it
//! answers *connectivity*, so it over-includes through any one-block gap and
//! around corners. This module is the escalation, behind the `raytest` cargo
//! feature and off by default: for every block in the horizon cube it casts one
//! ray from the camera to the block centre and keeps the block visible only when
//! the segment is clear. That answers **line of sight** instead of connectivity.
//!
//! # Semantics
//!
//! - The grid is the same as the flood fill: world **block** coordinates, one
//!   bit per block, a `(2·horizon + 1)³` Chebyshev cube around the camera block.
//!   Blocks outside the cube resolve visible, exactly like the fill.
//! - A block is visible when the ray from the camera to its centre enters the
//!   block without crossing an opaque **voxel**. The block's own voxels are not
//!   tested, so an opaque boundary block is visible, matching the fill's leaf
//!   rule.
//! - The camera's own block is always visible. If the camera sits inside a solid
//!   block, the ray skips that block's voxels (the same "resolve visible"
//!   degenerate-seed rule as the fill).
//! - Opacity is a caller-supplied voxel predicate. [`SvdagScene::new`] treats
//!   every non-air voxel as opaque; [`SvdagScene::with_opaque_material`] takes a
//!   material table so water, glass and leaves stay transparent.
//!
//! # Cost
//!
//! One ray per block in the horizon cube, each traversing voxels with an
//! Amanatides–Woo DDA. The cost is `O(horizon⁴)` for a full cube, which is why
//! this stays an experimental flag: M2-26 measures it against the fill and
//! decides whether it earns a default. [`RayTestFill::rays_cast`] and
//! [`RayTestFill::voxel_steps`] expose the work for that comparison.
//!
//! # The flag
//!
//! [`RayTestConfig`] mirrors the `occlusion.raytest` key (ADR-006, roadmap
//! M2-25). Its default is `enabled: false`; the `raytest` cargo feature gates the
//! code and wiring the config into the traversal is M2-26/M2-27 work.

use std::collections::HashMap;

use voxel_core::constants::{BLOCK_DIM, BLOCK_LEVEL, CHUNK_DIM, VOXEL_SIZE, aabb_extent};
use voxel_core::content_constants::OCCLUSION_HORIZON_BLOCKS;
use voxel_storage::dag::Svdag;
use voxel_storage::{AIR, ChunkPos};

/// World-space edge of one block, in metres.
pub const BLOCK_EDGE_M: f64 = aabb_extent(BLOCK_LEVEL) as f64;

/// Default horizon, in blocks (128 m).
pub const DEFAULT_HORIZON_BLOCKS: u32 = OCCLUSION_HORIZON_BLOCKS as u32;

/// Runtime switch for the optional occluder.
///
/// Mirrors the `occlusion.raytest` key (ADR-006, roadmap M2-25). The default is
/// off: the flood fill
/// stays primary (ADR-006). The `raytest` cargo feature gates the code itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayTestConfig {
    /// Whether the traversal should use the ray-test instead of the fill.
    pub enabled: bool,
    /// Chebyshev horizon, in blocks.
    pub horizon_blocks: u32,
}

impl Default for RayTestConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            horizon_blocks: DEFAULT_HORIZON_BLOCKS,
        }
    }
}

/// A voxel-resolution opacity source for the ray-test.
///
/// Coordinates are **world voxels** (one metre = 16 voxels). Returning `true`
/// stops a ray.
pub trait VoxelScene {
    /// True when the world voxel is opaque.
    fn opaque(&self, x: i32, y: i32, z: i32) -> bool;
}

/// Per-block visibility bits produced by one ray-test pass.
///
/// The layout matches the flood-fill reference: one bit per block, `x` fastest,
/// over the horizon cube around [`Self::seed`]. Blocks outside the cube resolve
/// visible, because the traversal lets the frustum decide beyond the horizon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RayTestFill {
    seed: [i32; 3],
    horizon: u32,
    min: [i32; 3],
    side: usize,
    words: Vec<u64>,
    visible: usize,
    rays: usize,
    steps: u64,
}

impl RayTestFill {
    /// The camera block the pass was centred on.
    pub const fn seed(&self) -> [i32; 3] {
        self.seed
    }

    /// The Chebyshev horizon, in blocks.
    pub const fn horizon_blocks(&self) -> u32 {
        self.horizon
    }

    /// The lowest block coordinate covered by the bitset.
    pub const fn origin(&self) -> [i32; 3] {
        self.min
    }

    /// Blocks per edge of the covered cube, `2 * horizon + 1`.
    pub const fn side(&self) -> usize {
        self.side
    }

    /// The raw one-bit-per-block words. Trailing bits in the last word are zero.
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// Blocks marked visible inside the horizon cube.
    pub const fn visible_count(&self) -> usize {
        self.visible
    }

    /// Blocks marked occluded inside the horizon cube.
    pub fn occluded_count(&self) -> usize {
        self.side * self.side * self.side - self.visible
    }

    /// Blocks tested, one ray each; the seed block is answered without a cast.
    pub const fn rays_cast(&self) -> usize {
        self.rays
    }

    /// Voxel cells stepped across all rays; the cost signal for M2-26.
    pub const fn voxel_steps(&self) -> u64 {
        self.steps
    }

    /// True when the block lies inside the horizon cube.
    pub fn is_within_horizon(&self, block: [i32; 3]) -> bool {
        let dx = i64::from(block[0]) - i64::from(self.seed[0]);
        let dy = i64::from(block[1]) - i64::from(self.seed[1]);
        let dz = i64::from(block[2]) - i64::from(self.seed[2]);
        dx.abs() <= i64::from(self.horizon)
            && dy.abs() <= i64::from(self.horizon)
            && dz.abs() <= i64::from(self.horizon)
    }

    /// True when the block is visible.
    ///
    /// Blocks outside the horizon cube are conservatively visible.
    pub fn is_visible(&self, block: [i32; 3]) -> bool {
        let Some(index) = self.index(block) else {
            return true;
        };
        self.words[index / 64] & (1u64 << (index % 64)) != 0
    }

    /// True when the block is inside the horizon and was occluded.
    pub fn is_occluded(&self, block: [i32; 3]) -> bool {
        self.is_within_horizon(block) && !self.is_visible(block)
    }

    /// Visible blocks inside the horizon, in `z, y, x` order with `x` fastest.
    pub fn visible_blocks(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        let side = self.side;
        (0..side).flat_map(move |z| {
            (0..side).flat_map(move |y| {
                (0..side).filter_map(move |x| {
                    let index = x + y * side + z * side * side;
                    if self.words[index / 64] & (1u64 << (index % 64)) == 0 {
                        return None;
                    }
                    Some([
                        self.min[0] + x as i32,
                        self.min[1] + y as i32,
                        self.min[2] + z as i32,
                    ])
                })
            })
        })
    }

    fn index(&self, block: [i32; 3]) -> Option<usize> {
        let local = [
            i64::from(block[0]) - i64::from(self.min[0]),
            i64::from(block[1]) - i64::from(self.min[1]),
            i64::from(block[2]) - i64::from(self.min[2]),
        ];
        let side = self.side as i64;
        if local
            .iter()
            .any(|coordinate| *coordinate < 0 || *coordinate >= side)
        {
            return None;
        }
        Some((local[0] + local[1] * side + local[2] * side * side) as usize)
    }
}

/// Runs the ray-test from a world-space camera position in metres.
///
/// `is_opaque(x, y, z)` classifies a **world voxel**; see [`VoxelScene`]. The
/// pass covers a `(2·horizon + 1)³` block cube and casts one ray per block.
///
/// # Panics
///
/// Panics if the camera is not finite or outside the world voxel range, if the
/// horizon is too large for the block coordinate range, or if the covered cube
/// overflows.
pub fn ray_test<F>(camera: [f64; 3], horizon_blocks: u32, mut is_opaque: F) -> RayTestFill
where
    F: FnMut(i32, i32, i32) -> bool,
{
    let seed = camera_block(camera);
    let horizon = i64::from(horizon_blocks);
    let side_i64 = horizon
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .expect("occlusion horizon overflows the block coordinate range");
    assert!(
        side_i64 <= i32::MAX as i64,
        "occlusion horizon is too large for a block-coordinate cube"
    );
    let side = usize::try_from(side_i64).expect("the horizon side is positive");
    let volume = side
        .checked_pow(3)
        .expect("the occlusion cube volume overflows usize");

    let radius = i32::try_from(horizon).expect("the horizon is a block count");
    let mut min = [0; 3];
    for axis in 0..3 {
        seed[axis]
            .checked_sub(radius)
            .expect("seed block leaves the block coordinate range");
        seed[axis]
            .checked_add(radius)
            .expect("seed block leaves the block coordinate range");
        min[axis] = seed[axis] - radius;
    }

    let mut origin = [0.0f64; 3];
    for axis in 0..3 {
        let voxel = camera[axis] / f64::from(VOXEL_SIZE);
        assert!(
            voxel >= i32::MIN as f64 && voxel <= i32::MAX as f64,
            "camera coordinate {} is outside the world voxel range",
            camera[axis]
        );
        origin[axis] = voxel;
    }

    let mut fill = RayTestFill {
        seed,
        horizon: horizon_blocks,
        min,
        side,
        words: vec![0u64; volume.div_ceil(64)],
        visible: 0,
        rays: 0,
        steps: 0,
    };
    // Well above the maximum L1 path through a diagonal cube (3 · side · 16);
    // the fallback below is conservative overdraw, never a hole.
    let max_steps = 4 * side * BLOCK_DIM + 4;

    for bz in min[2]..=seed[2] + radius {
        for by in min[1]..=seed[1] + radius {
            for bx in min[0]..=seed[0] + radius {
                fill.rays += 1;
                let target = [bx, by, bz];
                let visible = if target == seed {
                    true
                } else {
                    line_of_sight(
                        origin,
                        target,
                        seed,
                        &mut is_opaque,
                        &mut fill.steps,
                        max_steps,
                    )
                };
                if visible {
                    fill.visible += 1;
                    let local = [bx - min[0], by - min[1], bz - min[2]];
                    let index = local[0] as usize
                        + local[1] as usize * side
                        + local[2] as usize * side * side;
                    fill.words[index / 64] |= 1u64 << (index % 64);
                }
            }
        }
    }

    fill
}

/// Runs the ray-test against a [`VoxelScene`].
pub fn ray_test_scene<S>(camera: [f64; 3], horizon_blocks: u32, scene: &S) -> RayTestFill
where
    S: VoxelScene + ?Sized,
{
    ray_test(camera, horizon_blocks, |x, y, z| scene.opaque(x, y, z))
}

/// A [`VoxelScene`] backed by resident SVDAG chunks.
///
/// [`Self::new`] treats every non-air voxel as opaque. Water, glass and leaves
/// must stay transparent for the occluder to be safe: use
/// [`Self::with_opaque_material`] with the material table instead, or the
/// ray-test will cull everything behind them (a hole, not overdraw). Missing
/// chunks read as air.
pub struct SvdagScene<'a, F = fn(u16) -> bool> {
    chunks: &'a HashMap<ChunkPos, &'a Svdag>,
    opaque_material: F,
}

impl<'a> SvdagScene<'a, fn(u16) -> bool> {
    /// Wraps a map of resident chunks, treating non-air as opaque.
    pub fn new(chunks: &'a HashMap<ChunkPos, &'a Svdag>) -> Self {
        Self {
            chunks,
            opaque_material: |material| material != AIR,
        }
    }
}

impl<'a, F> SvdagScene<'a, F>
where
    F: Fn(u16) -> bool,
{
    /// Wraps a map of resident chunks with a material opacity predicate.
    ///
    /// The predicate must agree with the flood fill's block opacity rule for the
    /// subset relation to hold; see the module docs.
    pub fn with_opaque_material(
        chunks: &'a HashMap<ChunkPos, &'a Svdag>,
        opaque_material: F,
    ) -> Self {
        Self {
            chunks,
            opaque_material,
        }
    }

    /// Runs the ray-test against these chunks.
    pub fn ray_test(&self, camera: [f64; 3], horizon_blocks: u32) -> RayTestFill {
        ray_test_scene(camera, horizon_blocks, self)
    }
}

impl<F> VoxelScene for SvdagScene<'_, F>
where
    F: Fn(u16) -> bool,
{
    fn opaque(&self, x: i32, y: i32, z: i32) -> bool {
        let chunk = CHUNK_DIM as i32;
        let pos = ChunkPos::new(
            x.div_euclid(chunk),
            y.div_euclid(chunk),
            z.div_euclid(chunk),
        );
        let Some(dag) = self.chunks.get(&pos) else {
            return false;
        };
        (self.opaque_material)(dag.get(
            x.rem_euclid(chunk) as u32,
            y.rem_euclid(chunk) as u32,
            z.rem_euclid(chunk) as u32,
        ))
    }
}

/// The block containing a world-space camera position in metres.
fn camera_block(camera: [f64; 3]) -> [i32; 3] {
    assert!(
        camera.iter().all(|coordinate| coordinate.is_finite()),
        "a camera coordinate must be finite, got {camera:?}"
    );
    let block = BLOCK_EDGE_M;
    let mut seed = [0i32; 3];
    for axis in 0..3 {
        let coordinate = (camera[axis] / block).floor();
        assert!(
            coordinate >= i32::MIN as f64 && coordinate <= i32::MAX as f64,
            "camera coordinate {} is outside the block range",
            camera[axis]
        );
        seed[axis] = coordinate as i32;
    }
    seed
}

/// Casts one ray and returns true when it reaches `target` before an opaque
/// voxel.
fn line_of_sight<F>(
    origin: [f64; 3],
    target: [i32; 3],
    seed: [i32; 3],
    is_opaque: &mut F,
    steps: &mut u64,
    max_steps: usize,
) -> bool
where
    F: FnMut(i32, i32, i32) -> bool,
{
    let block = BLOCK_DIM as f64;
    let centre = [
        target[0] as f64 * block + block / 2.0,
        target[1] as f64 * block + block / 2.0,
        target[2] as f64 * block + block / 2.0,
    ];
    let delta = [
        centre[0] - origin[0],
        centre[1] - origin[1],
        centre[2] - origin[2],
    ];
    let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    if length == 0.0 {
        return true;
    }
    let direction = [delta[0] / length, delta[1] / length, delta[2] / length];
    let mut cell = [
        origin[0].floor() as i32,
        origin[1].floor() as i32,
        origin[2].floor() as i32,
    ];
    let mut step = [0i32; 3];
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for axis in 0..3 {
        let component = direction[axis];
        if component > 0.0 {
            step[axis] = 1;
            t_max[axis] = (cell[axis] as f64 + 1.0 - origin[axis]) / component;
            t_delta[axis] = 1.0 / component;
        } else if component < 0.0 {
            step[axis] = -1;
            t_max[axis] = (cell[axis] as f64 - origin[axis]) / component;
            t_delta[axis] = -1.0 / component;
        }
    }

    for _ in 0..max_steps {
        let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        *steps += 1;

        let cell_block_pos = cell_block(cell);
        if cell_block_pos == target {
            return true;
        }
        if cell_block_pos == seed {
            // The camera's own block is degenerate seed territory; the fill
            // marks it and its neighbours visible, so skip its own voxels.
            continue;
        }
        if is_opaque(cell[0], cell[1], cell[2]) {
            return false;
        }
    }

    // A ray that exceeds the conservative step bound is reported visible; a
    // false positive is overdraw, a false negative is a hole (ADR-006).
    true
}

/// The block coordinate containing a world voxel.
fn cell_block(cell: [i32; 3]) -> [i32; 3] {
    let block = BLOCK_DIM as i32;
    [
        cell[0].div_euclid(block),
        cell[1].div_euclid(block),
        cell[2].div_euclid(block),
    ]
}
