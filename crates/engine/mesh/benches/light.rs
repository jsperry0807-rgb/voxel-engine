//! M2-17: full-pass versus incremental block light on a border fixture.
//!
//! The fixture is a roofed chunk with a sky shaft and a torch in the one-cell
//! apron, so both channels enter across a chunk border. "Full" is
//! [`full_pass_at`]; "incremental" is one emitter addition through
//! [`LightState::apply_edit`]. The benchmark reports p95 for each and the ratio
//! the task's 20% limit is written against.
//!
//! Removals are not part of the ratio: at the production 4³/8³ light grids the
//! 15-cell removal front always reaches a face, so M2-16 escalates every removal
//! to a full relight. The benchmark asserts that the measured edit stays on the
//! incremental path.
//!
//! Measured 2026-10-29: incremental is ~25% of full at 64³ and ~31% at 128³ by
//! the criterion mean. That misses the 20% task limit because the edit raises
//! light in 21%/41% of the padded cells; the full result and the decision are in
//! `docs/m2-light-cost.md`.
//!
//! Set `LIGHT_PROFILE=1` and pass `--profile-time 10` to write a pprof flamegraph
//! under `target/criterion/light_edge_<edge>/<bench>/profile/flamegraph.svg`.
//! `docs/profile/m2-17-20pct.html` is the committed report.

use std::time::{Duration, Instant};

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use pprof::criterion::{Output, PProfProfiler};
use voxel_mesh::light::{BlockLight, CellEdit, LightFill, LightSource, LightState, full_pass_at};

/// Edges to measure: the 64³ chunk's 4³ light grid and the 128³ chunk's 8³.
const EDGES: [usize; 2] = [4, 8];
/// Samples for the p95 estimate; enough to be stable and still quick.
const SAMPLES: usize = 10_000;

/// A roofed chunk whose light arrives from a torch in the neighbour's apron.
struct BorderWorld {
    edge: i32,
}

impl BorderWorld {
    fn new(edge: usize) -> Self {
        Self { edge: edge as i32 }
    }
}

impl LightSource for BorderWorld {
    fn block(&self, x: i32, y: i32, z: i32) -> BlockLight {
        let edge = self.edge;
        // Floor and roof, with one open shaft at x = z = 0.
        if y <= 0 {
            return BlockLight::OPAQUE;
        }
        if y == edge - 1 && !(x == 0 && z == 0) {
            return BlockLight::OPAQUE;
        }
        // A torch in the neighbour's apron: the light crosses the border.
        if x == -1 && y == edge / 2 && z == edge / 2 {
            return BlockLight::emitter(15);
        }
        // An interior pillar, so propagation is not a bare box.
        if x == edge / 2 && z == 0 && y < edge - 1 {
            return BlockLight::OPAQUE;
        }
        BlockLight::AIR
    }

    fn sky(&self, x: i32, y: i32, z: i32) -> bool {
        // The open shaft, plus a seed in the neighbour's apron.
        (x == 0 && z == 0 && y > 0) || (x == -1 && y == self.edge - 1)
    }
}

/// One non-removal edit: add an emitter at the centre.
fn centre_edit(edge: usize) -> CellEdit {
    let middle = (edge / 2) as i32;
    CellEdit::new(middle, middle, middle, BlockLight::emitter(15), false)
}

/// The same world with the benchmark edit applied, for the equality check.
struct EditedWorld<'a> {
    base: &'a BorderWorld,
    edit: CellEdit,
}

impl LightSource for EditedWorld<'_> {
    fn block(&self, x: i32, y: i32, z: i32) -> BlockLight {
        if (x, y, z) == (self.edit.x, self.edit.y, self.edit.z) {
            self.edit.block
        } else {
            self.base.block(x, y, z)
        }
    }

    fn sky(&self, x: i32, y: i32, z: i32) -> bool {
        if (x, y, z) == (self.edit.x, self.edit.y, self.edit.z) {
            self.edit.sky
        } else {
            self.base.sky(x, y, z)
        }
    }
}

fn quantile(samples: &mut [Duration], fraction: f64) -> Duration {
    samples.sort_unstable();
    let rank = (samples.len() as f64 * fraction).ceil() as usize;
    samples[rank.saturating_sub(1).min(samples.len() - 1)]
}

