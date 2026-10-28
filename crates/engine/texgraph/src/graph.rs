//! Node-graph model, RON decoding and validation.
//!
//! A graph is a directed acyclic graph of scalar (mask) and colour nodes.
//! Nodes are listed in topological order and may only reference earlier ids.
//! Compilation parses colour strings, resolves inputs to indices and records
//! whether an op breaks horizontal or vertical periodicity.
//!
//! Semantics are fixed-point only: every scalar is a Q16.16 value in
//! `0..=65535` (nominal `0.0..=1.0`). Colour is 8-bit sRGB. The evaluator never
//! touches a float.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use voxel_block::texture_manifest::{AlphaClass, Tiling};

/// Required output edge; the box filter and atlas tile sizes assume this.
pub const GRAPH_SIZE: u32 = 256;

/// A graph file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub schema: u32,
    pub key: String,
    pub size: u32,
    pub tiling: Tiling,
    pub alpha: AlphaClass,
    /// `0` derives the seed from the key; nonzero overrides it.
    #[serde(default)]
    pub seed: u64,
    pub nodes: Vec<Node>,
    pub output: Output,
}

/// One graph node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: u32,
    pub op: Op,
    #[serde(default)]
    pub inputs: Vec<u32>,
    /// Editor position. Ignored by the evaluator and by the generator.
    #[serde(default)]
    pub ui: Option<UiPosition>,
}

/// Node position on the editor canvas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiPosition {
    pub x: f32,
    pub y: f32,
}

impl UiPosition {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// The final colour and alpha.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub color: u32,
    pub alpha: AlphaSource,
}

/// An alpha channel: a constant, or a scalar node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AlphaSource {
    Const(u16),
    Node(u32),
}

/// Axis selector for [`Op::Uv`] and [`Op::Stripes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
}

/// Worley output selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorleyMode {
    F1,
    F2,
    Edge,
}

/// The declarative op set. Parameters are integers; colour values are
/// `#rrggbb` or `#rrggbbaa` strings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    Const {
        value: u16,
    },
    Uv {
        axis: Axis,
    },
    ValueNoise {
        period: u32,
        octaves: u8,
    },
    Fbm {
        period: u32,
        octaves: u8,
    },
    Worley {
        period: u32,
        mode: WorleyMode,
    },
    Rings {
        scale: u32,
    },
    Stripes {
        axis: Axis,
        count: u32,
        wobble: u16,
    },
    Rect {
        x0: u16,
        y0: u16,
        x1: u16,
        y1: u16,
    },
    HBand {
        y0: u16,
        y1: u16,
    },
    Add,
    Mul,
    Min,
    Max,
    Invert,
    Remap {
        from_lo: u16,
        from_hi: u16,
        to_lo: u16,
        to_hi: u16,
    },
    Smoothstep {
        edge0: u16,
        edge1: u16,
    },
    Threshold {
        at: u16,
    },
    Quantize {
        levels: u8,
    },
    Mix,
    MixColor,
    Warp {
        amount: u16,
    },
    Ramp {
        stops: Vec<(u16, String)>,
    },
    Palette {
        colors: Vec<String>,
    },
    Multiply,
    Overlay,
    AlphaFromMask,
    AlphaBinary {
        at: u16,
    },
    AlphaBand {
        min: u16,
        max: u16,
    },
}

/// Input/output type of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Scalar,
    Color,
}

/// A compiled, validated graph ready to evaluate.
#[derive(Clone, Debug)]
pub struct Program {
    pub key: String,
    pub size: u32,
    pub tiling: Tiling,
    pub alpha: AlphaClass,
    pub seed: u64,
    pub nodes: Vec<ProgramNode>,
    pub output: ProgramOutput,
    pub breaks_h: bool,
    pub breaks_v: bool,
}

#[derive(Clone, Debug)]
pub struct ProgramNode {
    pub id: u32,
    pub op: ProgramOp,
    pub inputs: Vec<usize>,
    /// Per-node RNG stream, derived from the graph seed and node id.
    pub stream: u64,
}

