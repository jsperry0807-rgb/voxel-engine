//! Per-chunk memory distribution over the frozen fixture (M1-35).
//!
//! The gate asks one question about memory: how many bytes does a populated
//! chunk hold, by component, at the current chunk edge? This module answers it
//! by walking the M1-28 fixture through the M1-27 read-only scanner and adding
//! the fixed content volumes and the M1-31 greedy-mesh estimate:
//!
//! | Component | What is counted |
//! | --- | --- |
//! | DAG | Banded `ChunkDag`: 8 B internal nodes (padding included) + 2 B leaf materials |
//! | Overlay | Allocated GPU overlay storage: one block table per edited chunk plus 8 704 B per edited 16³ block |
//! | Light | [`LIGHT_BYTES_PER_CHUNK`] (1 KiB at 64³, 8 KiB at 128³) |
//! | Probes | [`PROBE_BYTES_PER_CHUNK`] (768 B at 64³, 6 KiB at 128³) |
//! | Mesh | M1-31 greedy quads × 76 B; count-only, no vertex/index buffer overhead |
//! | Total | The five components above, per chunk |
//!
//! The DAG row is the authoritative banded layout. The transitional v1 `Svdag`
//! pool is still resident for GPU and disk compatibility, so it is its own
//! [`MemoryComponents::legacy_v1`] component and is included in the total: a
//! representation that occupies memory must not disappear from the gate.
//!
//! The fixture is **provisional until the first real save**. Its provenance and
//! limitation are copied into the report so a reader cannot see the numbers
//! without the caveat. The report is distributed (p50/p95/max) over every live
//! chunk; no chunk is generated inside the benchmark process.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use voxel_core::content_constants::{
    LIGHT_BYTES_PER_CHUNK, LIGHT_CELLS_PER_CHUNK, PROBE_BYTES_PER_CHUNK,
};
use voxel_storage::gpu::{BLOCKS_PER_CHUNK, OVERLAY_WORDS, chunk_block};
use voxel_storage::reference::mesh::count_greedy_dense;
use voxel_storage::scan::scan_save;
use voxel_storage::{
    AIR, CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME, LEAF_MATERIAL_BYTES, Node, serialize,
};

use crate::fixture::{self, FixtureStatus, Manifest};
use crate::save_scan::Distribution;

/// Schema version of the memory report itself.
/// Version 2 counts the resident v1 `Svdag` pool in [`MemoryComponents::total`].
pub const SCHEMA_VERSION: u32 = 2;

/// Bytes per internal DAG node in the banded layout.
pub const NODE_BYTES: u64 = core::mem::size_of::<Node>() as u64;

/// Bytes per leaf material id.
pub const LEAF_BYTES: u64 = LEAF_MATERIAL_BYTES;

/// Bytes per 16³ GPU overlay slot: packed `u16` values plus presence bits.
pub const OVERLAY_SLOT_BYTES: u64 = (OVERLAY_WORDS * core::mem::size_of::<u32>()) as u64;

/// Bytes in one chunk's GPU block table.
pub const OVERLAY_BLOCK_TABLE_BYTES: u64 = (BLOCKS_PER_CHUNK * core::mem::size_of::<u32>()) as u64;

/// Nearest-rank percentiles plus the aggregate sum for one component.
///
/// Percentiles are over chunks. `total` is the sum across the fixture and is
/// not a percentile; it exists so a reader can reconcile the table with the
/// save-level size.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentDistribution {
    /// Nearest-rank median, or `None` when the fixture has no chunks.
    pub p50: Option<u64>,
    /// Nearest-rank 95th percentile, or `None` when the fixture has no chunks.
    pub p95: Option<u64>,
    /// Maximum chunk value, or `None` when the fixture has no chunks.
    pub max: Option<u64>,
    /// Sum over all chunks.
    pub total: u64,
}

impl ComponentDistribution {
    /// Builds the distribution from unsorted per-chunk values.
    fn from_values(values: &mut [u64]) -> Self {
        let total = values.iter().sum();
        let distribution = Distribution::from_values(values);
        Self {
            p50: distribution.p50,
            p95: distribution.p95,
            max: distribution.max,
            total,
        }
    }
}

