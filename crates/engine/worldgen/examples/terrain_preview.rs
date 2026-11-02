//! Render the macro terrain to a PNG preview.
//!
//! ```text
//! cargo run -p voxel-worldgen --release --example terrain_preview -- \
//!     [--seed N] [--size PX] [--out PATH]
//! ```
//!
//! The preview builds the full 8,000 × 8,000 cell macro heightfield, runs the
//! priority-flood hydrology pass, and rasterises it with hypsometric colours,
//! hill shading, rivers and lakes. `--size` is the output edge in pixels; the
//! default 4,096 is the design's 4k preview (M2.5-27), roughly 250 m per pixel.

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::time::Instant;

use voxel_worldgen::generator::{MacroGenerator, SEA_LEVEL_UNITS};
use voxel_worldgen::hydro::Hydrology;
use voxel_worldgen::macro_grid::{CELL_SIZE_M, WORLD_CELLS, height_from_q16};
use voxel_worldgen::rivers::RiverConfig;
use voxel_worldgen::splines::SEA_LEVEL_M;

fn main() -> Result<(), Box<dyn Error>> {
    let mut seed = 1u64;
    let mut size = 4_096usize;
    let mut out: Option<PathBuf> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--seed" => {
                seed = args.get(index + 1).ok_or("--seed needs a value")?.parse()?;
                index += 1;
            }
            "--size" => {
                size = args.get(index + 1).ok_or("--size needs a value")?.parse()?;
                index += 1;
            }
            "--out" => {
                out = Some(PathBuf::from(
                    args.get(index + 1).ok_or("--out needs a value")?,
                ));
                index += 1;
            }
            other => return Err(format!("unknown argument {other:?}").into()),
        }
        index += 1;
    }
    if !(64..=8_192).contains(&size) {
        return Err("--size must be 64..=8192".into());
    }
    let out = out.unwrap_or_else(|| PathBuf::from(format!("terrain-{seed}.png")));

    let cells = WORLD_CELLS as usize;
    println!("building {cells} × {cells} heightfield (seed {seed})…");
    let started = Instant::now();
    let generator = MacroGenerator::new(seed);
    let half_cell = CELL_SIZE_M as i64 / 2;
    let mut heights = Vec::with_capacity(cells * cells);
    for z in 0..cells {
        for x in 0..cells {
            let world_x = (x as i64 * CELL_SIZE_M as i64 + half_cell) << 16;
            let world_z = (z as i64 * CELL_SIZE_M as i64 + half_cell) << 16;
            heights.push(height_from_q16(generator.surface(world_x, world_z)));
        }
    }
    println!("  heights in {:.1?}", started.elapsed());

    println!("running hydrology…");
    let started = Instant::now();
    let sea_level = SEA_LEVEL_UNITS;
    let hydro = Hydrology::compute(heights, cells, cells, sea_level);
    println!(
        "  hydrology in {:.1?}: lake cells {:.2}% max accumulation {}",
        started.elapsed(),
        100.0 * hydro.lake_cells() as f64 / hydro.len() as f64,
        hydro.max_accumulation()
    );

    let river_config = RiverConfig::default();
    let heights = hydro.heights();
    let filled = hydro.filled();
    let accumulation = hydro.accumulation();
    let deep_lake_units = river_config.min_terminal_lake_depth as i32;

    // Per-pixel elevation in metres, sampled from the macro heightfield.
    println!("rasterising {size} × {size}…");
    let started = Instant::now();
    let mut elevation = vec![0f32; size * size];
    let mut stats = Stats::default();
    for py in 0..size {
        let z = py * cells / size;
        for px in 0..size {
            let x = px * cells / size;
            let metres = heights[z * cells + x] as f32 * 0.25;
            elevation[py * size + px] = metres;
            stats.add(metres);
        }
    }

    let mut pixels = vec![0u8; size * size * 3];
    // Horizontal spacing between preview pixels, in metres.
    let spacing = (WORLD_CELLS as f32 * CELL_SIZE_M as f32) / size as f32;
    for py in 0..size {
        let z0 = py * cells / size;
        let z1 = (((py + 1) * cells).div_ceil(size)).min(cells);
        for px in 0..size {
            let x0 = px * cells / size;
            let x1 = (((px + 1) * cells).div_ceil(size)).min(cells);
            let metres = elevation[py * size + px];

            let mut lake_depth = 0i32;
            let mut river = false;
            for z in z0..z1 {
                for x in x0..x1 {
                    let index = z * cells + x;
                    let depth = filled[index] as i32 - heights[index] as i32;
                    if depth > 0 {
                        lake_depth = lake_depth.max(depth);
                    }
                    if accumulation[index] >= river_config.threshold && depth < deep_lake_units {
                        river = true;
                    }
                }
            }

            let mut colour = if metres <= SEA_LEVEL_M as f32 {
                ocean_colour(SEA_LEVEL_M as f32 - metres)
            } else {
                land_colour(metres)
            };

            if metres > SEA_LEVEL_M as f32 {
                let shade = shade_at(&elevation, size, px, py, spacing);
                for channel in &mut colour {
                    *channel *= shade;
                }
            }

            if lake_depth > 0 {
                colour = lake_colour(lake_depth as f32 * 0.25);
            }
            if river {
                // Blend so dense low-order channels read as texture rather
                // than drowning the land colours.
                colour = [
                    colour[0] * 0.30 + RIVER[0] * 0.70,
                    colour[1] * 0.30 + RIVER[1] * 0.70,
                    colour[2] * 0.30 + RIVER[2] * 0.70,
                ];
            }

            let offset = (py * size + px) * 3;
            pixels[offset] = to_u8(colour[0]);
            pixels[offset + 1] = to_u8(colour[1]);
            pixels[offset + 2] = to_u8(colour[2]);
        }
    }
    println!("  raster in {:.1?}", started.elapsed());
    stats.print();

    let file = File::create(&out)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, size as u32, size as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    println!("wrote {}", out.display());
    Ok(())
}

