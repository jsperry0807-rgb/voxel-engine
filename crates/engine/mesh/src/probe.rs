//! Production SH4 probe bake (M2-18).
//!
//! One probe sits at every block centre on the near grid: 64 probes at 64³ and
//! 512 at 128³. Each valid probe casts [`PROBE_RAYS`] upper-hemisphere rays into
//! the chunk, projects the binary sky visibility onto four real SH coefficients
//! per RGB channel, and stores the result as [`PROBE_BYTES`] (12 B) of RGB8 plus
//! one packed validity bit.
//!
//! This is the production port of the M1-30 oracle in
//! [`voxel_storage::reference::probe`]; `tests/probe_bake.rs` pins byte equality
//! on three fixtures. The ray fan, the seed and the encoding are deliberately
//! the same geometry, because a different sample set or a different quantiser
//! would change the bytes and stop the oracle from being an oracle.
//!
//! # What production adds: a frame budget
//!
//! A full 128³ near-grid bake is at most 512 · 64 = **32,768 rays**, too much
//! for one frame. [`ProbeBake`] is a resumable state machine: the caller passes
//! a ray budget to [`ProbeBake::advance`], which casts at most that many rays
//! and keeps the in-flight probe (index, ray cursor, accumulator) for the next
//! call. A ray is the atomic work item and is traced to the scene boundary
//! inside the call, so the budget bounds rays, not wall-clock time. An invalid
//! probe costs one predicate call, not a ray.
//!
//! This is the content-timeline bake ADR-005 describes: the budget is a streaming
//! tick, not a render-frame deadline, and a caller may spread one bake over as
//! many ticks as it likes.
//!
//! Probes and rays are visited in ascending order and a probe's record is
//! written only when its last ray lands. The result therefore does not depend on
//! how the budget splits the work: one frame, sixty frames or a budget of one
//! ray produce the same SH payload and the same validity mask.
//!
//! # A frozen oracle
//!
//! The grid, seed, ray fan, traversal and quantiser are deliberately a copy of
//! the M1-30 reference. Editing both sides into agreement would make the
//! byte-equality tests vacuous, so treat the oracle as frozen and change this
//! port only together with a reference version-tag bump.
//!
//! # Determinism
//!
//! A probe seed is a pure hash of the chunk position, probe index and probe LOD.
//! Nothing is drawn from time, a thread-local RNG or work order, and the bake
//! owns no shared state. Two bakes of the same scene are byte-identical
//! regardless of budget, call order or thread.
//!
//! # Cost
//!
//! The bake is the slow path by design: it walks the SVDAG one voxel at a time
//! through an Amanatides–Woo DDA, exactly like the oracle. Hierarchical
//! node-skipping ray casts are the occlusion work (M2-25); the per-edit budget
//! is re-checked in M2-21, where the update touches only the probes an edit can
//! reach. Blooming this into a frame is the point of [`ProbeBake`].

use std::f64::consts::{PI, TAU};

use voxel_core::constants::BLOCK_DIM;
use voxel_core::content_constants::PROBE_BYTES;
use voxel_storage::dag::Svdag;
use voxel_storage::{AIR, CHUNK_DIM, ChunkPos};

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

/// Uniform-hemisphere estimator scale, `2π / N`.
const SH_SCALE: f64 = TAU / PROBE_RAYS as f64;

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
/// edge-independent, so a 512-probe bake can be exercised from a 64³ build.
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

    /// True when the probe centre was air when the probe was baked.
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

/// Work completed by one [`ProbeBake::advance`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BakeStep {
    /// Probes written during this call, including invalid ones.
    pub probes: usize,
    /// Rays cast during this call.
    pub rays: usize,
    /// True when the volume is complete.
    pub done: bool,
}

/// A probe currently between its first and last ray.
#[derive(Clone, Debug)]
struct PendingProbe {
    index: usize,
    centre: [f64; 3],
    rotation: f64,
    /// Next ray to cast, `0 .. PROBE_RAYS`.
    ray: usize,
    accumulated: [[f64; PROBE_CHANNELS]; SH_COEFFICIENTS],
}

/// A resumable near/mid/far-grid bake for one chunk (M2-18).
///
/// The state machine is single-threaded and owns its output buffers, so a
/// caller can keep it beside a streaming job and call [`Self::advance`] once per
/// frame without hurting determinism. The volume is complete when
/// [`Self::is_done`] returns true; [`Self::finish`] drains the rest and returns
/// the bytes.
pub struct ProbeBake<F> {
    chunk: ChunkPos,
    grid: ProbeGrid,
    solid_at: F,
    probes: Vec<Sh4Probe>,
    validity: Vec<u8>,
    /// Next probe index to start; the pending probe (if any) is before it.
    next: usize,
    pending: Option<PendingProbe>,
    rays_cast: u64,
}

