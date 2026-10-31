//! Save-level culling granularity (M2-30, gate row 9).
//!
//! M1-34 built the reference instrumentation: [`voxel_storage::reference::granularity`]
//! compares the block volume a hierarchical frustum traversal keeps against the
//! independently computed block-level visibility oracle. The M1 gate left row 9
//! unmeasured because the generator consumed M1-35…M1-38 only. This harness
//! closes that gap: it loads the frozen save, runs the reference over 100 poses
//! that look across the fixture, and reports the per-pose ratio.
//!
//! | Term | Meaning |
//! | --- | --- |
//! | traversed | block volume represented by the emitted nodes |
//! | visible | traversed blocks the occlusion fill marks reachable |
//! | invisible | `traversed - visible`, the granularity cost |
//! | missed | reference-visible blocks the traversal did not emit; must be 0 |
//!
//! The row compares the **p95** ratio with the frozen 15% limit, matching the
//! other latency rows. The max is reported separately so a single bad pose
//! cannot hide. This is the **reference** traversal: it is frustum-only and
//! deliberately over-includes solid interior blocks, so the number is the
//! baseline the production occlusion culler (M5) must beat, not a claim about
//! the raster path.
//!
//! The fixture is provisional until the first real save, like every other gate
//! harness; the report carries that limitation verbatim.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use voxel_core::constants::{BLOCK_DIM, BLOCK_LEVEL, CHUNK_DIM};
use voxel_storage::ChunkPos;
use voxel_storage::dag::Svdag;
use voxel_storage::reference::cull::{Camera, ChunkView, Frustum, Vec3};
use voxel_storage::reference::granularity::{GRANULARITY_GATE_LIMIT, measure};
use voxel_storage::reference::occlusion::flood_fill;
use voxel_storage::scan::scan_save;
use voxel_storage::{AIR, CHUNK_BITS, CHUNK_VOLUME, local_index};

use crate::fixture::{self, Manifest};

use super::memory::FixtureInfo;

/// Schema version of the granularity report.
pub const SCHEMA_VERSION: u32 = 1;

/// Poses per measurement. Matches the M1-34 100-pose test.
pub const POSES: usize = 100;

/// One pose's counts and ratio.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PoseSample {
    /// Pose index, `0..POSES`.
    pub pose: usize,
    /// Block volume represented by the emitted nodes.
    pub traversed_blocks: u64,
    /// Traversed blocks the reference marks visible.
    pub visible_blocks: u64,
    /// Reference-visible blocks the traversal missed; must be 0.
    pub missed_blocks: u64,
    /// `(traversed - visible) / traversed`, `0.0` for an empty traversal.
    pub ratio: f64,
}

/// Save-level granularity report (gate row 9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GranularityReport {
    /// Schema version.
    pub schema_version: u32,
    /// Fixture identity and limitation.
    pub fixture: FixtureInfo,
    /// Number of measured poses.
    pub poses: usize,
    /// Frozen limit, `0.15`.
    pub limit: f64,
    /// Median per-pose ratio.
    pub p50: f64,
    /// p95 per-pose ratio, the gate number.
    pub p95: f64,
    /// Worst per-pose ratio.
    pub max: f64,
    /// Mean per-pose ratio.
    pub mean: f64,
    /// Sum of traversed blocks over all poses.
    pub traversed_blocks: u64,
    /// Sum of visible blocks over all poses.
    pub visible_blocks: u64,
    /// Sum of missed blocks over all poses; must be 0.
    pub missed_blocks: u64,
    /// Every pose, for audit and for the per-pose printout.
    pub per_pose: Vec<PoseSample>,
}

impl GranularityReport {
    /// True when every pose's traversal was conservative.
    pub fn is_conservative(&self) -> bool {
        self.missed_blocks == 0
    }

    /// Renders the report as Markdown.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        writeln!(out, "# M1 gate granularity (M1-34, row 9)\n").unwrap();
        writeln!(
            out,
            "**Fixture:** `{}` ({})\n",
            self.fixture.id, self.fixture.status
        )
        .unwrap();
        writeln!(
            out,
            "**Poses:** {}; **limit:** {:.0}% traversed-but-invisible\n",
            self.poses,
            self.limit * 100.0
        )
        .unwrap();
        writeln!(
            out,
            "| Statistic | Ratio |\n|---|---:|\n| p50 | {:.2}% |\n| p95 | {:.2}% |\n| max | {:.2}% |\n| mean | {:.2}% |\n",
            self.p50 * 100.0,
            self.p95 * 100.0,
            self.max * 100.0,
            self.mean * 100.0
        )
        .unwrap();
        writeln!(
            out,
            "| Pose | Traversed | Visible | Missed | Ratio |\n|---:|---:|---:|---:|---:|\n"
        )
        .unwrap();
        for pose in &self.per_pose {
            writeln!(
                out,
                "| {} | {} | {} | {} | {:.2}% |",
                pose.pose,
                pose.traversed_blocks,
                pose.visible_blocks,
                pose.missed_blocks,
                pose.ratio * 100.0
            )
            .unwrap();
        }
        out
    }
}

