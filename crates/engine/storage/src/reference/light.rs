//! Block-light flood-fill reference (M1-29).
//!
//! This is the slow oracle for M2's full and incremental light passes. It
//! recomputes a chunk from scratch over the block-cell grid, including a
//! one-block apron on every face, and writes the two channels described by
//! ADR-004:
//!
//! - **sky** — seeded at cells exposed to the sky;
//! - **block/emissive** — seeded at cells whose material emits light.
//!
//! A cell is 16 B: two 8 B channels, each 16-bit fixed-point RGB plus an
//! intensity. The reference propagates the scalar intensity and fills RGB with
//! the same neutral value; the material registry carries no source colour, so
//! a coloured light reference would have nothing to seed from. Production
//! light is free to tint the same intensity fields.
//!
//! # Grid
//!
//! Light lives on **block cells**, not voxels. At the 64³ edge a chunk is
//! `4³ = 64` cells; at the pre-registered 128³ edge it is `8³ = 512` cells.
//! A fill for one chunk reads cells `-1 ..= edge` on each axis, so a 64³ chunk
//! evaluates a `6³` padded grid. Only the `edge³` cell results are chunk
//! content; the apron values are exposed for seam tests.
//!
//! # Propagation rule
//!
//! Entering a neighbour costs `1 + opacity` levels, where `opacity` comes from
//! the material and is in `0..=15`. Air therefore loses one level per step and
//! an opacity-15 block blocks the fill entirely. Leaving an emissive block is
//! not attenuated by the emitter's own opacity: the emission is the source
//! cell's value, and the first step charges the receiver.
//!
//! The fill is a level-ordered bucket traversal. Seeds are inserted in
//! `z, y, x` order (x fastest), levels are drained from 15 down to 1, and the
//! six neighbours are visited in the fixed [`NEIGHBOURS`] order. No hash map or
//! thread scheduling participates, so the same input produces byte-identical
//! cells on every run.

use std::array;
use std::mem::size_of;

use voxel_block::HotRecord;
use voxel_core::content_constants::{LIGHT_APRON_BLOCKS, LIGHT_BYTES_PER_CELL};

/// Maximum light level in either channel.
pub const MAX_LEVEL: u8 = 15;

/// Number of representable levels, including darkness (`0`).
pub const LEVELS: usize = MAX_LEVEL as usize + 1;

/// Fractional bits below one light level in a [`Channel`].
pub const FRACTION_BITS: u32 = 12;

/// Fixed-point value of one whole light level.
pub const LEVEL_STEP: u16 = 1 << FRACTION_BITS;

/// Fixed-point value of [`MAX_LEVEL`].
pub const MAX_INTENSITY: u16 = MAX_LEVEL as u16 * LEVEL_STEP;

/// Width of the apron in block cells.
pub const APRON_CELLS: i32 = LIGHT_APRON_BLOCKS as i32;

/// One 8 B light channel: RGB plus intensity, all 16-bit fixed point.
///
/// The reference fill has no source colour, so [`Self::from_level`] writes the
/// same neutral value into RGB and intensity. Keeping the full channel shape
/// makes the reference exercise the real 16 B cell rather than a scalar
/// stand-in.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    /// Neutral 16-bit fixed-point RGB.
    pub rgb: [u16; 3],
    /// 16-bit fixed-point intensity, `0..=`[`MAX_INTENSITY`].
    pub intensity: u16,
}

impl Channel {
    /// An unlit channel.
    pub const DARK: Self = Self {
        rgb: [0; 3],
        intensity: 0,
    };

    /// Builds a channel from raw fixed-point intensity.
    ///
    /// Values above [`MAX_INTENSITY`] are clamped; RGB is set to the same
    /// neutral value.
    pub const fn from_intensity(intensity: u16) -> Self {
        let intensity = if intensity > MAX_INTENSITY {
            MAX_INTENSITY
        } else {
            intensity
        };
        Self {
            rgb: [intensity; 3],
            intensity,
        }
    }

    /// Builds a channel at a whole light level, clamped to [`MAX_LEVEL`].
    pub const fn from_level(level: u8) -> Self {
        let level = if level > MAX_LEVEL { MAX_LEVEL } else { level };
        Self::from_intensity(level as u16 * LEVEL_STEP)
    }

    /// The whole level represented by this channel, `0..=15`.
    pub const fn level(self) -> u8 {
        (self.intensity / LEVEL_STEP) as u8
    }

    /// True when the channel is dark.
    pub const fn is_dark(self) -> bool {
        self.intensity == 0
    }
}

/// One 16 B light cell: an independent sky channel and block channel.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightCell {
    /// Skylight, including the vertical open-sky seed.
    pub sky: Channel,
    /// Light from emissive blocks.
    pub block: Channel,
}

impl LightCell {
    /// A cell that is dark in both channels.
    pub const DARK: Self = Self {
        sky: Channel::DARK,
        block: Channel::DARK,
    };

    /// Sky level, `0..=15`.
    pub const fn sky_level(self) -> u8 {
        self.sky.level()
    }

