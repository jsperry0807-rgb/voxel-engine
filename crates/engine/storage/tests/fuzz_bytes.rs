//! Fuzz the byte decoders (M1-17).
//!
//! Every decoder in this crate treats its input as untrusted. This test feeds
//! three of them arbitrary bytes and requires that each returns `Some` or
//! `None` without panicking:
//!
//! - [`ChunkDag::from_bytes`], the v2 banded container (M1-13);
//! - [`Svdag::from_bytes`], the public v1 pool view kept by M1-15;
//! - [`Overlay::from_bytes`], the edit snapshots the disk log also reads.
//!
//! The run has three parts:
//!
//! 1. a seed corpus of valid encodings, every useful truncation, and header
//!    fields set to their extremes;
//! 2. one million pseudo-random buffers with lengths from empty through
//!    several KiB, exercising the early magic/version/length gates;
//! 3. mutations of the small corpus entries, which reach the node, leaf and
//!    pool walkers rather than bouncing off the first magic check.
//!
//! The generator is a fixed-seed xorshift, so a failure is reproducible. The
//! seed corpus, a manifest and up to [`ACCEPTED_CAP`] buffers that decoded to
//! `Some` are saved under `CARGO_TARGET_TMPDIR/fuzz_bytes`; a panic is saved
//! there before the test fails, so the offending input survives the panic.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use voxel_storage::bands::BAND_COUNT;
use voxel_storage::dag::Svdag;
use voxel_storage::node_word::NODE_INDEX_MAX;
use voxel_storage::overlay::Overlay;
use voxel_storage::serialize::{CHUNK_BITS_OFFSET, HEADER_BYTES, VERSION};
use voxel_storage::{CHUNK_DIM, CHUNK_VOLUME, ChunkDag, local_index};

/// Number of completely random buffers.
const RANDOM_BUFFERS: usize = 1_000_000;

/// Number of corpus mutations.
const MUTATIONS: usize = 100_000;

/// Upper bound on saved buffers that decoded to `Some`.
const ACCEPTED_CAP: usize = 1_024;

/// Fixed seed; ASCII `M1_fuzz1`.
const SEED: u64 = 0x4d31_5f66_757a_7a31;

/// Longest random buffer. Longer inputs add time, not coverage: every header
/// gate runs before the declared sections are read.
const MAX_RANDOM_LEN: usize = 16 * 1_024;

#[derive(Clone)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn below(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0, "an empty range has no sample");
        (self.next_u64() % bound as u64) as usize
    }

    fn fill(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            let word = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}

/// A named corpus entry.
struct Seed {
    name: String,
    bytes: Vec<u8>,
}

impl Seed {
    fn new(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            bytes,
        }
    }
}

/// What each decoder returned for one buffer. `false` is `None`; the test does
/// not distinguish the rejection reason.
#[derive(Clone, Copy, Debug, Default)]
struct Decoded {
    chunk: bool,
    svdag: bool,
    overlay: bool,
}

impl Decoded {
    fn any(self) -> bool {
        self.chunk || self.svdag || self.overlay
    }
}

/// Runs all decoders, converting a panic into the name of the decoder that
/// raised it. `catch_unwind` keeps the fuzz loop alive long enough to persist
/// the offending bytes.
fn decode_all(bytes: &[u8]) -> Result<Decoded, &'static str> {
    let chunk = panic::catch_unwind(AssertUnwindSafe(|| ChunkDag::from_bytes(bytes).is_some()))
        .map_err(|_| "ChunkDag::from_bytes")?;
    let svdag = panic::catch_unwind(AssertUnwindSafe(|| Svdag::from_bytes(bytes).is_some()))
        .map_err(|_| "Svdag::from_bytes")?;
    let overlay = panic::catch_unwind(AssertUnwindSafe(|| Overlay::from_bytes(bytes).is_some()))
        .map_err(|_| "Overlay::from_bytes")?;
    Ok(Decoded {
        chunk,
        svdag,
        overlay,
    })
}