/// One component per gate row, plus the per-chunk total.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryComponents {
    /// Banded DAG bytes: internal nodes (with padding) plus leaf materials.
    pub dag: ComponentDistribution,
    /// Transitional v1 `Svdag` pool bytes. Resident until the banded disk/GPU
    /// migration (M6-09a); included in [`Self::total`].
    pub legacy_v1: ComponentDistribution,
    /// Allocated GPU overlay storage.
    pub overlay: ComponentDistribution,
    /// Fixed block-light volume.
    pub light: ComponentDistribution,
    /// Fixed near-LOD probe volume.
    pub probes: ComponentDistribution,
    /// M1-31 count-only greedy mesh estimate.
    pub mesh: ComponentDistribution,
    /// Sum of the five components above, per chunk.
    pub total: ComponentDistribution,
}

/// One live fixture chunk and its memory estimate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkMemory {
    /// Chunk x coordinate.
    pub x: i32,
    /// Chunk y coordinate.
    pub y: i32,
    /// Chunk z coordinate.
    pub z: i32,
    /// Banded DAG bytes included in the gate total.
    pub dag_bytes: u64,
    /// Internal banded node bytes, including alignment padding.
    pub dag_node_bytes: u64,
    /// Leaf-arena bytes.
    pub dag_leaf_bytes: u64,
    /// Transitional v1 `Svdag` pool bytes; resident and included in [`Self::total_bytes`].
    pub dag_v1_bytes: u64,
    /// Allocated GPU overlay bytes.
    pub overlay_bytes: u64,
    /// Live overlay entries after applying the chunk's diff.
    pub overlay_entries: u64,
    /// Block-light bytes.
    pub light_bytes: u64,
    /// Near-LOD probe bytes.
    pub probe_bytes: u64,
    /// Greedy quads counted by the M1-31 reference.
    pub mesh_quads: u64,
    /// Greedy mesh payload estimate (`mesh_quads × 76`).
    pub mesh_bytes: u64,
    /// Sum of DAG, the v1 pool, overlay, light, probes and mesh.
    pub total_bytes: u64,
}

/// Fixture identity and provenance carried by the report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureInfo {
    /// Manifest id, e.g. `m1-representative`.
    pub id: String,
    /// Lifecycle status from the manifest.
    pub status: String,
    /// True while the fixture is synthetic and must not be quoted as a gate result.
    pub provisional_until_first_real_save: bool,
    /// Verbatim limitation from the manifest.
    pub limitation: String,
    /// Manifest path used for the measurement.
    pub manifest: String,
    /// Resolved save directory.
    pub save_dir: String,
    /// BLAKE3 world hash recorded by the manifest.
    pub world_hash: String,
    /// Chunk edge bits compiled into this build.
    pub chunk_bits: u32,
    /// Chunk edge in voxels compiled into this build.
    pub chunk_dim: u32,
    /// Live chunks measured.
    pub chunks: usize,
    /// Non-air voxels after applying every overlay.
    pub total_voxels: u64,
    /// Live overlay entries across the fixture.
    pub overlay_entries: u64,
    /// Region files read by the scanner.
    pub region_files: usize,
}

/// The complete M1-35 memory report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReport {
    /// Schema version of this report.
    pub schema_version: u32,
    /// Fixture identity and provenance.
    pub fixture: FixtureInfo,
    /// Gate components and their per-chunk distributions.
    pub components: MemoryComponents,
    /// Mirror of [`MemoryComponents::legacy_v1`], kept for report consumers.
    pub dag_v1_view: ComponentDistribution,
    /// Every live chunk, sorted by `(x, y, z)`.
    pub chunks: Vec<ChunkMemory>,
    /// Definitions and caveats a reader needs to interpret the table.
    pub notes: Vec<String>,
}

