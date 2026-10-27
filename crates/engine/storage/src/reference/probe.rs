//! Probe-bake reference (M1-30).
//!
//! This is the slow oracle for M2's production SH4 bake. It places one probe at
//! every block centre on the near grid, casts a fixed hemisphere of rays into
//! the chunk's SVDAG and projects the binary sky visibility into four real SH
//! coefficients per RGB channel. The record is exactly 12 B, matching ADR-005
//! and [`PROBE_BYTES`].
//!
//! # Grid
//!
//! A chunk has `CHUNK_DIM / BLOCK_DIM` blocks per edge. The near grid therefore
//! holds one probe per block: **64 probes at 64³** and **512 probes at 128³**.
//! The mid grid uses every second block on each axis (one eighth of the probes)
//! and the far grid holds none, as fixed by ADR-005. Probe centres are the
//! centres of their block cells in chunk-local voxel coordinates.
//!
//! This reference module is independent of the compiled chunk edge: the grid
//! takes its own `edge_blocks` count, so a 512-probe bake can be exercised from
//! either build. [`bake_svdag`] is the convenience path that checks the grid
//! against the compiled edge.
//!
//! # Rays
//!
//! Each valid probe casts [`PROBE_RAYS`] (64) rays over the upper hemisphere.
//! The directions are a deterministic Fibonacci-area sequence: ray `i` has
//! `y = 1 - (i + 0.5) / 64`, so the 64 directions cover the hemisphere with a
//! uniform distribution in `y`. A per-probe azimuth derived from the probe seed
//! rotates the sequence without changing the latitude distribution.
//!
//! A ray that leaves the scene contributes unit neutral radiance ("sky"). A ray
//! that enters a solid voxel contributes zero radiance: this reference isolates
//! visibility, direction and encoding, not the production bounce term. The
//! material payload is not used, so the three RGB channels carry the same
//! value, just as the M1-29 light reference seeds a neutral value.
//!
//! # SH4 encoding
//!
//! The four real SH coefficients are, in record order,
//! `[Y₀₀, Y₁₋₁, Y₁₀, Y₁₁]` with
//! `Y₀₀ = 0.282095`, `Y₁₋₁ = 0.488603·y`, `Y₁₀ = 0.488603·z` and
//! `Y₁₁ = 0.488603·x`. For uniform-hemisphere samples the estimator is
//! `2π / N · Σ L·Y`, so open sky has a DC coefficient of `2π·Y₀₀ ≈ 1.772`.
//!
//! Each coefficient is stored in RGB8. The signed coefficient range
//! `[-`[`SH_RANGE`]`, +`[`SH_RANGE`]`]` is mapped to `1..=255` around a signed
//! zero at byte `128`; [`Sh4Probe::coefficients`] reverses the mapping. Invalid
//! probes carry the zero-radiance record; their payload is ignored by the
//! sampler, but keeping it neutral makes a raw byte dump easy to read.
//!
//! # Determinism
//!
//! A probe seed is a pure hash of the chunk position, probe index and probe LOD
//! ([`probe_seed`]). It is never drawn from time, a thread-local RNG or work
//! order. Validity is decided in the same pass from the occupancy of the probe
//! centre. The bake is serial and allocation-free inside the ray loop, so the
//! same input produces the same bytes on every run and on every thread.

use std::f64::consts::{PI, TAU};

use voxel_core::constants::BLOCK_DIM;
use voxel_core::content_constants::PROBE_BYTES;

use crate::dag::Svdag;
use crate::{AIR, CHUNK_DIM, ChunkPos};

/// Number of real SH coefficients in one probe: `l = 0` and `l = 1`.
pub const SH_COEFFICIENTS: usize = 4;

/// Number of colour channels in one coefficient.
pub const PROBE_CHANNELS: usize = 3;

/// Rays cast per valid probe.
pub const PROBE_RAYS: usize = 64;

/// Coefficients per probe in bytes, before the per-channel split.
pub const COEFFICIENT_BYTES: usize = SH_COEFFICIENTS * PROBE_CHANNELS;

/// Real SH basis value for `l = 0`.
pub const SH_L0: f64 = 0.282_094_791_773_878_14;

/// Real SH basis value for the three `l = 1` coefficients.
pub const SH_L1: f64 = 0.488_602_511_902_919_9;

/// Signed coefficient range represented by one RGB8 channel.
///
/// `2.0` covers the open-sky DC term (`≈ 1.772`) with headroom for directional
/// terms. The quantisation step is `2 · SH_RANGE / 255 ≈ 0.0157`.
pub const SH_RANGE: f64 = 2.0;

