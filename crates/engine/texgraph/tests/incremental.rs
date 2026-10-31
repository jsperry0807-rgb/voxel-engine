//! Incremental generation: one graph edit must write one PNG.
//!
//! These tests build a two-face workspace in a temp directory and drive the
//! real pipeline end to end, including the atlas bake.

use std::fs;
use std::path::PathBuf;

use voxel_block::texture_manifest::{AlphaClass, Tiling};
use voxel_texgraph::graph::{AlphaSource, Graph, Node, Op, Output, UiPosition};
use voxel_texgraph::pipeline::{self, Workspace};

const BLOCKS: &str = r#"(
    blocks: [
        (
            key: "voxel:toy_a",
            render_type: Solid,
            textures: Some((
                top: "voxel:block/toy_a",
                bottom: "voxel:block/toy_a",
                north: "voxel:block/toy_a",
                south: "voxel:block/toy_a",
                east: "voxel:block/toy_a",
                west: "voxel:block/toy_a",
            )),
            flags: 3,
            hardness: 1.0,
        ),
        (
            key: "voxel:toy_b",
            render_type: Solid,
            textures: Some((
                top: "voxel:block/toy_b",
                bottom: "voxel:block/toy_b",
                north: "voxel:block/toy_b",
                south: "voxel:block/toy_b",
                east: "voxel:block/toy_b",
                west: "voxel:block/toy_b",
            )),
            flags: 3,
            hardness: 1.0,
        ),
    ],
)
"#;

const FACES: &str = r#"(
    schema: 1,
    faces: [
        "voxel:block/toy_a",
        "voxel:block/toy_b",
    ],
)
"#;

fn toy_graph(key: &str, color: &str) -> Graph {
    Graph {
        schema: 1,
        key: key.to_owned(),
        size: 256,
        tiling: Tiling::Both,
        alpha: AlphaClass::Opaque,
        seed: 1,
        nodes: vec![
            Node {
                id: 0,
                op: Op::Const { value: 1 },
                inputs: Vec::new(),
                ui: Some(UiPosition::new(0.0, 0.0)),
            },
            Node {
                id: 1,
                op: Op::Palette {
                    colors: vec![color.to_owned()],
                },
                inputs: vec![0],
                ui: Some(UiPosition::new(0.0, 120.0)),
            },
        ],
        output: Output {
            color: 1,
            alpha: AlphaSource::Const(65_535),
        },
    }
}

fn temp_workspace(label: &str) -> Workspace {
    let root =
        std::env::temp_dir().join(format!("texgen-incremental-{}-{label}", std::process::id()));
    fs::remove_dir_all(&root).ok();
    let block = root.join("crates/engine/block");
    fs::create_dir_all(block.join("data")).expect("data dir");
    fs::create_dir_all(block.join("textures/graph")).expect("graph dir");
    fs::write(block.join("data/blocks.ron"), BLOCKS).expect("blocks.ron");
    fs::write(block.join("data/faces.ron"), FACES).expect("faces.ron");
    toy_graph("voxel:block/toy_a", "#808080")
        .save(&block.join("textures/graph/toy_a.ron"))
        .expect("toy_a graph");
    toy_graph("voxel:block/toy_b", "#404040")
        .save(&block.join("textures/graph/toy_b.ron"))
        .expect("toy_b graph");
    Workspace::at(&root).expect("workspace")
}

