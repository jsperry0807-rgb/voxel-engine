//! River extraction from the hydrology pass.
//!
//! A river is a chain of macro cells whose flow accumulation exceeds a
//! threshold. [`RiverNetwork::extract`] turns the hydrology forest into
//! polyline **segments**: each segment runs from a headwater or a lake outflow
//! to the next confluence, lake or outlet. Segments share their junction cell,
//! so the network is a DAG that consumers (carving, the map, navigation) can
//! walk without duplicating the trunk.
//!
//! # Width and depth
//!
//! Width and depth come from the catchment size at each point through two
//! monotone tables: 10 m at the threshold up to 200 m for a continental river.
//! The design asks for 10–200 m flow-varying width, and the tables are clamped
//! to that range, so the channel always fits the carving rules.
//!
//! # Order
//!
//! Strahler order over the flow forest: a cell with two or more children of its
//! maximum order gains one, otherwise it keeps the maximum child order. River
//! cells are visited in ascending accumulation, which is a valid topological
//! order because accumulation strictly grows downstream.
//!
//! # Mouths and deltas
//!
//! A terminal segment ends at the sea, at a map edge (a border outlet above sea
//! level; the generated world's ocean rim makes this rare), or at a lake deep
//! enough to be a real water body. Shallow single-cell puddles stay part of the
//! channel. A sea mouth is flagged as a **delta** when the river has an order of
//! at least [`DELTA_MIN_ORDER`] and the last stretch is flatter than
//! [`DELTA_GRADIENT_MM_PER_M`] — the design's "deltas where order and gradient
//! drop".
//!
//! # Acceptance
//!
//! Every segment that does not end at a confluence reaches sea or a lake, every
//! segment is 8-connected and never climbs, and width stays in `[10, 200] m`.
//! The tests assert all three over synthetic and random fields.

use crate::fixed::ONE;
use crate::hydro::{FlowDir, Hydrology};
use crate::macro_grid::CELL_SIZE_M;
use crate::splines::Spline;
use crate::tile::{fnv_start, fnv_u64};

/// Default accumulation needed for a cell to be part of a river.
pub const DEFAULT_THRESHOLD: u32 = 512;

/// A lake must be at least this deep (0.25 m units) to end a river.
///
/// Four units is one metre; the single-cell puddles the valley detail produces
/// are shallower and stay part of the channel.
pub const MIN_TERMINAL_LAKE_DEPTH: u16 = 4;

/// Smallest channel width, Q16.16 metres.
pub const MIN_WIDTH: i32 = 10 << 16;

/// Largest channel width, Q16.16 metres.
pub const MAX_WIDTH: i32 = 200 << 16;

/// Accumulation to width, Q16.16 metres. Monotone, clamped by the caller.
pub const WIDTH: Spline<6> = Spline::new([
    (512, 10 << 16),
    (4_096, 20 << 16),
    (32_768, 45 << 16),
    (262_144, 90 << 16),
    (2_097_152, 180 << 16),
    (5_000_000, 200 << 16),
]);

/// Accumulation to depth, Q16.16 metres. Monotone.
pub const DEPTH: Spline<5> = Spline::new([
    (512, ONE * 4 / 10),
    (4_096, ONE * 9 / 10),
    (32_768, 2 * ONE),
    (262_144, ONE * 9 / 2),
    (2_097_152, 9 * ONE),
]);

/// Points used to measure the gradient at a mouth.
pub const DELTA_SAMPLE_CELLS: usize = 8;

/// Minimum Strahler order for a delta.
pub const DELTA_MIN_ORDER: u8 = 2;

/// Maximum gradient (millimetres per metre) for a delta; 2 mm/m is 0.2%.
pub const DELTA_GRADIENT_MM_PER_M: i64 = 2;

/// Tuning knobs for extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiverConfig {
    /// Accumulation at or above which a cell is a river.
    pub threshold: u32,
    /// A lake at least this deep (0.25 m units) ends a river.
    pub min_terminal_lake_depth: u16,
}

