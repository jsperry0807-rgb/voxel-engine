//! Mutable O(1) edit overlay: local voxel index -> material (0 = carved to air).
use crate::CHUNK_VOLUME;
use crate::hash::FxMap;

#[derive(Default, Clone, Debug)]
pub struct Overlay {
    map: FxMap<u32, u16>,
}

impl Overlay {
    #[inline]
    pub fn get(&self, idx: u32) -> Option<u16> {
        self.map.get(&idx).copied()
    }
    #[inline]
    pub fn set(&mut self, idx: u32, v: u16) {
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
        let mut v = Vec::with_capacity(4 + self.map.len() * 6);
        v.extend_from_slice(&(self.map.len() as u32).to_le_bytes());
        for (i, m) in self.iter() {
            v.extend_from_slice(&i.to_le_bytes());
            v.extend_from_slice(&m.to_le_bytes());
        }
        v
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 4 {
            return None;
        }
        let n = u32::from_le_bytes(b[0..4].try_into().ok()?) as usize;
        if n.checked_mul(6).and_then(|bytes| bytes.checked_add(4)) != Some(b.len()) {
            return None;
        }
        let mut o = Overlay::default();
        o.map.reserve(n);
        for e in b[4..].as_chunks::<6>().0 {
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
