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

/// Verification budget for one region open.
///
/// Corrupt payloads can contain many magic-like headers, each declaring a
/// near-[`MAX_RECORD_BYTES`] payload. Charging reads that fail verification
/// against this budget bounds the total recovery work; a successful record
/// refunds its cost, so a large but valid region is not truncated.
const MAX_RECOVERY_BYTES: u64 = 64 << 20;

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
    if h[5] != PAYLOAD_TAG_V1 {
        // A record written by a newer format. Skipping it (rather than
        // misdecoding the payload) is what the tag is for.
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
    /// Access clock, for the open-file cap.
    last_used: u64,
}

/// Region handles kept open at once.
///
/// A world that streams over many regions would otherwise keep one descriptor
/// per region until the process hits `EMFILE`, at which point a save fails for
/// bookkeeping reasons. Closing a region is cheap: the next access re-scans it.
const MAX_OPEN_REGIONS: usize = 64;

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
    /// Monotonic access clock for the open-region cap.
    clock: u64,
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

/// Payload format tag in header byte 5.
///
/// `0` is the current v1 `Svdag` pool plus the (self-versioning) overlay. A
/// future format gets its own tag, so an older reader skips the record instead
/// of decoding bytes it does not understand.
const PAYLOAD_TAG_V1: u8 = 0;

fn encode_header(kind: u8, cell: u16, len: u32) -> [u8; HDR] {
    let mut h = [0u8; HDR];
    h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    h[4] = kind;
    h[5] = PAYLOAD_TAG_V1;
    h[6..8].copy_from_slice(&cell.to_le_bytes());
    h[8..12].copy_from_slice(&len.to_le_bytes());
    // CRC is filled by the caller, which has the payload.
    h
}

/// Reads and verifies one record, charging its bytes to `budget`.
///
/// Returns the record and the bytes actually read, so a caller that accepts a
/// record can refund the charge. A corrupt payload can contain many magic-like
/// headers that each declare a near-`MAX_RECORD_BYTES` payload; the budget
/// bounds the total CRC work, not just the number of candidates, so a damaged
/// save cannot stall recovery.
fn read_record_with_budget<R: Read>(
    r: &mut R,
    pos: u64,
    total: u64,
    budget: &mut u64,
) -> io::Result<(Option<RecordHeader>, u64)> {
    if *budget < HDR as u64 {
        *budget = 0;
        return Ok((None, 0));
    }
    *budget -= HDR as u64;
    let mut cost = HDR as u64;
    let mut h = [0u8; HDR];
    match r.read_exact(&mut h) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok((None, cost)),
        Err(error) => return Err(error),
    }
    let Some(record) = parse_header(&h) else {
        return Ok((None, cost));
    };
    if record.len > MAX_RECORD_BYTES || pos + HDR as u64 + record.len as u64 > total {
        return Ok((None, cost));
    }
    let payload_cost = u64::from(record.len);
    if payload_cost > *budget {
        *budget = 0;
        return Ok((None, cost));
    }
    *budget -= payload_cost;
    cost += payload_cost;
    let mut payload = vec![0u8; record.len as usize];
    match r.read_exact(&mut payload) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok((None, cost)),
        Err(error) => return Err(error),
    }
    Ok(((crc32(&payload) == record.crc).then_some(record), cost))
}

/// Searches `from..total` for the next CRC-valid record start.
///
/// Scans block by block for [`MAGIC`], verifying each candidate in full before
/// accepting it, so bytes that merely look like a header inside a payload do
/// not desynchronise the log.
/// Candidates `resync` verifies before it gives up on a corrupt run.
const MAX_RESYNC_FAILED: u32 = 256;

/// Outcome of searching a region for the next valid record.
enum Resync {
    /// A CRC-valid record starts at this offset.
    Found(u64),
    /// The scan reached the end of the file without finding another record.
    EndOfFile,
    /// The candidate or byte budget ran out; the rest of the file is unscanned.
    ///
    /// This is *not* a torn tail: the bytes after the damage may be intact and
    /// must not be truncated.
    Abandoned,
}