impl Default for RiverConfig {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            min_terminal_lake_depth: MIN_TERMINAL_LAKE_DEPTH,
        }
    }
}

/// Where a segment ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mouth {
    /// An outlet at or below sea level.
    Sea,
    /// A lake deep enough to be a water body.
    Lake,
    /// A border outlet above sea level.
    MapEdge,
    /// A confluence; the network continues downstream from the last point.
    Confluence,
}

/// One point of a river polyline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiverPoint {
    /// Macro cell coordinate, east.
    pub x: i32,
    /// Macro cell coordinate, south.
    pub z: i32,
    /// Flow accumulation at this cell, including itself.
    pub accumulation: u32,
    /// Channel width, Q16.16 metres, in `[10, 200]`.
    pub width: i32,
    /// Channel depth, Q16.16 metres.
    pub depth: i32,
}

impl RiverPoint {
    /// Centre of the cell, in metres from the world origin.
    pub fn centre_m(self) -> (i64, i64) {
        let half = CELL_SIZE_M as i64 / 2;
        (
            self.x as i64 * CELL_SIZE_M as i64 + half,
            self.z as i64 * CELL_SIZE_M as i64 + half,
        )
    }
}

/// A polyline segment between two junctions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct River {
    /// Points from upstream to downstream, 8-connected.
    pub points: Vec<RiverPoint>,
    /// Strahler order at the downstream end.
    pub order: u8,
    /// What the segment ends at.
    pub mouth: Mouth,
    /// True when a sea mouth sits on a wide, flat coastal plain.
    pub delta: bool,
}

impl River {
    /// Segment length in cells (point count minus one).
    pub fn length_cells(&self) -> usize {
        self.points.len().saturating_sub(1)
    }

    /// Segment length in metres.
    pub fn length_m(&self) -> i64 {
        self.length_cells() as i64 * CELL_SIZE_M as i64
    }

    /// Upstream cell coordinate.
    pub fn start(&self) -> Option<(i32, i32)> {
        self.points.first().map(|point| (point.x, point.z))
    }

    /// Downstream cell coordinate.
    pub fn end(&self) -> Option<(i32, i32)> {
        self.points.last().map(|point| (point.x, point.z))
    }
}

/// Every extracted segment.
#[derive(Clone, Debug)]
pub struct RiverNetwork {
    rivers: Vec<River>,
    threshold: u32,
    river_cells: usize,
}

impl RiverNetwork {
    /// Extract with the default threshold and lake depth.
    pub fn extract(hydro: &Hydrology) -> Self {
        Self::extract_with(hydro, RiverConfig::default())
    }

