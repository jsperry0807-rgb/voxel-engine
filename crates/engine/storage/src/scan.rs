//! Read-only save-directory scan (M1-27).
//!
//! [`scan_save`] walks the region logs and reports every live chunk without
//! opening a file for write. The live [`RegionStore`](crate::disk::RegionStore)
//! truncates a torn tail and resynchronises past a corrupt record on open; a
//! post-mortem tool must not modify the file, so the scanner leaves it
//! untouched and records a warning instead.
//!
//! A chunk is live when its cell has a DAG record. The last DAG record for a
//! cell supersedes everything before it (including diffs), and the last diff
//! record is the live overlay snapshot. Records that lost that race still
//! occupy file bytes, which [`ScanSummary::superseded_bytes`] reports.

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::ChunkPos;
use crate::dag::Svdag;
use crate::disk::{
    HDR, KIND_DAG, KIND_DIFF, MAX_RECORD_BYTES, RecordHeader, RegionPos, parse_header,
};
use crate::hash::{FxMap, crc32};
use crate::overlay::Overlay;

/// One live chunk decoded from a region log.
pub struct ScannedChunk {
    pub pos: ChunkPos,
    pub region: RegionPos,
    pub cell: u16,
    pub dag: Svdag,
    pub overlay: Overlay,
    /// Bytes in the live serialized DAG payload.
    pub dag_bytes: u32,
    /// Bytes in the live overlay snapshot payload (0 when there is none).
    pub diff_bytes: u32,
    /// Bytes every record for this chunk occupies, superseded ones included.
    pub record_bytes: u64,
}

/// Totals from a save-directory walk.
#[derive(Clone, Debug, Default)]
pub struct ScanSummary {
    /// Number of `r.*.vxr` files read.
    pub region_files: usize,
    /// Bytes in those files, including torn tails.
    pub file_bytes: u64,
    /// Bytes in complete, CRC-valid records.
    pub record_bytes: u64,
    /// Bytes in the records the scanner reports as live.
    pub live_bytes: u64,
    /// Live chunks handed to the callback.
    pub chunks: usize,
    /// Non-fatal problems: torn tails, corrupt records, unreadable files.
    pub warnings: Vec<String>,
}

impl ScanSummary {
    /// Bytes in complete records that a later record superseded.
    pub fn superseded_bytes(&self) -> u64 {
        self.record_bytes.saturating_sub(self.live_bytes)
    }
}

/// Walks `dir` and calls `on_chunk` once for every live chunk, in region/cell
/// order. The callback owns each decoded chunk, so memory stays bounded by one
/// region index and one chunk payload rather than the whole save.
///
/// A missing or unreadable directory is an error. Individual bad files and
/// records are reported in [`ScanSummary::warnings`] and skipped.
pub fn scan_save(
    dir: impl AsRef<Path>,
    mut on_chunk: impl FnMut(ScannedChunk),
) -> io::Result<ScanSummary> {
    let dir = dir.as_ref();
    let mut regions = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(region) = parse_region_name(name) else {
            continue;
        };
        regions.push((region, path));
    }
    // `read_dir` order is unspecified; sort so a report is reproducible.
    regions.sort_unstable_by_key(|(region, _)| *region);

    let mut summary = ScanSummary::default();
    for (region, path) in regions {
        summary.region_files += 1;
        if let Err(error) = scan_region(region, &path, &mut summary, &mut on_chunk) {
            summary
                .warnings
                .push(format!("{}: {error}", path.display()));
        }
    }
    Ok(summary)
}

/// Region coordinates a chunk can produce (`chunk >> 3`).
const REGION_MIN: i32 = i32::MIN >> 3;
const REGION_MAX: i32 = i32::MAX >> 3;
/// A region holds 8x8x8 chunks; headers are `u16`, so validate the cell.
const CELLS_PER_REGION: u16 = 512;

/// Extracts the region coordinates from `r.<x>.<y>.<z>.vxr`.
fn parse_region_name(name: &str) -> Option<RegionPos> {
    let rest = name.strip_prefix("r.")?.strip_suffix(".vxr")?;
    let mut parts = rest.split('.');
    let axis = |part: Option<&str>| -> Option<i32> {
        let value: i32 = part?.parse().ok()?;
        (REGION_MIN..=REGION_MAX).contains(&value).then_some(value)
    };
    let region = (
        axis(parts.next())?,
        axis(parts.next())?,
        axis(parts.next())?,
    );
    parts.next().is_none().then_some(region)
}

