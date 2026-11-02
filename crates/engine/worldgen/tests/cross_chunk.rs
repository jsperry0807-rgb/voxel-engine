//! M2.5-20: cross-chunk feature determinism.
//!
//! Every feature is generated from its own anchor, not from the chunk that
//! happens to ask. This suite cuts 1,000 real structure stamps and 1,000 tree
//! fixtures at a chunk border, builds each side independently through a
//! with-margin window, and compares the shared overlap block for block.

use std::collections::BTreeMap;

use voxel_worldgen::structures::{PlacedBlock, STRUCTURE_CELL_M, STRUCTURE_CELLS, StructureGrid};

/// A half-open block window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    min: [i32; 3],
    max: [i32; 3],
}

impl Window {
    fn new(min: [i32; 3], size: [i32; 3]) -> Self {
        Self {
            min,
            max: [min[0] + size[0], min[1] + size[1], min[2] + size[2]],
        }
    }

    fn contains(&self, point: [i32; 3]) -> bool {
        point
            .iter()
            .enumerate()
            .all(|(axis, value)| *value >= self.min[axis] && *value < self.max[axis])
    }

    fn overlap(&self, other: &Self) -> Option<Self> {
        let min = [
            self.min[0].max(other.min[0]),
            self.min[1].max(other.min[1]),
            self.min[2].max(other.min[2]),
        ];
        let max = [
            self.max[0].min(other.max[0]),
            self.max[1].min(other.max[1]),
            self.max[2].min(other.max[2]),
        ];
        if (0..3).any(|axis| min[axis] >= max[axis]) {
            None
        } else {
            Some(Self { min, max })
        }
    }
}

type BlockMap = BTreeMap<[i32; 3], &'static str>;

fn pair_across(border_x: i32, y0: i32, y1: i32, z0: i32, z1: i32) -> (Window, Window) {
    let height = y1 - y0;
    let depth = z1 - z0;
    (
        Window::new([border_x - 10, y0, z0], [12, height, depth]),
        Window::new([border_x - 2, y0, z0], [12, height, depth]),
    )
}
fn assert_shared_equal(a: &BlockMap, b: &BlockMap, shared: Window) -> usize {
    let mut checked = 0;
    for (point, key) in a {
        if shared.contains(*point) {
            assert_eq!(
                b.get(point).copied(),
                Some(*key),
                "missing/unequal block {point:?}"
            );
            checked += 1;
        }
    }
    for (point, key) in b {
        if shared.contains(*point) {
            assert_eq!(
                a.get(point).copied(),
                Some(*key),
                "missing/unequal block {point:?}"
            );
        }
    }
    checked
}

fn structures_in_window(
    grid: &StructureGrid,
    height: &impl Fn(i32, i32) -> i32,
    window: Window,
) -> BlockMap {
    let margin = 64i32;
    let cell = STRUCTURE_CELL_M as i32;
    let min_cell_x = ((window.min[0] - margin).div_euclid(cell)).clamp(0, STRUCTURE_CELLS - 1);
    let max_cell_x = ((window.max[0] + margin).div_euclid(cell)).clamp(0, STRUCTURE_CELLS - 1);
    let min_cell_z = ((window.min[2] - margin).div_euclid(cell)).clamp(0, STRUCTURE_CELLS - 1);
    let max_cell_z = ((window.max[2] + margin).div_euclid(cell)).clamp(0, STRUCTURE_CELLS - 1);

    let mut map = BlockMap::new();
    for cell_z in min_cell_z..=max_cell_z {
        for cell_x in min_cell_x..=max_cell_x {
            let Some(structure) = grid.structure(cell_x, cell_z) else {
                continue;
            };
            let stamp = structure.stamp(height);
            for PlacedBlock { x, y, z, key } in stamp.blocks {
                let point = [x, y, z];
                if window.contains(point) {
                    map.insert(point, key);
                }
            }
        }
    }
    map
}

#[derive(Clone, Copy, Debug)]
struct Tree {
    x: i32,
    z: i32,
    surface: i32,
}

