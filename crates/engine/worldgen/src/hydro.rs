//! Hydrology: priority-flood depression fill, D8 flow and accumulation.
//!
//! The macro heightfield is not drainable where it stands: a bowl has no
//! outlet and a river network cannot run through it. This module raises every
//! depression to its spill level and records, for every cell, the direction
//! water leaves it and how much water passes through.
//!
//! # Priority-flood
//!
//! The algorithm is Barnes, Lehman and Mulla (2014), *Priority-Flood*, in its
//! flow-direction form:
//!
//! 1. Seed a min-heap with every outlet — the map border and every cell at or
//!    below sea level — at its own elevation. An outlet drains off the map or
//!    into the sea, so it is never filled.
//! 2. Pop the lowest cell. For each unvisited neighbour, set its filled
//!    elevation to `max(height, popped level)`, push it, and point its flow
//!    back at the cell that reached it.
//! 3. The filled surface is the minimum, over all paths to an outlet, of the
//!    maximum elevation along the path. Every depression therefore sits at
//!    exactly its spill level, and no cell is raised more than it must be.
//!
//! The heap is ordered by `(filled, index)`, so equal elevations break ties by
//! row-major index and the result is independent of heap internals. The
//! neighbour scan uses a fixed compass order. The whole computation is integer
//! and deterministic.
//!
//! # Flow directions and accumulation
//!
//! Flow points from a cell to the neighbour that flooded it, one of eight
//! compass directions. Because that neighbour was popped earlier, following
//! the arrows always reaches an outlet: **the flow graph is a forest and
//! cannot contain a cycle**. Accumulation is computed in reverse pop order, so
//! a cell is added to its parent only after all its children have been added.
//!
//! On unflooded terrain the flooding neighbour is the lowest neighbour that
//! reached the cell, which is the steepest descent in the D8 sense. On a lake
//! surface all cells share the lake's elevation; the tree still routes them to
//! the spill point, which is what river extraction needs.
//!
//! # Boundary rule
//!
//! Every border cell is an outlet. The generated world is 35% ocean but the
//! square edge is not guaranteed to be sea, and a land border must still
//! drain; treating the frame as an outlet is the standard fix. Callers that
//! need a ring of ocean can zero the border heights before calling.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// One of the eight D8 neighbours, or an outlet.
///
/// The compass order is clockwise starting at north: `N, NE, E, SE, S, SW, W,
/// NW`. The opposite of a direction is its index plus four.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlowDir {
    /// Water leaves the map or enters the sea; no parent cell.
    Outlet,
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl FlowDir {
    /// The eight directions in scan order. Fixed, so tie-breaking is stable.
    pub const ALL: [Self; 8] = [
        Self::N,
        Self::NE,
        Self::E,
        Self::SE,
        Self::S,
        Self::SW,
        Self::W,
        Self::NW,
    ];

    /// Column and row step. `x` grows east, `z` grows south.
    pub const fn delta(self) -> (i32, i32) {
        match self {
            Self::Outlet => (0, 0),
            Self::N => (0, -1),
            Self::NE => (1, -1),
            Self::E => (1, 0),
            Self::SE => (1, 1),
            Self::S => (0, 1),
            Self::SW => (-1, 1),
            Self::W => (-1, 0),
            Self::NW => (-1, -1),
        }
    }

    /// The direction back to the other end of the edge.
    pub const fn opposite(self) -> Self {
        match self {
            Self::Outlet => Self::Outlet,
            Self::N => Self::S,
            Self::NE => Self::SW,
            Self::E => Self::W,
            Self::SE => Self::NW,
            Self::S => Self::N,
            Self::SW => Self::NE,
            Self::W => Self::E,
            Self::NW => Self::SE,
        }
    }

    /// True when the cell drains off the map or into the sea.
    pub const fn is_outlet(self) -> bool {
        matches!(self, Self::Outlet)
    }
}

/// One cell's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HydroCell {
    /// Stored elevation, in 0.25 m units.
    pub height: u16,
    /// Elevation after depression filling; `>= height`.
    pub filled: u16,
    /// Direction water leaves the cell.
    pub flow: FlowDir,
    /// Cells draining through this one, including itself.
    pub accumulation: u32,
}

impl HydroCell {
    /// True when the cell is under a lake surface.
    pub const fn is_lake(&self) -> bool {
        self.filled > self.height
    }

    /// Lake depth in 0.25 m units, zero outside a lake.
    pub const fn lake_depth(&self) -> u16 {
        self.filled - self.height
    }
}

/// A completed hydrology pass over a rectangular heightfield.
#[derive(Clone, Debug)]
pub struct Hydrology {
    width: usize,
    height: usize,
    sea_level: u16,
    heights: Vec<u16>,
    filled: Vec<u16>,
    flow: Vec<FlowDir>,
    accumulation: Vec<u32>,
    outlets: u32,
    lake_cells: u32,
    max_fill_depth: u16,
    max_accumulation: u32,
}

