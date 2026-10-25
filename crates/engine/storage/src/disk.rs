//! Tier 1: region files. One append-only log per 8x8x8-chunk region.
//!
//! Record: magic u32 | kind u8 | pad u8 | cell u16 | len u32 | crc32(payload) u32 | payload
//!   kind 1 = serialized Svdag (supersedes everything before it for that cell, incl. diffs)
//!   kind 2 = full Overlay snapshot (last one wins)
//! Open scans each log, rebuilds the index, truncates any torn/corrupt tail.
use crate::dag::Svdag;
use crate::hash::{crc32, FxMap};
use crate::overlay::Overlay;
use crate::ChunkPos;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const MAGIC: u32 = 0x5856_5231; // "1RVX"
const KIND_DAG: u8 = 1;
const KIND_DIFF: u8 = 2;
const HDR: usize = 16;

#[derive(Default, Clone, Copy)]
struct Entry {
    dag: Option<(u64, u32)>,  // (payload offset, len)
    diff: Option<(u64, u32)>,
}

struct Region {
    file: File,
    len: u64,
    index: FxMap<u16, Entry>,
}

impl Region {
    fn read(&mut self, off: u64, len: u32) -> io::Result<Vec<u8>> {
        let mut b = vec![0u8; len as usize];
        self.file.seek(SeekFrom::Start(off))?;
        self.file.read_exact(&mut b)?;
        Ok(b)
    }
}

type RegionPos = (i32, i32, i32);

pub struct RegionStore {
    dir: PathBuf,
    regions: FxMap<RegionPos, Region>,
    dirty: bool,
}

fn bad(msg: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, msg.to_string()) }

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

fn scan(file: &mut File) -> io::Result<(u64, FxMap<u16, Entry>)> {
    let total = file.metadata()?.len();
    let mut index: FxMap<u16, Entry> = FxMap::default();
    let mut pos = 0u64;
    {
        let mut r = BufReader::new(&*file);
        loop {
            let mut h = [0u8; HDR];
            if r.read_exact(&mut h).is_err() {
                break;
            }
            if u32::from_le_bytes(h[0..4].try_into().unwrap()) != MAGIC {
                break;
            }
            let kind = h[4];
            let cell = u16::from_le_bytes(h[6..8].try_into().unwrap());
            let len = u32::from_le_bytes(h[8..12].try_into().unwrap());
            let crc = u32::from_le_bytes(h[12..16].try_into().unwrap());
            if pos + HDR as u64 + len as u64 > total {
                break;
            }
            let mut p = vec![0u8; len as usize];
            if r.read_exact(&mut p).is_err() || crc32(&p) != crc {
                break;
            }
            let e = index.entry(cell).or_default();
            match kind {
                KIND_DAG => {
                    e.dag = Some((pos + HDR as u64, len));
                    e.diff = None;
                }
                KIND_DIFF => e.diff = Some((pos + HDR as u64, len)),
                _ => break,
            }
            pos += HDR as u64 + len as u64;
        }
    }
    if pos < total {
        file.set_len(pos)?;
    }
    Ok((pos, index))
}

impl RegionStore {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, regions: FxMap::default(), dirty: false })
    }

    fn path(&self, rp: RegionPos) -> PathBuf {
        self.dir.join(format!("r.{}.{}.{}.vxr", rp.0, rp.1, rp.2))
    }

    fn region(&mut self, rp: RegionPos, create: bool) -> io::Result<Option<&mut Region>> {
        if !self.regions.contains_key(&rp) {
            let path = self.path(rp);
            if !create && !path.exists() {
                return Ok(None);
            }
            let mut file = OpenOptions::new().read(true).write(true).create(true).open(&path)?;
            let (len, index) = scan(&mut file)?;
            self.regions.insert(rp, Region { file, len, index });
        }
        Ok(self.regions.get_mut(&rp))
    }

    fn append(&mut self, p: ChunkPos, kind: u8, payload: &[u8]) -> io::Result<()> {
        let cell = p.cell();
        let mut hdr = encode_header(kind, cell, payload.len() as u32);
        hdr[12..16].copy_from_slice(&crc32(payload).to_le_bytes());
        let reg = self.region(p.region(), true)?.unwrap();
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
        self.dirty = true;
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
        Ok(self.region(p.region(), false)?.map_or(false, |r| r.index.get(&cell).map_or(false, |e| e.dag.is_some())))
    }

    pub fn load(&mut self, p: ChunkPos) -> io::Result<Option<(Svdag, Overlay)>> {
        let cell = p.cell();
        let Some(reg) = self.region(p.region(), false)? else { return Ok(None) };
        let Some(e) = reg.index.get(&cell).copied() else { return Ok(None) };
        let Some((off, len)) = e.dag else { return Ok(None) };
        let dag = Svdag::from_bytes(&reg.read(off, len)?).ok_or_else(|| bad("corrupt dag record"))?;
        let ov = match e.diff {
            Some((o, l)) => Overlay::from_bytes(&reg.read(o, l)?).ok_or_else(|| bad("corrupt diff record"))?,
            None => Overlay::default(),
        };
        Ok(Some((dag, ov)))
    }

    pub fn sync(&mut self) -> io::Result<()> {
        if self.dirty {
            for r in self.regions.values() {
                r.file.sync_data()?;
            }
            self.dirty = false;
        }
        Ok(())
    }

    /// Rewrite a region keeping only live records (latest dag + latest diff per cell).
    pub fn compact(&mut self, rp: RegionPos) -> io::Result<()> {
        let Some(mut reg) = self.regions.remove(&rp) else { return Ok(()) };
        let path = self.path(rp);
        let tmp = path.with_extension("tmp");
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
        drop(reg);
        fs::rename(&tmp, &path)
    }

    pub fn open_regions(&self) -> Vec<RegionPos> { self.regions.keys().copied().collect() }
}