#[derive(Clone, Debug)]
pub enum ProgramOp {
    Const(u16),
    Uv(Axis),
    ValueNoise {
        period: u32,
        octaves: u8,
    },
    Fbm {
        period: u32,
        octaves: u8,
    },
    Worley {
        period: u32,
        mode: WorleyMode,
    },
    Rings {
        scale: u32,
    },
    Stripes {
        axis: Axis,
        count: u32,
        wobble: u16,
    },
    Rect {
        x0: u16,
        y0: u16,
        x1: u16,
        y1: u16,
    },
    HBand {
        y0: u16,
        y1: u16,
    },
    Add,
    Mul,
    Min,
    Max,
    Invert,
    Remap {
        from_lo: u16,
        from_hi: u16,
        to_lo: u16,
        to_hi: u16,
    },
    Smoothstep {
        edge0: u16,
        edge1: u16,
    },
    Threshold {
        at: u16,
    },
    Quantize {
        levels: u8,
    },
    Mix,
    MixColor,
    Warp {
        amount: u16,
    },
    Ramp {
        stops: Vec<(u16, [u8; 3])>,
    },
    Palette {
        colors: Vec<[u8; 3]>,
    },
    Multiply,
    Overlay,
    AlphaFromMask,
    AlphaBinary {
        at: u16,
    },
    AlphaBand {
        min: u16,
        max: u16,
    },
}

#[derive(Clone, Debug)]
pub struct ProgramOutput {
    pub color: usize,
    pub alpha: ProgramAlpha,
}

#[derive(Clone, Copy, Debug)]
pub enum ProgramAlpha {
    Const(u16),
    Node(usize),
}