impl MemoryReport {
    /// True while the measured fixture is the synthetic M1 placeholder.
    pub fn is_provisional(&self) -> bool {
        self.fixture.provisional_until_first_real_save
    }

    /// Renders the report as a Markdown table plus provenance and notes.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        writeln!(out, "# M1 gate memory (M1-35)\n").unwrap();
        writeln!(
            out,
            "**Fixture:** `{}` ({})\n",
            self.fixture.id, self.fixture.status
        )
        .unwrap();
        writeln!(out, "**Manifest:** `{}`\n", self.fixture.manifest).unwrap();
        writeln!(out, "**Save:** `{}`\n", self.fixture.save_dir).unwrap();
        writeln!(
            out,
            "**Format:** {}³ ({} bits), {} chunks, {} non-air voxels, {} overlay entries, {} region files\n",
            self.fixture.chunk_dim,
            self.fixture.chunk_bits,
            self.fixture.chunks,
            self.fixture.total_voxels,
            self.fixture.overlay_entries,
            self.fixture.region_files,
        )
        .unwrap();
        writeln!(out, "**World hash:** `{}`\n", self.fixture.world_hash).unwrap();

        if self.is_provisional() {
            writeln!(
                out,
                "> **Provisional fixture.** {}\n",
                self.fixture.limitation
            )
            .unwrap();
        }

        writeln!(out, "| Component | p50 | p95 | max | total |").unwrap();
        writeln!(out, "|---|---:|---:|---:|---:|").unwrap();
        for (name, distribution) in [
            ("DAG", &self.components.dag),
            ("DAG v1 pool (resident)", &self.components.legacy_v1),
            ("Overlay", &self.components.overlay),
            ("Light", &self.components.light),
            ("Probes", &self.components.probes),
            ("Mesh estimate", &self.components.mesh),
            ("Total", &self.components.total),
        ] {
            writeln!(
                out,
                "| {name} | {} | {} | {} | {} |",
                optional(distribution.p50),
                optional(distribution.p95),
                optional(distribution.max),
                distribution.total,
            )
            .unwrap();
        }

        for note in &self.notes {
            writeln!(out, "\n- {note}").unwrap();
        }
        out
    }
}

fn optional(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

/// Measures the default fixture manifest.
pub fn default_report() -> Result<MemoryReport, String> {
    report(&fixture::default_manifest())
}

/// Measures the fixture named by `manifest_path`.
///
/// The manifest is checked for schema and chunk edge before the save is read.
/// The scan must match the manifest's live chunk and voxel counts; a save that
/// was edited after freezing is an error, not a silently different report.
pub fn report(manifest_path: &Path) -> Result<MemoryReport, String> {
    let manifest = fixture::read_manifest(manifest_path)?;
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
             The 64³ fixture is intentionally not valid at another edge; re-freeze at the matching edge.",
            manifest.format.chunk_bits, manifest.format.chunk_dim, CHUNK_BITS, CHUNK_DIM
        ));
    }
    if manifest.format.chunk_version != serialize::VERSION {
        return Err(format!(
            "fixture chunk version {} is not supported (this build is {})",
            manifest.format.chunk_version,
            serialize::VERSION
        ));
    }

    let save_dir = fixture::save_dir(manifest_path, &manifest)?;
    measure(manifest_path, &save_dir, manifest)
}

