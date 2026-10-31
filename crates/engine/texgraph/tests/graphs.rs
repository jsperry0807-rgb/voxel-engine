//! Graph validation, determinism and committed-output tests.

use std::path::PathBuf;

use voxel_block::texture_manifest::AlphaClass;
use voxel_block::texture_manifest::Tiling;
use voxel_texgraph::bake;
use voxel_texgraph::eval;
use voxel_texgraph::graph::{AlphaSource, Graph, GraphError, Node, Op, Output, UiPosition};
use voxel_texgraph::pipeline::{self, PipelineError, Workspace};

fn workspace() -> Workspace {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_owned();
    Workspace::at(root).expect("workspace")
}

fn node(id: u32, op: Op, inputs: Vec<u32>) -> Node {
    Node {
        id,
        op,
        inputs,
        ui: Some(UiPosition::new(0.0, 0.0)),
    }
}

fn graph(nodes: Vec<Node>, color: u32) -> Graph {
    Graph {
        schema: 1,
        key: "voxel:block/test".to_owned(),
        size: 256,
        tiling: Tiling::Both,
        alpha: AlphaClass::Opaque,
        seed: 1,
        nodes,
        output: Output {
            color,
            alpha: AlphaSource::Const(65_535),
        },
    }
}

#[test]
fn rejects_a_cycle_or_forward_reference() {
    let graph = graph(
        vec![
            node(0, Op::Add, vec![1, 1]),
            node(1, Op::Const { value: 1 }, vec![]),
        ],
        1,
    );
    assert!(graph.compile().is_err());
}

#[test]
fn rejects_a_type_mismatch() {
    let graph = graph(
        vec![
            node(0, Op::Const { value: 1 }, vec![]),
            node(1, Op::Multiply, vec![0, 0]),
        ],
        1,
    );
    assert!(graph.compile().is_err());
}

#[test]
fn rejects_a_rect_under_both_tiling() {
    let graph = graph(
        vec![
            node(0, Op::Const { value: 1 }, vec![]),
            node(
                1,
                Op::Rect {
                    x0: 0,
                    y0: 0,
                    x1: 10,
                    y1: 10,
                },
                vec![],
            ),
            node(
                2,
                Op::Ramp {
                    stops: vec![(0, "#000000".to_owned()), (65_535, "#ffffff".to_owned())],
                },
                vec![0],
            ),
        ],
        2,
    );
    let mut graph = graph;
    graph.tiling = Tiling::Both;
    graph.output.alpha = AlphaSource::Node(1);
    // The Rect breaks both axes, which `Both` forbids.
    assert!(graph.compile().is_err());
    graph.tiling = Tiling::None;
    assert!(graph.compile().is_ok());
}

#[test]
fn evaluation_is_deterministic() {
    let path = workspace().graph_path("voxel:block/stone");
    let graph = Graph::load(&path).expect("stone graph");
    let program = graph.compile().expect("stone compiles");
    let a = eval::evaluate(&program);
    let b = eval::evaluate(&program);
    assert_eq!(a, b);
    assert_eq!(a.len(), 256 * 256 * 4);
}

#[test]
fn preview_sizes_sample_the_same_graph() {
    let path = workspace().graph_path("voxel:block/dirt");
    let graph = Graph::load(&path).expect("dirt graph");
    let program = graph.compile().expect("dirt compiles");
    for edge in [64, 128, 256] {
        assert_eq!(
            eval::evaluate_at(&program, edge).len(),
            edge as usize * edge as usize * 4
        );
    }
}

#[test]
fn graph_ron_round_trips_editor_positions() {
    let path = workspace().graph_path("voxel:block/stone");
    let mut graph = Graph::load(&path).expect("stone graph");
    for node in &mut graph.nodes {
        node.ui = Some(UiPosition::new(node.id as f32 * 10.0, 40.0));
    }
    let temp =
        std::env::temp_dir().join(format!("voxel-texgen-roundtrip-{}.ron", std::process::id()));
    graph.save(&temp).expect("save");
    let loaded = Graph::load(&temp).expect("load");
    let _ = std::fs::remove_file(&temp);
    assert_eq!(loaded, graph);
}

#[test]
fn caps_node_count_and_depth() {
    // A wide graph: 600 shallow nodes must be rejected on the node count.
    let mut nodes = vec![node(0, Op::Const { value: 1 }, vec![])];
    for id in 1..600 {
        nodes.push(node(id, Op::Invert, vec![0]));
    }
    let error = graph(nodes, 0)
        .compile()
        .expect_err("more nodes than the cap");
    assert!(error.to_string().contains("nodes"), "{error}");

    // A deep graph: the evaluator recurses once per node, so the chain must be
    // rejected before it can overflow the stack.
    let mut nodes = vec![node(0, Op::Const { value: 1 }, vec![])];
    for id in 1..200 {
        nodes.push(node(id, Op::Invert, vec![id - 1]));
    }
    let error = graph(nodes, 0).compile().expect_err("deeper than the cap");
    assert!(error.to_string().contains("deeper"), "{error}");
}

#[test]
fn save_reports_io_failures_as_io() {
    let dir = std::env::temp_dir().join(format!(
        "voxel-texgen-save-io-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    // A file cannot be a parent, so `create_dir_all` fails with ENOTDIR.
    let parent = dir.join("not-a-directory");
    std::fs::write(&parent, b"x").expect("write file");
    let graph = graph(vec![node(0, Op::Const { value: 1 }, vec![])], 0);
    let error = graph
        .save(&parent.join("graph.ron"))
        .expect_err("saving under a file must fail");
    assert!(
        matches!(error, GraphError::Io { .. }),
        "the io kind must survive: {error:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_atomic_write_leaves_no_temp_file() {
    let dir = std::env::temp_dir().join(format!(
        "voxel-texgen-atomic-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    // Renaming a file onto an existing directory fails, after the temp file
    // has been written and synced.
    let target = dir.join("target.ron");
    std::fs::create_dir(&target).expect("target directory");
    let error = pipeline::write_atomic(&target, b"data").expect_err("rename must fail");
    assert!(
        matches!(error, PipelineError::Io { .. }),
        "expected an io error: {error:?}"
    );
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .expect("read temp dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "a failed write left temp files behind: {leftovers:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The editor mints subgraph keys with a slash; building their path must not
/// panic (and must stay inside the graph directory).
#[test]
fn subgraph_keys_have_a_safe_path() {
    let workspace = workspace();
    let path = workspace.graph_path("voxel:block/_subgraph/fractal");
    assert_eq!(path.file_name().unwrap(), "_subgraph_fractal.ron");
    assert!(path.starts_with(workspace.graph_dir()));
}

#[test]
fn committed_outputs_are_current() {
    let workspace = workspace();
    let message = pipeline::check_all(&workspace).expect("committed outputs should be current");
    assert!(message.contains("current"), "{message}");
}

#[test]
fn atlas_hash_ron_uses_the_expected_filter() {
    let workspace = workspace();
    let build = bake::build_atlas(&workspace).expect("atlas builds");
    let text = bake::hash_ron(&build).expect("hash serializes");
    assert!(text.contains("box16-linear-premul-v1"));
    assert!(text.contains(&build.atlas_png_blake3));
}

#[test]
fn every_slice_has_a_graph_with_a_matching_key() {
    let workspace = workspace();
    for (key, _) in pipeline::SLICES {
        let path = workspace.graph_path(key);
        let graph = Graph::load(&path).unwrap_or_else(|error| panic!("{key}: {error}"));
        assert_eq!(graph.key, *key);
        graph
            .compile()
            .unwrap_or_else(|error| panic!("{key}: {error}"));
    }
}
