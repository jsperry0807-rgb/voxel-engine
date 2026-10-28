//! Committed texture manifest and generated material table tests.
//!
//! These are the checks named by `docs/generator.spec.md` section 10. They
//! read the files from the block crate as a clean checkout sees them; nothing
//! is generated in the test process except the in-memory material table.

use std::path::PathBuf;

use voxel_block::material_table;
use voxel_block::texture_manifest::{self, TextureManifest};

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn texture_manifest_matches_graphs_and_blocks() {
    let root = crate_root();
    let manifest = TextureManifest::load(&root.join(texture_manifest::MANIFEST_PATH))
        .expect("textures.ron should parse");
    texture_manifest::validate(&manifest, &root).expect("textures.ron should validate");

    assert_eq!(manifest.schema, 1);
    assert_eq!(manifest.textures.len(), 40);
    assert!(!manifest.license.trim().is_empty());
    assert!(!manifest.provenance.trim().is_empty());
    assert!(manifest.textures.iter().all(|entry| !entry.placeholder));
    assert_eq!(
        manifest
            .textures
            .iter()
            .map(|entry| entry.slot)
            .collect::<Vec<_>>(),
        (1..=40).collect::<Vec<_>>()
    );
}

#[test]
fn texture_manifest_sandstone_faces_agree() {
    // `validate` performs the band comparison; this test names it separately so
    // a failure is easy to find in CI output.
    let root = crate_root();
    let manifest = TextureManifest::load(&root.join(texture_manifest::MANIFEST_PATH))
        .expect("textures.ron should parse");
    texture_manifest::validate(&manifest, &root).expect("sandstone faces should agree");
}

#[test]
fn materials_gen_is_current() {
    let root = crate_root();
    let table = material_table::generate(&root).expect("material table should generate");
    let expected = material_table::to_ron(&table).expect("material table should serialize");
    let committed = std::fs::read_to_string(root.join("data/materials.gen.ron"))
        .expect("materials.gen.ron should exist");
    assert_eq!(
        committed, expected,
        "materials.gen.ron is stale; run `cargo run -p voxel-texgen -- generate`"
    );
    assert_eq!(table.schema, 1);
    assert_eq!(table.materials.len(), 40);
    assert_eq!(table.faces.len(), 33);
}