fn tree_blocks(tree: Tree) -> Vec<PlacedBlock> {
    let mut blocks = Vec::new();
    for dy in 1i32..=5 {
        blocks.push(PlacedBlock {
            x: tree.x,
            y: tree.surface + dy,
            z: tree.z,
            key: "voxel:oak_log",
        });
    }
    for dx in -2i32..=2 {
        for dy in 3i32..=6 {
            for dz in -2i32..=2 {
                if dx.abs() == 2 && dz.abs() == 2 && dy == 6 {
                    continue;
                }
                blocks.push(PlacedBlock {
                    x: tree.x + dx,
                    y: tree.surface + dy,
                    z: tree.z + dz,
                    key: "voxel:oak_leaves",
                });
            }
        }
    }
    blocks
}

fn trees_in_window(trees: &[Tree], window: Window) -> BlockMap {
    let mut map = BlockMap::new();
    for tree in trees {
        // Cheap bounding-box rejection before building the tree's blocks.
        if window.max[0] <= tree.x - 2
            || window.min[0] > tree.x + 2
            || window.max[1] <= tree.surface + 1
            || window.min[1] > tree.surface + 6
            || window.max[2] <= tree.z - 2
            || window.min[2] > tree.z + 2
        {
            continue;
        }
        for block in tree_blocks(*tree) {
            let point = [block.x, block.y, block.z];
            if window.contains(point) {
                map.insert(point, block.key);
            }
        }
    }
    map
}

#[test]
fn cross_chunk_structures_match_from_both_sides() {
    let grid = StructureGrid::new(3);
    let mut checked = 0usize;

    for structure in grid.iter().take(1_000) {
        let border = structure.x;
        let height = |_x: i32, _z: i32| structure.surface;
        let (left, right) = pair_across(
            border,
            structure.surface - 32,
            structure.surface + 32,
            structure.z - 24,
            structure.z + 24,
        );
        let left_map = structures_in_window(&grid, &height, left);
        let right_map = structures_in_window(&grid, &height, right);
        let shared = left.overlap(&right).expect("overlap");
        let count = assert_shared_equal(&left_map, &right_map, shared);
        assert!(
            count > 0,
            "structure {} at ({}, {}) does not cross the border",
            structure.kind.name(),
            structure.x,
            structure.z
        );
        checked += 1;
    }
    assert_eq!(checked, 1_000);
}

#[test]
fn cross_chunk_trees_match_from_both_sides() {
    let trees: Vec<Tree> = (0..1_000i32)
        .map(|index| Tree {
            x: 32 + (index % 100) * 20,
            z: 32 + (index / 100) * 20,
            surface: 50 + (index * 7).rem_euclid(9),
        })
        .collect();

    for tree in &trees {
        let (left, right) = pair_across(tree.x, 0, 100, tree.z - 8, tree.z + 8);
        let left_map = trees_in_window(&trees, left);
        let right_map = trees_in_window(&trees, right);
        let shared = left.overlap(&right).expect("overlap");
        assert!(
            assert_shared_equal(&left_map, &right_map, shared) > 0,
            "tree at ({}, {}) does not cross the border",
            tree.x,
            tree.z
        );
    }
}

#[test]
fn cross_chunk_window_overlap_is_half_open() {
    let (left, right) = pair_across(10, 0, 4, 0, 4);
    let shared = left.overlap(&right).expect("overlap");
    assert_eq!(shared.min, [8, 0, 0]);
    assert_eq!(shared.max, [12, 4, 4]);
    assert!(shared.contains([8, 0, 0]));
    assert!(!shared.contains([12, 0, 0]));
    assert_eq!(left.overlap(&Window::new([20, 0, 0], [2, 2, 2])), None);
}

#[test]
fn cross_chunk_structure_anchor_may_lie_outside_the_window() {
    // A chunk on the east side of a tower does not contain the anchor, but the
    // with-margin cell query must still find the tower's east wall.
    let grid = StructureGrid::new(4);
    let structure = grid.iter().next().expect("a structure");
    let height = |_x: i32, _z: i32| structure.surface;
    let window = Window::new(
        [structure.x + 1, structure.surface - 32, structure.z - 8],
        [8, 64, 16],
    );
    let found = structures_in_window(&grid, &height, window);
    let stamp = structure.stamp(height);
    let expected: Vec<PlacedBlock> = stamp
        .blocks
        .into_iter()
        .filter(|block| window.contains([block.x, block.y, block.z]))
        .collect();
    assert!(!expected.is_empty(), "the prefab does not reach the window");
    for block in expected {
        assert_eq!(
            found.get(&[block.x, block.y, block.z]).copied(),
            Some(block.key),
            "missing cross-window block {:?}",
            (block.x, block.y, block.z)
        );
    }
}
