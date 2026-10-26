//! Property test for random edit batches (M1-16).
//!
//! The property replays a concrete trace of `BATCHES` edit batches against two
//! implementations and a dense reference:
//!
//! - a [`ChunkDag`] edited in place, exercising the append-only COW path;
//! - the public [`Svdag`] facade, which applies the accumulated batches to a
//!   clone of the chunk and compacts it, exercising the immutable edit path;
//! - a `Vec<u16>` that applies the same batch in input order.
//!
//! Every batch contains at least one carve to air and one revert (an edit whose
//! material is the value that voxel held before its most recent edit), plus
//! random placements, carves and occasional duplicate edits. After each batch
//! the test compares edited voxels and random probes, expands the whole chunk
//! periodically, checks the banded invariants, and rebuilds the public chunk
//! from the dense buffer to pin the canonical legacy view.
//!
//! A failure is shrunk before it is reported: whole batches are removed first,
//! then individual edits, then materials and indices are simplified. Set
//! `VOXEL_PROPERTY_SEED` to the reported seed to replay the random case.

use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use voxel_storage::dag::Svdag;
use voxel_storage::{CHUNK_DIM, CHUNK_VOLUME, ChunkDag, local_index, local_xyz};

/// Number of batches in one property run.
const BATCHES: usize = 10_000;

/// Number of random probes checked after every batch.
const PROBES_PER_BATCH: usize = 32;

/// Expand both chunks and check invariants at this interval.
const FULL_CHECK_EVERY: usize = 128;

/// Rebuild the public chunk from the dense reference at this interval.
const REBUILD_EVERY: usize = 512;

/// Apply the accumulated batches to the public facade at this interval.
///
/// The facade compacts after every edit, so replaying each of the 10k batches
/// through it would spend most of the run in the offline compactor. Grouping
/// the same edits keeps the content check exact while exercising the public
/// entry point often enough to catch integration mistakes.
const PUBLIC_BATCH_EVERY: usize = 32;

/// Maximum number of replays the shrinker may spend.
const SHRINK_BUDGET: usize = 512;

/// Material ids used for placements. Air is `0` and is reserved for carves.
const MATERIALS: u16 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Edit {
    index: u32,
    material: u16,
}

impl Edit {
    const fn new(index: u32, material: u16) -> Self {
        Self { index, material }
    }
}

/// A reproducible sequence of edit batches.
#[derive(Clone, Debug)]
struct Trace {
    seed: u64,
    batches: Vec<Vec<Edit>>,
}

impl Trace {
    fn edit_count(&self) -> usize {
        self.batches.iter().map(Vec::len).sum()
    }
}

#[derive(Debug)]
struct Failure {
    batch: usize,
    detail: String,
}

impl Failure {
    fn new(batch: usize, detail: impl Into<String>) -> Self {
        Self {
            batch,
            detail: detail.into(),
        }
    }
}