/// The five layout fixtures, matching the serialization tests.
fn fixtures() -> Vec<(&'static str, Vec<u16>)> {
    let air = vec![0u16; CHUNK_VOLUME];
    let solid = vec![3u16; CHUNK_VOLUME];

    let mut slab = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for y in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                if y < 17 {
                    slab[local_index(x as u32, y as u32, z as u32) as usize] = 1 + (y % 3) as u16;
                }
            }
        }
    }

    let mut checker = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for y in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                checker[local_index(x as u32, y as u32, z as u32) as usize] =
                    if (x + y + z) % 2 == 0 { 1 } else { 2 };
            }
        }
    }

    let mut noise = vec![0u16; CHUNK_VOLUME];
    let mut rng = Rng::new(0x5eed_1e55_c0de_1234);
    for voxel in &mut noise {
        let sample = rng.next_u64();
        if sample % 100 < 30 {
            *voxel = 1 + ((sample >> 8) % 7) as u16;
        }
    }

    vec![
        ("air", air),
        ("solid", solid),
        ("slab", slab),
        ("checker", checker),
        ("noise", noise),
    ]
}

/// Valid encodings plus the empty formats. A v1 empty chunk is the eight-byte
/// header with root `NONE` and an empty pool.
fn base_seeds() -> Vec<Seed> {
    let mut seeds = Vec::new();
    for (name, dense) in fixtures() {
        seeds.push(Seed::new(
            format!("v2-{name}"),
            ChunkDag::from_dense(&dense).to_bytes(),
        ));
        seeds.push(Seed::new(
            format!("v1-{name}"),
            Svdag::from_dense(&dense).to_bytes(),
        ));
    }

    let mut overlay = Overlay::default();
    overlay.set(0, 1);
    overlay.set(local_index(1, 2, 3), 5);
    overlay.set(CHUNK_VOLUME as u32 - 1, u16::MAX);
    seeds.push(Seed::new("overlay-full", overlay.to_bytes()));
    seeds.push(Seed::new("overlay-empty", Overlay::default().to_bytes()));
    seeds.push(Seed::new(
        "v1-empty",
        vec![0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0],
    ));
    seeds
}

/// Prefix lengths worth trying for one valid encoding. All short prefixes are
/// included so the header boundary is covered exhaustively; longer payloads
/// are sampled at the quarter points.
fn prefix_lengths(len: usize) -> Vec<usize> {
    let mut ends = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17];
    if len >= HEADER_BYTES {
        ends.extend([
            HEADER_BYTES - 1,
            HEADER_BYTES,
            HEADER_BYTES + 1,
            HEADER_BYTES + 7,
            HEADER_BYTES + 8,
        ]);
    }
    ends.extend([
        len / 4,
        len / 2,
        len.saturating_mul(3) / 4,
        len.saturating_sub(2),
        len.saturating_sub(1),
        len,
    ]);
    ends.retain(|&end| end <= len);
    ends.sort_unstable();
    ends.dedup();
    ends
}

/// Every proper prefix of every seed.
fn prefix_seeds(seeds: &[Seed]) -> Vec<Seed> {
    let mut out = Vec::new();
    for seed in seeds {
        for end in prefix_lengths(seed.bytes.len()) {
            if end == seed.bytes.len() {
                continue;
            }
            out.push(Seed::new(
                format!("{}-cut-{end}", seed.name),
                seed.bytes[..end].to_vec(),
            ));
        }
    }
    out
}

fn with_u16(base: &[u8], offset: usize, value: u16) -> Vec<u8> {
    let mut bytes = base.to_vec();
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    bytes
}

fn with_u32(base: &[u8], offset: usize, value: u32) -> Vec<u8> {
    let mut bytes = base.to_vec();
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    bytes
}

fn with_u64(base: &[u8], offset: usize, value: u64) -> Vec<u8> {
    let mut bytes = base.to_vec();
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    bytes
}

