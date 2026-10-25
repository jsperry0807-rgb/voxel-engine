//! The six axis-aligned faces of a voxel.
//!
//! [`Face`] is shared by the block, storage and rendering layers: it names a
//! block's side ([`crate`] consumers use it for texture lookup), a chunk
//! neighbour direction (storage derives offsets from [`Face::normal`]), and a
//! face plane (storage's face views). It is pure geometry, so it lives here
//! rather than in any one of those crates.

use std::fmt;

/// One of the six axis-aligned faces of a voxel.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    North,
    South,
    East,
    West,
    Top,
    Bottom,
}

impl Face {
    /// All faces in a fixed, cache-friendly order.
    pub const ALL: [Face; 6] = [
        Face::North,
        Face::South,
        Face::East,
        Face::West,
        Face::Top,
        Face::Bottom,
    ];

    /// Position of this face in [`Face::ALL`].
    ///
    /// Face-indexed arrays (resolved texture sets, per-face light) use this
    /// order, so `Face::ALL[i].index() == i` holds for every `i`.
    pub const fn index(self) -> usize {
        match self {
            Face::North => 0,
            Face::South => 1,
            Face::East => 2,
            Face::West => 3,
            Face::Top => 4,
            Face::Bottom => 5,
        }
    }

    /// The face on the opposite side of the cube.
    pub const fn opposite(self) -> Face {
        match self {
            Face::North => Face::South,
            Face::South => Face::North,
            Face::East => Face::West,
            Face::West => Face::East,
            Face::Top => Face::Bottom,
            Face::Bottom => Face::Top,
        }
    }

    /// The outward normal of this face, in block space.
    ///
    /// The tuple is `(x, y, z)` with **+Y up**, **+X east** and **+Z south**,
    /// and exactly one component is non-zero:
    ///
    /// | Face | Normal |
    /// |---|---|
    /// | `North` | `(0, 0, -1)` |
    /// | `South` | `(0, 0, +1)` |
    /// | `East` | `(+1, 0, 0)` |
    /// | `West` | `(-1, 0, 0)` |
    /// | `Top` | `(0, +1, 0)` |
    /// | `Bottom` | `(0, -1, 0)` |
    ///
    /// This is the single source of truth for face orientation: chunk
    /// storage derives neighbour offsets and face-layer planes from it, so
    /// the geometry never has to be spelled out twice.
    ///
    /// ```
    /// use voxel_core::Face;
    ///
    /// assert_eq!(Face::North.normal(), (0, 0, -1));
    /// assert_eq!(Face::Top.normal(), (0, 1, 0));
    /// ```
    pub const fn normal(self) -> (i32, i32, i32) {
        match self {
            Face::North => (0, 0, -1),
            Face::South => (0, 0, 1),
            Face::East => (1, 0, 0),
            Face::West => (-1, 0, 0),
            Face::Top => (0, 1, 0),
            Face::Bottom => (0, -1, 0),
        }
    }
}

impl fmt::Display for Face {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Face::North => "north",
            Face::South => "south",
            Face::East => "east",
            Face::West => "west",
            Face::Top => "top",
            Face::Bottom => "bottom",
        })
    }
}

impl fmt::Debug for Face {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_opposite_roundtrips() {
        for face in Face::ALL {
            assert_eq!(face.opposite().opposite(), face);
            assert_ne!(face.opposite(), face);
        }
    }

    #[test]
    fn face_index_matches_the_all_order() {
        // Face-indexed arrays rely on this: `Face::ALL[i].index() == i`.
        for (index, face) in Face::ALL.iter().enumerate() {
            assert_eq!(face.index(), index);
        }
    }

    /// The orientation table is a contract, not an implementation detail:
    /// chunk storage derives neighbour offsets and face planes from it.
    #[test]
    fn face_normals_point_outward_and_are_axis_aligned() {
        let expected = [
            (Face::North, (0, 0, -1)),
            (Face::South, (0, 0, 1)),
            (Face::East, (1, 0, 0)),
            (Face::West, (-1, 0, 0)),
            (Face::Top, (0, 1, 0)),
            (Face::Bottom, (0, -1, 0)),
        ];

        for (face, normal) in expected {
            assert_eq!(face.normal(), normal, "{face}");
            // Exactly one axis is non-zero, and it is +/-1.
            let non_zero = [normal.0, normal.1, normal.2]
                .iter()
                .filter(|c| **c != 0)
                .count();
            assert_eq!(non_zero, 1, "{face} normal must use one axis");
            assert!(
                [normal.0, normal.1, normal.2]
                    .iter()
                    .all(|c| *c == 0 || *c == 1 || *c == -1)
            );
            // Opposite faces have exactly opposite normals.
            assert_eq!(
                face.opposite().normal(),
                (-normal.0, -normal.1, -normal.2),
                "{face}"
            );
        }
    }
}