/// Byte value that encodes a zero coefficient.
pub const SH8_ZERO: u8 = 128;

/// Steps from zero to [`SH_RANGE`] in the RGB8 encoding.
const SH8_STEPS: f64 = 127.5;

/// Golden angle used by the Fibonacci hemisphere sequence.
const GOLDEN_ANGLE: f64 = PI * (3.0 - 2.236_067_977_499_79);

/// Probe density ring fixed by ADR-005.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProbeLod {
    /// One probe per block (1 m spacing).
    Near,
    /// One probe per 2×2×2 blocks (2 m spacing).
    Mid,
    /// No probe volume.
    Far,
}

impl ProbeLod {
    /// All probe densities, ordered from the camera outward.
    pub const ALL: [ProbeLod; 3] = [ProbeLod::Near, ProbeLod::Mid, ProbeLod::Far];

    /// Probe spacing in blocks; `0` means no probes.
    pub const fn spacing_blocks(self) -> usize {
        match self {
            ProbeLod::Near => 1,
            ProbeLod::Mid => 2,
            ProbeLod::Far => 0,
        }
    }

    /// Stable tag mixed into the probe seed.
    pub const fn seed_tag(self) -> u64 {
        match self {
            ProbeLod::Near => 0,
            ProbeLod::Mid => 1,
            ProbeLod::Far => 2,
        }
    }
}

/// Probe grid description: scene edge in blocks plus a density ring.
///
/// `edge_blocks` is the block count per chunk edge, not the voxel count. The
/// scene is `edge_blocks · BLOCK_DIM` voxels on each axis. The grid is
/// edge-independent and its probe count follows directly from the block count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProbeGrid {
    edge_blocks: usize,
    lod: ProbeLod,
}

impl ProbeGrid {
    /// Builds a grid at an explicit block edge and probe LOD.
    ///
    /// # Panics
    ///
    /// Panics if `edge_blocks` is zero, or if a mid grid's edge is not an even
    /// number of blocks.
    pub fn new(edge_blocks: usize, lod: ProbeLod) -> Self {
        assert!(edge_blocks > 0, "a probe grid needs at least one block");
        if lod == ProbeLod::Mid {
            assert!(
                edge_blocks.is_multiple_of(2),
                "a mid probe grid needs an even block edge"
            );
        }
        Self { edge_blocks, lod }
    }

    /// Near grid: one probe per block.
    pub fn near(edge_blocks: usize) -> Self {
        Self::new(edge_blocks, ProbeLod::Near)
    }

    /// Mid grid: one probe per 2×2×2 blocks.
    pub fn mid(edge_blocks: usize) -> Self {
        Self::new(edge_blocks, ProbeLod::Mid)
    }

    /// Far grid: no probes.
    pub fn far(edge_blocks: usize) -> Self {
        Self::new(edge_blocks, ProbeLod::Far)
    }

    /// The grid for the compiled chunk edge at `lod`.
    ///
    /// This is 4 blocks per edge (64 probes near) at 64³ and 8 blocks per edge
    /// (512 probes near) at 128³.
    pub fn for_chunk(lod: ProbeLod) -> Self {
        Self::new(CHUNK_DIM / BLOCK_DIM, lod)
    }

    /// Blocks per edge in the scene.
    pub const fn edge_blocks(self) -> usize {
        self.edge_blocks
    }

    /// Density ring.
    pub const fn lod(self) -> ProbeLod {
        self.lod
    }

    /// Probe spacing in blocks.
    pub const fn spacing_blocks(self) -> usize {
        self.lod.spacing_blocks()
    }

    /// Probes per grid edge.
    pub const fn probes_per_edge(self) -> usize {
        match self.lod {
            ProbeLod::Far => 0,
            _ => self.edge_blocks / self.spacing_blocks(),
        }
    }

    /// Total probes in the grid (`0` for far).
    pub const fn len(self) -> usize {
        let edge = self.probes_per_edge();
        edge * edge * edge
    }

    /// True when the grid has no probes.
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Scene edge in voxels.
    pub const fn scene_edge_voxels(self) -> usize {
        self.edge_blocks * BLOCK_DIM
    }

    /// Block coordinates of a probe, packed `x` fastest, then `y`, then `z`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is outside `0 .. len`.
    pub fn block_coords(self, index: usize) -> (usize, usize, usize) {
        let edge = self.probes_per_edge();
        assert!(
            index < self.len(),
            "probe index {index} is outside the grid"
        );
        let spacing = self.spacing_blocks();
        let x = index % edge;
        let y = (index / edge) % edge;
        let z = index / (edge * edge);
        (x * spacing, y * spacing, z * spacing)
    }

