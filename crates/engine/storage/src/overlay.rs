//! Mutable O(1) edit overlay: local voxel index -> material (0 = carved to air).
//!
//! The snapshot payload is versioned and tagged with the chunk edge: a 64³
//! snapshot validated in a 128³ build addresses different voxels, and without
//! the tag nothing could tell them apart.
use crate::hash::FxMap;
use crate::{CHUNK_BITS, CHUNK_VOLUME};

/// Magic at the start of a versioned overlay payload (`OVLY`).
const OVERLAY_MAGIC: u32 = 0x4F56_4C59;
/// Current overlay payload version.
const OVERLAY_VERSION: u8 = 1;
/// Bytes before the entries: magic, version, chunk bits, reserved, count.
const OVERLAY_HEADER: usize = 12;

#[derive(Default, Clone, Debug)]
pub struct Overlay {
    map: FxMap<u32, u16>,
}

impl Overlay {
    #[inline]
    pub fn get(&self, idx: u32) -> Option<u16> {
        self.map.get(&idx).copied()
    }
    /// Sets one voxel.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is not inside the chunk; a larger index would write a
    /// record that can never be read back.
    #[inline]
    pub fn set(&mut self, idx: u32, v: u16) {
        assert!(
            (idx as usize) < CHUNK_VOLUME,
            "overlay index {idx} is outside the chunk"
        );
        self.map.insert(idx, v);
    }
    #[inline]
    pub fn remove(&mut self, idx: u32) {
        self.map.remove(&idx);
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn clear(&mut self) {
        self.map.clear();
    }
    pub fn iter(&self) -> impl Iterator<Item = (u32, u16)> + '_ {
        self.map.iter().map(|(&k, &v)| (k, v))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(OVERLAY_HEADER + self.map.len() * 6);
        v.extend_from_slice(&OVERLAY_MAGIC.to_le_bytes());
        v.push(OVERLAY_VERSION);
        v.push(CHUNK_BITS as u8);
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&(self.map.len() as u32).to_le_bytes());
        for (i, m) in self.iter() {
            v.extend_from_slice(&i.to_le_bytes());
            v.extend_from_slice(&m.to_le_bytes());
        }
        v
    }

    /// Parses a snapshot.
    ///
    /// Both the versioned format and the pre-version prefix-less payload are
    /// accepted, so saves written before the tag keep loading. A versioned
    /// payload from a different chunk edge is rejected rather than read as
    /// different voxels.
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let (start, n) = if b.len() >= OVERLAY_HEADER
            && u32::from_le_bytes(b[0..4].try_into().ok()?) == OVERLAY_MAGIC
        {
            if b[4] != OVERLAY_VERSION || b[5] != CHUNK_BITS as u8 {
                return None;
            }
            (
                OVERLAY_HEADER,
                u32::from_le_bytes(b[8..12].try_into().ok()?) as usize,
            )
        } else {
            if b.len() < 4 {
                return None;
            }
            (4, u32::from_le_bytes(b[0..4].try_into().ok()?) as usize)
        };
        if n.checked_mul(6).and_then(|bytes| bytes.checked_add(start)) != Some(b.len()) {
            return None;
        }
        let mut o = Overlay::default();
        o.map.reserve(n);
        for e in b[start..].as_chunks::<6>().0 {
            let i = u32::from_le_bytes(e[0..4].try_into().unwrap());
            let m = u16::from_le_bytes(e[4..6].try_into().unwrap());
            if i as usize >= CHUNK_VOLUME {
                return None;
            }
            o.map.insert(i, m);
        }
        Some(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_snapshot_round_trips_and_keeps_the_edge_tag() {
        let mut overlay = Overlay::default();
        overlay.set(1, 7);
        overlay.set(5, 9);
        let bytes = overlay.to_bytes();
        let parsed = Overlay::from_bytes(&bytes).expect("parse the snapshot");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.get(1), Some(7));
        assert_eq!(parsed.get(5), Some(9));

        // A snapshot from a different chunk edge must be refused, not read as
        // different voxels.
        let mut wrong_edge = bytes.clone();
        wrong_edge[5] = wrong_edge[5].wrapping_add(1);
        assert!(Overlay::from_bytes(&wrong_edge).is_none());
    }

    #[test]
    fn legacy_snapshots_still_load() {
        // Pre-version payload: count = 1, index = 2, material = 3.
        let legacy = [1, 0, 0, 0, 2, 0, 0, 0, 3, 0];
        let overlay = Overlay::from_bytes(&legacy).expect("legacy payload");
        assert_eq!(overlay.get(2), Some(3));
    }

    #[test]
    #[should_panic(expected = "outside the chunk")]
    fn set_rejects_an_index_outside_the_chunk() {
        Overlay::default().set(CHUNK_VOLUME as u32, 1);
    }
}