fn resync<R: Read + Seek>(
    r: &mut R,
    from: u64,
    total: u64,
    budget: &mut u64,
) -> io::Result<Resync> {
    const BLOCK: usize = 64 * 1024;
    // A corrupt payload can hold thousands of accidental magic values, and each
    // verification CRC-reads a record of up to MAX_RECORD_BYTES. Bound both the
    // number of misses and the shared byte budget, so a 17 MiB region cannot
    // cause gigabytes of CRC work.
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
            let (record, cost) = read_record_with_budget(r, candidate, total, budget)?;
            if record.is_some() {
                // The caller re-reads this record to decode it; refund the
                // verification so a large valid record after corruption does
                // not exhaust the recovery budget.
                *budget = budget.saturating_add(cost);
                return Ok(Resync::Found(candidate));
            }
            failed += 1;
            if failed == MAX_RESYNC_FAILED || *budget == 0 {
                return Ok(Resync::Abandoned);
            }
        }
        if filled < want {
            break;
        }
        // Overlap three bytes so a magic split across blocks is found.
        base += (filled - 3) as u64;
    }
    Ok(Resync::EndOfFile)
}

struct Scan {
    len: u64,
    index: FxMap<u16, Entry>,
    recovered: u64,
    /// The file had a corrupt tail that `scan` truncated.
    truncated: bool,
}