/// A graph validation failure.
#[derive(Debug)]
pub enum GraphError {
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Parse(ron::error::SpannedError),
    Invalid(String),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse(error) => write!(f, "graph parse error: {error}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for GraphError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

/// Pretty-printing configuration used for every graph and manifest write.
///
/// Fixed field order, four-space indent and a trailing newline keep diffs
/// small and make committed files byte-reproducible.
pub fn pretty_config() -> ron::ser::PrettyConfig {
    ron::ser::PrettyConfig::default()
        .struct_names(false)
        .indentor("    ".to_owned())
        .new_line("\n".to_owned())
        .separate_tuple_members(false)
        .enumerate_arrays(false)
        .compact_arrays(false)
}

impl Graph {
    /// Reads and parses a graph file without compiling it.
    pub fn load(path: &Path) -> Result<Self, GraphError> {
        let text = std::fs::read_to_string(path).map_err(|source| GraphError::Io {
            path: path.to_owned(),
            source,
        })?;
        ron::from_str(&text).map_err(GraphError::Parse)
    }

    /// Validates and compiles the graph.
    pub fn compile(&self) -> Result<Program, GraphError> {
        if self.schema != 1 {
            return Err(invalid(format!(
                "{}: schema {} is not supported (expected 1)",
                self.key, self.schema
            )));
        }
        if self.size != GRAPH_SIZE {
            return Err(invalid(format!(
                "{}: size {} is not supported (expected {GRAPH_SIZE})",
                self.key, self.size
            )));
        }
        if !self.key.starts_with("voxel:block/") {
            return Err(invalid(format!(
                "{}: key must start with `voxel:block/`",
                self.key
            )));
        }

        let mut types = Vec::with_capacity(self.nodes.len());
        let mut ids = HashMap::with_capacity(self.nodes.len());
        let mut program = Vec::with_capacity(self.nodes.len());
        let mut breaks_h = Vec::with_capacity(self.nodes.len());
        let mut breaks_v = Vec::with_capacity(self.nodes.len());

        for (index, node) in self.nodes.iter().enumerate() {
            if ids.insert(node.id, index).is_some() {
                return Err(invalid(format!(
                    "{}: duplicate node id {}",
                    self.key, node.id
                )));
            }
            let mut inputs = Vec::with_capacity(node.inputs.len());
            for input in &node.inputs {
                let Some(&source) = ids.get(input) else {
                    return Err(invalid(format!(
                        "{}: node {} references {} which is missing or not earlier in the list",
                        self.key, node.id, input
                    )));
                };
                if source >= index {
                    return Err(invalid(format!(
                        "{}: node {} may only reference earlier node ids, not {}",
                        self.key, node.id, input
                    )));
                }
                inputs.push(source);
            }
            let ty = op_type(&node.op);
            check_arity(&self.key, node, &inputs, &types)?;

            let mut h = op_breaks_h(&node.op);
            let mut v = op_breaks_v(&node.op);
            for input in &inputs {
                h |= breaks_h[*input];
                v |= breaks_v[*input];
            }
            if h && matches!(self.tiling, Tiling::Both | Tiling::Horizontal) {
                return Err(invalid(format!(
                    "{}: node {} breaks horizontal periodicity but tiling is {:?}",
                    self.key, node.id, self.tiling
                )));
            }
            if v && self.tiling == Tiling::Both {
                return Err(invalid(format!(
                    "{}: node {} breaks vertical periodicity but tiling is Both",
                    self.key, node.id
                )));
            }

            let stream = node_stream(self.seed_for_key(), node.id);
            program.push(ProgramNode {
                id: node.id,
                op: compile_op(&self.key, node.id, &node.op)?,
                inputs,
                stream,
            });
            types.push(ty);
            breaks_h.push(h);
            breaks_v.push(v);
        }

        let Some(&color) = ids.get(&self.output.color) else {
            return Err(invalid(format!(
                "{}: output colour node {} does not exist",
                self.key, self.output.color
            )));
        };
        if types[color] != Type::Color {
            return Err(invalid(format!(
                "{}: output colour node {} is not a colour node",
                self.key, self.output.color
            )));
        }
        let alpha = match self.output.alpha {
            AlphaSource::Const(value) => ProgramAlpha::Const(value),
            AlphaSource::Node(id) => {
                let Some(&index) = ids.get(&id) else {
                    return Err(invalid(format!(
                        "{}: output alpha node {id} does not exist",
                        self.key
                    )));
                };
                if types[index] != Type::Scalar {
                    return Err(invalid(format!(
                        "{}: output alpha node {id} is not a scalar node",
                        self.key
                    )));
                }
                ProgramAlpha::Node(index)
            }
        };

        let mut final_h = breaks_h[color];
        let mut final_v = breaks_v[color];
        if let ProgramAlpha::Node(index) = alpha {
            final_h |= breaks_h[index];
            final_v |= breaks_v[index];
        }

        Ok(Program {
            key: self.key.clone(),
            size: self.size,
            tiling: self.tiling,
            alpha: self.alpha,
            seed: self.seed_for_key(),
            nodes: program,
            output: ProgramOutput { color, alpha },
            breaks_h: final_h,
            breaks_v: final_v,
        })
    }

    /// Fills missing editor positions from the graph's topological depth.
    ///
    /// Hand-authored graphs do not carry `ui`; the editor lays them out once
    /// and writes the positions back on save.
    pub fn ensure_layout(&mut self) {
        if self.nodes.iter().all(|node| node.ui.is_some()) {
            return;
        }
        let mut depth = std::collections::HashMap::new();
        let mut ids = std::collections::HashMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            ids.insert(node.id, index);
            let parent = node
                .inputs
                .iter()
                .filter_map(|input| depth.get(input).copied())
                .max()
                .map(|depth: u32| depth + 1)
                .unwrap_or(0);
            depth.insert(node.id, parent);
        }
        let mut rows: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for node in &mut self.nodes {
            if node.ui.is_some() {
                continue;
            }
            let column = depth.get(&node.id).copied().unwrap_or(0);
            let row = rows.entry(column).or_insert(0);
            node.ui = Some(UiPosition::new(
                40.0 + column as f32 * 240.0,
                40.0 + *row as f32 * 120.0,
            ));
            *row += 1;
        }
    }

    /// Serializes the graph in canonical RON form, with a trailing newline.
    pub fn to_ron(&self) -> Result<String, ron::Error> {
        let mut text = ron::ser::to_string_pretty(self, pretty_config())?;
        text.push('\n');
        Ok(text)
    }

    /// Writes the graph in canonical RON form.
    pub fn save(&self, path: &Path) -> Result<(), GraphError> {
        let text = self
            .to_ron()
            .map_err(|error| GraphError::Invalid(format!("could not serialize graph: {error}")))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| GraphError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        std::fs::write(path, text).map_err(|source| GraphError::Io {
            path: path.to_owned(),
            source,
        })
    }

    /// The effective seed: the explicit override, or BLAKE3(key) per section
    /// 8.2.
    pub fn seed_for_key(&self) -> u64 {
        if self.seed != 0 {
            return self.seed;
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"voxel-stuff/texgen/v1/");
        hasher.update(self.key.as_bytes());
        let hash = hasher.finalize();
        u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
    }
}

impl Op {
    /// Human-readable node title.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Const { .. } => "Const",
            Self::Uv { .. } => "UV",
            Self::ValueNoise { .. } => "Value Noise",
            Self::Fbm { .. } => "FBM",
            Self::Worley { .. } => "Worley",
            Self::Rings { .. } => "Rings",
            Self::Stripes { .. } => "Stripes",
            Self::Rect { .. } => "Rect",
            Self::HBand { .. } => "H Band",
            Self::Add => "Add",
            Self::Mul => "Multiply",
            Self::Min => "Minimum",
            Self::Max => "Maximum",
            Self::Invert => "Invert",
            Self::Remap { .. } => "Remap",
            Self::Smoothstep { .. } => "Smoothstep",
            Self::Threshold { .. } => "Threshold",
            Self::Quantize { .. } => "Quantize",
            Self::Mix => "Mix",
            Self::MixColor => "Mix Color",
            Self::Warp { .. } => "Warp",
            Self::Ramp { .. } => "Ramp",
            Self::Palette { .. } => "Palette",
            Self::Multiply => "Color Multiply",
            Self::Overlay => "Overlay",
            Self::AlphaFromMask => "Alpha From Mask",
            Self::AlphaBinary { .. } => "Alpha Binary",
            Self::AlphaBand { .. } => "Alpha Band",
        }
    }

    /// Node-library category.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Const { .. } | Self::Uv { .. } => "Source",
            Self::ValueNoise { .. }
            | Self::Fbm { .. }
            | Self::Worley { .. }
            | Self::Rings { .. }
            | Self::Stripes { .. }
            | Self::Rect { .. }
            | Self::HBand { .. } => "Pattern",
            Self::Add
            | Self::Mul
            | Self::Min
            | Self::Max
            | Self::Invert
            | Self::Remap { .. }
            | Self::Smoothstep { .. }
            | Self::Threshold { .. }
            | Self::Quantize { .. }
            | Self::Mix
            | Self::Warp { .. } => "Math",
            Self::MixColor => "Color",
            Self::Ramp { .. } | Self::Palette { .. } | Self::Multiply | Self::Overlay => "Color",
            Self::AlphaFromMask | Self::AlphaBinary { .. } | Self::AlphaBand { .. } => "Alpha",
        }
    }

    /// One-line description shown in the node library.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Const { .. } => "A constant scalar.",
            Self::Uv { .. } => "Normalized X or Y position.",
            Self::ValueNoise { .. } => "Periodic value noise.",
            Self::Fbm { .. } => "Multi-octave periodic value noise.",
            Self::Worley { .. } => "Worley cells: F1, F2 or cell edges.",
            Self::Rings { .. } => "Periodic radial rings.",
            Self::Stripes { .. } => "Periodic stripes with optional wobble.",
            Self::Rect { .. } => "Axis-aligned rectangle mask. Breaks both axes.",
            Self::HBand { .. } => "Horizontal band mask. Breaks the vertical axis.",
            Self::Add => "Clamped addition of two masks.",
            Self::Mul => "Mask multiplication.",
            Self::Min => "Smaller of two masks.",
            Self::Max => "Larger of two masks.",
            Self::Invert => "One minus the mask.",
            Self::Remap { .. } => "Map a range onto another range.",
            Self::Smoothstep { .. } => "Smooth interpolation between two edges.",
            Self::Threshold { .. } => "Hard black/white threshold.",
            Self::Quantize { .. } => "Reduce a mask to a number of levels.",
            Self::Mix => "Linear mix of two masks by a third.",
            Self::MixColor => "Linear mix of two colours by a mask.",
            Self::Warp { .. } => "Displace a mask by one or two warp fields.",
            Self::Ramp { .. } => "Map a mask through a colour gradient.",
            Self::Palette { .. } => "Map a mask to a fixed colour palette.",
            Self::Multiply => "Multiply two colours.",
            Self::Overlay => "Overlay blend two colours.",
            Self::AlphaFromMask => "Alpha equals the mask.",
            Self::AlphaBinary { .. } => "Binary alpha above a threshold.",
            Self::AlphaBand { .. } => "Map a mask into an alpha range.",
        }
    }
}