impl<F> ProbeBake<F>
where
    F: FnMut(u32, u32, u32) -> bool,
{
    /// Starts a bake of `grid` against `solid_at`.
    ///
    /// `solid_at(x, y, z)` is called in chunk-local voxel coordinates over a
    /// scene of `grid.scene_edge_voxels()` voxels per axis, and must be a pure
    /// function of the coordinate for the determinism guarantee.
    pub fn new(chunk: ChunkPos, grid: ProbeGrid, solid_at: F) -> Self {
        let count = grid.len();
        Self {
            chunk,
            grid,
            solid_at,
            probes: vec![Sh4Probe::ZERO; count],
            validity: vec![0u8; count.div_ceil(8)],
            next: 0,
            pending: None,
            rays_cast: 0,
        }
    }

    /// The grid being baked.
    pub const fn grid(&self) -> ProbeGrid {
        self.grid
    }

    /// Total probes in the grid.
    pub fn probes_total(&self) -> usize {
        self.probes.len()
    }

    /// Probes already written.
    pub fn probes_completed(&self) -> usize {
        self.next - usize::from(self.pending.is_some())
    }

    /// Rays cast so far. A complete volume has `valid_probes · PROBE_RAYS`.
    pub const fn rays_cast(&self) -> u64 {
        self.rays_cast
    }

    /// True when every probe has been written.
    pub fn is_done(&self) -> bool {
        self.next == self.probes.len() && self.pending.is_none()
    }

    /// Casts at most `ray_budget` rays and returns what this call did.
    ///
    /// A ray is traced to completion or to the scene boundary inside the call,
    /// so the budget bounds rays, not voxel steps. Invalid probes cost no rays
    /// and are completed as they are reached. A budget of zero makes no
    /// progress; every non-zero budget makes at least one ray of progress once
    /// a valid probe is reached.
    ///
    /// The output bytes do not depend on the budget: a probe writes its record
    /// only after its last ray.
    pub fn advance(&mut self, ray_budget: usize) -> BakeStep {
        let probes_before = self.probes_completed();
        let mut rays = 0;
        while rays < ray_budget && !self.is_done() {
            if self.pending.is_none() && !self.start_next() {
                break;
            }
            if self.pending.is_some() {
                self.cast_ray();
                rays += 1;
            }
        }
        BakeStep {
            probes: self.probes_completed() - probes_before,
            rays,
            done: self.is_done(),
        }
    }

    /// Drains the remaining probes and returns the volume.
    ///
    /// This is the convenience path for tests, offline tools and the first
    /// bake of a chunk that may block one content update.
    pub fn finish(mut self) -> ProbeVolume {
        if !self.is_done() {
            self.advance(usize::MAX);
        }
        debug_assert!(self.is_done(), "an unbounded budget must finish the bake");
        ProbeVolume {
            grid: self.grid,
            probes: self.probes,
            validity: self.validity,
        }
    }

    /// Validates and starts the next probe.
    ///
    /// Returns false when the grid is exhausted. An invalid probe (its centre
    /// lies in a solid voxel) is completed here: the record stays
    /// [`Sh4Probe::ZERO`] and the validity bit stays clear.
    fn start_next(&mut self) -> bool {
        if self.next >= self.probes.len() {
            return false;
        }
        let index = self.next;
        self.next += 1;
        let centre = self.grid.centre(index);
        let centre_voxel = [
            centre[0].floor() as u32,
            centre[1].floor() as u32,
            centre[2].floor() as u32,
        ];
        if (self.solid_at)(centre_voxel[0], centre_voxel[1], centre_voxel[2]) {
            return true;
        }
        let rotation = probe_rotation(probe_seed(self.chunk, self.grid.lod(), index as u32));
        self.pending = Some(PendingProbe {
            index,
            centre,
            rotation,
            ray: 0,
            accumulated: [[0.0; PROBE_CHANNELS]; SH_COEFFICIENTS],
        });
        true
    }

    /// Casts one ray of the pending probe, finishing the record on the last.
    fn cast_ray(&mut self) {
        let Self {
            grid,
            solid_at,
            pending,
            probes,
            validity,
            rays_cast,
            ..
        } = self;
        let Some(probe) = pending.as_mut() else {
            debug_assert!(false, "cast_ray is only called with a pending probe");
            return;
        };

        let direction = hemisphere_ray(probe.ray, probe.rotation);
        if !trace(solid_at, grid.scene_edge_voxels(), probe.centre, direction) {
            let basis = sh_basis(direction);
            for (coefficient, value) in probe.accumulated.iter_mut().zip(basis) {
                for channel in coefficient.iter_mut() {
                    *channel += value;
                }
            }
        }
        probe.ray += 1;
        *rays_cast += 1;

        if probe.ray < PROBE_RAYS {
            return;
        }

        let coefficients = probe
            .accumulated
            .map(|basis| basis.map(|value| value * SH_SCALE));
        let index = probe.index;
        *pending = None;
        probes[index] = Sh4Probe::from_coefficients(coefficients);
        validity[index / 8] |= 1 << (index % 8);
    }
}