fn full_stats(edge: usize, world: &BorderWorld, samples: usize) -> (Duration, Duration) {
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        black_box(full_pass_at(edge, [0, 0, 0], world));
        durations.push(start.elapsed());
    }
    (
        quantile(&mut durations, 0.50),
        quantile(&mut durations, 0.95),
    )
}

/// One edit's measured cost and the number of padded cells it changes.
fn edit_stats(
    edge: usize,
    world: &BorderWorld,
    pristine: &LightState,
    initial: &LightFill,
    edit: CellEdit,
    samples: usize,
) -> (Duration, Duration, usize) {
    let mut state = pristine.clone();
    state.apply_edit(edit);
    assert_eq!(
        state.full_relights(),
        0,
        "matrix edit at ({}, {}, {}) must stay incremental",
        edit.x,
        edit.y,
        edit.z
    );
    let edited = full_pass_at(edge, [0, 0, 0], &EditedWorld { base: world, edit });
    assert_matches_full(&state, &edited, edge);

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        state.clone_from(pristine);
        let start = Instant::now();
        state.apply_edit(edit);
        durations.push(start.elapsed());
    }
    let p50 = quantile(&mut durations, 0.50);
    let p95 = quantile(&mut durations, 0.95);

    let mut changed = 0;
    let last = edge as i32;
    for z in -1..=last {
        for y in -1..=last {
            for x in -1..=last {
                if initial.with_apron(x, y, z) != edited.with_apron(x, y, z) {
                    changed += 1;
                }
            }
        }
    }
    (p50, p95, changed)
}

/// Prints the edit matrix as CSV. Enabled with `LIGHT_MATRIX=1`.
fn matrix(edge: usize) {
    let world = BorderWorld::new(edge);
    let initial = full_pass_at(edge, [0, 0, 0], &world);
    let pristine = LightState::new(
        edge,
        |x, y, z| world.block(x, y, z),
        |x, y, z| world.sky(x, y, z),
    );
    let samples = 2_000;
    let (full_p50, full_p95) = full_stats(edge, &world, samples);
    let middle = (edge / 2) as i32;
    let edits = [
        ("centre", centre_edit(edge)),
        (
            "corner",
            CellEdit::new(1, 1, 1, BlockLight::emitter(15), false),
        ),
        (
            "border_side",
            CellEdit::new(0, middle, middle, BlockLight::emitter(15), false),
        ),
        (
            "behind_pillar",
            CellEdit::new(middle, 1, middle, BlockLight::emitter(15), false),
        ),
        ("sky_gain", CellEdit::new(-1, 1, 1, BlockLight::AIR, true)),
        (
            "opacity_drop",
            CellEdit::new(middle, middle, 0, BlockLight::new(0, 4), false),
        ),
    ];

    println!(
        "edge,label,x,y,z,kind,changed,total,full_p50_us,full_p95_us,inc_p50_us,inc_p95_us,ratio_p50,ratio_p95"
    );
    for (label, edit) in edits {
        let (p50, p95, changed) = edit_stats(edge, &world, &pristine, &initial, edit, samples);
        let total = (edge + 2).pow(3);
        let ratio_p50 = p50.as_secs_f64() / full_p50.as_secs_f64() * 100.0;
        let ratio_p95 = p95.as_secs_f64() / full_p95.as_secs_f64() * 100.0;
        println!(
            "{edge},{label},{},{},{},{},{changed},{total},{:.3},{:.3},{:.3},{:.3},{ratio_p50:.1},{ratio_p95:.1}",
            edit.x,
            edit.y,
            edit.z,
            match (edit.block.emission, edit.block.opacity, edit.sky) {
                (0, 0, true) => "sky",
                (0, _, _) => "opacity",
                _ => "emitter",
            },
            full_p50.as_secs_f64() * 1e6,
            full_p95.as_secs_f64() * 1e6,
            p50.as_secs_f64() * 1e6,
            p95.as_secs_f64() * 1e6,
        );
    }
    let _ = pristine;
}

fn assert_matches_full(state: &LightState, full: &LightFill, edge: usize) {
    let last = edge as i32;
    for z in -1..=last {
        for y in -1..=last {
            for x in -1..=last {
                let got = state.with_apron(x, y, z);
                let expected = full.with_apron(x, y, z);
                assert_eq!(got.sky_level(), expected.sky_level(), "sky ({x}, {y}, {z})");
                assert_eq!(
                    got.block_level(),
                    expected.block_level(),
                    "block ({x}, {y}, {z})"
                );
            }
        }
    }
}