/// Every op the editor can add, grouped by [`Op::category`] in the UI.
pub fn catalog() -> Vec<Op> {
    vec![
        Op::Const { value: 32_768 },
        Op::Uv { axis: Axis::X },
        Op::ValueNoise {
            period: 8,
            octaves: 3,
        },
        Op::Fbm {
            period: 16,
            octaves: 4,
        },
        Op::Worley {
            period: 8,
            mode: WorleyMode::F1,
        },
        Op::Rings { scale: 6 },
        Op::Stripes {
            axis: Axis::X,
            count: 8,
            wobble: 4_000,
        },
        Op::Rect {
            x0: 8_192,
            y0: 8_192,
            x1: 57_344,
            y1: 57_344,
        },
        Op::HBand { y0: 0, y1: 16_384 },
        Op::Add,
        Op::Mul,
        Op::Min,
        Op::Max,
        Op::Invert,
        Op::Remap {
            from_lo: 0,
            from_hi: 65_535,
            to_lo: 0,
            to_hi: 65_535,
        },
        Op::Smoothstep {
            edge0: 10_000,
            edge1: 50_000,
        },
        Op::Threshold { at: 32_768 },
        Op::Quantize { levels: 4 },
        Op::Mix,
        Op::MixColor,
        Op::Warp { amount: 8_000 },
        Op::Ramp {
            stops: vec![(0, "#000000".to_owned()), (65_535, "#ffffff".to_owned())],
        },
        Op::Palette {
            colors: vec![
                "#2a6a1c".to_owned(),
                "#5fa83a".to_owned(),
                "#c8282a".to_owned(),
            ],
        },
        Op::Multiply,
        Op::Overlay,
        Op::AlphaFromMask,
        Op::AlphaBinary { at: 32_768 },
        Op::AlphaBand {
            min: 35_000,
            max: 50_000,
        },
    ]
}