/// Header-edge cases for the v2 container: each field is pushed to zero, one,
/// `u64::MAX` or the `u48` address-space boundary while the rest stays valid.
fn v2_header_seeds(base: &[u8]) -> Vec<Seed> {
    let mut out = Vec::new();
    if base.len() < HEADER_BYTES {
        return out;
    }

    for offset in 0..4 {
        let mut bytes = base.to_vec();
        bytes[offset] ^= 0xff;
        out.push(Seed::new(format!("v2-magic-{offset}"), bytes));
    }
    for (label, version) in [
        ("zero", 0),
        ("one", 1),
        ("future", VERSION.wrapping_add(1)),
        ("max", u16::MAX),
    ] {
        out.push(Seed::new(
            format!("v2-version-{label}"),
            with_u16(base, 4, version),
        ));
    }
    for (label, bits) in [
        ("zero", 0u8),
        ("one", 1),
        ("below", (voxel_storage::CHUNK_BITS as u8).wrapping_sub(1)),
        ("above", (voxel_storage::CHUNK_BITS as u8).wrapping_add(1)),
        ("max", u8::MAX),
    ] {
        let mut bytes = base.to_vec();
        bytes[CHUNK_BITS_OFFSET] = bits;
        out.push(Seed::new(format!("v2-bits-{label}"), bytes));
    }
    for (label, present) in [("two", 2u8), ("three", 3), ("max", u8::MAX)] {
        let mut bytes = base.to_vec();
        bytes[7] = present;
        out.push(Seed::new(format!("v2-root-present-{label}"), bytes));
    }
    for (label, index) in [
        ("zero", 0),
        ("one", 1),
        ("u48", NODE_INDEX_MAX),
        ("u48-plus-one", NODE_INDEX_MAX.saturating_add(1)),
        ("max", u64::MAX),
    ] {
        let mut bytes = with_u64(base, 8, index);
        bytes[7] = 1;
        out.push(Seed::new(format!("v2-root-index-{label}"), bytes));
    }
    for level in 0..BAND_COUNT {
        let offset = 16 + level * 8;
        for (label, length) in [
            ("zero", 0),
            ("one", 1),
            ("u48", NODE_INDEX_MAX),
            ("u48-plus-one", NODE_INDEX_MAX.saturating_add(1)),
            ("max", u64::MAX),
        ] {
            out.push(Seed::new(
                format!("v2-band-{level}-{label}"),
                with_u64(base, offset, length),
            ));
        }
    }
    let leaf_offset = HEADER_BYTES - 8;
    for (label, length) in [
        ("zero", 0),
        ("one", 1),
        ("u48", NODE_INDEX_MAX),
        ("u48-plus-one", NODE_INDEX_MAX.saturating_add(1)),
        ("max", u64::MAX),
    ] {
        out.push(Seed::new(
            format!("v2-leaves-{label}"),
            with_u64(base, leaf_offset, length),
        ));
    }
    let mut absent_nonzero = base.to_vec();
    absent_nonzero[7] = 0;
    absent_nonzero[8..16].copy_from_slice(&1u64.to_le_bytes());
    out.push(Seed::new("v2-absent-root-nonzero", absent_nonzero));

    for extra in [1usize, 2, 7, 8, 16] {
        let mut bytes = base.to_vec();
        bytes.resize(bytes.len() + extra, 0xa5);
        out.push(Seed::new(format!("v2-trailing-{extra}"), bytes));
    }
    out
}