/// Runs the reference granularity measurement over `manifest_path`.
pub fn report(manifest_path: &Path) -> Result<GranularityReport, String> {
    let manifest = fixture::read_manifest(manifest_path)?;
    check_manifest(&manifest)?;
    let save_dir = fixture::save_dir(manifest_path, &manifest)?;
    measure_save(manifest_path, &manifest, &save_dir)
}

fn check_manifest(manifest: &Manifest) -> Result<(), String> {
    if manifest.schema_version != fixture::SCHEMA_VERSION {
        return Err(format!(
            "manifest schema {} is not supported (this build is {})",
            manifest.schema_version,
            fixture::SCHEMA_VERSION
        ));
    }
    manifest.recipe.validate()?;
    if manifest.format.chunk_bits != CHUNK_BITS || manifest.format.chunk_dim != CHUNK_DIM as u32 {
        return Err(format!(
            "fixture was frozen at {}-bit chunks ({}³); this build uses {}-bit chunks ({}³). \
             Re-freeze at the matching edge.",
            manifest.format.chunk_bits, manifest.format.chunk_dim, CHUNK_BITS, CHUNK_DIM
        ));
    }
    Ok(())
}

/// A chunk's DAG plus one opaque bit per 16³ block.
struct Loaded {
    dag: Svdag,
    blocks_per_edge: usize,
    opaque: Vec<bool>,
}

