//! Save-scan report (M1-27).
//!
//! Reads a save directory without modifying it and turns each live chunk into
//! a JSON-friendly record: bytes of the serialized node graph, voxel count,
//! overlay size, and a material histogram. The report also carries the
//! distribution of per-chunk node bytes and voxel counts (p50/p95/max) and an
//! aggregate material histogram.
//!
//! This is the reference reader for the region-log format: it uses the same
//! header parser as the live store, but never truncates a torn tail. Air
//! (material `0`) is excluded from every histogram and voxel count.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use voxel_storage::scan::scan_save;
use voxel_storage::{AIR, CHUNK_VOLUME};

/// Per-chunk percentile summary. `None` for an empty save.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Distribution {
    pub p50: Option<u64>,
    pub p95: Option<u64>,
    pub max: Option<u64>,
}

impl Distribution {
    /// Nearest-rank percentiles over `values`, sorted in place.
    pub fn from_values(values: &mut [u64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        values.sort_unstable();
        Self {
            p50: Some(percentile(values, 50)),
            p95: Some(percentile(values, 95)),
            max: Some(values[values.len() - 1]),
        }
    }
}

/// Nearest-rank percentile: the smallest value whose rank reaches `p`.
fn percentile(sorted: &[u64], p: u32) -> u64 {
    debug_assert!(!sorted.is_empty());
    let rank = (sorted.len() * p as usize).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

/// One live chunk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRecord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Containing 8x8x8-chunk region.
    pub region: [i32; 3],
    /// Cell inside the region (0..512).
    pub cell: u16,
    /// Live serialized DAG payload: root + word count + node words.
    pub dag_bytes: u64,
    /// Internal node words in the serialized graph (the v1 pool).
    pub node_words: u64,
    /// `node_words * 4`; the per-chunk node byte count.
    pub node_bytes: u64,
    /// Live overlay-snapshot payload, or 0 when the chunk has no diff.
    pub diff_bytes: u64,
    /// All bytes this chunk's records occupy, including superseded records.
    pub record_bytes: u64,
    /// Non-air voxels after applying the overlay.
    pub voxels: u64,
    /// Live overlay entries (edits not yet folded into the DAG).
    pub overrides: u64,
    /// Non-air voxels per material id; keys are decimal material ids in JSON.
    pub materials: BTreeMap<u16, u64>,
}

/// The whole save.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveScanReport {
    pub path: String,
    pub region_files: usize,
    /// Bytes in the region files, torn tails included.
    pub file_bytes: u64,
    /// Bytes in complete, CRC-valid records.
    pub record_bytes: u64,
    /// Bytes in the live DAG and overlay records.
    pub live_bytes: u64,
    /// Bytes in records a later record superseded.
    pub superseded_bytes: u64,
    pub live_chunks: usize,
    /// Non-air voxels across every live chunk.
    pub total_voxels: u64,
    /// Live overlay entries across every live chunk.
    pub total_overrides: u64,
    /// Non-air voxels per material id across the whole save.
    pub materials: BTreeMap<u16, u64>,
    /// Distribution of per-chunk [`ChunkRecord::node_bytes`].
    pub node_bytes: Distribution,
    /// Distribution of per-chunk [`ChunkRecord::voxels`].
    pub voxels: Distribution,
    /// Every live chunk, sorted by region and cell.
    pub chunks: Vec<ChunkRecord>,
    /// Non-fatal scan problems: torn tails, corrupt records, unreadable files.
    pub warnings: Vec<String>,
}

/// Scans `path` and builds the report. A missing directory is an error; an
/// empty one produces an empty report.
pub fn scan(path: impl AsRef<Path>) -> io::Result<SaveScanReport> {
    let path = path.as_ref();
    let mut dense = vec![0u16; CHUNK_VOLUME];
    let mut chunks = Vec::new();
    let mut materials = BTreeMap::new();
    let mut node_bytes = Vec::new();
    let mut voxel_counts = Vec::new();
    let mut total_voxels = 0u64;
    let mut total_overrides = 0u64;

    let summary = scan_save(path, |chunk| {
        chunk.dag.expand(&mut dense);
        for (index, material) in chunk.overlay.iter() {
            dense[index as usize] = material;
        }

        let mut histogram = BTreeMap::new();
        let mut voxels = 0u64;
        for &material in &dense {
            if material == AIR {
                continue;
            }
            voxels += 1;
            *histogram.entry(material).or_insert(0) += 1;
        }
        for (&material, &count) in &histogram {
            *materials.entry(material).or_insert(0) += count;
        }

        let node_words = chunk.dag.words() as u64;
        let bytes = node_words * 4;
        node_bytes.push(bytes);
        voxel_counts.push(voxels);
        total_voxels += voxels;
        total_overrides += chunk.overlay.len() as u64;

        chunks.push(ChunkRecord {
            x: chunk.pos.x,
            y: chunk.pos.y,
            z: chunk.pos.z,
            region: [chunk.region.0, chunk.region.1, chunk.region.2],
            cell: chunk.cell,
            dag_bytes: chunk.dag_bytes as u64,
            node_words,
            node_bytes: bytes,
            diff_bytes: chunk.diff_bytes as u64,
            record_bytes: chunk.record_bytes,
            voxels,
            overrides: chunk.overlay.len() as u64,
            materials: histogram,
        });
    })?;

    Ok(SaveScanReport {
        path: path.display().to_string(),
        region_files: summary.region_files,
        file_bytes: summary.file_bytes,
        record_bytes: summary.record_bytes,
        live_bytes: summary.live_bytes,
        superseded_bytes: summary.superseded_bytes(),
        live_chunks: summary.chunks,
        total_voxels,
        total_overrides,
        materials,
        node_bytes: Distribution::from_values(&mut node_bytes),
        voxels: Distribution::from_values(&mut voxel_counts),
        chunks,
        warnings: summary.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles_cover_small_samples() {
        assert_eq!(Distribution::from_values(&mut []), Distribution::default());
        assert_eq!(
            Distribution::from_values(&mut [42]),
            Distribution {
                p50: Some(42),
                p95: Some(42),
                max: Some(42),
            }
        );

        let mut values: Vec<u64> = (1..=100).collect();
        let dist = Distribution::from_values(&mut values);
        assert_eq!(dist.p50, Some(50));
        assert_eq!(dist.p95, Some(95));
        assert_eq!(dist.max, Some(100));

        let mut two = [9, 1];
        assert_eq!(
            Distribution::from_values(&mut two),
            Distribution {
                p50: Some(1),
                p95: Some(9),
                max: Some(9),
            }
        );
    }
}
