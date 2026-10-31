//! Every op in the library must compile, evaluate and be deterministic, and
//! match its committed output hash.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use voxel_block::texture_manifest::{AlphaClass, Tiling};
use voxel_texgraph::eval;
use voxel_texgraph::graph::{AlphaSource, Graph, Node, Op, Output, UiPosition};

fn node(id: u32, op: Op, inputs: Vec<u32>) -> Node {
    Node {
        id,
        op,
        inputs,
        ui: Some(UiPosition::new(0.0, 0.0)),
    }
}

fn graph(tiling: Tiling, nodes: Vec<Node>, color: u32) -> Graph {
    Graph {
        schema: 1,
        key: "voxel:block/ops_test".to_owned(),
        size: 256,
        tiling,
        alpha: AlphaClass::Opaque,
        seed: 7,
        nodes,
        output: Output {
            color,
            alpha: AlphaSource::Const(65_535),
        },
    }
}

/// A grey colour source at node 1, fed by a constant at node 0.
fn colour_source() -> (Vec<Node>, u32) {
    (
        vec![
            node(0, Op::Const { value: 40_000 }, vec![]),
            node(
                1,
                Op::Ramp {
                    stops: vec![(0, "#202020".to_owned()), (65_535, "#e0e0e0".to_owned())],
                },
                vec![0],
            ),
        ],
        1,
    )
}

fn run(tiling: Tiling, scalar_ops: &[Op], colour_ops: &[Op]) {
    for op in scalar_ops {
        let (mut nodes, _colour) = colour_source();
        nodes.push(node(2, op.clone(), vec![]));
        nodes.push(node(
            3,
            Op::Ramp {
                stops: vec![(0, "#000000".to_owned()), (65_535, "#ffffff".to_owned())],
            },
            vec![2],
        ));
        let graph = graph(tiling, nodes, 3);
        let program = graph
            .compile()
            .unwrap_or_else(|error| panic!("{}: {error}", op.label()));
        let a = eval::evaluate_at(&program, 64);
        let b = eval::evaluate_at(&program, 64);
        assert_eq!(a, b, "{} is not deterministic", op.label());
        assert_eq!(a.len(), 64 * 64 * 4);
        check_golden(tiling, op.label(), "scalar", &a);
    }

    for op in colour_ops {
        let (mut nodes, colour) = colour_source();
        let inputs = match op {
            Op::Screen
            | Op::AddColor
            | Op::Subtract
            | Op::Difference
            | Op::Lighten
            | Op::Darken => vec![colour, colour],
            _ => vec![colour],
        };
        nodes.push(node(2, op.clone(), inputs));
        let graph = graph(tiling, nodes, 2);
        let program = graph
            .compile()
            .unwrap_or_else(|error| panic!("{}: {error}", op.label()));
        let a = eval::evaluate_at(&program, 64);
        let b = eval::evaluate_at(&program, 64);
        assert_eq!(a, b, "{} is not deterministic", op.label());
        assert_eq!(a.len(), 64 * 64 * 4);
        check_golden(tiling, op.label(), "colour", &a);
    }
}

#[test]
fn periodic_pattern_and_shape_ops_evaluate() {
    run(
        Tiling::Both,
        &[
            Op::Checker { period: 6 },
            Op::Bricks {
                rows: 5,
                columns: 4,
                mortar: 2_000,
                offset: 32_768,
            },
            Op::Wave {
                axis: voxel_texgraph::graph::Axis::X,
                count: 5,
                phase: 4_000,
            },
            Op::Spiral { turns: 4, arms: 3 },
            Op::RadialGradient { radius: 32_768 },
        ],
        &[],
    );
    run(
        Tiling::None,
        &[
            Op::Circle {
                radius: 20_000,
                softness: 1_500,
            },
            Op::Star {
                points: 5,
                inner: 22_000,
                rotation: 0,
                softness: 1_200,
            },
            Op::Polygon {
                sides: 6,
                radius: 24_000,
                rotation: 0,
                softness: 1_200,
            },
        ],
        &[],
    );
}

#[test]
fn domain_transform_ops_evaluate() {
    run(
        Tiling::Both,
        &[],
        &[
            Op::Tile { x: 2, y: 3 },
            Op::Offset { x: 8_192, y: 4_096 },
            Op::BrightnessContrast {
                brightness: 2_000,
                contrast: 40_000,
            },
            Op::Saturate { amount: 24_000 },
            Op::HueShift { angle: 10_000 },
            Op::Blur { radius: 2 },
            Op::Emboss { strength: 40_000 },
            Op::NormalMap { strength: 40_000 },
            Op::Bevel {
                radius: 2,
                strength: 40_000,
            },
        ],
    );
    run(
        Tiling::None,
        &[],
        &[
            Op::Rotate { angle: 4_096 },
            Op::Mirror {
                axis: voxel_texgraph::graph::Axis::X,
            },
        ],
    );
    run(Tiling::Horizontal, &[], &[Op::Polar, Op::Cartesian]);
}

#[test]
fn blend_ops_evaluate() {
    run(
        Tiling::Both,
        &[],
        &[
            Op::Screen,
            Op::AddColor,
            Op::Subtract,
            Op::Difference,
            Op::Lighten,
            Op::Darken,
        ],
    );
}

#[test]
fn luminance_bridges_colour_to_mask() {
    let (mut nodes, colour) = colour_source();
    nodes.push(node(2, Op::Luminance, vec![colour]));
    nodes.push(node(
        3,
        Op::Ramp {
            stops: vec![(0, "#000000".to_owned()), (65_535, "#ffffff".to_owned())],
        },
        vec![2],
    ));
    let graph = graph(Tiling::Both, nodes, 3);
    let program = graph.compile().expect("luminance compiles");
    let pixels = eval::evaluate_at(&program, 64);
    assert_eq!(pixels.len(), 64 * 64 * 4);
    assert!(pixels.as_chunks::<4>().0.iter().any(|texel| texel[0] > 0));
}