/// Header-edge cases for the v1 pool: count/root disagreement and a malformed
/// first node, which makes the recursive walker reject rather than index past
/// the pool or write outside the dense buffer.
fn v1_header_seeds(base: &[u8]) -> Vec<Seed> {
    let mut out = Vec::new();
    if base.len() < 8 {
        return out;
    }
    let count = u32::from_le_bytes(base[4..8].try_into().expect("four-byte count"));
    for (label, value) in [
        ("zero", 0),
        ("one", 1),
        ("count-plus-one", count.saturating_add(1)),
        ("max", u32::MAX),
    ] {
        out.push(Seed::new(
            format!("v1-count-{label}"),
            with_u32(base, 4, value),
        ));
    }
    for (label, root) in [
        ("none", u32::MAX),
        ("count", count),
        ("count-plus-one", count.saturating_add(1)),
        ("max-minus-one", u32::MAX - 1),
    ] {
        out.push(Seed::new(
            format!("v1-root-{label}"),
            with_u32(base, 0, root),
        ));
    }
    if base.len() >= 12 {
        let mut bytes = base.to_vec();
        bytes[9] = bytes[9].wrapping_add(1);
        out.push(Seed::new("v1-first-level", bytes));
    }
    if base.len() >= 16 {
        let mut bytes = base.to_vec();
        bytes[8..12].copy_from_slice(&0u32.to_le_bytes());
        out.push(Seed::new("v1-first-header-zero", bytes));
        let mut bytes = base.to_vec();
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        out.push(Seed::new("v1-first-child-max", bytes));
    }
    for extra in [1usize, 2, 4, 8] {
        let mut bytes = base.to_vec();
        bytes.resize(bytes.len() + extra, 0xa5);
        out.push(Seed::new(format!("v1-trailing-{extra}"), bytes));
    }
    out
}

/// Header-edge cases for an overlay snapshot.
fn overlay_seeds(base: &[u8]) -> Vec<Seed> {
    let mut out = Vec::new();
    if base.len() < 4 {
        return out;
    }
    let count = u32::from_le_bytes(base[0..4].try_into().expect("four-byte count"));
    for (label, value) in [
        ("zero", 0),
        ("one", 1),
        ("count-plus-one", count.saturating_add(1)),
        ("max", u32::MAX),
    ] {
        out.push(Seed::new(
            format!("overlay-count-{label}"),
            with_u32(base, 0, value),
        ));
    }
    if base.len() >= 10 {
        let mut bytes = base.to_vec();
        bytes[4..8].copy_from_slice(&(CHUNK_VOLUME as u32).to_le_bytes());
        out.push(Seed::new("overlay-index-out-of-range", bytes));
    }
    for extra in [1usize, 2, 6, 12] {
        let mut bytes = base.to_vec();
        bytes.resize(bytes.len() + extra, 0xa5);
        out.push(Seed::new(format!("overlay-trailing-{extra}"), bytes));
    }
    out
}

/// Valid encodings, their truncations and the format-specific header edges.
fn corpus() -> Vec<Seed> {
    let base = base_seeds();
    let mut seeds = Vec::new();
    seeds.extend(base.iter().map(|seed| Seed {
        name: seed.name.clone(),
        bytes: seed.bytes.clone(),
    }));
    seeds.extend(prefix_seeds(&base));

    let v2 = base
        .iter()
        .find(|seed| seed.name == "v2-solid")
        .expect("the solid v2 fixture exists");
    seeds.extend(v2_header_seeds(&v2.bytes));
    let v1 = base
        .iter()
        .find(|seed| seed.name == "v1-solid")
        .expect("the solid v1 fixture exists");
    seeds.extend(v1_header_seeds(&v1.bytes));
    let overlay = base
        .iter()
        .find(|seed| seed.name == "overlay-full")
        .expect("the overlay fixture exists");
    seeds.extend(overlay_seeds(&overlay.bytes));
    seeds
}

/// A random buffer length, weighted toward tiny inputs but reaching the KiB
/// range often enough to cross the header and length tables.
fn random_len(rng: &mut Rng) -> usize {
    match rng.below(100) {
        0..=79 => rng.below(64),
        80..=94 => 64 + rng.below(448),
        95..=98 => 512 + rng.below(3_584),
        _ => 4_096 + rng.below(MAX_RANDOM_LEN - 4_096),
    }
}

