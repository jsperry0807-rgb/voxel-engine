//! Production full-pass block light (M2-13).
//!
//! Light lives on the block-cell grid: 4³ cells at the 64³ chunk edge and 8³ at
//! 128³. A fill reads one cell of apron on every face and produces two
//! independent channels:
//!
//! - **sky**, seeded at cells exposed to the sky;
//! - **block**, seeded at emissive cells.
//!
//! Entering a neighbour costs one level plus the neighbour's opacity. The
//! traversal is level-ordered with a fixed seed and neighbour order, so the
//! result is byte-identical across runs. This is the production port of the
//! M1-29 reference; `tests/light.rs` compares it against that oracle.

use std::array;

/// Maximum light level in either channel.
pub const MAX_LEVEL: u8 = 15;

/// Number of representable levels, including darkness.
pub const LEVELS: usize = MAX_LEVEL as usize + 1;

/// Fractional bits below one light level.
pub const FRACTION_BITS: u32 = 12;

/// Fixed-point value of one whole light level.
pub const LEVEL_STEP: u16 = 1 << FRACTION_BITS;

/// Fixed-point value of [`MAX_LEVEL`].
pub const MAX_INTENSITY: u16 = MAX_LEVEL as u16 * LEVEL_STEP;

/// Width of the apron in block cells.
pub const APRON_CELLS: i32 = 1;

/// One 8 B channel: RGB plus intensity, all 16-bit fixed point.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub rgb: [u16; 3],
    pub intensity: u16,
}

impl Channel {
    /// An unlit channel.
    pub const DARK: Self = Self {
        rgb: [0; 3],
        intensity: 0,
    };

    /// Builds a channel from raw fixed-point intensity.
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

    /// Builds a channel at a whole level, clamped to [`MAX_LEVEL`].
    pub const fn from_level(level: u8) -> Self {
        Self::from_intensity(if level > MAX_LEVEL {
            MAX_LEVEL as u16 * LEVEL_STEP
        } else {
            level as u16 * LEVEL_STEP
        })
    }

    /// The whole level represented by this channel.
    pub const fn level(self) -> u8 {
        (self.intensity / LEVEL_STEP) as u8
    }

    /// True when the channel is dark.
    pub const fn is_dark(self) -> bool {
        self.intensity == 0
    }
}

/// One 16 B light cell: independent sky and block channels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightCell {
    pub sky: Channel,
    pub block: Channel,
}

impl LightCell {
    /// A cell dark in both channels.
    pub const DARK: Self = Self {
        sky: Channel::DARK,
        block: Channel::DARK,
    };

    pub const fn sky_level(self) -> u8 {
        self.sky.level()
    }

    pub const fn block_level(self) -> u8 {
        self.block.level()
    }

    pub const fn is_dark(self) -> bool {
        self.sky.is_dark() && self.block.is_dark()
    }
}

/// Light-relevant material properties for one cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockLight {
    /// Emission level, `0..=15`.
    pub emission: u8,
    /// Extra absorption when light enters, `0..=15`.
    pub opacity: u8,
}

impl BlockLight {
    pub const AIR: Self = Self {
        emission: 0,
        opacity: 0,
    };
    pub const OPAQUE: Self = Self {
        emission: 0,
        opacity: MAX_LEVEL,
    };

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

    pub const fn emitter(level: u8) -> Self {
        Self::new(level, 0)
    }

    pub const fn is_fully_opaque(self) -> bool {
        self.opacity >= MAX_LEVEL
    }
}

/// A full-pass light result, including the one-cell apron.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightFill {
    edge: usize,
    padded_edge: usize,
    cells: Vec<LightCell>,
}

impl LightFill {
    pub const fn edge_cells(&self) -> usize {
        self.edge
    }

    pub const fn padded_edge_cells(&self) -> usize {
        self.padded_edge
    }

    /// Reads a chunk-local cell.
    pub fn local(&self, x: usize, y: usize, z: usize) -> LightCell {
        assert!(x < self.edge && y < self.edge && z < self.edge);
        self.cells[self.index(x as i32, y as i32, z as i32)]
    }

    /// Reads a cell including the apron.
    pub fn with_apron(&self, x: i32, y: i32, z: i32) -> LightCell {
        let last = self.edge as i32;
        assert!(
            (-APRON_CELLS..=last).contains(&x)
                && (-APRON_CELLS..=last).contains(&y)
                && (-APRON_CELLS..=last).contains(&z)
        );
        self.cells[self.index(x, y, z)]
    }

    /// All cells including the apron, `z, y, x` order with x fastest.
    pub fn padded_cells(&self) -> &[LightCell] {
        &self.cells
    }