fn scan(file: &mut File) -> io::Result<Scan> {
    let total = file.metadata()?.len();
    let mut index: FxMap<u16, Entry> = FxMap::default();
    let mut pos = 0u64;
    let mut good_end = 0u64;
    let mut skipped = 0u64;
    let mut abandoned = false;
    {
        let mut r = BufReader::new(&*file);
        let mut budget = MAX_RECOVERY_BYTES;
        while pos < total {
            let (record, cost) = read_record_with_budget(&mut r, pos, total, &mut budget)?;
            let Some(record) = record else {
                // A corrupt record does not end the log: look for the next
                // valid one instead of truncating everything after it.
                match resync(&mut r, pos + 1, total, &mut budget)? {
                    Resync::Found(next) => {
                        skipped += next - pos;
                        pos = next;
                        // `resync` left the reader at the end of the record it
                        // validated; rewind so the next iteration decodes it.
                        r.seek(SeekFrom::Start(pos))?;
                        continue;
                    }
                    Resync::EndOfFile => break,
                    Resync::Abandoned => {
                        // Recovery stopped looking, not the file. Keeping the
                        // tail means a read can never destroy intact records;
                        // the unscanned bytes are still counted as recovered.
                        abandoned = true;
                        break;
                    }
                }
            };
            // This record verified, so scanning it is real work rather than
            // adversarial recovery; refund it and keep the budget for failures.
            budget = budget.saturating_add(cost);
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
    if !abandoned && good_end < total {
        // Only a genuine torn tail is safe to drop. Truncating when recovery
        // gave up would delete records that were never examined.
        file.set_len(good_end)?;
    }
    let recovered = skipped + (total - good_end);
    Ok(Scan {
        // Appends land after the unscanned tail rather than over it.
        len: if abandoned { total } else { good_end },
        index,
        recovered,
        truncated: !abandoned && good_end < total,
    })
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
            clock: 0,
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
            // Record the directory entry as soon as the file exists: if `scan`
            // fails, the empty file is still there and must be persisted.
            self.dir_dirty |= created;
            let scan = scan(&mut file)?;
            self.recovered_bytes += scan.recovered;
            self.regions.insert(
                rp,
                Region {
                    file,
                    len: scan.len,
                    index: scan.index,
                    // A truncated tail is a metadata write; sync it with the
                    // region's contents.
                    dirty: scan.truncated,
                    last_used: 0,
                },
            );
            // Keep the descriptor table bounded: drop the least-recently used
            // clean regions when it grows past the cap. Dirty ones stay (they
            // still need their fsync).
            while self.regions.len() > MAX_OPEN_REGIONS {
                let victim = self
                    .regions
                    .iter()
                    .filter(|(pos, region)| **pos != rp && !region.dirty)
                    .min_by_key(|(_, region)| region.last_used)
                    .map(|(pos, _)| *pos);
                let Some(victim) = victim else {
                    break;
                };
                self.regions.remove(&victim);
            }
        }
        self.clock += 1;
        let clock = self.clock;
        let region = self.regions.get_mut(&rp).expect("region exists");
        region.last_used = clock;
        Ok(Some(region))
    }

    /// Closes a clean region's handle; the next access re-scans it.
    pub fn close(&mut self, rp: RegionPos) {
        if self.regions.get(&rp).is_some_and(|region| !region.dirty) {
            self.regions.remove(&rp);
        }
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
        let rp = p.region();
        let mut lost = 0u64;
        let loaded = {
            let Some(reg) = self.region(rp, false)? else {
                return Ok(None);
            };
            let Some(e) = reg.index.get(&cell).copied() else {
                return Ok(None);
            };
            let Some((off, len)) = e.dag else {
                return Ok(None);
            };
            let bytes = reg.read(off, len)?;
            // `from_bytes` checks framing only; parent bases and band tails are
            // validated here. A CRC-valid but semantically invalid record is
            // dropped rather than returned as a permanent error: the caller can
            // regenerate the chunk, but it could never un-stick an `Err`.
            let dag = match Svdag::from_bytes(&bytes) {
                Some(dag) if dag.check().is_ok() => Some(dag),
                _ => {
                    reg.index.remove(&cell);
                    lost = len as u64;
                    None
                }
            };
            match dag {
                None => None,
                Some(dag) => {
                    let mut ov = Overlay::default();
                    if let Some((o, l)) = e.diff {
                        match Overlay::from_bytes(&reg.read(o, l)?) {
                            Some(parsed) => ov = parsed,
                            None => {
                                // The diff is an edit snapshot on top of the DAG;
                                // losing it costs the chunk's unsaved edits, not
                                // the chunk itself.
                                lost = l as u64;
                                if let Some(entry) = reg.index.get_mut(&cell) {
                                    entry.diff = None;
                                }
                            }
                        }
                    }
                    Some((dag, ov))
                }
            }
        };
        self.recovered_bytes += lost;
        Ok(loaded)
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
        let path = self.path(rp);
        let tmp = path.with_extension("tmp");
        let Some(reg) = self.regions.get_mut(&rp) else {
            return Ok(());
        };
        // The rename below changes a directory entry. Mark it before trying,
        // so a failed (or unsupported) directory fsync is retried rather than
        // leaving the new inode's link unpersisted.
        self.dir_dirty = true;
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
        // On failure the region stays in the map with its dirty bit, so unsynced
        // appends are not forgotten and a retry re-attempts the rewrite.
        let result = result.and_then(|()| {
            // The old handle points at the replaced inode; drop it before
            // reopening so a later append cannot write to the unlinked file.
            self.regions.remove(&rp);
            let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
            let scan = scan(&mut file)?;
            self.recovered_bytes += scan.recovered;
            self.clock += 1;
            let last_used = self.clock;
            self.regions.insert(
                rp,
                Region {
                    file,
                    len: scan.len,
                    index: scan.index,
                    dirty: false,
                    last_used,
                },
            );
            // Renaming changes the directory entry, so persist the directory too.
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
        let mut budget = u64::MAX;
        assert!(
            read_record_with_budget(&mut reader, 0, total, &mut budget)
                .unwrap()
                .0
                .is_none()
        );
    }

    #[test]
    fn budgeted_read_rejects_a_payload_it_cannot_afford() {
        let header = encode_header(KIND_DAG, 0, 32);
        let total = HDR as u64 + 32;
        let mut reader = HeaderOnly {
            header,
            served: false,
        };
        // One byte short of the header plus payload: the payload must not be
        // read, and the budget is reported as exhausted.
        let mut budget = HDR as u64 + 31;
        let (record, cost) = read_record_with_budget(&mut reader, 0, total, &mut budget).unwrap();
        assert!(record.is_none());
        assert_eq!(cost, HDR as u64, "only the header was read");
        assert_eq!(budget, 0, "an unaffordable payload exhausts the budget");
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

    /// A corrupt run that exhausts the recovery budget must not take the bytes
    /// after it with it. The old scan truncated at the last good record, so a
    /// read could delete intact chunks that simply had not been examined yet.
    #[test]
    fn abandoned_recovery_keeps_the_unscanned_tail() {
        let dir = std::env::temp_dir().join(format!(
            "voxel-disk-abandoned-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        {
            let mut store = RegionStore::open(&dir).expect("open store");
            store
                .put_diff(ChunkPos::new(0, 0, 0), &Overlay::default())
                .expect("write the first record");
            store.sync().expect("sync");
        }
        let path = dir.join("r.0.0.0.vxr");
        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open the region");
            // `MAX_RESYNC_FAILED` headers with valid framing and a bad CRC.
            for _ in 0..MAX_RESYNC_FAILED {
                let mut header = encode_header(KIND_DIFF, 1, 1);
                header[12..16].copy_from_slice(&0u32.to_le_bytes());
                file.write_all(&header).expect("write header");
                file.write_all(&[0]).expect("write payload");
            }
            // An intact record after the damage.
            file.write_all(&encode(KIND_DIFF, 1, &[0u8; 8]))
                .expect("write trailing record");
        }
        let before = fs::metadata(&path).expect("metadata").len();
        let mut store = RegionStore::open(&dir).expect("reopen store");
        // Regions are scanned lazily; touching one runs recovery.
        store
            .load(ChunkPos::new(0, 0, 0))
            .expect("load the first chunk");
        assert_eq!(
            fs::metadata(&path).expect("metadata").len(),
            before,
            "recovery must not truncate records it never examined"
        );
        assert!(store.recovered_bytes() > 0);

        // Appends land after the unscanned tail, not over it.
        store
            .put_diff(ChunkPos::new(1, 0, 0), &Overlay::default())
            .expect("append after the abandoned tail");
        assert!(fs::metadata(&path).expect("metadata").len() > before);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A CRC-valid diff whose payload cannot be decoded must not make the chunk
    /// unloadable: the DAG is still good, so only the edits are lost.
    #[test]
    fn a_bad_diff_falls_back_to_the_dag() {
        let dir = std::env::temp_dir().join(format!(
            "voxel-disk-bad-diff-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let pos = ChunkPos::new(0, 0, 0);
        let dag = Svdag::from_dense(&vec![1u16; crate::CHUNK_VOLUME]);
        {
            let mut store = RegionStore::open(&dir).expect("open store");
            store.put_dag(pos, &dag).expect("write dag");
            store
                .put_diff(pos, &Overlay::default())
                .expect("write diff");
            store.sync().expect("sync");
        }
        let path = dir.join("r.0.0.0.vxr");
        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open the region");
            // A plausible entry count with no entry bytes: framing and CRC are
            // valid, the overlay payload is not.
            file.write_all(&encode(KIND_DIFF, pos.cell(), &[1, 0, 0, 0]))
                .expect("append a bad diff");
        }

        let mut store = RegionStore::open(&dir).expect("reopen store");
        let (loaded, overlay) = store
            .load(pos)
            .expect("a bad diff is not fatal")
            .expect("the committed DAG still loads");
        let mut expected = vec![0u16; crate::CHUNK_VOLUME];
        dag.expand(&mut expected);
        let mut actual = vec![0u16; crate::CHUNK_VOLUME];
        loaded.expand(&mut actual);
        assert_eq!(actual, expected, "the DAG content must survive");
        assert!(overlay.is_empty(), "the undecodable diff is dropped");
        assert!(store.recovered_bytes() >= 4);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Streaming over many regions must not keep one descriptor per region.
    #[test]
    fn open_region_table_is_bounded() {
        let dir = std::env::temp_dir().join(format!(
            "voxel-disk-fd-cap-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let mut store = RegionStore::open(&dir).expect("open store");
        for i in 0..80i32 {
            store
                .put_diff(ChunkPos::new(i * 8, 0, 0), &Overlay::default())
                .expect("write a region");
        }
        store.sync().expect("sync");
        // One more open past the cap closes clean regions.
        store
            .put_diff(ChunkPos::new(640, 0, 0), &Overlay::default())
            .expect("open one more region");
        assert!(
            store.open_regions().len() <= MAX_OPEN_REGIONS,
            "{} region handles are open",
            store.open_regions().len()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_with_an_unknown_payload_tag_is_skipped() {
        let header = encode_header(KIND_DIFF, 1, 0);
        assert!(parse_header(&header).is_some());
        let mut newer = header;
        newer[5] = 9;
        assert!(
            parse_header(&newer).is_none(),
            "a future payload tag must not be decoded as v1"
        );
    }
}