#[derive(Default, Clone, Copy)]
struct LiveEntry {
    dag: Option<(u64, u32)>,
    diff: Option<(u64, u32)>,
    record_bytes: u64,
}

fn scan_region(
    region: RegionPos,
    path: &Path,
    summary: &mut ScanSummary,
    on_chunk: &mut impl FnMut(ScannedChunk),
) -> io::Result<()> {
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    summary.file_bytes += total;

    let mut index: FxMap<u16, LiveEntry> = FxMap::default();
    let mut parsed = 0u64;
    {
        let mut reader = BufReader::new(&mut file);
        loop {
            let mut header = [0u8; HDR];
            if let Err(error) = reader.read_exact(&mut header) {
                if error.kind() == io::ErrorKind::UnexpectedEof {
                    break;
                }
                return Err(error);
            }
            let Some(record) = parse_header(&header) else {
                summary.warnings.push(format!(
                    "{}: bad record header at byte {parsed}",
                    path.display()
                ));
                break;
            };
            let Some(end) = parsed
                .checked_add(HDR as u64)
                .and_then(|end| end.checked_add(record.len as u64))
            else {
                summary
                    .warnings
                    .push(format!("{}: record length overflow", path.display()));
                break;
            };
            if end > total {
                summary.warnings.push(format!(
                    "{}: truncated record at byte {parsed} ({} bytes declared, {} in file)",
                    path.display(),
                    record.len,
                    total.saturating_sub(parsed + HDR as u64),
                ));
                break;
            }
            if record.len > MAX_RECORD_BYTES {
                summary.warnings.push(format!(
                    "{}: record at byte {parsed} declares {} bytes, over the {} byte cap",
                    path.display(),
                    record.len,
                    MAX_RECORD_BYTES,
                ));
                break;
            }
            let mut payload = vec![0u8; record.len as usize];
            reader.read_exact(&mut payload)?;
            if crc32(&payload) != record.crc {
                summary
                    .warnings
                    .push(format!("{}: CRC mismatch at byte {parsed}", path.display()));
                break;
            }

            let entry = index.entry(record.cell).or_default();
            let offset = parsed + HDR as u64;
            apply_record(entry, record, offset);
            entry.record_bytes += HDR as u64 + record.len as u64;
            parsed = end;
        }

        if parsed < total {
            let trailing = total - parsed;
            summary.warnings.push(format!(
                "{}: {trailing} trailing bytes left unread (torn or corrupt tail)",
                path.display()
            ));
        }
        summary.record_bytes += parsed;

        let mut cells: Vec<u16> = index.keys().copied().collect();
        cells.sort_unstable();
        for cell in cells {
            let entry = index[&cell];
            let Some((dag_offset, dag_len)) = entry.dag else {
                if entry.diff.is_some() {
                    summary.warnings.push(format!(
                        "{}: cell {cell} has an overlay snapshot but no DAG",
                        path.display()
                    ));
                }
                continue;
            };
            if cell >= CELLS_PER_REGION {
                summary.warnings.push(format!(
                    "{}: cell {cell} is outside 0..{CELLS_PER_REGION}",
                    path.display()
                ));
                continue;
            }

            // Count the live record bytes before decoding: a corrupt payload is
            // still a live record on disk, not a superseded one.
            summary.live_bytes += HDR as u64 + dag_len as u64;
            if let Some((_, len)) = entry.diff {
                summary.live_bytes += HDR as u64 + len as u64;
            }

            let dag_bytes = read_at(&mut reader, dag_offset, dag_len)?;
            let Some(dag) = Svdag::from_bytes(&dag_bytes) else {
                summary.warnings.push(format!(
                    "{}: cell {cell} has a corrupt DAG payload",
                    path.display()
                ));
                continue;
            };
            let overlay = match entry.diff {
                Some((offset, len)) => {
                    let bytes = read_at(&mut reader, offset, len)?;
                    let Some(overlay) = Overlay::from_bytes(&bytes) else {
                        summary.warnings.push(format!(
                            "{}: cell {cell} has a corrupt overlay payload",
                            path.display()
                        ));
                        continue;
                    };
                    overlay
                }
                None => Overlay::default(),
            };

            summary.chunks += 1;
            on_chunk(ScannedChunk {
                pos: ChunkPos::from_region_cell(region, cell),
                region,
                cell,
                dag,
                overlay,
                dag_bytes: dag_len,
                diff_bytes: entry.diff.map_or(0, |(_, len)| len),
                record_bytes: entry.record_bytes,
            });
        }
    }
    Ok(())
}