impl Hydrology {
    /// Compute the filled surface, flow directions and accumulation.
    ///
    /// `heights` is row-major (`z * width + x`), in 0.25 m units. Outlets are
    /// every border cell and every cell with `height <= sea_level`.
    ///
    /// # Panics
    ///
    /// If `width` or `height` is zero, or `heights` is not `width * height`
    /// long.
    #[must_use]
    pub fn compute(heights: Vec<u16>, width: usize, height: usize, sea_level: u16) -> Self {
        assert!(width > 0 && height > 0, "the grid must not be empty");
        let cells = width * height;
        assert_eq!(
            heights.len(),
            cells,
            "heightfield has {} cells, expected {width} × {height}",
            heights.len()
        );

        let mut filled = heights.clone();
        let mut flow = vec![FlowDir::Outlet; cells];
        let mut visited = vec![false; cells];
        let mut order: Vec<u32> = Vec::with_capacity(cells);
        let mut heap: BinaryHeap<Reverse<(u16, u32)>> = BinaryHeap::new();
        let mut outlets = 0u32;

        for z in 0..height {
            for x in 0..width {
                let index = z * width + x;
                let is_border = x == 0 || z == 0 || x + 1 == width || z + 1 == height;
                if is_border || heights[index] <= sea_level {
                    visited[index] = true;
                    order.push(index as u32);
                    heap.push(Reverse((heights[index], index as u32)));
                    outlets += 1;
                }
            }
        }

        while let Some(Reverse((level, index))) = heap.pop() {
            let index = index as usize;
            let x = (index % width) as i32;
            let z = (index / width) as i32;
            for direction in FlowDir::ALL {
                let (dx, dz) = direction.delta();
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                    continue;
                }
                let neighbour = nz as usize * width + nx as usize;
                if visited[neighbour] {
                    continue;
                }
                visited[neighbour] = true;
                let surface = heights[neighbour].max(level);
                filled[neighbour] = surface;
                // Water leaves the neighbour towards this cell.
                flow[neighbour] = direction.opposite();
                order.push(neighbour as u32);
                heap.push(Reverse((surface, neighbour as u32)));
            }
        }

        // Reverse pop order: every child is processed before its parent.
        let mut accumulation = vec![1u32; cells];
        let mut max_accumulation = 1u32;
        for &index in order.iter().rev() {
            let index = index as usize;
            let direction = flow[index];
            if !direction.is_outlet() {
                let (dx, dz) = direction.delta();
                let parent = (index / width) as i32 + dz;
                let parent_column = (index % width) as i32 + dx;
                let parent = parent as usize * width + parent_column as usize;
                accumulation[parent] = accumulation[parent].saturating_add(accumulation[index]);
            }
            max_accumulation = max_accumulation.max(accumulation[index]);
        }

        let mut lake_cells = 0u32;
        let mut max_fill_depth = 0u16;
        for index in 0..cells {
            if filled[index] > heights[index] {
                lake_cells += 1;
                max_fill_depth = max_fill_depth.max(filled[index] - heights[index]);
            }
        }

        Self {
            width,
            height,
            sea_level,
            heights,
            filled,
            flow,
            accumulation,
            outlets,
            lake_cells,
            max_fill_depth,
            max_accumulation,
        }
    }

    /// Grid width in cells.
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Grid height in cells.
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Cells in the grid.
    pub const fn len(&self) -> usize {
        self.width * self.height
    }

    /// True when the grid has no cells (never for a constructed value).
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The sea level used to seed outlets.
    pub const fn sea_level(&self) -> u16 {
        self.sea_level
    }

    /// Original elevations.
    pub fn heights(&self) -> &[u16] {
        &self.heights
    }

    /// Elevations after depression filling.
    pub fn filled(&self) -> &[u16] {
        &self.filled
    }

    /// Flow direction per cell.
    pub fn flow(&self) -> &[FlowDir] {
        &self.flow
    }

    /// Accumulation per cell, including the cell itself.
    pub fn accumulation(&self) -> &[u32] {
        &self.accumulation
    }

    /// Number of outlet cells (border plus sea).
    pub const fn outlets(&self) -> u32 {
        self.outlets
    }

    /// Cells under a lake surface.
    pub const fn lake_cells(&self) -> u32 {
        self.lake_cells
    }

    /// Deepest fill in 0.25 m units.
    pub const fn max_fill_depth(&self) -> u16 {
        self.max_fill_depth
    }

    /// Largest accumulation.
    pub const fn max_accumulation(&self) -> u32 {
        self.max_accumulation
    }

    /// Index of a cell.
    ///
    /// # Panics
    ///
    /// If `x` or `z` is outside the grid.
    pub fn index_of(&self, x: usize, z: usize) -> usize {
        assert!(
            x < self.width && z < self.height,
            "cell ({x}, {z}) is outside {}×{}",
            self.width,
            self.height
        );
        z * self.width + x
    }

    /// One cell's result.
    ///
    /// # Panics
    ///
    /// If `x` or `z` is outside the grid.
    pub fn cell(&self, x: usize, z: usize) -> HydroCell {
        let index = self.index_of(x, z);
        HydroCell {
            height: self.heights[index],
            filled: self.filled[index],
            flow: self.flow[index],
            accumulation: self.accumulation[index],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hydro_flow_directions_pair_up() {
        for direction in FlowDir::ALL {
            assert_eq!(direction.opposite().opposite(), direction);
            let (dx, dz) = direction.delta();
            let (ox, oz) = direction.opposite().delta();
            assert_eq!((dx + ox, dz + oz), (0, 0));
            assert!(!direction.is_outlet());
        }
        assert!(FlowDir::Outlet.is_outlet());
        assert_eq!(FlowDir::Outlet.opposite(), FlowDir::Outlet);
    }

    #[test]
    fn hydro_compass_order_is_clockwise() {
        let expected = [
            (0, -1),
            (1, -1),
            (1, 0),
            (1, 1),
            (0, 1),
            (-1, 1),
            (-1, 0),
            (-1, -1),
        ];
        for (direction, delta) in FlowDir::ALL.iter().zip(expected) {
            assert_eq!(direction.delta(), delta);
        }
    }
}