    /// Iterates chunk-local cells in `z, y, x` order.
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

/// Fixed neighbour order for deterministic propagation.
pub const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// A source of block-light properties in cell coordinates.
///
/// [`full_pass_at`] shifts the fill's padded reads by a chunk origin, so the
/// caller can supply a world view that crosses chunk borders.
pub trait LightSource {
    /// Material light properties at a world cell.
    fn block(&self, x: i32, y: i32, z: i32) -> BlockLight;
    /// Whether the cell is exposed to the sky.
    fn sky(&self, x: i32, y: i32, z: i32) -> bool;
}

/// Runs a full pass for the chunk whose cell origin is `origin`.
///
/// This is the cross-chunk entry point: the one-cell apron reads through
/// `source`, so neighbour border cells seed this chunk's fill and vice versa.
pub fn full_pass_at<S: LightSource>(edge_cells: usize, origin: [i32; 3], source: &S) -> LightFill {
    full_pass_at_seeded(edge_cells, origin, source, |_, _, _| LightCell::DARK)
}

/// Like [`full_pass_at`], with a world-coordinate seed lookup.
pub fn full_pass_at_seeded<S, L>(
    edge_cells: usize,
    origin: [i32; 3],
    source: &S,
    mut seed_at: L,
) -> LightFill
where
    S: LightSource,
    L: FnMut(i32, i32, i32) -> LightCell,
{
    full_pass_seeded(
        edge_cells,
        |x, y, z| source.block(origin[0] + x, origin[1] + y, origin[2] + z),
        |x, y, z| source.sky(origin[0] + x, origin[1] + y, origin[2] + z),
        |x, y, z| seed_at(origin[0] + x, origin[1] + y, origin[2] + z),
    )
}

/// One block or sky edit in chunk-local cell coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellEdit {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub block: BlockLight,
    pub sky: bool,
}

impl CellEdit {
    pub const fn new(x: i32, y: i32, z: i32, block: BlockLight, sky: bool) -> Self {
        Self {
            x,
            y,
            z,
            block,
            sky,
        }
    }
}

/// Persistent light state for one chunk, updated by dirty-region edits.
///
/// A light change cannot travel more than [`MAX_LEVEL`] cells because every
/// step costs at least one level. Each edit batch therefore recomputes only the
/// edited cells expanded by that radius, using the unchanged cells outside the
/// region as fixed seeds. At the 4³/8³ chunk-cell edges the region often covers
/// the whole chunk; the API is still chunk-local and allocation-free after
/// construction.
pub struct LightState {
    edge: usize,
    padded_edge: usize,
    blocks: Vec<BlockLight>,
    sky_seeds: Vec<bool>,
    sky: Vec<u8>,
    block: Vec<u8>,
}

impl LightState {
    /// Builds a full-pass state from chunk-local cell closures.
    pub fn new<B, S>(edge_cells: usize, mut block_at: B, mut sky_at: S) -> Self
    where
        B: FnMut(i32, i32, i32) -> BlockLight,
        S: FnMut(i32, i32, i32) -> bool,
    {
        assert!(edge_cells > 0);
        let padded_edge = edge_cells + 2 * APRON_CELLS as usize;
        let volume = padded_edge * padded_edge * padded_edge;
        let lower = -APRON_CELLS;
        let upper = edge_cells as i32 + APRON_CELLS - 1;

        let mut blocks = Vec::with_capacity(volume);
        let mut sky_seeds = Vec::with_capacity(volume);
        for z in lower..=upper {
            for y in lower..=upper {
                for x in lower..=upper {
                    blocks.push(block_at(x, y, z));
                    sky_seeds.push(sky_at(x, y, z));
                }
            }
        }

        let mut sky = vec![0u8; volume];
        let mut block = vec![0u8; volume];
        seed_channels(&blocks, &sky_seeds, &mut sky, &mut block);
        propagate(&blocks, &mut sky, padded_edge);
        propagate(&blocks, &mut block, padded_edge);

        Self {
            edge: edge_cells,
            padded_edge,
            blocks,
            sky_seeds,
            sky,
            block,
        }
    }

    /// Applies a batch of edits and recomputes only the dirty region.
    pub fn apply_edits(&mut self, edits: &[CellEdit]) {
        if edits.is_empty() {
            return;
        }
        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        for edit in edits {
            let index = index_at(edit.x, edit.y, edit.z, self.padded_edge);
            self.blocks[index] = edit.block;
            self.sky_seeds[index] = edit.sky;
            let cell = [edit.x, edit.y, edit.z];
            for axis in 0..3 {
                min[axis] = min[axis].min(cell[axis]);
                max[axis] = max[axis].max(cell[axis]);
            }
        }

        let last = self.edge as i32;
        let radius = i32::from(MAX_LEVEL);
        for axis in 0..3 {
            min[axis] = (min[axis] - radius).max(-APRON_CELLS);
            max[axis] = (max[axis] + radius).min(last);
        }
        self.recompute_region(min, max);
    }

    /// Applies one edit.
    pub fn apply_edit(&mut self, edit: CellEdit) {
        self.apply_edits(std::slice::from_ref(&edit));
    }

