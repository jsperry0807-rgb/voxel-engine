//! M2.5-01: golden noise vectors and cross-thread determinism.
//!
//! The golden file pins every sample the generation path will depend on:
//! 2D/3D gradient noise and their fractal sums, for three seeds, over a grid
//! that spans fractional, negative and world-edge coordinates. Each section
//! stores an FNV-1a 64 hash and its first 16 samples, so a mismatch names the
//! section, the sample index and both values rather than only a hash.
//!
//! Regenerate after an intentional noise change (and say so in the task docs):
//!
//! ```sh
//! UPDATE_GOLDEN=1 cargo test -p voxel-worldgen noise
//! cargo test -p voxel-worldgen noise
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use voxel_worldgen::fixed::{Coord, ONE, coord_from_f32};
use voxel_worldgen::noise::Noise;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const HEADER_SAMPLES: usize = 16;

/// One named golden section: every sample, hashed, plus a readable prefix.
struct Section {
    name: String,
    samples: Vec<i32>,
}

/// A 2D grid that covers fractions, both signs and far-out world coordinates.
fn grid_2d(count: usize) -> Vec<(Coord, Coord)> {
    (0..count)
        .map(|i| {
            let i = i as i64;
            // ±16 m, then a separate world-edge sweep appended below.
            let x = (i * 0x1_9A37) % 0x20_0000 - 0x10_0000;
            let y = (i * i * 0x9E37) % 0x40_0000 - 0x20_0000;
            (x, y)
        })
        .collect()
}

/// A 3D grid with the same coverage as [`grid_2d`].
fn grid_3d(count: usize) -> Vec<(Coord, Coord, Coord)> {
    (0..count)
        .map(|i| {
            let i = i as i64;
            let x = (i * 0x1_9A37) % 0x20_0000 - 0x10_0000;
            let y = (i * i * 0x9E37) % 0x40_0000 - 0x20_0000;
            let z = (i * i * i * 0x5BD1) % 0x80_0000 - 0x40_0000;
            (x, y, z)
        })
        .collect()
}

/// The 1,024 km edges of the world, with fractional offsets so the samples are
/// not sitting on lattice points (where gradient noise is exactly zero).
fn world_edge_points() -> Vec<(Coord, Coord)> {
    let edge = coord_from_f32(1_024_000.0);
    let offsets = [0.5, -0.75, 3.25, -2.5, 11.125, -7.0625];
    let mut points = Vec::new();
    for (index, &offset) in offsets.iter().enumerate() {
        let signed = if index % 2 == 0 { edge } else { -edge };
        points.push((
            signed + coord_from_f32(offset),
            edge + coord_from_f32(-offset),
        ));
    }
    points.push((edge + coord_from_f32(0.5), -edge + coord_from_f32(-3.25)));
    points.push((-edge + coord_from_f32(9.75), edge + coord_from_f32(1.5)));
    points
}

fn sections() -> Vec<Section> {
    let mut sections = Vec::new();
    let points_2d = grid_2d(128);
    let points_3d = grid_3d(128);

    for seed in [0u64, 1, 0xDEAD_BEEF_CAFE_BABE] {
        let noise = Noise::new(seed);

        sections.push(Section {
            name: format!("value_2d/seed={seed}"),
            samples: points_2d
                .iter()
                .map(|&(x, y)| noise.value_2d(x, y))
                .collect(),
        });
        sections.push(Section {
            name: format!("value_3d/seed={seed}"),
            samples: points_3d
                .iter()
                .map(|&(x, y, z)| noise.value_3d(x, y, z))
                .collect(),
        });
        sections.push(Section {
            name: format!("fbm_2d/seed={seed}"),
            samples: points_2d
                .iter()
                .map(|&(x, y)| noise.fbm_2d(x, y, 4, 2 * ONE, ONE / 2))
                .collect(),
        });
        sections.push(Section {
            name: format!("fbm_3d/seed={seed}"),
            samples: points_3d
                .iter()
                .map(|&(x, y, z)| noise.fbm_3d(x, y, z, 3, 2 * ONE, ONE / 2))
                .collect(),
        });
    }

    let noise = Noise::new(7);
    sections.push(Section {
        name: "value_2d/world_edge".to_owned(),
        samples: world_edge_points()
            .iter()
            .map(|&(x, y)| noise.value_2d(x, y))
            .collect(),
    });
    sections.push(Section {
        name: "value_3d/world_edge".to_owned(),
        samples: world_edge_points()
            .iter()
            .map(|&(x, y)| noise.value_3d(x, y, -x))
            .collect(),
    });

    sections
}