const RIVER: [f32; 3] = [0.28, 0.55, 0.86];

/// Deep water to shallow water, from depth below sea level in metres.
fn ocean_colour(depth: f32) -> [f32; 3] {
    const STOPS: &[(f32, [f32; 3])] = &[
        (0.0, [0.24, 0.52, 0.68]),
        (80.0, [0.16, 0.40, 0.62]),
        (600.0, [0.09, 0.26, 0.48]),
        (1_800.0, [0.04, 0.12, 0.30]),
    ];
    ramp(depth, STOPS)
}

/// Lake surface by depth, slightly lighter than the ocean shelf.
fn lake_colour(depth: f32) -> [f32; 3] {
    const STOPS: &[(f32, [f32; 3])] = &[
        (0.0, [0.36, 0.62, 0.76]),
        (40.0, [0.22, 0.48, 0.70]),
        (200.0, [0.14, 0.35, 0.58]),
    ];
    ramp(depth, STOPS)
}

/// Hypsometric land ramp, from absolute elevation in metres.
///
/// The shape splines put the sea at 6,600 m and the peak at 12,000 m, so the
/// whole land ramp lives in that band; the stops follow the measured
/// distribution rather than a sea-level-relative one.
fn land_colour(metres: f32) -> [f32; 3] {
    const STOPS: &[(f32, [f32; 3])] = &[
        (SEA_LEVEL_M as f32, [0.78, 0.72, 0.52]),
        (6_650.0, [0.52, 0.62, 0.34]),
        (6_900.0, [0.36, 0.52, 0.27]),
        (7_600.0, [0.31, 0.46, 0.25]),
        (8_600.0, [0.42, 0.42, 0.27]),
        (9_600.0, [0.50, 0.44, 0.37]),
        (10_600.0, [0.62, 0.60, 0.58]),
        (11_400.0, [0.88, 0.90, 0.92]),
        (12_000.0, [0.97, 0.98, 0.99]),
    ];
    ramp(metres, STOPS)
}