fn read(path: PathBuf) -> Vec<u8> {
    fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn main_asset_paths(workspace: &Workspace) -> [PathBuf; 5] {
    [
        workspace.texture_path("voxel:block/toy_a"),
        workspace.texture_path("voxel:block/toy_b"),
        workspace.manifest_path(),
        workspace.materials_path(),
        workspace.atlas_png_path(),
    ]
}

/// Edits the palette colour of `toy_a` and saves the graph.
fn recolor_toy_a(workspace: &Workspace, color: &str) {
    let path = workspace.graph_path("voxel:block/toy_a");
    let mut graph = Graph::load(&path).expect("toy_a graph loads");
    for node in &mut graph.nodes {
        if let Op::Palette { colors } = &mut node.op {
            colors[0] = color.to_owned();
        }
    }
    graph.save(&path).expect("toy_a graph saves");
}

#[test]
fn generate_keys_writes_only_the_named_face() {
    let workspace = temp_workspace("writes");
    pipeline::update_all(&workspace).expect("initial full generate + atlas");
    let [a, b, manifest, materials, atlas] = main_asset_paths(&workspace);
    let png_a = read(a.clone());
    let png_b = read(b.clone());
    let manifest_before = read(manifest.clone());
    let materials_before = read(materials.clone());
    let atlas_before = read(atlas.clone());

    // Nothing changed: the command must not even touch the files.
    let status = pipeline::generate_keys(&workspace, &["toy_a".to_owned()]).expect("no-op run");
    assert!(status.contains("1 texture already current"), "{status}");
    assert!(status.contains("manifest unchanged"), "{status}");
    assert!(status.contains("materials unchanged"), "{status}");
    assert!(status.contains("atlas unchanged"), "{status}");
    assert_eq!(read(a.clone()), png_a);
    assert_eq!(read(manifest.clone()), manifest_before);

    // A graph edit that changes pixels rewrites one PNG, the manifest entry
    // and the atlas, and leaves the other face's bytes alone.
    recolor_toy_a(&workspace, "#ff0000");
    assert!(
        pipeline::check_keys(&workspace, &["toy_a".to_owned()]).is_err(),
        "check must see the edited graph as stale"
    );
    let status = pipeline::generate_keys(&workspace, &["toy_a".to_owned()]).expect("partial run");
    assert!(status.contains("1 texture written (1 changed)"), "{status}");
    assert!(status.contains("manifest updated"), "{status}");
    // The material table embeds the manifest hash, so it is rewritten too.
    assert!(status.contains("materials updated"), "{status}");
    assert!(status.contains("atlas rebuilt"), "{status}");
    assert_ne!(read(a), png_a);
    assert_eq!(read(b), png_b);
    assert_ne!(read(manifest), manifest_before);
    assert_ne!(read(materials), materials_before);
    assert_ne!(read(atlas), atlas_before);

    pipeline::check_keys(&workspace, &["toy_a".to_owned()]).expect("toy_a is current");
    pipeline::check_all(&workspace).expect("committed set is current");
    fs::remove_dir_all(workspace.root()).ok();
}

#[test]
fn graph_only_change_keeps_the_atlas_pixels() {
    let workspace = temp_workspace("graph-only");
    pipeline::update_all(&workspace).expect("initial full generate + atlas");
    let [a, _, manifest, _, atlas] = main_asset_paths(&workspace);
    let png_a = read(a.clone());
    let manifest_before = read(manifest.clone());
    let atlas_before = read(atlas.clone());
    let hash_path = workspace.atlas_hash_path();
    let hash_before = read(hash_path.clone());

    // Moving a node changes the graph bytes (and so `graph_hash`) but renders
    // the same pixels: the PNG and atlas pixels stay byte-identical, while the
    // manifest and the atlas hash (which embeds the manifest hash) move.
    let path = workspace.graph_path("voxel:block/toy_a");
    let mut graph = Graph::load(&path).expect("toy_a graph loads");
    graph.nodes[1].ui = Some(UiPosition::new(40.0, 80.0));
    graph.save(&path).expect("toy_a graph saves");

    let status = pipeline::generate_keys(&workspace, &["toy_a".to_owned()]).expect("partial run");
    assert!(status.contains("1 texture already current"), "{status}");
    assert!(status.contains("manifest updated"), "{status}");
    assert!(status.contains("atlas rebuilt"), "{status}");
    assert_eq!(read(a), png_a);
    assert_eq!(read(atlas), atlas_before);
    assert_ne!(read(hash_path), hash_before);
    assert_ne!(read(manifest), manifest_before);

    pipeline::check_all(&workspace).expect("committed set is current after a graph-only edit");
    fs::remove_dir_all(workspace.root()).ok();
}

#[test]
fn generate_keys_rejects_unknown_or_missing_faces() {
    let workspace = temp_workspace("rejects");
    pipeline::update_all(&workspace).expect("initial full generate + atlas");

    let error = pipeline::generate_keys(&workspace, &["not_a_face".to_owned()])
        .expect_err("unknown face must fail");
    assert!(error.to_string().contains("unknown face"), "{error}");

    // A face may be listed before its graph exists; that must name the file.
    fs::write(
        workspace.block_crate().join("data/faces.ron"),
        FACES.replace(
            "\"voxel:block/toy_b\",",
            "\"voxel:block/toy_b\",\n        \"voxel:block/toy_c\",",
        ),
    )
    .expect("extended faces.ron");
    let workspace = Workspace::at(workspace.root()).expect("reload");
    let error = pipeline::generate_keys(&workspace, &["toy_c".to_owned()])
        .expect_err("missing graph must fail");
    assert!(error.to_string().contains("toy_c.ron"), "{error}");

    fs::remove_dir_all(workspace.root()).ok();
}