/// Deterministic xorshift64, so a failing trace replays exactly.
#[inline]
fn next(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// A blocky base with a solid lower half and sparse upper structures.
fn fixture() -> Vec<u16> {
    let mut dense = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM as u32 {
        for y in 0..CHUNK_DIM as u32 {
            for x in 0..CHUNK_DIM as u32 {
                let material = if y < 20 {
                    1
                } else if y < 30 && (x / 8 + z / 8) % 3 == 0 {
                    2
                } else if (40..48).contains(&y) && (x + z) % 11 == 0 {
                    3
                } else {
                    0
                };
                dense[local_index(x, y, z) as usize] = material;
            }
        }
    }
    dense
}

/// Generates a trace while maintaining the dense state needed to choose
/// meaningful carves and reverts.
struct TraceGen {
    state: u64,
    reference: Vec<u16>,
    /// Indices that currently hold a non-air material.
    solid: Vec<u32>,
    solid_pos: HashMap<u32, usize>,
    /// Every index that has been edited, with duplicates used as a weighted
    /// pick list.
    edited: Vec<u32>,
    /// The material each index held before its most recent edit.
    before: HashMap<u32, u16>,
}

impl TraceGen {
    fn new(seed: u64) -> Self {
        let reference = fixture();
        let mut solid = Vec::new();
        let mut solid_pos = HashMap::new();
        for (index, &material) in reference.iter().enumerate() {
            if material != 0 {
                solid_pos.insert(index as u32, solid.len());
                solid.push(index as u32);
            }
        }
        Self {
            state: seed | 1,
            reference,
            solid,
            solid_pos,
            edited: Vec::new(),
            before: HashMap::new(),
        }
    }

    fn random_index(&mut self) -> u32 {
        (next(&mut self.state) % CHUNK_VOLUME as u64) as u32
    }

    /// Applies an edit to the generation reference and records the old value.
    fn set(&mut self, index: u32, material: u16) -> Edit {
        let old = self.reference[index as usize];
        if old == 0 && material != 0 {
            self.solid_pos.insert(index, self.solid.len());
            self.solid.push(index);
        } else if old != 0
            && material == 0
            && let Some(pos) = self.solid_pos.remove(&index)
        {
            let last = self.solid.pop().expect("the solid set is not empty");
            if pos < self.solid.len() {
                self.solid[pos] = last;
                self.solid_pos.insert(last, pos);
            }
        }

        self.before.insert(index, old);
        self.edited.push(index);
        self.reference[index as usize] = material;
        Edit::new(index, material)
    }

    fn place(&mut self) -> Edit {
        let index = self.random_index();
        let material = 1 + (next(&mut self.state) % u64::from(MATERIALS)) as u16;
        self.set(index, material)
    }

    fn carve(&mut self) -> Edit {
        let index = if self.solid.is_empty() {
            self.random_index()
        } else {
            let slot = (next(&mut self.state) % self.solid.len() as u64) as usize;
            self.solid[slot]
        };
        self.set(index, 0)
    }

    fn revert(&mut self) -> Edit {
        let slot = (next(&mut self.state) % self.edited.len() as u64) as usize;
        let index = self.edited[slot];
        let material = self.before[&index];
        self.set(index, material)
    }

    fn batch(&mut self, number: usize) -> Vec<Edit> {
        let mut batch = Vec::with_capacity(9);

        // Every batch exercises both required operations: remove a solid voxel
        // and undo the most recent edit of some voxel.
        batch.push(self.carve());
        batch.push(self.revert());

        let extra = (next(&mut self.state) % 7) as usize;
        for _ in 0..extra {
            let kind = next(&mut self.state) % 8;
            let edit = match kind {
                0..=2 => self.carve(),
                3..=6 => self.place(),
                _ => self.revert(),
            };
            batch.push(edit);
        }

        // Duplicate edits in one batch must behave like the batch applied in
        // input order: the last value wins.
        if number.is_multiple_of(17) {
            let slot = (next(&mut self.state) % batch.len() as u64) as usize;
            let index = batch[slot].index;
            let material = (next(&mut self.state) % 5) as u16;
            batch.push(self.set(index, material));
        }

        batch
    }
}

fn generate(seed: u64, batches: usize) -> Trace {
    let mut generator = TraceGen::new(seed);
    Trace {
        seed,
        batches: (0..batches).map(|n| generator.batch(n)).collect(),
    }
}

fn compare(
    source: &str,
    batch: usize,
    index: u32,
    expected: u16,
    actual: u16,
) -> Result<(), Failure> {
    if actual == expected {
        return Ok(());
    }
    let (x, y, z) = local_xyz(index);
    Err(Failure::new(
        batch,
        format!("{source}: voxel {index} at ({x}, {y}, {z}) expected {expected}, got {actual}"),
    ))
}

fn expand_and_compare(
    source: &str,
    batch: usize,
    chunk: &dyn Fn(&mut [u16]),
    dense: &[u16],
) -> Result<(), Failure> {
    let mut out = vec![0u16; CHUNK_VOLUME];
    chunk(&mut out);
    let Some(index) = out.iter().zip(dense).position(|(a, b)| a != b) else {
        return Ok(());
    };
    let (x, y, z) = local_xyz(index as u32);
    Err(Failure::new(
        batch,
        format!(
            "{source}: expanded chunk differs at index {index} ({x}, {y}, {z}): \
             expected {}, got {}",
            dense[index], out[index]
        ),
    ))
}

fn check_invariants(source: &str, batch: usize, chunk: &ChunkDag) -> Result<(), Failure> {
    chunk
        .check()
        .map_err(|error| Failure::new(batch, format!("{source}: {error}")))
}

/// Replays `trace`, returning the first mismatch with the dense reference.
fn check(trace: &Trace) -> Result<(), Failure> {
    let mut dense = fixture();
    let mut banded = ChunkDag::from_dense(&dense);
    let mut public = Svdag::from_dense(&dense);
    let mut pending = Vec::new();
    let mut probes = trace.seed ^ 0xd1b5_4a32_d192_ed03;

    for (number, batch) in trace.batches.iter().enumerate() {
        for edit in batch {
            dense[edit.index as usize] = edit.material;
            pending.push(*edit);
        }

        banded.apply_edits(batch.iter().map(|edit| (edit.index, edit.material)));

        let final_batch = number + 1 == trace.batches.len();
        let flush = number % PUBLIC_BATCH_EVERY == 0 || final_batch;
        if flush {
            public = public.apply_edits(pending.iter().map(|edit| (edit.index, edit.material)));
            pending.clear();
        }

        // The touched voxels are the cases the batch is about: placed,
        // carved, reverted, and overwritten by a duplicate.
        for edit in batch {
            let (x, y, z) = local_xyz(edit.index);
            let expected = dense[edit.index as usize];
            compare("banded", number, edit.index, expected, banded.get(x, y, z))?;
            if flush {
                compare("public", number, edit.index, expected, public.get(x, y, z))?;
            }
        }

        for _ in 0..PROBES_PER_BATCH {
            let index = (next(&mut probes) % CHUNK_VOLUME as u64) as u32;
            let (x, y, z) = local_xyz(index);
            let expected = dense[index as usize];
            compare("banded probe", number, index, expected, banded.get(x, y, z))?;
            if flush {
                compare("public probe", number, index, expected, public.get(x, y, z))?;
            }
        }

        if number % FULL_CHECK_EVERY == 0 || final_batch {
            if !pending.is_empty() {
                public = public.apply_edits(pending.iter().map(|edit| (edit.index, edit.material)));
                pending.clear();
            }
            expand_and_compare("banded", number, &|out| banded.expand(out), &dense)?;
            expand_and_compare("public", number, &|out| public.expand(out), &dense)?;
            check_invariants("banded", number, &banded)?;
            public
                .check()
                .map_err(|error| Failure::new(number, format!("public: {error}")))?;
        }

        if number % REBUILD_EVERY == 0 || final_batch {
            let rebuilt = Svdag::from_dense(&dense);
            if public != rebuilt {
                return Err(Failure::new(
                    number,
                    "public: canonical legacy view differs from a dense rebuild",
                ));
            }
        }
    }

    Ok(())
}

/// Greedily removes batches, edits, then simplifies fields while `fails`
/// continues to hold. Bounded by [`SHRINK_BUDGET`] replays.
fn shrink(trace: &Trace, fails: impl Fn(&Trace) -> bool) -> Trace {
    let mut best = trace.clone();
    if !fails(&best) {
        return best;
    }
    let mut budget = SHRINK_BUDGET;

    // Remove contiguous whole batches, coarse to fine.
    let mut chunk = (best.batches.len() / 2).max(1);
    while budget > 0 {
        let mut start = 0usize;
        while start < best.batches.len() && budget > 0 {
            let end = (start + chunk).min(best.batches.len());
            let mut candidate = best.clone();
            candidate.batches.drain(start..end);
            budget -= 1;
            if fails(&candidate) {
                best = candidate;
            } else {
                start += chunk;
            }
        }
        if chunk == 1 {
            break;
        }
        chunk = (chunk / 2).max(1);
    }

    // Remove individual edits, dropping a batch when its last edit goes.
    let mut batch = 0usize;
    while batch < best.batches.len() && budget > 0 {
        let mut edit = 0usize;
        while edit < best.batches[batch].len() && budget > 0 {
            let mut candidate = best.clone();
            candidate.batches[batch].remove(edit);
            budget -= 1;
            if fails(&candidate) {
                best = candidate;
            } else {
                edit += 1;
            }
        }
        if best.batches[batch].is_empty() {
            let mut candidate = best.clone();
            candidate.batches.remove(batch);
            budget -= 1;
            if fails(&candidate) {
                best = candidate;
                continue;
            }
        }
        batch += 1;
    }

    // Simplify each field: air first, then the first placement material, then
    // the lowest voxel index.
    for batch in 0..best.batches.len() {
        for edit in 0..best.batches[batch].len() {
            for material in [0, 1] {
                if budget == 0 {
                    break;
                }
                if best.batches[batch][edit].material == material {
                    continue;
                }
                let mut candidate = best.clone();
                candidate.batches[batch][edit].material = material;
                budget -= 1;
                if fails(&candidate) {
                    best = candidate;
                }
            }
            for index in [0, 1] {
                if budget == 0 {
                    break;
                }
                if best.batches[batch][edit].index == index {
                    continue;
                }
                let mut candidate = best.clone();
                candidate.batches[batch][edit].index = index;
                budget -= 1;
                if fails(&candidate) {
                    best = candidate;
                }
            }
        }
    }

    best
}

/// Renders the minimal failing trace without dumping thousands of edits.
fn describe(trace: &Trace) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "shrunk to {} batches / {} edits",
        trace.batches.len(),
        trace.edit_count()
    );
    for (number, batch) in trace.batches.iter().enumerate() {
        let _ = writeln!(out, "  batch {number}: {batch:?}");
    }
    out
}