fn ramp(value: f32, stops: &[(f32, [f32; 3])]) -> [f32; 3] {
    if value <= stops[0].0 {
        return stops[0].1;
    }
    for pair in stops.windows(2) {
        let ((low, low_colour), (high, high_colour)) = (pair[0], pair[1]);
        if value <= high {
            let t = (value - low) / (high - low);
            return [
                low_colour[0] + (high_colour[0] - low_colour[0]) * t,
                low_colour[1] + (high_colour[1] - low_colour[1]) * t,
                low_colour[2] + (high_colour[2] - low_colour[2]) * t,
            ];
        }
    }
    stops[stops.len() - 1].1
}

/// Lambert shading from a north-west light, with a little ambient.
fn shade_at(height: &[f32], size: usize, px: usize, py: usize, spacing: f32) -> f32 {
    let west = height[py * size + px.saturating_sub(1)];
    let east = height[py * size + (px + 1).min(size - 1)];
    let north = height[py.saturating_sub(1) * size + px];
    let south = height[(py + 1).min(size - 1) * size + px];
    let exaggeration = 1.6;
    let dx = (east - west) * exaggeration / (2.0 * spacing);
    let dz = (south - north) * exaggeration / (2.0 * spacing);
    // Surface normal (-dx, -dz, 1), light from the north-west at 45°.
    let inverse = 1.0 / (dx * dx + dz * dz + 1.0).sqrt();
    let light = [
        -0.5f32 * std::f32::consts::SQRT_2,
        -0.5f32 * std::f32::consts::SQRT_2,
        0.5f32 * std::f32::consts::SQRT_2,
    ];
    let dot = (-dx * light[0] - dz * light[1] + light[2]) * inverse;
    (0.42 + 0.75 * dot.max(0.0)).clamp(0.35, 1.25)
}

fn to_u8(value: f32) -> u8 {
    (value * 255.0).clamp(0.0, 255.0) as u8
}

#[derive(Default)]
struct Stats {
    sampled: Vec<f32>,
    land_sampled: Vec<f32>,
    min: f32,
    max: f32,
    total: usize,
    land: usize,
}

impl Stats {
    fn add(&mut self, metres: f32) {
        if self.total.is_multiple_of(1_000) {
            self.sampled.push(metres);
        }
        if self.total == 0 || metres < self.min {
            self.min = metres;
        }
        if metres > self.max {
            self.max = metres;
        }
        if metres > SEA_LEVEL_M as f32 {
            self.land += 1;
            if self.total.is_multiple_of(1_000) {
                self.land_sampled.push(metres);
            }
        }
        self.total += 1;
    }

    fn print(&mut self) {
        self.sampled.sort_by(f32::total_cmp);
        let percentile = |p: f32| {
            let index = ((self.sampled.len() - 1) as f32 * p) as usize;
            self.sampled[index]
        };
        let land_fraction = self.land as f64 / self.total.max(1) as f64;
        println!(
            "  elevation m: min {:.0} p50 {:.0} p90 {:.0} p99 {:.0} max {:.0}",
            self.min,
            percentile(0.50),
            percentile(0.90),
            percentile(0.99),
            self.max
        );
        println!("  land fraction {:.1}%", land_fraction * 100.0);
        self.land_sampled.sort_by(f32::total_cmp);
        if !self.land_sampled.is_empty() {
            let land_percentile = |p: f32| {
                let index = ((self.land_sampled.len() - 1) as f32 * p) as usize;
                self.land_sampled[index]
            };
            println!(
                "  land elevation m: p50 {:.0} p90 {:.0} p99 {:.0} max {:.0}",
                land_percentile(0.50),
                land_percentile(0.90),
                land_percentile(0.99),
                self.land_sampled[self.land_sampled.len() - 1],
            );
        }
    }
}