fn measure(edge: usize) -> (Duration, Duration) {
    let world = BorderWorld::new(edge);
    let edit = centre_edit(edge);

    let initial = full_pass_at(edge, [0, 0, 0], &world);
    let mut state = LightState::new(
        edge,
        |x, y, z| world.block(x, y, z),
        |x, y, z| world.sky(x, y, z),
    );
    let pristine = state.clone();
    assert_matches_full(&state, &initial, edge);
    state.apply_edit(edit);
    assert_eq!(
        state.full_relights(),
        0,
        "edge {edge}: the measured single edit must stay incremental"
    );
    let edited = full_pass_at(edge, [0, 0, 0], &EditedWorld { base: &world, edit });
    assert_matches_full(&state, &edited, edge);

    let mut full = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        black_box(full_pass_at(edge, [0, 0, 0], &world));
        full.push(start.elapsed());
    }

    // `clone_from` restores the four arrays without allocating, so the timed
    // section is just the edit and the profile is not dominated by setup.
    let mut incremental = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        state.clone_from(&pristine);
        let start = Instant::now();
        state.apply_edit(edit);
        incremental.push(start.elapsed());
        black_box(&state);
    }

    let full_p50 = quantile(&mut full, 0.50);
    let full_p95 = quantile(&mut full, 0.95);
    let incremental_p50 = quantile(&mut incremental, 0.50);
    let incremental_p95 = quantile(&mut incremental, 0.95);
    let p50_ratio = incremental_p50.as_secs_f64() / full_p50.as_secs_f64() * 100.0;
    let p95_ratio = incremental_p95.as_secs_f64() / full_p95.as_secs_f64() * 100.0;
    let mut changed = 0;
    let last = edge as i32;
    for z in -1..=last {
        for y in -1..=last {
            for x in -1..=last {
                if initial.with_apron(x, y, z) != edited.with_apron(x, y, z) {
                    changed += 1;
                }
            }
        }
    }
    println!(
        "M2-17 edge={edge}: full p50 {full_p50:?} p95 {full_p95:?}; \
         incremental p50 {incremental_p50:?} p95 {incremental_p95:?}; \
         ratio p50 {p50_ratio:.1}% p95 {p95_ratio:.1}%; \
         changed {changed}/{} cells",
        (edge + 2).pow(3)
    );
    (full_p95, incremental_p95)
}

fn light_cost(c: &mut Criterion) {
    if std::env::var_os("LIGHT_MATRIX").is_some() {
        for edge in EDGES {
            matrix(edge);
        }
        return;
    }
    for edge in EDGES {
        // The manual p95 sweep runs both paths; skip it when profiling a single
        // criterion benchmark.
        if std::env::var_os("LIGHT_SKIP_P95").is_none() {
            measure(edge);
        }

        let world = BorderWorld::new(edge);
        let edit = centre_edit(edge);
        // `clone_from` reuses the four arrays; the timed region is the edit and
        // the profile is not dominated by state construction or allocation.
        let mut state = LightState::new(
            edge,
            |x, y, z| world.block(x, y, z),
            |x, y, z| world.sky(x, y, z),
        );
        let pristine = state.clone();
        let mut group = c.benchmark_group(format!("light/edge_{edge}"));
        group.bench_function("full_pass", |b| {
            b.iter(|| black_box(full_pass_at(edge, [0, 0, 0], &world)))
        });
        group.bench_function("incremental_edit", |b| {
            b.iter_custom(|iterations| {
                let mut total = Duration::ZERO;
                for _ in 0..iterations {
                    state.clone_from(&pristine);
                    let start = Instant::now();
                    state.apply_edit(edit);
                    total += start.elapsed();
                    black_box(&state);
                }
                total
            })
        });
        group.finish();
    }
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = light_cost
}
criterion_main!(benches);

/// Attaches the pprof sampler only when a flamegraph is requested, so the
/// recorded timing runs are not perturbed by SIGPROF.
fn criterion_config() -> Criterion {
    let config = Criterion::default();
    if std::env::var_os("LIGHT_PROFILE").is_some() {
        config.with_profiler(PProfProfiler::new(997, Output::Flamegraph(None)))
    } else {
        config
    }
}
