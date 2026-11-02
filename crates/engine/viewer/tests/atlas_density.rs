//! The runtime tile-density path: build the atlas from the committed 256²
//! sources and check the layout, determinism and source fidelity.

use std::path::Path;

use voxel_texgraph::pipeline::Workspace;
use voxel_viewer::atlas::{Atlas, AtlasLayout, MipLevel};

fn workspace() -> Workspace {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    Workspace::at(root).expect("the viewer test runs inside the workspace")
}

fn build(layout: AtlasLayout) -> Atlas {
    let workspace = workspace();
    let levels = voxel_texgraph::bake::build_levels_at(&workspace, layout)
        .expect("committed sources bake at every supported density");
    let levels = levels
        .into_iter()
        .map(|level| MipLevel::new(level.width, level.height, level.pixels).expect("level size"))
        .collect();
    Atlas::from_levels(layout, levels).expect("layout matches the baked levels")
}

#[test]
fn every_supported_density_builds_a_complete_atlas() {
    for edge in [16, 32, 64, 128] {
        let layout = AtlasLayout::new(edge);
        let atlas = build(layout);

        assert_eq!(atlas.tile_size(), edge, "{edge}px tile");
        assert_eq!(atlas.atlas_size(), edge * 16, "{edge}px atlas");
        assert_eq!(atlas.mip_count(), layout.mip_count(), "{edge}px mips");
        assert_eq!(
            atlas.level(layout.max_mip()).unwrap().width(),
            16,
            "{edge}px deepest level is one texel per tile"
        );
        assert_eq!(
            atlas.byte_len() as u32 as u64,
            layout.byte_len() as u64,
            "{edge}px byte count"
        );
    }
}

#[test]
fn the_same_density_bakes_byte_identically() {
    let layout = AtlasLayout::new(32);
    let first = build(layout);
    let second = build(layout);

    assert_eq!(first.byte_len(), second.byte_len());
    for (left, right) in first.levels().iter().zip(second.levels()) {
        assert_eq!(left.pixels(), right.pixels());
    }
}

#[test]
fn opaque_sources_keep_full_coverage_at_every_density() {
    // Slot 1 is `voxel:block/stone`, an opaque face in the manifest. Every
    // texel of its tile must be filled at every density, including the
    // highest one, where the bake is a straight copy.
    for edge in [16, 64, 256] {
        let layout = AtlasLayout::new(edge);
        let atlas = build(layout);
        let base = atlas.level(0).unwrap();
        let (origin_x, origin_y) = layout.tile_origin(1);
        let mut distinct = std::collections::BTreeSet::new();
        for y in 0..edge {
            for x in 0..edge {
                let texel = base
                    .texel(origin_x + x, origin_y + y)
                    .expect("tile is inside the atlas");
                assert_eq!(texel[3], 255, "{edge}px stone texel at ({x}, {y})");
                distinct.insert(texel[..3].to_vec());
            }
        }
        assert!(
            distinct.len() > 1,
            "{edge}px stone tile should not be a flat colour"
        );
    }
}