fn scalar_preview(op: Op, tiling: Tiling) -> Vec<u8> {
    let mut nodes = vec![node(0, op, vec![])];
    nodes.push(node(
        1,
        Op::Ramp {
            stops: vec![(0, "#000000".to_owned()), (65_535, "#ffffff".to_owned())],
        },
        vec![0],
    ));
    let graph = graph(tiling, nodes, 1);
    let program = graph.compile().expect("scalar op compiles");
    eval::evaluate_at(&program, 64)
}

#[test]
fn patterns_and_shapes_produce_contrast() {
    for (op, tiling) in [
        (Op::Checker { period: 6 }, Tiling::Both),
        (
            Op::Bricks {
                rows: 5,
                columns: 4,
                mortar: 2_000,
                offset: 32_768,
            },
            Tiling::Both,
        ),
        (
            Op::Wave {
                axis: voxel_texgraph::graph::Axis::X,
                count: 5,
                phase: 0,
            },
            Tiling::Both,
        ),
        (
            Op::Circle {
                radius: 20_000,
                softness: 1_500,
            },
            Tiling::None,
        ),
        (
            Op::Star {
                points: 5,
                inner: 22_000,
                rotation: 0,
                softness: 1_200,
            },
            Tiling::None,
        ),
        (
            Op::Polygon {
                sides: 6,
                radius: 24_000,
                rotation: 0,
                softness: 1_200,
            },
            Tiling::None,
        ),
    ] {
        let pixels = scalar_preview(op.clone(), tiling);
        let min = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|texel| texel[0])
            .min()
            .unwrap();
        let max = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|texel| texel[0])
            .max()
            .unwrap();
        assert!(
            max.saturating_sub(min) > 32,
            "{} should produce more than a flat field",
            op.label()
        );
    }
}

#[test]
fn circle_is_filled_at_its_centre() {
    let pixels = scalar_preview(
        Op::Circle {
            radius: 20_000,
            softness: 1_000,
        },
        Tiling::None,
    );
    let edge = 64usize;
    let centre = (32 * edge + 32) * 4;
    let corner = 0;
    assert!(pixels[centre] > 200, "centre should be inside the circle");
    assert!(pixels[corner] < 40, "corner should be outside the circle");
}

#[test]
fn shape_ops_declare_their_periodicity_cost() {
    let breaker = [
        Op::Rotate { angle: 4_096 },
        Op::Mirror {
            axis: voxel_texgraph::graph::Axis::X,
        },
        Op::Circle {
            radius: 10_000,
            softness: 0,
        },
        Op::Star {
            points: 5,
            inner: 10_000,
            rotation: 0,
            softness: 0,
        },
        Op::Polygon {
            sides: 5,
            radius: 10_000,
            rotation: 0,
            softness: 0,
        },
    ];
    for op in breaker {
        assert!(
            voxel_texgraph::graph::op_breaks_h(&op),
            "{} should break horizontal periodicity",
            op.label()
        );
        assert!(
            voxel_texgraph::graph::op_breaks_v(&op),
            "{} should break vertical periodicity",
            op.label()
        );
    }
    assert!(voxel_texgraph::graph::op_breaks_v(&Op::Polar));
    assert!(voxel_texgraph::graph::op_breaks_v(&Op::Cartesian));
    assert!(!voxel_texgraph::graph::op_breaks_h(&Op::Tile {
        x: 2,
        y: 2
    }));
}

/// Committed per-op output hashes.
///
/// The suite otherwise only proves that an op compiles and is deterministic;
/// a wrong-but-stable op (a changed scale, an off-by-one in a shape) passes
/// every other assertion. Regenerate after an intentional change with
/// `UPDATE_OP_GOLDEN=1 cargo test -p voxel-texgraph --test ops`.
fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/op_goldens.txt")
}

fn goldens() -> &'static Mutex<BTreeMap<String, u64>> {
    static GOLDENS: OnceLock<Mutex<BTreeMap<String, u64>>> = OnceLock::new();
    GOLDENS.get_or_init(|| {
        let mut map = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(golden_path()) {
            for line in text.lines() {
                // Labels contain spaces ("Radial Gradient", "Brightness /
                // Contrast"), so the hash is the last token.
                let Some((label, hash)) = line.rsplit_once(' ') else {
                    continue;
                };
                if let Ok(hash) = u64::from_str_radix(hash, 16) {
                    map.insert(label.to_owned(), hash);
                }
            }
        }
        Mutex::new(map)
    })
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn check_golden(tiling: Tiling, label: &str, kind: &str, bytes: &[u8]) {
    let key = format!("{tiling:?}/{kind}/{label}");
    let hash = fnv(bytes);
    let mut map = goldens().lock().expect("golden map");
    if std::env::var_os("UPDATE_OP_GOLDEN").is_some() {
        map.insert(key, hash);
        let text: String = map
            .iter()
            .map(|(label, hash)| format!("{label} {hash:016x}\n"))
            .collect();
        std::fs::write(golden_path(), text).expect("write op goldens");
        return;
    }
    match map.get(&key) {
        Some(&expected) => assert_eq!(
            hash, expected,
            "{key}: op output changed (regenerate with UPDATE_OP_GOLDEN=1 if intended)"
        ),
        None => panic!(
            "{key}: no committed hash; run UPDATE_OP_GOLDEN=1 cargo test -p voxel-texgraph --test ops"
        ),
    }
}
