//! Tier 1: region files. One append-only log per 8x8x8-chunk region.
//!
//! Record: magic u32 | kind u8 | pad u8 | cell u16 | len u32 | crc32(payload) u32 | payload
//!   kind 1 = serialized Svdag (supersedes everything before it for that cell, incl. diffs)
//!   kind 2 = full Overlay snapshot (last one wins)
//! Open scans each log, rebuilds the index, and truncates a torn tail. A
//! corrupt record in the middle is skipped: scanning resumes at the next
//! CRC-valid record, and [`RegionStore::recovered_bytes`] reports the bytes
//! that could not be used.
use crate::ChunkPos;
use crate::dag::Svdag;
use crate::hash::{FxMap, crc32};
use crate::overlay::Overlay;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

pub(crate) const MAGIC: u32 = 0x5856_5231; // "1RVX"
pub(crate) const KIND_DAG: u8 = 1;
pub(crate) const KIND_DIFF: u8 = 2;
pub(crate) const HDR: usize = 16;

/// Largest payload a record may declare.
///
/// A real chunk is orders of magnitude smaller; the cap stops a corrupt header
/// from driving a multi-gigabyte allocation before the CRC rejects it.
pub(crate) const MAX_RECORD_BYTES: u32 = 16 << 20;

/// A decoded record header, shared by the live store and the read-only scanner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecordHeader {
    pub kind: u8,
    pub cell: u16,
    pub len: u32,
    pub crc: u32,
}

/// Decodes a record header, or returns `None` for an unknown magic/kind.
pub(crate) fn parse_header(h: &[u8; HDR]) -> Option<RecordHeader> {
    if u32::from_le_bytes(h[0..4].try_into().unwrap()) != MAGIC {
        return None;
    }
    let kind = h[4];
    if kind != KIND_DAG && kind != KIND_DIFF {
        return None;
    }
    Some(RecordHeader {
        kind,
        cell: u16::from_le_bytes(h[6..8].try_into().unwrap()),
        len: u32::from_le_bytes(h[8..12].try_into().unwrap()),
        crc: u32::from_le_bytes(h[12..16].try_into().unwrap()),
    })
}

#[derive(Default, Clone, Copy)]
struct Entry {
    dag: Option<(u64, u32)>, // (payload offset, len)
    diff: Option<(u64, u32)>,
}

struct Region {
    file: File,
    len: u64,
    index: FxMap<u16, Entry>,
    /// Appended since the last [`RegionStore::sync`].
    dirty: bool,
}

impl Region {
    fn read(&mut self, off: u64, len: u32) -> io::Result<Vec<u8>> {
        let mut b = vec![0u8; len as usize];
        self.file.seek(SeekFrom::Start(off))?;
        self.file.read_exact(&mut b)?;
        Ok(b)
    }
}

pub type RegionPos = (i32, i32, i32);