    /// Block-light level, `0..=15`.
    pub const fn block_level(self) -> u8 {
        self.block.level()
    }

    /// True when both channels are dark.
    pub const fn is_dark(self) -> bool {
        self.sky.is_dark() && self.block.is_dark()
    }
}

/// The light-relevant properties of one block cell.
///
/// This is a projection of [`HotRecord`], not a second registry: callers read
/// the material once per padded cell and pass the result to [`full_pass`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockLight {
    /// Light emitted by this cell, `0..=15`.
    pub emission: u8,
    /// Extra absorption charged when a fill enters this cell, `0..=15`.
    ///
    /// Air is `0`; a fully opaque block is `15`. Every step also has a baseline
    /// cost of one level, so opacity `15` admits no light at all.
    pub opacity: u8,
}

impl BlockLight {
    /// Clear air: no emission, no absorption.
    pub const AIR: Self = Self {
        emission: 0,
        opacity: 0,
    };

    /// A fully opaque, non-emissive block.
    pub const OPAQUE: Self = Self {
        emission: 0,
        opacity: MAX_LEVEL,
    };

    /// Creates a block sample, clamping both fields to `0..=15`.
    pub const fn new(emission: u8, opacity: u8) -> Self {
        Self {
            emission: if emission > MAX_LEVEL {
                MAX_LEVEL
            } else {
                emission
            },
            opacity: if opacity > MAX_LEVEL {
                MAX_LEVEL
            } else {
                opacity
            },
        }
    }

    /// A transparent block that emits `level` light.
    pub const fn emitter(level: u8) -> Self {
        Self::new(level, 0)
    }

    /// Reads the emission and opacity from a registry hot record.
    pub const fn from_hot(record: HotRecord) -> Self {
        Self {
            emission: record.light_emission(),
            opacity: record.light_opacity(),
        }
    }

    /// True when light cannot enter this cell.
    pub const fn is_fully_opaque(self) -> bool {
        self.opacity >= MAX_LEVEL
    }
}

impl From<HotRecord> for BlockLight {
    fn from(record: HotRecord) -> Self {
        Self::from_hot(record)
    }
}

/// The result of one full-pass light fill for a chunk.
///
/// Cells are stored with the one-block apron included. [`Self::local`] reads
/// chunk content; [`Self::with_apron`] also reads the seam values that a
/// neighbour's fill would seed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightFill {
    edge: usize,
    padded_edge: usize,
    cells: Vec<LightCell>,
}

impl LightFill {
    /// Cells per chunk edge (`4` at 64³, `8` at 128³).
    pub const fn edge_cells(&self) -> usize {
        self.edge
    }

    /// Cells per padded edge, including both apron cells.
    pub const fn padded_edge_cells(&self) -> usize {
        self.padded_edge
    }

    /// Reads a chunk-local cell.
    ///
    /// # Panics
    ///
    /// Panics if any coordinate is outside `0 .. edge`.
    pub fn local(&self, x: usize, y: usize, z: usize) -> LightCell {
        assert!(
            x < self.edge && y < self.edge && z < self.edge,
            "cell ({x}, {y}, {z}) is outside the {0}³ chunk",
            self.edge
        );
        self.cells[self.index(x as i32, y as i32, z as i32)]
    }

    /// Reads a cell including the apron.
    ///
    /// # Panics
    ///
    /// Panics if any coordinate is outside `-1 ..= edge`.
    pub fn with_apron(&self, x: i32, y: i32, z: i32) -> LightCell {
        let last = self.edge as i32;
        assert!(
            (-APRON_CELLS..=last).contains(&x)
                && (-APRON_CELLS..=last).contains(&y)
                && (-APRON_CELLS..=last).contains(&z),
            "cell ({x}, {y}, {z}) is outside the padded {0}³ grid",
            self.padded_edge
        );
        self.cells[self.index(x, y, z)]
    }

    /// All cells including the apron, in `z, y, x` order with x fastest.
    pub fn padded_cells(&self) -> &[LightCell] {
        &self.cells
    }

    /// Iterates chunk-local cells in `z, y, x` order with x fastest.
    pub fn iter_local(&self) -> impl Iterator<Item = ((usize, usize, usize), LightCell)> + '_ {
        let edge = self.edge;
        (0..edge).flat_map(move |z| {
            (0..edge).flat_map(move |y| (0..edge).map(move |x| ((x, y, z), self.local(x, y, z))))
        })
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        index_at(x, y, z, self.padded_edge)
    }
}