fn run(seed: u64, label: &str) {
    let trace = generate(seed, BATCHES);
    let Err(failure) = check(&trace) else {
        return;
    };

    let shrunk = shrink(&trace, |candidate| check(candidate).is_err());
    let shrunk_failure = check(&shrunk).expect_err("the shrunk trace still fails");
    panic!(
        "{label} property failed at batch {} (seed {seed:#018x}): {}\n\
         minimal failure at batch {}: {}\n\
         {}\n\
         reproduce with VOXEL_PROPERTY_SEED={seed}",
        failure.batch,
        failure.detail,
        shrunk_failure.batch,
        shrunk_failure.detail,
        describe(&shrunk),
    );
}

#[test]
fn property_edits_fixed_seed() {
    run(0x005e_ed0f_1e2d_3c4b, "fixed-seed");
}

#[test]
fn property_edits_random_seed() {
    let seed = match env::var("VOXEL_PROPERTY_SEED") {
        Ok(raw) => {
            let raw = raw.trim();
            match raw.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16)
                    .expect("VOXEL_PROPERTY_SEED must be a decimal or 0x-prefixed u64"),
                None => raw
                    .parse()
                    .expect("VOXEL_PROPERTY_SEED must be a decimal or 0x-prefixed u64"),
            }
        }
        Err(_) => {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("the system clock is after the Unix epoch")
                .as_nanos() as u64;
            nanos ^ u64::from(std::process::id()).rotate_left(32)
        }
    };
    run(seed, "random-seed");
}

/// The shrinker is itself part of the property: a failure surrounded by
/// unrelated batches and edits must reduce to the single offending edit.
#[test]
fn property_edits_shrinker_reduces_a_counterexample() {
    let trace = Trace {
        seed: 7,
        batches: vec![
            vec![Edit::new(5, 2), Edit::new(6, 3)],
            vec![Edit::new(42, 0)],
            vec![Edit::new(7, 4)],
            vec![Edit::new(8, 0), Edit::new(42, 1)],
        ],
    };
    let fails = |candidate: &Trace| {
        candidate
            .batches
            .iter()
            .flatten()
            .any(|edit| edit.index == 42 && edit.material == 0)
    };

    let shrunk = shrink(&trace, fails);

    assert_eq!(shrunk.edit_count(), 1, "expected a one-edit counterexample");
    assert_eq!(shrunk.batches.len(), 1, "the empty batches were removed");
    assert_eq!(shrunk.batches[0][0], Edit::new(42, 0));
}