pub struct RegionStore {
    dir: PathBuf,
    /// Handle used to fsync the directory itself.
    ///
    /// Syncing a new region file's contents is not enough: without a directory
    /// fsync the file's very existence can be lost on power failure.
    dir_file: File,
    regions: FxMap<RegionPos, Region>,
    /// A region file was created since the last [`RegionStore::sync`].
    dir_dirty: bool,
    recovered_bytes: u64,
    /// Directory fsyncs performed, so tests can pin the durability call.
    #[cfg(test)]
    dir_syncs: usize,
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn encode(kind: u8, cell: u16, payload: &[u8]) -> Vec<u8> {
    let mut h = encode_header(kind, cell, payload.len() as u32);
    h[12..16].copy_from_slice(&crc32(payload).to_le_bytes());
    let mut b = Vec::with_capacity(HDR + payload.len());
    b.extend_from_slice(&h);
    b.extend_from_slice(payload);
    b
}

fn encode_header(kind: u8, cell: u16, len: u32) -> [u8; HDR] {
    let mut h = [0u8; HDR];
    h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    h[4] = kind;
    h[6..8].copy_from_slice(&cell.to_le_bytes());
    h[8..12].copy_from_slice(&len.to_le_bytes());
    // CRC is filled by the caller, which has the payload.
    h
}

/// Reads one record at `pos`, or `None` when the header, declared length, or
/// CRC is bad. Returns an I/O error only for a real read failure.
fn read_record<R: Read>(r: &mut R, pos: u64, total: u64) -> io::Result<Option<RecordHeader>> {
    let mut h = [0u8; HDR];
    match r.read_exact(&mut h) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let Some(record) = parse_header(&h) else {
        return Ok(None);
    };
    if record.len > MAX_RECORD_BYTES || pos + HDR as u64 + record.len as u64 > total {
        return Ok(None);
    }
    let mut payload = vec![0u8; record.len as usize];
    match r.read_exact(&mut payload) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    Ok((crc32(&payload) == record.crc).then_some(record))
}

/// Searches `from..total` for the next CRC-valid record start.
///
/// Scans block by block for [`MAGIC`], verifying each candidate in full before
/// accepting it, so bytes that merely look like a header inside a payload do
/// not desynchronise the log.
fn resync<R: Read + Seek>(r: &mut R, from: u64, total: u64) -> io::Result<Option<u64>> {
    const BLOCK: usize = 64 * 1024;
    // A corrupt payload can hold thousands of accidental magic values, and each
    // verification CRC-reads a record of up to MAX_RECORD_BYTES. Give up after a
    // bounded number of misses instead of grinding through a 16 MiB blob.
    const MAX_FAILED: u32 = 256;
    let magic = MAGIC.to_le_bytes();
    let mut buf = vec![0u8; BLOCK + 3];
    let mut failed = 0u32;
    let mut base = from;
    while base + HDR as u64 <= total {
        r.seek(SeekFrom::Start(base))?;
        let want = (total - base).min(buf.len() as u64) as usize;
        let mut filled = 0;
        while filled < want {
            match r.read(&mut buf[filled..want])? {
                0 => break,
                n => filled += n,
            }
        }
        if filled < HDR {
            break;
        }
        for i in 0..=filled - HDR {
            if buf[i..i + 4] != magic {
                continue;
            }
            let candidate = base + i as u64;
            r.seek(SeekFrom::Start(candidate))?;
            if read_record(r, candidate, total)?.is_some() {
                return Ok(Some(candidate));
            }
            failed += 1;
            if failed == MAX_FAILED {
                return Ok(None);
            }
        }
        if filled < want {
            break;
        }
        // Overlap three bytes so a magic split across blocks is found.
        base += (filled - 3) as u64;
    }
    Ok(None)
}

fn scan(file: &mut File) -> io::Result<(u64, FxMap<u16, Entry>, u64)> {
    let total = file.metadata()?.len();
    let mut index: FxMap<u16, Entry> = FxMap::default();
    let mut pos = 0u64;
    let mut good_end = 0u64;
    let mut skipped = 0u64;
    {
        let mut r = BufReader::new(&*file);
        while pos < total {
            let Some(record) = read_record(&mut r, pos, total)? else {
                // A corrupt record does not end the log: keep every record
                // after it instead of truncating them away.
                let Some(next) = resync(&mut r, pos + 1, total)? else {
                    break;
                };
                skipped += next - pos;
                pos = next;
                // `resync` left the reader at the end of the record it
                // validated; rewind so the next iteration decodes it.
                r.seek(SeekFrom::Start(pos))?;
                continue;
            };
            let e = index.entry(record.cell).or_default();
            match record.kind {
                KIND_DAG => {
                    e.dag = Some((pos + HDR as u64, record.len));
                    e.diff = None;
                }
                KIND_DIFF => e.diff = Some((pos + HDR as u64, record.len)),
                _ => unreachable!("parse_header rejects unknown kinds"),
            }
            pos += HDR as u64 + record.len as u64;
            good_end = pos;
        }
    }
    if good_end < total {
        file.set_len(good_end)?;
    }
    let recovered = skipped + (total - good_end);
    Ok((good_end, index, recovered))
}

impl RegionStore {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let dir_file = File::open(&dir)?;
        Ok(Self {
            dir,
            dir_file,
            regions: FxMap::default(),
            dir_dirty: false,
            recovered_bytes: 0,
            #[cfg(test)]
            dir_syncs: 0,
        })
    }

    fn path(&self, rp: RegionPos) -> PathBuf {
        self.dir.join(format!("r.{}.{}.{}.vxr", rp.0, rp.1, rp.2))
    }

    fn region(&mut self, rp: RegionPos, create: bool) -> io::Result<Option<&mut Region>> {
        if !self.regions.contains_key(&rp) {
            let path = self.path(rp);
            let created = !path.exists();
            if !create && !path.exists() {
                return Ok(None);
            }
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            let (len, index, recovered) = scan(&mut file)?;
            self.recovered_bytes += recovered;
            self.dir_dirty |= created;
            self.regions.insert(
                rp,
                Region {
                    file,
                    len,
                    index,
                    dirty: false,
                },
            );
        }
        Ok(self.regions.get_mut(&rp))
    }