fn measure(
    manifest_path: &Path,
    save_dir: &Path,
    manifest: Manifest,
) -> Result<MemoryReport, String> {
    let mut dense = vec![0u16; CHUNK_VOLUME];
    let mut chunks = Vec::with_capacity(manifest.observed.live_chunks);
    let mut total_voxels = 0u64;
    let mut overlay_entries = 0u64;

    let summary = scan_save(save_dir, |chunk| {
        chunk.dag.expand(&mut dense);
        for (index, material) in chunk.overlay.iter() {
            dense[index as usize] = material;
        }

        let voxels = dense.iter().filter(|&&material| material != AIR).count() as u64;
        total_voxels += voxels;

        let (overlay_bytes, entries) = overlay_memory(chunk.overlay.iter().map(|(index, _)| index));
        overlay_entries += entries;

        let banded = chunk.dag.chunk();
        let dag_node_bytes = banded.node_words() * NODE_BYTES;
        let dag_leaf_bytes = banded.leaf_words() * LEAF_BYTES;
        let dag_bytes = dag_node_bytes + dag_leaf_bytes;
        let dag_v1_bytes = chunk.dag.words() as u64 * core::mem::size_of::<u32>() as u64;

        let mesh = count_greedy_dense(CHUNK_DIM, &dense);
        let mesh_quads = mesh.quads() as u64;
        let mesh_bytes = mesh.bytes() as u64;
        let light_bytes = LIGHT_BYTES_PER_CHUNK as u64;
        let probe_bytes = PROBE_BYTES_PER_CHUNK as u64;

        chunks.push(ChunkMemory {
            x: chunk.pos.x,
            y: chunk.pos.y,
            z: chunk.pos.z,
            dag_bytes,
            dag_node_bytes,
            dag_leaf_bytes,
            dag_v1_bytes,
            overlay_bytes,
            overlay_entries: entries,
            light_bytes,
            probe_bytes,
            mesh_quads,
            mesh_bytes,
            total_bytes: dag_bytes
                + dag_v1_bytes
                + overlay_bytes
                + light_bytes
                + probe_bytes
                + mesh_bytes,
        });
    })
    .map_err(|error| format!("failed to scan fixture {}: {error}", save_dir.display()))?;

    if !summary.warnings.is_empty() {
        return Err(format!(
            "fixture scan reported warnings; refusing to build a gate report from a damaged save: {:?}",
            summary.warnings
        ));
    }
    if summary.chunks != manifest.observed.live_chunks
        || total_voxels != manifest.observed.total_voxels
        || overlay_entries != manifest.observed.total_overrides
        || summary.region_files != manifest.observed.region_files
    {
        return Err(format!(
            "fixture observations differ from the manifest (chunks {} vs {}, voxels {} vs {}, \
             overlays {} vs {}, regions {} vs {}); run `voxel-bench fixture verify` or re-freeze",
            summary.chunks,
            manifest.observed.live_chunks,
            total_voxels,
            manifest.observed.total_voxels,
            overlay_entries,
            manifest.observed.total_overrides,
            summary.region_files,
            manifest.observed.region_files,
        ));
    }

    chunks.sort_unstable_by_key(|chunk| (chunk.x, chunk.y, chunk.z));

    let components = MemoryComponents {
        dag: distribution(chunks.iter().map(|chunk| chunk.dag_bytes)),
        legacy_v1: distribution(chunks.iter().map(|chunk| chunk.dag_v1_bytes)),
        overlay: distribution(chunks.iter().map(|chunk| chunk.overlay_bytes)),
        light: distribution(chunks.iter().map(|chunk| chunk.light_bytes)),
        probes: distribution(chunks.iter().map(|chunk| chunk.probe_bytes)),
        mesh: distribution(chunks.iter().map(|chunk| chunk.mesh_bytes)),
        total: distribution(chunks.iter().map(|chunk| chunk.total_bytes)),
    };
    let dag_v1_view = distribution(chunks.iter().map(|chunk| chunk.dag_v1_bytes));

    let status = match manifest.status {
        FixtureStatus::Provisional => "provisional".to_owned(),
    };
    let fixture = FixtureInfo {
        id: manifest.id.clone(),
        status,
        provisional_until_first_real_save: manifest.provisional_until_first_real_save,
        limitation: manifest.limitation.clone(),
        manifest: manifest_path.display().to_string(),
        save_dir: save_dir.display().to_string(),
        world_hash: manifest.integrity.world_hash.clone(),
        chunk_bits: CHUNK_BITS,
        chunk_dim: CHUNK_DIM as u32,
        chunks: summary.chunks,
        total_voxels,
        overlay_entries,
        region_files: summary.region_files,
    };

    let notes = vec![
        format!(
            "DAG = {NODE_BYTES} B/internal node (banded layout, full-run padding included) + \
             {LEAF_BYTES} B/leaf material. The transitional v1 Svdag pool is resident for GPU and \
             disk compatibility and is included in the total; M6-09a removes it."
        ),
        format!(
            "Overlay = {OVERLAY_BLOCK_TABLE_BYTES} B block table + {OVERLAY_SLOT_BYTES} B per \
             edited 16³ block, the allocated GPU layout. This fixture has {overlay_entries} \
             pending overlay entries, so its overlay component is zero."
        ),
        format!(
            "Light = {LIGHT_BYTES_PER_CHUNK} B/chunk ({LIGHT_CELLS_PER_CHUNK} cells × 16 B); \
             probes = {PROBE_BYTES_PER_CHUNK} B/chunk at the near LOD. Both are fixed by the \
             compiled chunk edge, not by content."
        ),
        "Mesh estimate = M1-31 count-only greedy quads × 76 B; it excludes vertex/index buffer \
         overhead and treats every non-air material as opaque. The reference has no neighbour \
         apron, so faces on a chunk boundary are counted as exposed and the estimate is an \
         upper bound for those faces."
            .to_owned(),
        "A/B root overhead is not part of this measurement; the DAG row counts the shared banded \
         chunk, not the duplicated path runs."
            .to_owned(),
    ];

    Ok(MemoryReport {
        schema_version: SCHEMA_VERSION,
        fixture,
        components,
        dag_v1_view,
        chunks,
        notes,
    })
}