/// Bakes a whole volume without a frame budget.
///
/// Equivalent to [`ProbeBake::new`] followed by [`ProbeBake::finish`].
pub fn bake<F>(chunk: ChunkPos, grid: ProbeGrid, solid_at: F) -> ProbeVolume
where
    F: FnMut(u32, u32, u32) -> bool,
{
    ProbeBake::new(chunk, grid, solid_at).finish()
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

/// Derives the deterministic seed for one probe.
///
/// The seed is a pure function of the chunk position, the probe LOD and the
/// probe's linear index. It is mixed with SplitMix64 so adjacent probes get
/// independent azimuths without any RNG state. Chunk coordinates are signed;
/// their two's-complement bits are part of the hash.
pub fn probe_seed(chunk: ChunkPos, lod: ProbeLod, index: u32) -> u64 {
    let mut hash = 0x5052_4f42_455f_0001; // "PROB" plus a version tag
    hash ^= mix64(chunk.x as i64 as u64);
    hash = hash.rotate_left(17) ^ mix64(chunk.y as i64 as u64);
    hash = hash.rotate_left(17) ^ mix64(chunk.z as i64 as u64);
    hash = hash.rotate_left(17) ^ mix64(u64::from(index));
    hash = hash.rotate_left(17) ^ mix64(lod.seed_tag());
    mix64(hash)
}

/// Maps a probe seed to its azimuth rotation in radians.
pub fn probe_rotation(seed: u64) -> f64 {
    ((seed >> 11) as f64) * (TAU / (1u64 << 53) as f64)
}

/// Returns ray `index` of the 64-ray hemisphere, rotated by `rotation` radians.
///
/// Directions are unit length and always have `y > 0`. The same Fibonacci-area
/// sequence is used for every probe; only the azimuth changes with the seed.
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
/// Returns `true` on entering a solid voxel; a ray that steps outside the scene
/// returns `false` because the reference treats the chunk boundary as open sky.
/// The origin cell is known air — [`ProbeBake`] checks it before the first ray —
/// so the traversal starts on the first boundary crossing.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_bake_seed_depends_only_on_chunk_index_and_lod() {
        let chunk = ChunkPos::new(4, -5, 6);
        let seed = probe_seed(chunk, ProbeLod::Near, 17);
        assert_eq!(seed, probe_seed(chunk, ProbeLod::Near, 17));
        assert_ne!(
            seed,
            probe_seed(ChunkPos::new(5, -5, 6), ProbeLod::Near, 17)
        );
        assert_ne!(seed, probe_seed(chunk, ProbeLod::Mid, 17));
        assert_ne!(seed, probe_seed(chunk, ProbeLod::Near, 18));
    }

    #[test]
    fn probe_bake_record_round_trips_inside_one_quantisation_step() {
        let coefficients = [[-1.75, 0.0, 1.75], [0.5, -0.5, 0.0], [0.0; 3], [0.25; 3]];
        let decoded = Sh4Probe::from_coefficients(coefficients).coefficients();
        for (basis, expected) in decoded.into_iter().zip(coefficients) {
            for (channel, expected) in basis.into_iter().zip(expected) {
                assert!((channel - expected).abs() <= 2.0 / 255.0);
            }
        }
        assert_eq!(Sh4Probe::ZERO.to_bytes(), [SH8_ZERO; PROBE_BYTES]);
    }

    #[test]
    fn probe_bake_open_sky_is_valid_and_has_the_dc_term() {
        let volume = bake(ChunkPos::new(0, 0, 0), ProbeGrid::near(2), |_, _, _| false);
        let expected = 2.0 * PI * SH_L0;
        assert_eq!(volume.len(), 8);
        for index in 0..volume.len() {
            assert!(volume.is_valid(index));
            for channel in volume.probe(index).coefficients()[0] {
                assert!((channel - expected).abs() < 0.03);
            }
        }
    }

    #[test]
    fn probe_bake_zero_budget_makes_no_progress_and_a_positive_one_does() {
        let grid = ProbeGrid::near(2);
        let mut bake = ProbeBake::new(ChunkPos::new(0, 0, 0), grid, |_, _, _| false);
        assert!(!bake.is_done());
        assert_eq!(bake.advance(0), BakeStep::default());
        assert_eq!(bake.probes_completed(), 0);

        let step = bake.advance(1);
        assert_eq!(step.rays, 1);
        assert_eq!(step.probes, 0);
        assert!(!step.done);
        bake.advance(usize::MAX);
        assert!(bake.is_done());
        assert_eq!(bake.probes_completed(), grid.len());
        assert_eq!(bake.rays_cast(), (grid.len() * PROBE_RAYS) as u64);
    }
}