    /// Extract with explicit tuning.
    pub fn extract_with(hydro: &Hydrology, config: RiverConfig) -> Self {
        let width = hydro.width();
        let height = hydro.height();
        let cells = hydro.len();
        let flow = hydro.flow();
        let accumulation = hydro.accumulation();
        let filled = hydro.filled();
        let heights = hydro.heights();
        let sea_level = hydro.sea_level();

        // 1. River cells: accumulation above the threshold, excluding lakes
        //    deep enough to end a river.
        let mut is_river = vec![false; cells];
        let mut river_cells = Vec::new();
        for index in 0..cells {
            let is_lake = filled[index] > heights[index];
            let deep_lake = is_lake
                && filled[index].saturating_sub(heights[index]) >= config.min_terminal_lake_depth;
            if accumulation[index] >= config.threshold && !deep_lake {
                is_river[index] = true;
                river_cells.push(index as u32);
            }
        }

        // 2. Strahler order. Ascending accumulation is a topological order:
        //    a parent's accumulation is always larger than its child's.
        let mut order = vec![0u8; cells];
        let mut max_child = vec![0u8; cells];
        let mut max_child_count = vec![0u8; cells];
        let mut child_count = vec![0u8; cells];
        river_cells.sort_unstable_by_key(|&index| (accumulation[index as usize], index));
        for &index in &river_cells {
            let index = index as usize;
            let ordinal = if max_child_count[index] >= 2 {
                max_child[index].saturating_add(1)
            } else {
                max_child[index].max(1)
            };
            order[index] = ordinal;
            if !flow[index].is_outlet() {
                let parent = parent_index(index, width, height, flow[index]);
                if is_river[parent] {
                    if ordinal > max_child[parent] {
                        max_child[parent] = ordinal;
                        max_child_count[parent] = 1;
                    } else if ordinal == max_child[parent] {
                        max_child_count[parent] = max_child_count[parent].saturating_add(1);
                    }
                    child_count[parent] = child_count[parent].saturating_add(1);
                }
            }
        }

        // 3. Segments start at headwaters and at confluences (then run
        //    downstream). Every river cell belongs to at least one segment.
        let mut rivers = Vec::new();
        for &index in &river_cells {
            let index = index as usize;
            let starts = child_count[index] == 0 || child_count[index] >= 2;
            if !starts {
                continue;
            }
            let mut points = Vec::new();
            let mut current = index;
            loop {
                points.push(point(accumulation[current], current, width));
                let direction = flow[current];
                if direction.is_outlet() {
                    let mouth = if heights[current] <= sea_level {
                        Mouth::Sea
                    } else {
                        Mouth::MapEdge
                    };
                    let delta = mouth == Mouth::Sea
                        && is_delta(order[current], mouth_gradient(&points, filled, width));
                    rivers.push(River {
                        points,
                        order: order[current],
                        mouth,
                        delta,
                    });
                    break;
                }
                let parent = parent_index(current, width, height, direction);
                if !is_river[parent] {
                    // The downstream cell is a terminal lake.
                    rivers.push(River {
                        points,
                        order: order[current],
                        mouth: Mouth::Lake,
                        delta: false,
                    });
                    break;
                }
                if child_count[parent] >= 2 {
                    // End the segment at the confluence, which the downstream
                    // segment also starts from.
                    points.push(point(accumulation[parent], parent, width));
                    rivers.push(River {
                        points,
                        order: order[parent],
                        mouth: Mouth::Confluence,
                        delta: false,
                    });
                    break;
                }
                current = parent;
            }
        }

        Self {
            rivers,
            threshold: config.threshold,
            river_cells: river_cells.len(),
        }
    }

    /// The segments.
    pub fn rivers(&self) -> &[River] {
        &self.rivers
    }

    /// Accumulation threshold used.
    pub const fn threshold(&self) -> u32 {
        self.threshold
    }

    /// Number of river cells (above threshold, excluding terminal lakes).
    pub const fn river_cells(&self) -> usize {
        self.river_cells
    }

    /// Segment count.
    pub fn len(&self) -> usize {
        self.rivers.len()
    }

    /// True when there is no river.
    pub fn is_empty(&self) -> bool {
        self.rivers.is_empty()
    }

    /// Total polyline length in metres.
    pub fn total_length_m(&self) -> i64 {
        self.rivers.iter().map(River::length_m).sum()
    }

    /// Highest Strahler order in the network.
    pub fn max_order(&self) -> u8 {
        self.rivers
            .iter()
            .map(|river| river.order)
            .max()
            .unwrap_or(0)
    }

    /// Segments flagged as deltas.
    pub fn delta_count(&self) -> usize {
        self.rivers.iter().filter(|river| river.delta).count()
    }

    /// Width range over all points, Q16.16 metres.
    pub fn width_range(&self) -> Option<(i32, i32)> {
        let mut range: Option<(i32, i32)> = None;
        for point in self.rivers.iter().flat_map(|river| &river.points) {
            range = Some(match range {
                None => (point.width, point.width),
                Some((low, high)) => (low.min(point.width), high.max(point.width)),
            });
        }
        range
    }