/// Allocated GPU overlay bytes and live entry count for one chunk.
///
/// The block table is allocated on the first edit in a chunk. Each distinct
/// edited 16³ block then owns one full overlay slot, whether it holds one edit
/// or many.
fn overlay_memory(indices: impl IntoIterator<Item = u32>) -> (u64, u64) {
    let mut blocks = BTreeSet::new();
    let mut entries = 0u64;
    for index in indices {
        entries += 1;
        blocks.insert(chunk_block(index));
    }
    if entries == 0 {
        return (0, 0);
    }
    (
        OVERLAY_BLOCK_TABLE_BYTES + blocks.len() as u64 * OVERLAY_SLOT_BYTES,
        entries,
    )
}

fn distribution(values: impl IntoIterator<Item = u64>) -> ComponentDistribution {
    let mut values: Vec<u64> = values.into_iter().collect();
    ComponentDistribution::from_values(&mut values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_distribution_keeps_percentiles_and_sum() {
        let mut values = [100, 1, 20];
        let measured = ComponentDistribution::from_values(&mut values);
        assert_eq!(measured.p50, Some(20));
        assert_eq!(measured.p95, Some(100));
        assert_eq!(measured.max, Some(100));
        assert_eq!(measured.total, 121);

        let empty = distribution(std::iter::empty());
        assert_eq!(empty, ComponentDistribution::default());
    }

    #[test]
    fn overlay_memory_charges_one_slot_per_edited_block() {
        // Two voxels in the first 16³ block, one in the next block along y.
        // Derive the indices from the compiled edge; a hardcoded local index
        // would silently mean a different coordinate in a 128³ build.
        let block = voxel_core::constants::BLOCK_DIM as u32;
        let indices = [
            voxel_storage::local_index(0, 0, 0),
            voxel_storage::local_index(1, 0, 0),
            voxel_storage::local_index(0, block, 0),
        ];
        let (bytes, entries) = overlay_memory(indices);
        assert_eq!(entries, 3);
        assert_eq!(bytes, OVERLAY_BLOCK_TABLE_BYTES + 2 * OVERLAY_SLOT_BYTES);

        assert_eq!(overlay_memory(std::iter::empty()), (0, 0));
    }

    #[test]
    fn fixed_components_follow_the_compiled_edge() {
        assert_eq!(LIGHT_BYTES_PER_CHUNK as u64, BLOCKS_PER_CHUNK as u64 * 16);
        assert_eq!(PROBE_BYTES_PER_CHUNK as u64, BLOCKS_PER_CHUNK as u64 * 12);
    }
}