    /// Reads a chunk-local cell.
    pub fn local(&self, x: usize, y: usize, z: usize) -> LightCell {
        assert!(x < self.edge && y < self.edge && z < self.edge);
        self.cell(x as i32, y as i32, z as i32)
    }

    /// Reads a cell including the apron.
    pub fn with_apron(&self, x: i32, y: i32, z: i32) -> LightCell {
        self.cell(x, y, z)
    }

    fn cell(&self, x: i32, y: i32, z: i32) -> LightCell {
        let index = index_at(x, y, z, self.padded_edge);
        LightCell {
            sky: Channel::from_level(self.sky[index]),
            block: Channel::from_level(self.block[index]),
        }
    }

    fn recompute_region(&mut self, min: [i32; 3], max: [i32; 3]) {
        for z in min[2]..=max[2] {
            for y in min[1]..=max[1] {
                for x in min[0]..=max[0] {
                    let index = index_at(x, y, z, self.padded_edge);
                    let block = self.blocks[index];
                    self.sky[index] = if !block.is_fully_opaque() && self.sky_seeds[index] {
                        MAX_LEVEL
                    } else {
                        0
                    };
                    self.block[index] = block.emission;
                }
            }
        }
        propagate_region(&self.blocks, &mut self.sky, self.padded_edge, min, max);
        propagate_region(&self.blocks, &mut self.block, self.padded_edge, min, max);
    }
}

fn seed_channels(blocks: &[BlockLight], sky_seeds: &[bool], sky: &mut [u8], block: &mut [u8]) {
    for (index, sample) in blocks.iter().copied().enumerate() {
        block[index] = sample.emission;
        sky[index] = if !sample.is_fully_opaque() && sky_seeds[index] {
            MAX_LEVEL
        } else {
            0
        };
    }
}

fn propagate_region(
    blocks: &[BlockLight],
    levels: &mut [u8],
    padded_edge: usize,
    min: [i32; 3],
    max: [i32; 3],
) {
    let mut buckets: [Vec<usize>; LEVELS] = array::from_fn(|_| Vec::new());
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
                if nx < min[0]
                    || nx > max[0]
                    || ny < min[1]
                    || ny > max[1]
                    || nz < min[2]
                    || nz > max[2]
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

/// Runs a full light pass for one chunk plus a one-cell apron.
///
/// `block_at` and `sky_at` are read over `-1 ..= edge` in `z, y, x` order with
/// x fastest. `sky_at` is only consulted for non-opaque cells.
pub fn full_pass<B, S>(edge_cells: usize, block_at: B, sky_at: S) -> LightFill
where
    B: FnMut(i32, i32, i32) -> BlockLight,
    S: FnMut(i32, i32, i32) -> bool,
{
    full_pass_seeded(edge_cells, block_at, sky_at, |_, _, _| LightCell::DARK)
}

/// Runs a full pass with light already present in the apron.
///
/// `seed_at` is consulted for every padded cell; the fill starts each channel
/// at the maximum of the local seed and the imported value. The cross-chunk
/// caller seeds the one-cell apron from the neighbour's border light, then the
/// normal propagation fills the interior. Interior seeds are allowed but the
/// streaming path uses only the apron.
pub fn full_pass_seeded<B, S, L>(
    edge_cells: usize,
    mut block_at: B,
    mut sky_at: S,
    mut seed_at: L,
) -> LightFill
where
    B: FnMut(i32, i32, i32) -> BlockLight,
    S: FnMut(i32, i32, i32) -> bool,
    L: FnMut(i32, i32, i32) -> LightCell,
{
    assert!(edge_cells > 0);
    let padded_edge = edge_cells + 2 * APRON_CELLS as usize;
    let volume = padded_edge * padded_edge * padded_edge;
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
                let seed = seed_at(x, y, z);
                sky[index] = sky[index].max(seed.sky_level());
                block[index] = block[index].max(seed.block_level());
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

fn propagate(blocks: &[BlockLight], levels: &mut [u8], padded_edge: usize) {
    let mut buckets: [Vec<usize>; LEVELS] = array::from_fn(|_| Vec::new());
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

fn index_at(x: i32, y: i32, z: i32, padded_edge: usize) -> usize {
    let px = (x + APRON_CELLS) as usize;
    let py = (y + APRON_CELLS) as usize;
    let pz = (z + APRON_CELLS) as usize;
    px + py * padded_edge + pz * padded_edge * padded_edge
}

fn coords_at(index: usize, padded_edge: usize) -> (i32, i32, i32) {
    let x = (index % padded_edge) as i32 - APRON_CELLS;
    let y = ((index / padded_edge) % padded_edge) as i32 - APRON_CELLS;
    let z = (index / (padded_edge * padded_edge)) as i32 - APRON_CELLS;
    (x, y, z)
}

const _: () = {
    assert!(std::mem::size_of::<Channel>() == 8);
    assert!(std::mem::size_of::<LightCell>() == 16);
};