    fn append(&mut self, p: ChunkPos, kind: u8, payload: &[u8]) -> io::Result<()> {
        // The reader treats an over-cap record as the end of the region
        // (`read_record`), so writing one would lose the payload on reopen.
        // Reject it at the writer instead.
        if payload.len() > MAX_RECORD_BYTES as usize {
            return Err(bad("record payload exceeds MAX_RECORD_BYTES"));
        }
        let cell = p.cell();
        let mut hdr = encode_header(kind, cell, payload.len() as u32);
        hdr[12..16].copy_from_slice(&crc32(payload).to_le_bytes());
        let Some(reg) = self.region(p.region(), true)? else {
            return Err(bad("region file was not created"));
        };
        reg.file.seek(SeekFrom::Start(reg.len))?;
        reg.file.write_all(&hdr)?;
        reg.file.write_all(payload)?;
        let off = reg.len + HDR as u64;
        reg.len += (HDR + payload.len()) as u64;
        let e = reg.index.entry(cell).or_default();
        let loc = Some((off, payload.len() as u32));
        if kind == KIND_DAG {
            e.dag = loc;
            e.diff = None;
        } else {
            e.diff = loc;
        }
        reg.dirty = true;
        Ok(())
    }

    pub fn put_dag(&mut self, p: ChunkPos, dag: &Svdag) -> io::Result<()> {
        self.append(p, KIND_DAG, &dag.to_bytes())
    }

    /// Snapshot of the whole overlay (replaces any earlier snapshot for this chunk).
    pub fn put_diff(&mut self, p: ChunkPos, ov: &Overlay) -> io::Result<()> {
        self.append(p, KIND_DIFF, &ov.to_bytes())
    }

    pub fn has_dag(&mut self, p: ChunkPos) -> io::Result<bool> {
        let cell = p.cell();
        Ok(self
            .region(p.region(), false)?
            .is_some_and(|r| r.index.get(&cell).is_some_and(|e| e.dag.is_some())))
    }

    pub fn load(&mut self, p: ChunkPos) -> io::Result<Option<(Svdag, Overlay)>> {
        let cell = p.cell();
        let Some(reg) = self.region(p.region(), false)? else {
            return Ok(None);
        };
        let Some(e) = reg.index.get(&cell).copied() else {
            return Ok(None);
        };
        let Some((off, len)) = e.dag else {
            return Ok(None);
        };
        let dag =
            Svdag::from_bytes(&reg.read(off, len)?).ok_or_else(|| bad("corrupt dag record"))?;
        // `from_bytes` checks framing only; parent bases and band tails are
        // validated here so a CRC-valid but semantically invalid record cannot
        // panic later in the chunk accessors.
        dag.check()
            .map_err(|error| bad(&format!("invalid dag record: {error}")))?;
        let ov = match e.diff {
            Some((o, l)) => {
                Overlay::from_bytes(&reg.read(o, l)?).ok_or_else(|| bad("corrupt diff record"))?
            }
            None => Overlay::default(),
        };
        Ok(Some((dag, ov)))
    }

    /// Flushes appended records and any new directory entry.
    ///
    /// Only regions that were appended to since the last sync are fsynced: the
    /// world ticks consolidation every frame, and syncing every open region
    /// would turn a one-region edit into one fsync per file. A failed sync
    /// leaves the dirty bits set so the next attempt retries.
    pub fn sync(&mut self) -> io::Result<()> {
        for region in self.regions.values_mut() {
            if region.dirty {
                region.file.sync_data()?;
                region.dirty = false;
            }
        }
        // A region file created since the last sync must also have its
        // directory entry persisted, or it can vanish after power loss.
        if self.dir_dirty {
            self.dir_file.sync_all()?;
            self.dir_dirty = false;
            #[cfg(test)]
            {
                self.dir_syncs += 1;
            }
        }
        Ok(())
    }

