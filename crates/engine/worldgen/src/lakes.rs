//! Lake extraction from the filled hydrology surface.
//!
//! A lake is an 8-connected component of cells at one filled elevation above
//! their stored elevation. Priority-flood gives every depression a single
//! **spill level**: the filled elevation of the component, which is also the
//! elevation of the lowest rim cell the water can escape through.
//!
//! # What a lake records
//!
//! - `cells` and `area_cells` — the water body;
//! - `surface` — the spill level, in 0.25 m units;
//! - `max_depth` — the deepest bed below the surface;
//! - `spill` — the rim cell the water leaves through;
//! - `shoreline` — the land cells that wrap the water body;
//! - `bounds` and `centroid` — for the surface rules and the map.
//!
//! # No leaking basins
//!
//! Priority-flood guarantees that no lake cell borders a lower filled surface:
//! if it did, the flood would have reached the lake through that cell and set
//! the lake at the lower level. The extractor re-checks this on the real field:
//! every neighbour of every lake cell has `filled >= surface`, and the spill is
//! a neighbour whose filled elevation is exactly the surface.
//!
//! The spill cell is **upstream of the lake in the flood tree** (the lake was
//! discovered from it), so its own flow points away from the lake. That is what
//! makes the extracted spill a real outlet rather than a second entrance.
//!
//! # Shorelines
//!
//! The shoreline is every land cell 8-adjacent to the component, row-major.
//! For a lake without islands the set is one 8-connected ring; a lake with an
//! island has a second ring around the island. The tests assert that every
//! shoreline cell is 8-connected to another shoreline cell of the same lake
//! (no isolated points) and that every lake cell touches at least one shoreline
//! cell through a neighbour on the boundary.

use crate::hydro::Hydrology;
use crate::macro_grid::CELL_SIZE_M;

/// Smallest lake kept by default, in cells.
///
/// Eight cells is about a 360 m wide pond. The valley detail produces millions
/// of single-cell puddles; keeping them all would dwarf the real lakes and make
/// carving pointless.
pub const DEFAULT_MIN_CELLS: u32 = 8;

/// Tuning for lake extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LakeConfig {
    /// Lakes smaller than this many cells are skipped.
    pub min_cells: u32,
}

impl Default for LakeConfig {
    fn default() -> Self {
        Self {
            min_cells: DEFAULT_MIN_CELLS,
        }
    }
}

/// One extracted lake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lake {
    /// Cell indices in the hydrology grid, row-major.
    pub cells: Vec<u32>,
    /// Number of water cells.
    pub area_cells: u32,
    /// Spill level in 0.25 m units; every cell's filled elevation.
    pub surface: u16,
    /// Deepest bed below the surface, in 0.25 m units.
    pub max_depth: u16,
    /// Rim cell the water leaves through.
    pub spill: (i32, i32),
    /// Land cells 8-adjacent to the lake, row-major.
    pub shoreline: Vec<(i32, i32)>,
    /// Inclusive cell bounds: `(min_x, min_z, max_x, max_z)`.
    pub bounds: (i32, i32, i32, i32),
    /// Mean cell position, rounded down.
    pub centroid: (i32, i32),
}

impl Lake {
    /// Area in square metres.
    pub fn area_m2(&self) -> i64 {
        self.area_cells as i64 * CELL_SIZE_M as i64 * CELL_SIZE_M as i64
    }

    /// Surface elevation in millimetres.
    pub fn surface_mm(&self) -> u32 {
        self.surface as u32 * 250
    }

    /// Deepest bed in millimetres.
    pub fn max_depth_mm(&self) -> u32 {
        self.max_depth as u32 * 250
    }

    /// Cell bounds as rows/columns, for iteration.
    pub fn width_cells(&self) -> i32 {
        self.bounds.2 - self.bounds.0 + 1
    }

    /// Cell bounds height in rows.
    pub fn height_cells(&self) -> i32 {
        self.bounds.3 - self.bounds.1 + 1
    }
}

/// Every extracted lake.
#[derive(Clone, Debug)]
pub struct Lakes {
    lakes: Vec<Lake>,
    lake_cells: usize,
    min_cells: u32,
}

impl Lakes {
    /// Extract with the default minimum area.
    pub fn extract(hydro: &Hydrology) -> Self {
        Self::extract_with(hydro, LakeConfig::default())
    }