    /// 64-bit hash of the whole network, for run-to-run comparison.
    pub fn content_hash(&self) -> u64 {
        let mut hash = fnv_start();
        for river in &self.rivers {
            hash = fnv_u64(hash, river.order as u64);
            hash = fnv_u64(hash, mouth_code(river.mouth) as u64);
            hash = fnv_u64(hash, river.delta as u64);
            for point in &river.points {
                hash = fnv_u64(hash, point.x as u32 as u64);
                hash = fnv_u64(hash, point.z as u32 as u64);
                hash = fnv_u64(hash, point.accumulation as u64);
                hash = fnv_u64(hash, point.width as u32 as u64);
                hash = fnv_u64(hash, point.depth as u32 as u64);
            }
        }
        hash
    }
}

/// True when a sea mouth is a delta.
///
/// A delta needs a confluent river (order at least [`DELTA_MIN_ORDER`]) and a
/// coastal plain flatter than [`DELTA_GRADIENT_MM_PER_M`].
pub const fn is_delta(order: u8, gradient_mm_per_m: i64) -> bool {
    order >= DELTA_MIN_ORDER && gradient_mm_per_m <= DELTA_GRADIENT_MM_PER_M
}

/// Mean gradient over the last points of a segment, in millimetres per metre.
fn mouth_gradient(points: &[RiverPoint], filled: &[u16], width: usize) -> i64 {
    let sample = points.len().min(DELTA_SAMPLE_CELLS);
    if sample < 2 {
        return i64::MAX;
    }
    let first = &points[points.len() - sample];
    let last = &points[points.len() - 1];
    let first_index = last.z as usize * width + first.x as usize;
    let last_index = last.z as usize * width + last.x as usize;
    let drop = (filled[first_index] as i64 - filled[last_index] as i64).max(0);
    let drop_mm = drop * 250;
    let run_m = (sample as i64 - 1) * CELL_SIZE_M as i64;
    drop_mm / run_m
}

fn point(accumulation: u32, index: usize, grid_width: usize) -> RiverPoint {
    let x = (index % grid_width) as i32;
    let z = (index / grid_width) as i32;
    let accumulation = accumulation.max(1);
    let width_value = WIDTH.eval(accumulation.min(i32::MAX as u32) as i32);
    let depth_value = DEPTH.eval(accumulation.min(i32::MAX as u32) as i32);
    RiverPoint {
        x,
        z,
        accumulation,
        width: width_value.clamp(MIN_WIDTH, MAX_WIDTH),
        depth: depth_value.max(ONE / 4),
    }
}

fn parent_index(index: usize, width: usize, height: usize, direction: FlowDir) -> usize {
    let (dx, dz) = direction.delta();
    let x = (index % width) as i32 + dx;
    let z = (index / width) as i32 + dz;
    assert!(
        x >= 0 && z >= 0 && (x as usize) < width && (z as usize) < height,
        "flow direction leaves the grid"
    );
    z as usize * width + x as usize
}

fn mouth_code(mouth: Mouth) -> u8 {
    match mouth {
        Mouth::Sea => 0,
        Mouth::Lake => 1,
        Mouth::MapEdge => 2,
        Mouth::Confluence => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rivers_tables_are_monotone_and_clamped() {
        let mut previous_width = 0;
        let mut previous_depth = 0;
        for accumulation in [512i32, 4_096, 32_768, 262_144, 2_097_152, 5_000_000] {
            let width = WIDTH.eval(accumulation);
            let depth = DEPTH.eval(accumulation);
            assert!(width >= previous_width, "width fell at {accumulation}");
            assert!(depth >= previous_depth, "depth fell at {accumulation}");
            previous_width = width;
            previous_depth = depth;
        }
        assert_eq!(WIDTH.eval(0), MIN_WIDTH);
        assert_eq!(WIDTH.eval(i32::MAX), MAX_WIDTH);
    }
}
