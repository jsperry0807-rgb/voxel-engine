//! Per-entity location records.

/// Where an entity currently lives: its table (archetype) and row.
///
/// Sparse membership is deliberately **not** recorded here; it is derived
/// solely from each sparse set's own sparse array (spec §3.3).
#[derive(Clone, Copy, Debug)]
pub(crate) struct EntityLocation {
    pub generation: u32,
    pub archetype: u32,
    pub row: u32,
}

impl EntityLocation {
    pub(crate) const INVALID: Self = Self {
        generation: 0,
        archetype: u32::MAX,
        row: u32::MAX,
    };
}