fn measure_save(
    manifest_path: &Path,
    manifest: &Manifest,
    save_dir: &Path,
) -> Result<GranularityReport, String> {
    let mut chunks: HashMap<ChunkPos, Loaded> = HashMap::new();
    scan_save(save_dir, |chunk| {
        let mut dense = vec![0u16; CHUNK_VOLUME];
        chunk.dag.expand(&mut dense);
        for (index, material) in chunk.overlay.iter() {
            dense[index as usize] = material;
        }
        let blocks_per_edge = CHUNK_DIM / BLOCK_DIM;
        let mut opaque = vec![false; blocks_per_edge * blocks_per_edge * blocks_per_edge];
        for bz in 0..blocks_per_edge {
            for by in 0..blocks_per_edge {
                for bx in 0..blocks_per_edge {
                    let mut solid = true;
                    'block: for z in 0..BLOCK_DIM {
                        for y in 0..BLOCK_DIM {
                            for x in 0..BLOCK_DIM {
                                let index = local_index(
                                    (bx * BLOCK_DIM + x) as u32,
                                    (by * BLOCK_DIM + y) as u32,
                                    (bz * BLOCK_DIM + z) as u32,
                                );
                                if dense[index as usize] == AIR {
                                    solid = false;
                                    break 'block;
                                }
                            }
                        }
                    }
                    opaque[bx + by * blocks_per_edge + bz * blocks_per_edge * blocks_per_edge] =
                        solid;
                }
            }
        }
        chunks.insert(
            chunk.pos,
            Loaded {
                dag: chunk.dag,
                blocks_per_edge,
                opaque,
            },
        );
    })
    .map_err(|error| format!("failed to scan fixture {}: {error}", save_dir.display()))?;

    if chunks.is_empty() {
        return Err("the fixture has no live chunks to measure".to_owned());
    }

    let fixture = FixtureInfo {
        id: manifest.id.clone(),
        status: format!("{:?}", manifest.status).to_lowercase(),
        provisional_until_first_real_save: manifest.provisional_until_first_real_save,
        limitation: manifest.limitation.clone(),
        manifest: manifest_path.display().to_string(),
        save_dir: save_dir.display().to_string(),
        world_hash: manifest.integrity.world_hash.clone(),
        chunk_bits: CHUNK_BITS,
        chunk_dim: CHUNK_DIM as u32,
        chunks: chunks.len(),
        total_voxels: manifest.observed.total_voxels,
        overlay_entries: manifest.observed.total_overrides,
        region_files: manifest.observed.region_files,
    };

    let views: Vec<ChunkView<'_>> = chunks
        .iter()
        .map(|(pos, loaded)| ChunkView::new(*pos, &loaded.dag))
        .collect();

    // World block bounds of the recipe, from its chunk box. One block is one
    // metre, so block coordinates are also metres.
    let blocks_per_edge = CHUNK_DIM / BLOCK_DIM;
    let recipe = &manifest.recipe;
    let min = [
        recipe.chunk_min[0] * blocks_per_edge as i32,
        recipe.chunk_min[1] * blocks_per_edge as i32,
        recipe.chunk_min[2] * blocks_per_edge as i32,
    ];
    let max = [
        (recipe.chunk_max[0] + 1) * blocks_per_edge as i32,
        (recipe.chunk_max[1] + 1) * blocks_per_edge as i32,
        (recipe.chunk_max[2] + 1) * blocks_per_edge as i32,
    ];
    let centre = Vec3::new(
        (min[0] + max[0]) as f64 * 0.5,
        (min[1] + max[1]) as f64 * 0.5,
        (min[2] + max[2]) as f64 * 0.5,
    );
    let half = ((max[0] - min[0]).max(max[2] - min[2])) as f64 * 0.5;
    let radius = (half * 2.0_f64.sqrt() + 2.0).max(4.0);
    let eye_height = max[1] as f64 + 2.0;
    let horizon = (radius + half.max(max[1] as f64 - min[1] as f64)).ceil() as u32 + 4;

    let mut per_pose = Vec::with_capacity(POSES);
    for pose in 0..POSES {
        let angle = TAU * pose as f64 / POSES as f64;
        let eye = Vec3::new(
            centre.x + radius * angle.cos(),
            eye_height + 0.75 * (2.0 * angle).sin(),
            centre.z + radius * angle.sin(),
        );
        let camera = Camera {
            eye,
            target: Vec3::new(centre.x, min[1] as f64 + 1.5, centre.z),
            up: Vec3::new(0.0, 1.0, 0.0),
            fov_y: PI / 3.0,
            aspect: 16.0 / 9.0,
            near: 0.1,
            far: 64.0,
        };
        let frustum = Frustum::from_camera(&camera);
        let fill = flood_fill(eye, horizon, |bx, by, bz| is_opaque(&chunks, bx, by, bz));
        let measured = measure(&frustum, &views, &fill, BLOCK_LEVEL);
        let ratio = measured.ratio();
        per_pose.push(PoseSample {
            pose,
            traversed_blocks: measured.traversed_blocks(),
            visible_blocks: measured.visible_blocks(),
            missed_blocks: measured.missed_blocks(),
            ratio,
        });
    }

    let mut ratios: Vec<f64> = per_pose.iter().map(|pose| pose.ratio).collect();
    let (p50, p95, max) = percentiles(&mut ratios);
    let mean = per_pose.iter().map(|pose| pose.ratio).sum::<f64>() / per_pose.len() as f64;
    let traversed_blocks = per_pose.iter().map(|pose| pose.traversed_blocks).sum();
    let visible_blocks = per_pose.iter().map(|pose| pose.visible_blocks).sum();
    let missed_blocks = per_pose.iter().map(|pose| pose.missed_blocks).sum();

    Ok(GranularityReport {
        schema_version: SCHEMA_VERSION,
        fixture,
        poses: POSES,
        limit: GRANULARITY_GATE_LIMIT,
        p50,
        p95,
        max,
        mean,
        traversed_blocks,
        visible_blocks,
        missed_blocks,
        per_pose,
    })
}

/// Nearest-rank p50/p95/max over finite ratios, sorted in place.
fn percentiles(values: &mut [f64]) -> (f64, f64, f64) {
    debug_assert!(!values.is_empty());
    values.sort_by(|left, right| {
        left.partial_cmp(right)
            .expect("granularity ratios are finite")
    });
    let rank = |p: usize| {
        let rank = (values.len() * p).div_ceil(100).max(1);
        values[rank.min(values.len()) - 1]
    };
    (rank(50), rank(95), values[values.len() - 1])
}

/// Samples the opaque bit for one world block, treating anything outside the
/// loaded set as air (non-opaque), matching the reference's conservative rule.
fn is_opaque(chunks: &HashMap<ChunkPos, Loaded>, bx: i32, by: i32, bz: i32) -> bool {
    let blocks_per_edge = CHUNK_DIM / BLOCK_DIM;
    let chunk = ChunkPos::new(
        bx.div_euclid(blocks_per_edge as i32),
        by.div_euclid(blocks_per_edge as i32),
        bz.div_euclid(blocks_per_edge as i32),
    );
    let Some(loaded) = chunks.get(&chunk) else {
        return false;
    };
    let local = [
        bx.rem_euclid(blocks_per_edge as i32) as usize,
        by.rem_euclid(blocks_per_edge as i32) as usize,
        bz.rem_euclid(blocks_per_edge as i32) as usize,
    ];
    let edge = loaded.blocks_per_edge;
    loaded.opaque[local[0] + local[1] * edge + local[2] * edge * edge]
}