fn apply_record(entry: &mut LiveEntry, record: RecordHeader, offset: u64) {
    match record.kind {
        KIND_DAG => {
            entry.dag = Some((offset, record.len));
            entry.diff = None;
        }
        KIND_DIFF => entry.diff = Some((offset, record.len)),
        _ => unreachable!("parse_header rejects unknown kinds"),
    }
}

fn read_at<R: Read + Seek>(reader: &mut R, offset: u64, len: u32) -> io::Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0u8; len as usize];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// The region file a store uses for `region`, useful to tests and tooling.
pub fn region_path(dir: &Path, region: RegionPos) -> PathBuf {
    dir.join(format!("r.{}.{}.{}.vxr", region.0, region.1, region.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::{HDR, KIND_DAG, MAGIC, RegionStore};
    use crate::hash::crc32;
    use crate::{CHUNK_VOLUME, local_index};

    fn tmp(name: &str) -> PathBuf {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("vscan-{name}-{}-{t}", std::process::id()))
    }

    fn dag(material: u16) -> Svdag {
        Svdag::from_dense(&vec![material; CHUNK_VOLUME])
    }

    #[test]
    fn region_names_are_strict() {
        assert_eq!(parse_region_name("r.0.1.-2.vxr"), Some((0, 1, -2)));
        assert_eq!(
            parse_region_name(&format!("r.{}.0.0.vxr", REGION_MAX)),
            Some((REGION_MAX, 0, 0))
        );
        for name in [
            "r.0.1.vxr",
            "r.0.1.2.3.vxr",
            "r.a.1.2.vxr",
            "r.0.1.2.tmp",
            "x.0.1.2.vxr",
            "r.0.1.2.vxr.bak",
        ] {
            assert_eq!(parse_region_name(name), None, "{name}");
        }
        // A valid integer outside the chunk-coordinate range would overflow
        // when mapped back to a chunk, so it is not a region file.
        assert_eq!(
            parse_region_name(&format!("r.{}.0.0.vxr", REGION_MAX as i64 + 1)),
            None
        );
        assert_eq!(
            parse_region_name(&format!("r.{}.0.0.vxr", REGION_MIN as i64 - 1)),
            None
        );
    }

    #[test]
    fn scan_reports_live_chunks_and_superseded_bytes() {
        let dir = tmp("live");
        let a = ChunkPos::new(0, 0, 0);
        let b = ChunkPos::new(1, 2, 3);
        let c = ChunkPos::new(9, 0, -1);
        {
            let mut store = RegionStore::open(&dir).unwrap();
            store.put_dag(a, &dag(3)).unwrap();
            let mut overlay = Overlay::default();
            overlay.set(local_index(1, 2, 3), 7);
            store.put_diff(a, &overlay).unwrap();
            store.put_dag(a, &dag(4)).unwrap(); // supersedes the diff
            store.put_dag(b, &dag(5)).unwrap();
            store.put_dag(c, &dag(6)).unwrap();
            store.sync().unwrap();
        }

        let mut seen = Vec::new();
        let summary = scan_save(&dir, |chunk| seen.push(chunk)).unwrap();

        assert_eq!(summary.region_files, 2);
        assert_eq!(summary.chunks, 3);
        assert_eq!(seen.len(), 3);
        assert!(summary.file_bytes > 0);
        assert!(summary.record_bytes > 0);
        assert!(
            summary.superseded_bytes() > 0,
            "the two superseded records for `a` are still on disk"
        );
        assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);

        // Regions sort first, then cells. Cell 0 is `a`, cell 209 is `b`.
        assert_eq!(seen[0].pos, a);
        assert_eq!(seen[0].cell, 0);
        assert_eq!(seen[1].pos, b);
        assert_eq!(seen[1].cell, 1 | (2 << 3) | (3 << 6));
        assert_eq!(seen[2].pos, c);
        assert_eq!(seen[2].region, (1, 0, -1));
        assert_eq!(seen[2].cell, 1 | (7 << 6));

        assert_eq!(seen[0].dag.get(0, 0, 0), 4, "the latest DAG wins");
        assert!(seen[0].overlay.is_empty(), "a DAG supersedes the diff");
        assert!(seen[0].diff_bytes == 0);
        assert!(seen[0].dag_bytes > 8);
        assert!(seen[0].record_bytes > seen[0].dag_bytes as u64);
        assert!(summary.live_bytes > 0);
    }

    #[test]
    fn scan_keeps_a_torn_tail_and_warns() {
        let dir = tmp("torn");
        let p = ChunkPos::new(0, 0, 0);
        {
            let mut store = RegionStore::open(&dir).unwrap();
            store.put_dag(p, &dag(3)).unwrap();
            store.sync().unwrap();
        }
        let path = region_path(&dir, (0, 0, 0));
        let before = fs::metadata(&path).unwrap().len();
        {
            use std::io::Write;
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(&[0xde, 0xad, 0xbe, 0xef, 0x00]).unwrap();
        }
        let torn = fs::metadata(&path).unwrap().len();

        let mut seen = 0;
        let summary = scan_save(&dir, |_| seen += 1).unwrap();
        assert_eq!(seen, 1, "the complete record is still reported");
        assert!(summary.superseded_bytes() == 0);
        assert!(
            summary
                .warnings
                .iter()
                .any(|w| w.contains("trailing bytes")),
            "{:?}",
            summary.warnings
        );
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            torn,
            "a read-only scan must not truncate the tail"
        );
        assert_ne!(torn, before);
    }

    fn record_header(kind: u8, cell: u16, payload: &[u8]) -> [u8; HDR] {
        let mut header = [0u8; HDR];
        header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        header[4] = kind;
        header[6..8].copy_from_slice(&cell.to_le_bytes());
        header[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        header[12..16].copy_from_slice(&crc32(payload).to_le_bytes());
        header
    }

    #[test]
    fn scan_rejects_an_out_of_range_cell_without_panicking() {
        let dir = tmp("bad-cell");
        fs::create_dir_all(&dir).unwrap();
        let payload = dag(3).to_bytes();
        let mut bytes = record_header(KIND_DAG, 900, &payload).to_vec();
        bytes.extend_from_slice(&payload);
        fs::write(region_path(&dir, (0, 0, 0)), bytes).unwrap();

        let mut seen = 0;
        let summary = scan_save(&dir, |_| seen += 1).unwrap();
        assert_eq!(seen, 0);
        assert_eq!(summary.chunks, 0);
        assert!(
            summary
                .warnings
                .iter()
                .any(|warning| warning.contains("outside")),
            "{:?}",
            summary.warnings
        );
    }

    #[test]
    fn scan_does_not_allocate_from_an_unbacked_record_length() {
        let dir = tmp("huge-len");
        fs::create_dir_all(&dir).unwrap();
        let mut header = record_header(KIND_DAG, 0, &[]);
        header[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(region_path(&dir, (0, 0, 0)), header).unwrap();

        let summary = scan_save(&dir, |_| panic!("no payload to report")).unwrap();
        assert!(
            summary
                .warnings
                .iter()
                .any(|warning| warning.contains("truncated")),
            "{:?}",
            summary.warnings
        );
    }

    #[test]
    fn scan_caps_a_record_length_the_file_can_back() {
        let dir = tmp("cap");
        fs::create_dir_all(&dir).unwrap();
        let path = region_path(&dir, (0, 0, 0));
        let mut header = record_header(KIND_DAG, 0, &[]);
        header[8..12].copy_from_slice(&(MAX_RECORD_BYTES + 1).to_le_bytes());
        {
            use std::io::Write;
            let mut file = fs::File::create(&path).unwrap();
            file.write_all(&header).unwrap();
            // Back the declared length so the file-size check passes and the
            // cap is what rejects the record.
            file.set_len(HDR as u64 + MAX_RECORD_BYTES as u64 + 1)
                .unwrap();
        }

        let summary = scan_save(&dir, |_| panic!("no payload to report")).unwrap();
        assert!(
            summary
                .warnings
                .iter()
                .any(|warning| warning.contains("cap")),
            "{:?}",
            summary.warnings
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn scan_empty_directory_is_empty_and_missing_directory_errors() {
        let dir = tmp("empty");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.txt"), b"ignored").unwrap();
        let summary = scan_save(&dir, |_| panic!("no chunks expected")).unwrap();
        assert_eq!(summary.region_files, 0);
        assert_eq!(summary.chunks, 0);
        assert_eq!(summary.file_bytes, 0);
        assert_eq!(summary.superseded_bytes(), 0);
        assert!(summary.warnings.is_empty());

        let missing = dir.join("missing");
        assert!(scan_save(&missing, |_| {}).is_err());
        fs::remove_dir_all(dir).ok();
    }
}