    /// Centre of a probe's block in chunk-local voxel coordinates.
    ///
    /// # Panics
    ///
    /// Panics if `index` is outside `0 .. len`.
    pub fn centre(self, index: usize) -> [f64; 3] {
        let (x, y, z) = self.block_coords(index);
        let half = (BLOCK_DIM / 2) as f64;
        [
            x as f64 * BLOCK_DIM as f64 + half,
            y as f64 * BLOCK_DIM as f64 + half,
            z as f64 * BLOCK_DIM as f64 + half,
        ]
    }
}

/// One SH4 RGB8 probe: four coefficients times three 8-bit channels.
///
/// The record is exactly [`PROBE_BYTES`] (12 B) and is `#[repr(C)]` so it can be
/// copied directly into the probe buffer. The fields are encoded coefficients,
/// not linear radiance; use [`Self::coefficients`] to decode them.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sh4Probe {
    rgb8: [[u8; PROBE_CHANNELS]; SH_COEFFICIENTS],
}

impl Sh4Probe {
    /// The neutral record: every coefficient decodes to zero radiance.
    pub const ZERO: Self = Self {
        rgb8: [[SH8_ZERO; PROBE_CHANNELS]; SH_COEFFICIENTS],
    };

    /// Wraps already-encoded RGB8 coefficients.
    pub const fn from_rgb8(rgb8: [[u8; PROBE_CHANNELS]; SH_COEFFICIENTS]) -> Self {
        Self { rgb8 }
    }

    /// The encoded RGB8 coefficients, in `[l0, l1m1, l10, l11]` order.
    pub const fn rgb8(self) -> [[u8; PROBE_CHANNELS]; SH_COEFFICIENTS] {
        self.rgb8
    }

    /// Encodes linear SH coefficients, one RGB triple per basis function.
    pub fn from_coefficients(coefficients: [[f64; PROBE_CHANNELS]; SH_COEFFICIENTS]) -> Self {
        let mut rgb8 = [[SH8_ZERO; PROBE_CHANNELS]; SH_COEFFICIENTS];
        for (basis, encoded) in coefficients.into_iter().zip(rgb8.iter_mut()) {
            for (channel, byte) in basis.into_iter().zip(encoded.iter_mut()) {
                *byte = encode_coefficient(channel);
            }
        }
        Self { rgb8 }
    }

    /// Decodes the RGB8 coefficients back to linear values.
    pub fn coefficients(self) -> [[f64; PROBE_CHANNELS]; SH_COEFFICIENTS] {
        let mut coefficients = [[0.0; PROBE_CHANNELS]; SH_COEFFICIENTS];
        for (basis, encoded) in self.rgb8.into_iter().zip(coefficients.iter_mut()) {
            for (channel, value) in basis.into_iter().zip(encoded.iter_mut()) {
                *value = decode_coefficient(channel);
            }
        }
        coefficients
    }

    /// The 12 bytes stored in the probe volume, in coefficient-major order.
    pub const fn to_bytes(self) -> [u8; PROBE_BYTES] {
        let mut bytes = [0u8; PROBE_BYTES];
        let mut coefficient = 0;
        while coefficient < SH_COEFFICIENTS {
            let mut channel = 0;
            while channel < PROBE_CHANNELS {
                bytes[coefficient * PROBE_CHANNELS + channel] = self.rgb8[coefficient][channel];
                channel += 1;
            }
            coefficient += 1;
        }
        bytes
    }
}

impl Default for Sh4Probe {
    fn default() -> Self {
        Self::ZERO
    }
}

/// The baked probe volume for one chunk.
///
/// The SH payload and the packed validity mask are kept separate, matching the
/// ADR-005 storage split (12 B per probe plus one bit per probe). Invalid probes
/// remain in the payload as [`Sh4Probe::ZERO`]; samplers consult the mask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeVolume {
    grid: ProbeGrid,
    probes: Vec<Sh4Probe>,
    validity: Vec<u8>,
}

impl ProbeVolume {
    /// The grid that produced this volume.
    pub const fn grid(&self) -> ProbeGrid {
        self.grid
    }

    /// Number of probes in the SH payload.
    pub fn len(&self) -> usize {
        self.probes.len()
    }

    /// True when the volume holds no probes (a far-LOD bake).
    pub fn is_empty(&self) -> bool {
        self.probes.is_empty()
    }

