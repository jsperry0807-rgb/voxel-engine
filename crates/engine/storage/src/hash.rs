use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct Fx(u64);

impl Fx {
    #[inline]
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for Fx {
    fn write(&mut self, b: &[u8]) {
        let (chunks, remainder) = b.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        for &x in remainder {
            self.add(x as u64);
        }
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i)
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64)
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxBuild = BuildHasherDefault<Fx>;
pub type FxMap<K, V> = HashMap<K, V, FxBuild>;
pub type FxSet<K> = HashSet<K, FxBuild>;

/// 64-bit content hash for DAG node words.
#[inline]
pub fn hash_words(words: &[u32]) -> u64 {
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    for &w in words {
        h = (h ^ w as u64).wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 32;
    }
    h
}

/// CRC-32/ISO-HDLC for the region log's record framing.
///
/// The implementation lives in `voxel-core::checksum` so the worldgen macro
/// tile uses exactly the same polynomial; this re-export keeps the storage
/// call sites and tests unchanged.
pub use voxel_core::checksum::crc32;