/// One mutation of a small corpus entry. Ruining a header field is the
/// interesting case; truncation and extension cover the length gates.
fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut bytes = seed.to_vec();
    match rng.below(8) {
        0 => {
            let end = rng.below(bytes.len() + 1);
            bytes.truncate(end);
        }
        1 => {
            let extra = 1 + rng.below(32);
            let start = bytes.len();
            bytes.resize(start + extra, 0);
            rng.fill(&mut bytes[start..]);
        }
        2 => {
            let flips = 1 + rng.below(4);
            for _ in 0..flips {
                if bytes.is_empty() {
                    break;
                }
                let index = rng.below(bytes.len());
                bytes[index] ^= 1 << rng.below(8);
            }
        }
        3 => {
            if !bytes.is_empty() {
                let start = rng.below(bytes.len());
                let len = 1 + rng.below((bytes.len() - start).min(8));
                rng.fill(&mut bytes[start..start + len]);
            }
        }
        4 => {
            let words = bytes.len() / 8;
            if words > 0 {
                let offset = rng.below(words) * 8;
                let value = match rng.below(4) {
                    0 => 0,
                    1 => u64::MAX,
                    2 => NODE_INDEX_MAX,
                    _ => rng.next_u64(),
                };
                bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        5 => {
            if bytes.len() >= HEADER_BYTES {
                let level = rng.below(BAND_COUNT);
                let offset = 16 + level * 8;
                let value = match rng.below(4) {
                    0 => 0,
                    1 => 1,
                    2 => NODE_INDEX_MAX,
                    _ => u64::MAX,
                };
                bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        6 => {
            if !bytes.is_empty() {
                let index = rng.below(bytes.len());
                bytes[index] = match rng.below(3) {
                    0 => 0,
                    1 => 0xff,
                    _ => rng.next_u64() as u8,
                };
            }
        }
        _ => {
            if bytes.len() > 1 {
                let offset = rng.below(bytes.len());
                bytes.rotate_left(offset);
            }
        }
    }
    bytes
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// `CARGO_TARGET_TMPDIR` is set for integration tests, so the corpus lives
/// under `target/tmp/`; the fallback only matters when that variable is absent.
fn corpus_dir() -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("voxel-storage-fuzz"));
    base.join("fuzz_bytes")
}

fn prepare_corpus_dir(dir: &Path) {
    if dir.exists() {
        fs::remove_dir_all(dir).expect("remove the previous fuzz corpus");
    }
    fs::create_dir_all(dir).expect("create the fuzz corpus directory");
}

fn write_seeds(dir: &Path, seeds: &[Seed]) -> Vec<String> {
    seeds
        .iter()
        .enumerate()
        .map(|(index, seed)| {
            let file = format!("seed-{index:04}-{}.bin", sanitize(&seed.name));
            fs::write(dir.join(&file), &seed.bytes).expect("save a seed corpus entry");
            file
        })
        .collect()
}

fn write_manifest(
    dir: &Path,
    seeds: &[Seed],
    seed_files: &[String],
    accepted: &[Vec<u8>],
    accepted_files: &[String],
    checks: usize,
) {
    let mut text = String::new();
    writeln!(text, "# voxel-storage from_bytes fuzz corpus (M1-17)")
        .expect("formatting to a String cannot fail");
    writeln!(text, "seed\t{SEED:#018x}").expect("formatting to a String cannot fail");
    writeln!(text, "random_buffers\t{RANDOM_BUFFERS}").expect("formatting to a String cannot fail");
    writeln!(text, "mutations\t{MUTATIONS}").expect("formatting to a String cannot fail");
    writeln!(text, "checks\t{checks}").expect("formatting to a String cannot fail");
    writeln!(text, "accepted_saved\t{}", accepted.len())
        .expect("formatting to a String cannot fail");
    for (file, seed) in seed_files.iter().zip(seeds) {
        writeln!(text, "entry\t{file}\t{}\t{}", seed.bytes.len(), seed.name)
            .expect("formatting to a String cannot fail");
    }
    for (file, bytes) in accepted_files.iter().zip(accepted) {
        writeln!(text, "accepted\t{file}\t{}", bytes.len())
            .expect("formatting to a String cannot fail");
    }
    fs::write(dir.join("manifest.txt"), text).expect("save the fuzz corpus manifest");
}

struct Fuzzer {
    rng: Rng,
    dir: PathBuf,
    checks: usize,
    accepted: Vec<Vec<u8>>,
    seen: HashSet<Vec<u8>>,
    accepted_any: bool,
}

impl Fuzzer {
    fn new(dir: PathBuf) -> Self {
        Self {
            rng: Rng::new(SEED),
            dir,
            checks: 0,
            accepted: Vec::new(),
            seen: HashSet::new(),
            accepted_any: false,
        }
    }

    /// Feeds one buffer to all decoders. `record` controls whether an accepted
    /// buffer joins the saved corpus; seeds are already written separately.
    fn exercise(&mut self, bytes: &[u8], record: bool) {
        self.checks += 1;
        match decode_all(bytes) {
            Ok(decoded) => {
                self.accepted_any |= decoded.any();
                if record
                    && decoded.any()
                    && self.accepted.len() < ACCEPTED_CAP
                    && self.seen.insert(bytes.to_vec())
                {
                    self.accepted.push(bytes.to_vec());
                }
            }
            Err(target) => {
                let panic_file = format!("panic-{:08}.bin", self.checks);
                let path = self.dir.join(&panic_file);
                let _ = fs::write(&path, bytes);
                panic!(
                    "{target} panicked on a {} byte buffer; the input is saved at {}",
                    bytes.len(),
                    path.display()
                );
            }
        }
    }

    fn save_accepted(&self) -> Vec<String> {
        self.accepted
            .iter()
            .enumerate()
            .map(|(index, bytes)| {
                let file = format!("accepted-{index:04}.bin");
                fs::write(self.dir.join(&file), bytes).expect("save an accepted corpus entry");
                file
            })
            .collect()
    }
}

#[test]
fn fuzz_bytes() {
    let seeds = corpus();
    assert!(!seeds.is_empty(), "the corpus has no entries");

    // A valid buffer from each format must still be accepted, otherwise the
    // fuzz run is only exercising the early reject path.
    let find = |name: &str| {
        seeds
            .iter()
            .find(|seed| seed.name == name)
            .expect("the named seed exists")
            .bytes
            .clone()
    };
    assert!(ChunkDag::from_bytes(&find("v2-solid")).is_some());
    assert!(Svdag::from_bytes(&find("v1-solid")).is_some());
    assert!(Overlay::from_bytes(&find("overlay-full")).is_some());

    let dir = corpus_dir();
    prepare_corpus_dir(&dir);
    let seed_files = write_seeds(&dir, &seeds);

    let mut fuzzer = Fuzzer::new(dir.clone());
    for seed in &seeds {
        fuzzer.exercise(&seed.bytes, false);
    }

    let mut random_buffer = vec![0u8; MAX_RANDOM_LEN];
    for _ in 0..RANDOM_BUFFERS {
        let len = random_len(&mut fuzzer.rng);
        fuzzer.rng.fill(&mut random_buffer[..len]);
        fuzzer.exercise(&random_buffer[..len], true);
    }

    let mutation_seeds: Vec<&Seed> = seeds
        .iter()
        .filter(|seed| !seed.bytes.is_empty() && seed.bytes.len() <= 4_096)
        .collect();
    assert!(
        !mutation_seeds.is_empty(),
        "no small corpus entry is available to mutate"
    );
    for _ in 0..MUTATIONS {
        let seed = mutation_seeds[fuzzer.rng.below(mutation_seeds.len())]
            .bytes
            .as_slice();
        let mutated = mutate(&mut fuzzer.rng, seed);
        fuzzer.exercise(&mutated, true);
    }

    assert_eq!(
        fuzzer.checks,
        seeds.len() + RANDOM_BUFFERS + MUTATIONS,
        "every scheduled buffer was checked"
    );
    assert!(
        fuzzer.accepted_any,
        "no input in the whole run decoded to Some"
    );

    let accepted_files = fuzzer.save_accepted();
    write_manifest(
        &dir,
        &seeds,
        &seed_files,
        &fuzzer.accepted,
        &accepted_files,
        fuzzer.checks,
    );

    println!(
        "fuzz_bytes: checked {} buffers ({} random, {} mutations); saved {} seed and {} accepted entries under {}",
        fuzzer.checks,
        RANDOM_BUFFERS,
        MUTATIONS,
        seeds.len(),
        fuzzer.accepted.len(),
        dir.display()
    );
}