/// Fixed neighbour order for the deterministic bucket traversal.
///
/// Axes in `x`, `y`, `z` order, positive direction before negative on each axis.
pub const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Runs a full-pass light fill over one chunk plus a one-block apron.
///
/// `block_at(x, y, z)` returns the material light properties for a block cell
/// in chunk-local cell coordinates; `sky_at(x, y, z)` says whether that cell is
/// exposed to the sky and should seed skylight 15. Both closures are consulted
/// over the padded range `-1 ..= edge` on every axis, in `z, y, x` order with x
/// fastest; `sky_at` is only called for cells that are not fully opaque. An
/// opaque cell still blocks propagation.
///
/// The returned fill contains the chunk cells and the apron cells. The result
/// is deterministic: fixed seed order, fixed neighbour order, fixed level
/// buckets.
///
/// # Panics
///
/// Panics if `edge_cells` is zero or so large that the padded volume overflows
/// `usize`. Production edges are `4` and `8`.
pub fn full_pass<B, S>(edge_cells: usize, mut block_at: B, mut sky_at: S) -> LightFill
where
    B: FnMut(i32, i32, i32) -> BlockLight,
    S: FnMut(i32, i32, i32) -> bool,
{
    assert!(edge_cells > 0, "a light fill needs at least one cell");
    let padded_edge = edge_cells
        .checked_add(2 * usize::try_from(APRON_CELLS).expect("the apron is positive"))
        .expect("padded light edge overflows usize");
    let volume = padded_edge
        .checked_pow(3)
        .expect("padded light volume overflows usize");

    let lower = -APRON_CELLS;
    let upper = edge_cells as i32 + APRON_CELLS - 1;

    let mut blocks = Vec::with_capacity(volume);
    let mut sky = vec![0u8; volume];
    let mut block = vec![0u8; volume];

    for z in lower..=upper {
        for y in lower..=upper {
            for x in lower..=upper {
                let sample = block_at(x, y, z);
                let index = index_at(x, y, z, padded_edge);
                if sample.emission > 0 {
                    block[index] = sample.emission.min(MAX_LEVEL);
                }
                if !sample.is_fully_opaque() && sky_at(x, y, z) {
                    sky[index] = MAX_LEVEL;
                }
                blocks.push(sample);
            }
        }
    }

    propagate(&blocks, &mut sky, padded_edge);
    propagate(&blocks, &mut block, padded_edge);

    let cells = sky
        .into_iter()
        .zip(block)
        .map(|(sky, block)| LightCell {
            sky: Channel::from_level(sky),
            block: Channel::from_level(block),
        })
        .collect();

    LightFill {
        edge: edge_cells,
        padded_edge,
        cells,
    }
}

/// Level-ordered flood fill over one channel.
fn propagate(blocks: &[BlockLight], levels: &mut [u8], padded_edge: usize) {
    let mut buckets: [Vec<usize>; LEVELS] = array::from_fn(|_| Vec::new());

    // Seed order is the same row-major order the samples were read in. A cell
    // can only appear in one bucket, and buckets are drained from high to low,
    // so the first assignment of a lower level is final.
    for (index, level) in levels.iter().copied().enumerate() {
        if level > 0 {
            buckets[level as usize].push(index);
        }
    }

    let lower = -APRON_CELLS;
    let upper = padded_edge as i32 - APRON_CELLS - 1;

    for level in (1..=MAX_LEVEL).rev() {
        let mut cursor = 0;
        while cursor < buckets[level as usize].len() {
            let index = buckets[level as usize][cursor];
            cursor += 1;

            // The cell may have been raised after it was queued; it is then in
            // a higher bucket and this entry is stale.
            if levels[index] != level {
                continue;
            }

            let (x, y, z) = coords_at(index, padded_edge);
            for (dx, dy, dz) in NEIGHBOURS {
                let nx = x + dx;
                let ny = y + dy;
                let nz = z + dz;
                if nx < lower || nx > upper || ny < lower || ny > upper || nz < lower || nz > upper
                {
                    continue;
                }

                let neighbour = index_at(nx, ny, nz, padded_edge);
                let next = level.saturating_sub(1 + blocks[neighbour].opacity);
                if next == 0 {
                    continue;
                }
                if next > levels[neighbour] {
                    levels[neighbour] = next;
                    buckets[next as usize].push(neighbour);
                }
            }
        }
    }
}

/// Row-major padded index: x fastest, then y, then z.
fn index_at(x: i32, y: i32, z: i32, padded_edge: usize) -> usize {
    let px = (x + APRON_CELLS) as usize;
    let py = (y + APRON_CELLS) as usize;
    let pz = (z + APRON_CELLS) as usize;
    px + py * padded_edge + pz * padded_edge * padded_edge
}

/// Inverse of [`index_at`].
fn coords_at(index: usize, padded_edge: usize) -> (i32, i32, i32) {
    let x = (index % padded_edge) as i32 - APRON_CELLS;
    let y = ((index / padded_edge) % padded_edge) as i32 - APRON_CELLS;
    let z = (index / (padded_edge * padded_edge)) as i32 - APRON_CELLS;
    (x, y, z)
}

// The cell layout is a contract with the GPU light buffer and ADR-004.
const _: () = {
    assert!(APRON_CELLS == 1);
    assert!(size_of::<Channel>() == 8);
    assert!(size_of::<LightCell>() == LIGHT_BYTES_PER_CELL);
    assert!(LIGHT_BYTES_PER_CELL == 16);
};