    /// One SH record.
    ///
    /// # Panics
    ///
    /// Panics if `index` is outside `0 .. len`.
    pub fn probe(&self, index: usize) -> Sh4Probe {
        self.probes[index]
    }

    /// All SH records in grid order.
    pub fn probes(&self) -> &[Sh4Probe] {
        &self.probes
    }

    /// True when the probe centre is in an air cell.
    ///
    /// # Panics
    ///
    /// Panics if `index` is outside `0 .. len`.
    pub fn is_valid(&self, index: usize) -> bool {
        assert!(
            index < self.probes.len(),
            "probe index {index} is out of range"
        );
        self.validity[index / 8] & (1 << (index % 8)) != 0
    }

    /// The packed validity mask, one bit per probe, LSB first.
    pub fn validity_bytes(&self) -> &[u8] {
        &self.validity
    }

    /// The SH payload bytes, exactly `PROBE_BYTES · len`.
    pub fn sh_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.probes.len() * PROBE_BYTES);
        for probe in &self.probes {
            bytes.extend_from_slice(&probe.to_bytes());
        }
        bytes
    }
}

/// Derives the deterministic seed for one probe.
///
/// The seed is a pure function of the chunk position, the probe LOD and the
/// probe's linear index. It is mixed with SplitMix64 so adjacent probes get
/// independent azimuths without any RNG state. Chunk coordinates are signed;
/// their two's-complement bits are part of the hash, so negative chunks hash
/// differently from their positive counterparts.
pub fn probe_seed(chunk: ChunkPos, lod: ProbeLod, index: u32) -> u64 {
    let mut hash = 0x5052_4f42_455f_0001; // "PROB" plus a version tag
    hash ^= mix64(chunk.x as i64 as u64);
    hash = hash.rotate_left(17) ^ mix64(chunk.y as i64 as u64);
    hash = hash.rotate_left(17) ^ mix64(chunk.z as i64 as u64);
    hash = hash.rotate_left(17) ^ mix64(u64::from(index));
    hash = hash.rotate_left(17) ^ mix64(lod.seed_tag());
    mix64(hash)
}

/// Returns ray `index` of the 64-ray hemisphere, rotated by `rotation` radians.
///
/// Directions are unit length and always have `y > 0`. The same sequence is
/// used for every probe; only the azimuth changes with the seed.
///
/// # Panics
///
/// Panics if `index >= PROBE_RAYS`.
pub fn hemisphere_ray(index: usize, rotation: f64) -> [f64; 3] {
    assert!(
        index < PROBE_RAYS,
        "ray index {index} is outside the hemisphere"
    );
    let latitude = 1.0 - (index as f64 + 0.5) / PROBE_RAYS as f64;
    let radius = (1.0 - latitude * latitude).sqrt();
    let azimuth = index as f64 * GOLDEN_ANGLE + rotation;
    [radius * azimuth.cos(), latitude, radius * azimuth.sin()]
}

/// Bakes one probe volume from a solidity predicate.
///
/// `solid_at(x, y, z)` is called in chunk-local voxel coordinates over a scene
/// of `grid.scene_edge_voxels()` voxels per axis. A probe is valid when the
/// voxel containing its centre is air; an invalid probe gets the neutral SH
/// record and no rays. A ray leaving the scene is sky (unit radiance); a ray
/// entering a solid voxel is zero radiance.
///
/// The output is byte-for-byte deterministic for a given scene, grid, chunk
/// position and predicate.
pub fn bake<F>(chunk_pos: ChunkPos, grid: ProbeGrid, mut solid_at: F) -> ProbeVolume
where
    F: FnMut(u32, u32, u32) -> bool,
{
    let count = grid.len();
    let mut probes = Vec::with_capacity(count);
    let mut validity = vec![0u8; count.div_ceil(8)];

    if count == 0 {
        return ProbeVolume {
            grid,
            probes,
            validity,
        };
    }

    let scene_edge = grid.scene_edge_voxels();
    let rotation_scale = TAU / (1u64 << 53) as f64;

    for index in 0..count {
        let centre = grid.centre(index);
        let centre_voxel = [
            centre[0].floor() as u32,
            centre[1].floor() as u32,
            centre[2].floor() as u32,
        ];
        let valid = !solid_at(centre_voxel[0], centre_voxel[1], centre_voxel[2]);
        if valid {
            validity[index / 8] |= 1 << (index % 8);
        }

        if !valid {
            probes.push(Sh4Probe::ZERO);
            continue;
        }

        let seed = probe_seed(chunk_pos, grid.lod(), index as u32);
        let rotation = ((seed >> 11) as f64) * rotation_scale;
        let mut accumulated = [[0.0f64; PROBE_CHANNELS]; SH_COEFFICIENTS];

        for ray in 0..PROBE_RAYS {
            let direction = hemisphere_ray(ray, rotation);
            if trace(&mut solid_at, scene_edge, centre, direction) {
                continue;
            }

            let basis = sh_basis(direction);
            for (coefficient, value) in accumulated.iter_mut().zip(basis) {
                coefficient[0] += value;
                coefficient[1] += value;
                coefficient[2] += value;
            }
        }

        let scale = TAU / PROBE_RAYS as f64;
        let coefficients = accumulated.map(|basis| basis.map(|value| value * scale));
        probes.push(Sh4Probe::from_coefficients(coefficients));
    }

    ProbeVolume {
        grid,
        probes,
        validity,
    }
}