fn invalid(message: String) -> GraphError {
    GraphError::Invalid(message)
}

fn node_stream(seed: u64, id: u32) -> u64 {
    splitmix64(seed ^ (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// SplitMix64, used only to decorrelate per-node streams from one seed.
pub fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

pub fn op_type(op: &Op) -> Type {
    match op {
        Op::Ramp { .. } | Op::Palette { .. } | Op::Multiply | Op::Overlay | Op::MixColor => {
            Type::Color
        }
        _ => Type::Scalar,
    }
}

pub fn op_breaks_h(op: &Op) -> bool {
    matches!(op, Op::Rect { .. })
}

pub fn op_breaks_v(op: &Op) -> bool {
    matches!(op, Op::Rect { .. } | Op::HBand { .. })
}

pub fn expected_inputs(op: &Op) -> Vec<Type> {
    use Type::{Color, Scalar};
    match op {
        Op::Const { .. }
        | Op::Uv { .. }
        | Op::ValueNoise { .. }
        | Op::Fbm { .. }
        | Op::Worley { .. }
        | Op::Rings { .. }
        | Op::Stripes { .. }
        | Op::Rect { .. }
        | Op::HBand { .. } => Vec::new(),
        Op::Add | Op::Mul | Op::Min | Op::Max => vec![Scalar, Scalar],
        Op::Invert
        | Op::Remap { .. }
        | Op::Smoothstep { .. }
        | Op::Threshold { .. }
        | Op::Quantize { .. }
        | Op::Ramp { .. }
        | Op::Palette { .. }
        | Op::AlphaFromMask
        | Op::AlphaBinary { .. }
        | Op::AlphaBand { .. } => vec![Scalar],
        Op::Mix => vec![Scalar, Scalar, Scalar],
        Op::MixColor => vec![Color, Color, Scalar],
        Op::Warp { .. } => vec![Scalar, Scalar, Scalar],
        Op::Multiply | Op::Overlay => vec![Color, Color],
    }
}

fn check_arity(key: &str, node: &Node, inputs: &[usize], types: &[Type]) -> Result<(), GraphError> {
    let expected = expected_inputs(&node.op);
    let optional_warp = matches!(node.op, Op::Warp { .. });
    let arity_ok = if optional_warp {
        inputs.len() == 2 || inputs.len() == 3
    } else {
        inputs.len() == expected.len()
    };
    if !arity_ok {
        return Err(invalid(format!(
            "{}: node {} takes {} input(s), got {}",
            key,
            node.id,
            expected.len(),
            inputs.len()
        )));
    }
    for (slot, input) in inputs.iter().enumerate() {
        let want = expected.get(slot).copied().unwrap_or(Type::Scalar);
        if types[*input] != want {
            return Err(invalid(format!(
                "{}: node {} input {slot} is {:?}, expected {want:?}",
                key, node.id, types[*input]
            )));
        }
    }
    Ok(())
}

fn compile_op(key: &str, id: u32, op: &Op) -> Result<ProgramOp, GraphError> {
    let parsed = match op {
        Op::Const { value } => ProgramOp::Const(*value),
        Op::Uv { axis } => ProgramOp::Uv(*axis),
        Op::ValueNoise { period, octaves } => {
            check_period(key, id, *period)?;
            check_octaves(key, id, *octaves)?;
            ProgramOp::ValueNoise {
                period: *period,
                octaves: *octaves,
            }
        }
        Op::Fbm { period, octaves } => {
            check_period(key, id, *period)?;
            check_octaves(key, id, *octaves)?;
            ProgramOp::Fbm {
                period: *period,
                octaves: *octaves,
            }
        }
        Op::Worley { period, mode } => {
            check_period(key, id, *period)?;
            ProgramOp::Worley {
                period: *period,
                mode: *mode,
            }
        }
        Op::Rings { scale } => {
            if *scale == 0 {
                return Err(invalid(format!("{key}: node {id} has scale 0")));
            }
            ProgramOp::Rings { scale: *scale }
        }
        Op::Stripes {
            axis,
            count,
            wobble,
        } => {
            if *count == 0 {
                return Err(invalid(format!("{key}: node {id} has count 0")));
            }
            ProgramOp::Stripes {
                axis: *axis,
                count: *count,
                wobble: *wobble,
            }
        }
        Op::Rect { x0, y0, x1, y1 } => {
            if x0 >= x1 || y0 >= y1 {
                return Err(invalid(format!(
                    "{key}: node {id} has an empty or inverted Rect"
                )));
            }
            ProgramOp::Rect {
                x0: *x0,
                y0: *y0,
                x1: *x1,
                y1: *y1,
            }
        }
        Op::HBand { y0, y1 } => {
            if y0 >= y1 {
                return Err(invalid(format!(
                    "{key}: node {id} has an empty or inverted HBand"
                )));
            }
            ProgramOp::HBand { y0: *y0, y1: *y1 }
        }
        Op::Add => ProgramOp::Add,
        Op::Mul => ProgramOp::Mul,
        Op::Min => ProgramOp::Min,
        Op::Max => ProgramOp::Max,
        Op::Invert => ProgramOp::Invert,
        Op::Remap {
            from_lo,
            from_hi,
            to_lo,
            to_hi,
        } => {
            if from_lo == from_hi {
                return Err(invalid(format!(
                    "{key}: node {id} has an empty Remap range"
                )));
            }
            ProgramOp::Remap {
                from_lo: *from_lo,
                from_hi: *from_hi,
                to_lo: *to_lo,
                to_hi: *to_hi,
            }
        }
        Op::Smoothstep { edge0, edge1 } => {
            if edge0 >= edge1 {
                return Err(invalid(format!(
                    "{key}: node {id} has an empty Smoothstep range"
                )));
            }
            ProgramOp::Smoothstep {
                edge0: *edge0,
                edge1: *edge1,
            }
        }
        Op::Threshold { at } => ProgramOp::Threshold { at: *at },
        Op::Quantize { levels } => {
            if *levels < 2 {
                return Err(invalid(format!("{key}: node {id} needs at least 2 levels")));
            }
            ProgramOp::Quantize { levels: *levels }
        }
        Op::Mix => ProgramOp::Mix,
        Op::MixColor => ProgramOp::MixColor,
        Op::Warp { amount } => ProgramOp::Warp { amount: *amount },
        Op::Ramp { stops } => {
            if stops.is_empty() {
                return Err(invalid(format!("{key}: node {id} has no ramp stops")));
            }
            let mut parsed = Vec::with_capacity(stops.len());
            for (position, color) in stops {
                parsed.push((*position, parse_color(key, id, color)?));
            }
            parsed.sort_by_key(|(position, _)| *position);
            ProgramOp::Ramp { stops: parsed }
        }
        Op::Palette { colors } => {
            if colors.is_empty() {
                return Err(invalid(format!("{key}: node {id} has an empty palette")));
            }
            let parsed = colors
                .iter()
                .map(|color| parse_color(key, id, color))
                .collect::<Result<Vec<_>, _>>()?;
            ProgramOp::Palette { colors: parsed }
        }
        Op::Multiply => ProgramOp::Multiply,
        Op::Overlay => ProgramOp::Overlay,
        Op::AlphaFromMask => ProgramOp::AlphaFromMask,
        Op::AlphaBinary { at } => ProgramOp::AlphaBinary { at: *at },
        Op::AlphaBand { min, max } => {
            if min > max {
                return Err(invalid(format!("{key}: node {id} has min > max")));
            }
            ProgramOp::AlphaBand {
                min: *min,
                max: *max,
            }
        }
    };
    Ok(parsed)
}

fn check_period(key: &str, id: u32, period: u32) -> Result<(), GraphError> {
    if period == 0 || period > 1024 {
        return Err(invalid(format!(
            "{key}: node {id} period {period} is outside 1..=1024"
        )));
    }
    Ok(())
}

fn check_octaves(key: &str, id: u32, octaves: u8) -> Result<(), GraphError> {
    if octaves == 0 || octaves > 8 {
        return Err(invalid(format!(
            "{key}: node {id} octaves {octaves} is outside 1..=8"
        )));
    }
    Ok(())
}

fn parse_color(key: &str, id: u32, color: &str) -> Result<[u8; 3], GraphError> {
    let hex = color.strip_prefix('#').ok_or_else(|| {
        invalid(format!(
            "{key}: node {id} colour {color:?} must start with `#`"
        ))
    })?;
    if hex.len() != 6 && hex.len() != 8 {
        return Err(invalid(format!(
            "{key}: node {id} colour {color:?} must be #rrggbb or #rrggbbaa"
        )));
    }
    let channel = |at: usize| {
        u8::from_str_radix(&hex[at..at + 2], 16).map_err(|_| {
            invalid(format!(
                "{key}: node {id} colour {color:?} has a non-hex channel"
            ))
        })
    };
    Ok([channel(0)?, channel(2)?, channel(4)?])
}