    /// Rewrite a region keeping only live records (latest dag + latest diff per cell).
    pub fn compact(&mut self, rp: RegionPos) -> io::Result<()> {
        let Some(mut reg) = self.regions.remove(&rp) else {
            return Ok(());
        };
        let path = self.path(rp);
        let tmp = path.with_extension("tmp");
        let result = (|| -> io::Result<()> {
            let mut out = File::create(&tmp)?;
            let mut cells: Vec<u16> = reg.index.keys().copied().collect();
            cells.sort_unstable();
            for c in cells {
                let e = reg.index[&c];
                if let Some((o, l)) = e.dag {
                    out.write_all(&encode(KIND_DAG, c, &reg.read(o, l)?))?;
                }
                if let Some((o, l)) = e.diff {
                    out.write_all(&encode(KIND_DIFF, c, &reg.read(o, l)?))?;
                }
            }
            out.sync_all()?;
            drop(out);
            fs::rename(&tmp, &path)
        })();
        // Renaming changes the directory entry, so persist the directory too.
        let result = result.and_then(|()| {
            self.dir_file.sync_all()?;
            #[cfg(test)]
            {
                self.dir_syncs += 1;
            }
            // The directory fsync above also covers any other region created
            // since the last sync.
            self.dir_dirty = false;
            Ok(())
        });
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    pub fn open_regions(&self) -> Vec<RegionPos> {
        self.regions.keys().copied().collect()
    }

    /// Bytes that recovery could not use: corrupt runs skipped to reach a
    /// later valid record, plus any truncated tail.
    ///
    /// Recovery happens when a region is first opened, so this stays zero
    /// until a damaged region has been read. A non-zero value means the log
    /// lost records; the store itself is still consistent.
    #[must_use]
    pub fn recovered_bytes(&self) -> u64 {
        self.recovered_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves one header and fails if anything asks for the payload.
    struct HeaderOnly {
        header: [u8; HDR],
        served: bool,
    }

    impl Read for HeaderOnly {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            assert!(!self.served, "an over-cap payload must not be read");
            self.served = true;
            buf[..HDR].copy_from_slice(&self.header);
            Ok(HDR)
        }
    }

    #[test]
    fn read_record_rejects_an_over_cap_length_without_reading_it() {
        let header = encode_header(KIND_DAG, 0, MAX_RECORD_BYTES + 1);
        let total = HDR as u64 + MAX_RECORD_BYTES as u64 + 1;
        let mut reader = HeaderOnly {
            header,
            served: false,
        };
        assert!(read_record(&mut reader, 0, total).unwrap().is_none());
    }

    #[test]
    fn append_rejects_a_payload_over_the_record_cap() {
        let dir = std::env::temp_dir().join(format!(
            "voxel-disk-cap-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let mut store = RegionStore::open(&dir).expect("open store");
        let payload = vec![0u8; MAX_RECORD_BYTES as usize + 1];
        let error = store
            .append(ChunkPos::new(0, 0, 0), KIND_DAG, &payload)
            .expect_err("over-cap payload must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            !dir.join("r.0.0.0.vxr").exists(),
            "a rejected append must not create a region file"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_flushes_a_new_region_directory_entry() {
        let dir = std::env::temp_dir().join(format!(
            "voxel-disk-sync-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let pos = ChunkPos::new(0, 0, 0);
        {
            let mut store = RegionStore::open(&dir).expect("open store");
            store
                .put_diff(pos, &Overlay::default())
                .expect("write diff");
            store.sync().expect("sync");
            // The reopen below only proves the page cache has the file; this
            // proves the durability path ran. Before the fix the test passed
            // with no directory fsync at all.
            assert_eq!(store.dir_syncs, 1, "a new region must fsync the directory");
            store.sync().expect("second sync");
            assert_eq!(
                store.dir_syncs, 1,
                "an unchanged directory must not fsync again"
            );
        }
        assert!(dir.join("r.0.0.0.vxr").exists());
        let mut reopened = RegionStore::open(&dir).expect("reopen store");
        assert!(
            reopened.region((0, 0, 0), false).expect("scan").is_some(),
            "the synced region file must be discoverable after reopen"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Appending to one region must not fsync the others on every tick.
    #[test]
    fn sync_only_flushes_regions_that_changed() {
        let dir = std::env::temp_dir().join(format!(
            "voxel-disk-region-dirty-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let mut store = RegionStore::open(&dir).expect("open store");
        store
            .put_diff(ChunkPos::new(0, 0, 0), &Overlay::default())
            .expect("write first region");
        store
            .put_diff(ChunkPos::new(8, 0, 0), &Overlay::default())
            .expect("write second region");
        store.sync().expect("sync");
        let regions: Vec<_> = store.regions.keys().copied().collect();
        assert_eq!(regions.len(), 2, "chunks 0 and 8 are in different regions");

        store
            .put_diff(ChunkPos::new(0, 0, 0), &Overlay::default())
            .expect("append to the first region");
        let dirty: Vec<_> = store
            .regions
            .iter()
            .filter(|(_, region)| region.dirty)
            .map(|(pos, _)| *pos)
            .collect();
        assert_eq!(dirty, vec![(0, 0, 0)], "only the appended region is dirty");
        store.sync().expect("sync");
        assert!(store.regions.values().all(|region| !region.dirty));
        let _ = fs::remove_dir_all(&dir);
    }
}