/// Bakes the compiled chunk edge from an [`Svdag`].
///
/// The grid must match the compiled chunk; [`ProbeGrid::for_chunk`] is the
/// normal source. For an explicit edge (for example the 512-probe 128³ case in
/// a 64³ build), use [`bake`] with a closure.
///
/// # Panics
///
/// Panics if `grid.scene_edge_voxels()` is not the compiled [`CHUNK_DIM`].
pub fn bake_svdag(chunk: &Svdag, chunk_pos: ChunkPos, grid: ProbeGrid) -> ProbeVolume {
    assert_eq!(
        grid.scene_edge_voxels(),
        CHUNK_DIM,
        "the probe grid must match the compiled chunk edge"
    );
    bake(chunk_pos, grid, |x, y, z| chunk.get(x, y, z) != AIR)
}

fn encode_coefficient(value: f64) -> u8 {
    let quantized = (value * SH_RANGE.recip() * SH8_STEPS).round();
    (quantized.clamp(-SH8_STEPS, SH8_STEPS) as i32 + SH8_ZERO as i32) as u8
}

fn decode_coefficient(value: u8) -> f64 {
    (i32::from(value) - SH8_ZERO as i32) as f64 * SH_RANGE / SH8_STEPS
}

fn sh_basis([x, y, z]: [f64; 3]) -> [f64; SH_COEFFICIENTS] {
    [SH_L0, SH_L1 * y, SH_L1 * z, SH_L1 * x]
}

/// Traverses one ray through the scene with an Amanatides–Woo DDA.
///
/// Returns `true` on entering a solid voxel. A ray that steps outside the
/// scene returns `false`: the reference treats the chunk boundary as open sky.
fn trace<F>(solid_at: &mut F, scene_edge: usize, origin: [f64; 3], direction: [f64; 3]) -> bool
where
    F: FnMut(u32, u32, u32) -> bool,
{
    let edge = scene_edge as i32;
    let mut cell = [
        origin[0].floor() as i32,
        origin[1].floor() as i32,
        origin[2].floor() as i32,
    ];
    let mut step = [0i32; 3];
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];

    for axis in 0..3 {
        let component = direction[axis];
        if component > 0.0 {
            step[axis] = 1;
            t_max[axis] = (cell[axis] as f64 + 1.0 - origin[axis]) / component;
            t_delta[axis] = 1.0 / component;
        } else if component < 0.0 {
            step[axis] = -1;
            t_max[axis] = (cell[axis] as f64 - origin[axis]) / component;
            t_delta[axis] = -1.0 / component;
        }
    }

    loop {
        // Fixed axis tie-break: x, then y, then z. This keeps a ray that
        // crosses an edge exactly on a diagonal deterministic.
        let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];

        if cell
            .iter()
            .any(|&coordinate| coordinate < 0 || coordinate >= edge)
        {
            return false;
        }

        if solid_at(cell[0] as u32, cell[1] as u32, cell[2] as u32) {
            return true;
        }
    }
}

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

// The record and the grid are contracts with the content constants and
// ADR-005. Every probe is 12 B and the near grid has one probe per block.
const _: () = {
    assert!(PROBE_BYTES == COEFFICIENT_BYTES);
    assert!(PROBE_BYTES == 12);
    assert!(SH_COEFFICIENTS == 4);
    assert!(PROBE_CHANNELS == 3);
    assert!(PROBE_RAYS == 64);
    assert!(std::mem::size_of::<Sh4Probe>() == PROBE_BYTES);
    assert!(std::mem::align_of::<Sh4Probe>() == 1);
};