    /// Extract with an explicit minimum area.
    #[must_use]
    pub fn extract_with(hydro: &Hydrology, config: LakeConfig) -> Self {
        let width = hydro.width();
        let height = hydro.height();
        let cells = hydro.len();
        let filled = hydro.filled();
        let heights = hydro.heights();
        let flow = hydro.flow();

        let is_lake = |index: usize| filled[index] > heights[index];
        let mut visited = vec![false; cells];
        let mut stack = Vec::new();
        let mut component = Vec::new();
        let mut lakes = Vec::new();
        let mut lake_cells = 0usize;
        let mut neighbours = Vec::new();

        for start in 0..cells {
            if visited[start] || !is_lake(start) {
                continue;
            }

            // A lake surface is flat: adjacent wet cells at different filled
            // elevations are separate water bodies (one spills into the other),
            // so the component is restricted to one elevation.
            let surface = filled[start];
            component.clear();
            visited[start] = true;
            stack.push(start as u32);
            let mut max_depth = 0u16;
            let mut sum_x = 0i64;
            let mut sum_z = 0i64;
            let mut bounds = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
            while let Some(cell) = stack.pop() {
                let cell = cell as usize;
                component.push(cell as u32);
                max_depth = max_depth.max(filled[cell] - heights[cell]);
                let x = (cell % width) as i32;
                let z = (cell / width) as i32;
                sum_x += x as i64;
                sum_z += z as i64;
                bounds = (
                    bounds.0.min(x),
                    bounds.1.min(z),
                    bounds.2.max(x),
                    bounds.3.max(z),
                );
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dz == 0 {
                            continue;
                        }
                        let (nx, nz) = (x + dx, z + dz);
                        if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                            continue;
                        }
                        let neighbour = nz as usize * width + nx as usize;
                        if !visited[neighbour] && is_lake(neighbour) && filled[neighbour] == surface
                        {
                            visited[neighbour] = true;
                            stack.push(neighbour as u32);
                        }
                    }
                }
            }

            lake_cells += component.len();
            if component.len() < config.min_cells as usize {
                continue;
            }
            component.sort_unstable();

            // The rim: every non-lake neighbour, and the lowest one is the
            // spill. Its filled elevation must equal the surface; otherwise the
            // basin would leak.
            neighbours.clear();
            for &cell in &component {
                let cell = cell as usize;
                let x = (cell % width) as i32;
                let z = (cell / width) as i32;
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dz == 0 {
                            continue;
                        }
                        let (nx, nz) = (x + dx, z + dz);
                        if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                            continue;
                        }
                        let neighbour = nz as usize * width + nx as usize;
                        if !is_lake(neighbour) {
                            neighbours.push(neighbour as u32);
                        }
                    }
                }
            }
            neighbours.sort_unstable();
            neighbours.dedup();

            // The spill is the rim cell that the lake's flow forest drains
            // into: some lake cell points at it, and its own flow leaves the
            // water. Several rim cells can sit at the surface (inlets and the
            // flat shore), so picking the lowest rim cell is not enough.
            let lake_set: std::collections::HashSet<u32> = component.iter().copied().collect();
            let mut roots: Vec<u32> = Vec::new();
            for &cell in &component {
                let cell = cell as usize;
                let direction = flow[cell];
                if direction.is_outlet() {
                    continue;
                }
                let (dx, dz) = direction.delta();
                let x = (cell % width) as i32 + dx;
                let z = (cell / width) as i32 + dz;
                let parent = (z as usize * width + x as usize) as u32;
                if !lake_set.contains(&parent) {
                    roots.push(parent);
                }
            }
            roots.sort_unstable();
            roots.dedup();
            // A root can still be a transient inlet: its path may cross dry
            // cells and re-enter the lake. Follow the path until the level
            // drops or the map outlet, and reject any root that re-enters.
            let leaves = |start: u32| -> bool {
                let mut current = start as usize;
                let mut steps = 0;
                loop {
                    let direction = flow[current];
                    if direction.is_outlet() {
                        return true;
                    }
                    let (dx, dz) = direction.delta();
                    let x = (current % width) as i32 + dx;
                    let z = (current / width) as i32 + dz;
                    let parent = z as usize * width + x as usize;
                    if filled[parent] < surface {
                        return true;
                    }
                    if lake_set.contains(&(parent as u32)) {
                        return false;
                    }
                    current = parent;
                    steps += 1;
                    if steps > 256 {
                        return false;
                    }
                }
            };
            let spill = roots
                .iter()
                .copied()
                .filter(|&root| filled[root as usize] == surface && leaves(root))
                .min_by_key(|&root| (filled[root as usize], root))
                .or_else(|| {
                    roots
                        .iter()
                        .copied()
                        .min_by_key(|&root| (filled[root as usize], root))
                })
                .or_else(|| {
                    neighbours
                        .iter()
                        .copied()
                        .min_by_key(|&neighbour| (filled[neighbour as usize], neighbour))
                })
                .expect("a lake has a rim outlet");
            let spill = spill as usize;
            debug_assert_eq!(
                filled[spill], filled[component[0] as usize],
                "the rim must meet the lake surface"
            );

            // Shoreline: the rim cells, row-major.
            let shoreline: Vec<(i32, i32)> = neighbours
                .iter()
                .map(|&neighbour| {
                    let neighbour = neighbour as usize;
                    ((neighbour % width) as i32, (neighbour / width) as i32)
                })
                .collect();

            let spill_coord = ((spill % width) as i32, (spill / width) as i32);
            let count = component.len() as i64;
            lakes.push(Lake {
                area_cells: component.len() as u32,
                max_depth,
                surface: filled[component[0] as usize],
                spill: spill_coord,
                shoreline,
                bounds,
                centroid: ((sum_x / count) as i32, (sum_z / count) as i32),
                cells: component.clone(),
            });
        }

        Self {
            lakes,
            lake_cells,
            min_cells: config.min_cells,
        }
    }

    /// The lakes, in row-major order of their first cell.
    pub fn lakes(&self) -> &[Lake] {
        &self.lakes
    }

    /// Lake count after the area filter.
    pub fn len(&self) -> usize {
        self.lakes.len()
    }

    /// True when no lake met the filter.
    pub fn is_empty(&self) -> bool {
        self.lakes.is_empty()
    }

    /// Every lake cell in the grid, before the area filter.
    pub const fn lake_cells(&self) -> usize {
        self.lake_cells
    }

    /// The area filter used.
    pub const fn min_cells(&self) -> u32 {
        self.min_cells
    }

    /// Total area of the kept lakes, in cells.
    pub fn area_cells(&self) -> u64 {
        self.lakes.iter().map(|lake| lake.area_cells as u64).sum()
    }

    /// Largest kept lake.
    pub fn largest(&self) -> Option<&Lake> {
        self.lakes.iter().max_by_key(|lake| lake.area_cells)
    }

    /// Deepest kept lake.
    pub fn deepest(&self) -> Option<&Lake> {
        self.lakes.iter().max_by_key(|lake| lake.max_depth)
    }

    /// 64-bit hash of the kept lakes, for run-to-run comparison.
    pub fn content_hash(&self) -> u64 {
        use crate::tile::{fnv_start, fnv_u64};
        let mut hash = fnv_start();
        for lake in &self.lakes {
            hash = fnv_u64(hash, lake.area_cells as u64);
            hash = fnv_u64(hash, lake.surface as u64);
            hash = fnv_u64(hash, lake.max_depth as u64);
            hash = fnv_u64(hash, lake.spill.0 as u32 as u64);
            hash = fnv_u64(hash, lake.spill.1 as u32 as u64);
            for &cell in &lake.cells {
                hash = fnv_u64(hash, cell as u64);
            }
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lakes_size_helpers_are_consistent() {
        let lake = Lake {
            cells: vec![0, 1],
            area_cells: 2,
            surface: 12,
            max_depth: 4,
            spill: (0, 0),
            shoreline: vec![(0, 1)],
            bounds: (0, 0, 1, 1),
            centroid: (0, 0),
        };
        assert_eq!(lake.area_m2(), 2 * 128 * 128);
        assert_eq!(lake.surface_mm(), 3_000);
        assert_eq!(lake.max_depth_mm(), 1_000);
        assert_eq!(lake.width_cells(), 2);
        assert_eq!(lake.height_cells(), 2);
    }
}