fn fnv1a(samples: &[i32]) -> u64 {
    let mut hash = FNV_OFFSET;
    for sample in samples {
        for byte in sample.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("noise.txt")
}

fn format_section(section: &Section) -> String {
    let mut line = format!("{} {:016x}", section.name, fnv1a(&section.samples));
    for sample in section.samples.iter().take(HEADER_SAMPLES) {
        line.push(' ');
        line.push_str(&sample.to_string());
    }
    line.push('\n');
    line
}

fn parse_golden(text: &str) -> BTreeMap<String, (u64, Vec<i32>)> {
    let mut parsed = BTreeMap::new();
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let name = parts.next().expect("section name").to_owned();
        let hash = u64::from_str_radix(parts.next().expect("section hash"), 16)
            .unwrap_or_else(|_| panic!("line {}: bad hash", line_number + 1));
        let samples = parts
            .map(|part| {
                part.parse::<i32>()
                    .unwrap_or_else(|_| panic!("line {}: bad sample {part}", line_number + 1))
            })
            .collect();
        parsed.insert(name, (hash, samples));
    }
    parsed
}

#[test]
fn noise_matches_committed_golden_vectors() {
    let sections = sections();
    let path = golden_path();

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let mut text = String::from(
            "# M2.5-01 noise golden vectors.\n\
             # Regenerate with UPDATE_GOLDEN=1 cargo test -p voxel-worldgen noise\n\
             # name fnv1a64 sample0 .. sample15\n",
        );
        for section in &sections {
            text.push_str(&format_section(section));
        }
        std::fs::create_dir_all(path.parent().expect("golden directory"))
            .expect("create golden dir");
        std::fs::write(&path, text).expect("write golden file");
        eprintln!("wrote {}", path.display());
        return;
    }

    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let golden = parse_golden(&text);

    for section in &sections {
        let expected = golden
            .get(&section.name)
            .unwrap_or_else(|| panic!("golden section {:?} is missing", section.name));
        let actual_hash = fnv1a(&section.samples);
        if expected.0 != actual_hash {
            let index = expected
                .1
                .iter()
                .zip(&section.samples)
                .position(|(want, got)| want != got);
            let detail = index
                .map(|index| {
                    format!(
                        "first difference at sample {index}: golden {} != current {}",
                        expected.1[index], section.samples[index]
                    )
                })
                .unwrap_or_else(|| "the stored prefix agrees; a later sample differs".to_owned());
            panic!(
                "golden mismatch in {:?}: {detail}; \
                 regenerate with UPDATE_GOLDEN=1 if the change is intentional",
                section.name
            );
        }
        assert!(
            section.samples.len() >= expected.1.len(),
            "golden section {:?} stores more samples than the test generates",
            section.name
        );
    }

    for name in golden.keys() {
        assert!(
            sections.iter().any(|section| &section.name == name),
            "golden section {name:?} is no longer generated"
        );
    }
}

/// The goldens must not be the only source of truth: the same bytes have to
/// come out when the work is split across 1, 2 or 8 threads.
#[test]
fn noise_is_identical_across_thread_counts() {
    let noise = Noise::new(0x5EED);
    let points = grid_3d(600);

    let single: Vec<i32> = points
        .iter()
        .map(|&(x, y, z)| noise.value_3d(x, y, z) ^ noise.value_2d(x, y))
        .collect();

    for threads in [1usize, 2, 8] {
        let chunk = points.len().div_ceil(threads);
        let mut gathered = Vec::with_capacity(points.len());
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for slice in points.chunks(chunk.max(1)) {
                handles.push(scope.spawn(move || {
                    slice
                        .iter()
                        .map(|&(x, y, z)| noise.value_3d(x, y, z) ^ noise.value_2d(x, y))
                        .collect::<Vec<i32>>()
                }));
            }
            for handle in handles {
                gathered.extend(handle.join().expect("worker panicked"));
            }
        });
        assert_eq!(single, gathered, "{threads} thread(s) diverged");
    }
}

/// Sampling order must not matter: grouped calls and interleaved calls produce
/// the same values, so no hidden mutable state exists in the field.
#[test]
fn noise_is_independent_of_call_order() {
    let noise = Noise::new(42);
    let points = grid_3d(256);
    let grouped: Vec<i32> = points
        .iter()
        .map(|&(x, y, z)| noise.value_2d(x, y) ^ noise.value_3d(x, y, z))
        .collect();

    let mut twos = Vec::with_capacity(points.len());
    let mut threes = Vec::with_capacity(points.len());
    for &(x, y, z) in &points {
        twos.push(noise.value_2d(x, y));
        threes.push(noise.value_3d(x, y, z));
    }
    let interleaved: Vec<i32> = twos
        .into_iter()
        .zip(threes)
        .map(|(two, three)| two ^ three)
        .collect();

    assert_eq!(grouped, interleaved);
}
