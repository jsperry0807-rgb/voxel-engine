//! Deterministic fixed-point evaluator.
//!
//! Values are Q16.16 integers with `65536` representing `1.0`; scalar masks
//! are clamped to `0..=65535`. There is no `f32`/`f64` in this module, so the
//! same graph produces the same bytes on every host.
//!
//! Every spatial op wraps its lattice coordinates, which is what makes a graph
//! built from periodic ops tile at the tile boundary rather than merely look
//! close there.

use crate::graph::{
    AlphaSource, Axis, Program, ProgramAlpha, ProgramNode, ProgramOp, WorleyMode, splitmix64,
};
use crate::luts::{ATAN, SINE};

const ONE: i64 = 65_536;
const MAX: i32 = 65_535;
/// One source texel in Q16.16 UV space (graph size is 256).
const TEXEL: i32 = 256;

/// Renders a compiled graph at its declared size to row-major RGBA8 pixels.
pub fn evaluate(program: &Program) -> Vec<u8> {
    evaluate_at(program, program.size)
}

/// Renders a compiled graph at an arbitrary square edge.
///
/// The graph stays authored at 256x256; this only changes the sampling grid,
/// which is how the editor gets a fast 64x64 live preview. The final generator
/// always renders at [`crate::graph::GRAPH_SIZE`].
pub fn evaluate_at(program: &Program, size: u32) -> Vec<u8> {
    let edge = size as usize;
    let mut pixels = vec![0u8; edge * edge * 4];
    let mut memo = Memo::new(program.nodes.len());

    for y in 0..edge {
        let v = ((y as i64 * ONE) / edge as i64) as i32;
        for x in 0..edge {
            let u = ((x as i64 * ONE) / edge as i64) as i32;
            let color = eval_color(program, &mut memo, program.output.color, u, v);
            let alpha = match program.output.alpha {
                ProgramAlpha::Const(value) => value as i32,
                ProgramAlpha::Node(index) => eval_scalar(program, &mut memo, index, u, v),
            };
            let offset = (y * edge + x) * 4;
            pixels[offset] = color[0];
            pixels[offset + 1] = color[1];
            pixels[offset + 2] = color[2];
            pixels[offset + 3] = to_byte(alpha);
        }
    }
    pixels
}

/// Maps a Q16.16 scalar to an 8-bit value, round-half-up.
pub fn to_byte(value: i32) -> u8 {
    ((value.clamp(0, MAX) as i64 * 255 + MAX as i64 / 2) / MAX as i64) as u8
}

/// Last value computed per node, tagged with the coordinate it was computed at.
///
/// Spatial ops (`Warp`, `Rotate`, `Blur`, …) re-enter the evaluator at a
/// shifted coordinate, so an entry is only usable when both the node and the
/// coordinate match; otherwise it is a miss, never a wrong answer. This turns
/// a repeated-input fan-out (`Add(inputs: [i-1, i-1])`) from 2^n evaluations
/// per texel into one evaluation per node per coordinate.
#[derive(Default)]
struct Memo {
    scalar: Vec<Option<(i32, i32, i32)>>,
    color: Vec<Option<(i32, i32, [u8; 3])>>,
}

impl Memo {
    fn new(nodes: usize) -> Self {
        Self {
            scalar: vec![None; nodes],
            color: vec![None; nodes],
        }
    }
}

/// Counts uncached evaluations so a test can prove the memo is working.
#[cfg(test)]
static EVALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn eval_scalar(program: &Program, memo: &mut Memo, index: usize, u: i32, v: i32) -> i32 {
    if let Some((cached_u, cached_v, value)) = memo.scalar[index]
        && cached_u == u
        && cached_v == v
    {
        return value;
    }
    #[cfg(test)]
    EVALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let value = eval_scalar_uncached(program, memo, index, u, v);
    memo.scalar[index] = Some((u, v, value));
    value
}

fn eval_color(program: &Program, memo: &mut Memo, index: usize, u: i32, v: i32) -> [u8; 3] {
    if let Some((cached_u, cached_v, value)) = memo.color[index]
        && cached_u == u
        && cached_v == v
    {
        return value;
    }
    #[cfg(test)]
    EVALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let value = eval_color_uncached(program, memo, index, u, v);
    memo.color[index] = Some((u, v, value));
    value
}

fn scalar_input(
    program: &Program,
    memo: &mut Memo,
    node: &ProgramNode,
    slot: usize,
    u: i32,
    v: i32,
) -> i32 {
    eval_scalar(program, memo, node.inputs[slot], u, v)
}

fn eval_scalar_uncached(program: &Program, memo: &mut Memo, index: usize, u: i32, v: i32) -> i32 {
    let node = &program.nodes[index];
    let op: &ProgramOp = &node.op;
    match op {
        ProgramOp::Const(value) => *value as i32,
        ProgramOp::Uv(axis) => match axis {
            Axis::X => u,
            Axis::Y => v,
        },
        ProgramOp::ValueNoise { period, octaves } | ProgramOp::Fbm { period, octaves } => {
            value_noise(u, v, *period, *octaves, node.stream)
        }
        ProgramOp::Worley { period, mode } => worley(u, v, *period, *mode, node.stream),
        ProgramOp::Rings { scale } => rings(u, v, *scale),
        ProgramOp::Stripes {
            axis,
            count,
            wobble,
        } => stripes(*axis, *count, *wobble, u, v, node.stream),
        ProgramOp::Rect { x0, y0, x1, y1 } => {
            if u >= *x0 as i32 && u < *x1 as i32 && v >= *y0 as i32 && v < *y1 as i32 {
                MAX
            } else {
                0
            }
        }
        ProgramOp::HBand { y0, y1 } => {
            if v >= *y0 as i32 && v < *y1 as i32 {
                MAX
            } else {
                0
            }
        }
        ProgramOp::Checker { period } => {
            let period = *period as i64;
            let x = (u as i64 * period) >> 16;
            let y = (v as i64 * period) >> 16;
            if ((x + y) & 1) == 0 { MAX } else { 0 }
        }
        ProgramOp::Bricks {
            rows,
            columns,
            mortar,
            offset,
        } => {
            let rows = *rows as i64;
            let columns = *columns as i64;
            let px = u as i64 * columns;
            let py = v as i64 * rows;
            let row = py >> 16;
            let fy = (py & 0xFFFF) as i32;
            let shift = if (row & 1) == 0 { 0 } else { *offset as i64 };
            let fx = ((px + shift).rem_euclid(ONE)) as i32;
            let half = *mortar as i32 / 2;
            if fx >= half && fx <= MAX - half && fy >= half && fy <= MAX - half {
                MAX
            } else {
                0
            }
        }
        ProgramOp::Wave { axis, count, phase } => {
            let coordinate = match axis {
                Axis::X => u,
                Axis::Y => v,
            };
            sine(
                coordinate
                    .wrapping_mul(*count as i32)
                    .wrapping_add(*phase as i32),
            )
        }
        ProgramOp::Spiral { turns, arms } => {
            let (dx, dy) = centre_delta(u, v);
            let phase = (angle_of(dx, dy) as i64 * *arms as i64
                + toroidal_distance(u, v) as i64 * *turns as i64) as i32;
            sine(phase)
        }
        ProgramOp::RadialGradient { radius } => {
            let distance = toroidal_distance(u, v) as i64;
            let value = distance * MAX as i64 / (*radius as i64).max(1);
            (MAX as i64 - value).clamp(0, MAX as i64) as i32
        }
        ProgramOp::Circle { radius, softness } => {
            soft_mask(toroidal_distance(u, v), *radius as i32, *softness as i32)
        }
        ProgramOp::Star {
            points,
            inner,
            rotation,
            softness,
        } => {
            let (dx, dy) = centre_delta(u, v);
            let angle = angle_of(dx, dy);
            let phase = angle
                .wrapping_mul(*points as i32)
                .wrapping_add(*rotation as i32);
            let t = triangle(phase) as i64;
            let inner = *inner as i64 * 46_340 / MAX as i64;
            let limit = inner + (46_340 - inner) * t / MAX as i64;
            soft_mask(toroidal_distance(u, v), limit as i32, *softness as i32)
        }
        ProgramOp::Polygon {
            sides,
            radius,
            rotation,
            softness,
        } => {
            let (dx, dy) = centre_delta(u, v);
            let angle = angle_of(dx, dy).wrapping_add(*rotation as i32);
            let segment = 65_536 / *sides as i32;
            let local = angle.rem_euclid(65_536) % segment;
            let offset = local - segment / 2;
            let cos_offset = cosine(offset).max(1) as i64;
            let cos_segment = cosine(segment / 2).max(1) as i64;
            let circumradius = *radius as i64 * 46_340 / MAX as i64;
            let inradius = circumradius * cos_segment / MAX as i64;
            let limit = (inradius * MAX as i64 / cos_offset).min(46_340);
            soft_mask(toroidal_distance(u, v), limit as i32, *softness as i32)
        }
        ProgramOp::Add => {
            let a = scalar_input(program, memo, node, 0, u, v);
            let b = scalar_input(program, memo, node, 1, u, v);
            (a + b).clamp(0, MAX)
        }
        ProgramOp::Mul => {
            let a = scalar_input(program, memo, node, 0, u, v);
            let b = scalar_input(program, memo, node, 1, u, v);
            qmul(a, b)
        }
        ProgramOp::Min => {
            let a = scalar_input(program, memo, node, 0, u, v);
            let b = scalar_input(program, memo, node, 1, u, v);
            a.min(b)
        }
        ProgramOp::Max => {
            let a = scalar_input(program, memo, node, 0, u, v);
            let b = scalar_input(program, memo, node, 1, u, v);
            a.max(b)
        }
        ProgramOp::Invert => MAX - scalar_input(program, memo, node, 0, u, v),
        ProgramOp::Remap {
            from_lo,
            from_hi,
            to_lo,
            to_hi,
        } => {
            let value = scalar_input(program, memo, node, 0, u, v);
            remap(value, *from_lo, *from_hi, *to_lo, *to_hi)
        }
        ProgramOp::Smoothstep { edge0, edge1 } => {
            let value = scalar_input(program, memo, node, 0, u, v);
            smoothstep(*edge0 as i32, *edge1 as i32, value)
        }
        ProgramOp::Threshold { at } => {
            if scalar_input(program, memo, node, 0, u, v) >= *at as i32 {
                MAX
            } else {
                0
            }
        }
        ProgramOp::Quantize { levels } => {
            let value = scalar_input(program, memo, node, 0, u, v) as i64;
            let levels = *levels as i64;
            let step = ((value * levels + MAX as i64 / 2) / MAX as i64).min(levels);
            ((step * MAX as i64) / levels) as i32
        }
        ProgramOp::Mix => {
            let a = scalar_input(program, memo, node, 0, u, v);
            let b = scalar_input(program, memo, node, 1, u, v);
            let mask = scalar_input(program, memo, node, 2, u, v);
            lerp(a, b, mask)
        }
        ProgramOp::Warp { amount } => {
            let field = node.inputs[0];
            let warp_x = scalar_input(program, memo, node, 1, u, v);
            let dx = (((warp_x - 32_768) as i64 * *amount as i64) / MAX as i64) as i32;
            let dy = if node.inputs.len() > 2 {
                let warp_y = scalar_input(program, memo, node, 2, u, v);
                (((warp_y - 32_768) as i64 * *amount as i64) / MAX as i64) as i32
            } else {
                0
            };
            eval_scalar(
                program,
                memo,
                field,
                wrap(u.saturating_add(dx)),
                wrap(v.saturating_add(dy)),
            )
        }
        ProgramOp::AlphaFromMask => scalar_input(program, memo, node, 0, u, v),
        ProgramOp::AlphaBinary { at } => {
            if scalar_input(program, memo, node, 0, u, v) >= *at as i32 {
                MAX
            } else {
                0
            }
        }
        ProgramOp::AlphaBand { min, max } => {
            let value = scalar_input(program, memo, node, 0, u, v);
            remap(value, 0, MAX as u16, *min, *max)
        }
        ProgramOp::Luminance => {
            let color = eval_color(program, memo, node.inputs[0], u, v);
            (luma(color).min(255) * 257) as i32
        }
        ProgramOp::Ramp { .. }
        | ProgramOp::Palette { .. }
        | ProgramOp::Multiply
        | ProgramOp::Overlay
        | ProgramOp::MixColor
        | ProgramOp::Rotate { .. }
        | ProgramOp::Mirror { .. }
        | ProgramOp::Tile { .. }
        | ProgramOp::Offset { .. }
        | ProgramOp::Polar
        | ProgramOp::Cartesian
        | ProgramOp::Screen
        | ProgramOp::AddColor
        | ProgramOp::Subtract
        | ProgramOp::Difference
        | ProgramOp::Lighten
        | ProgramOp::Darken
        | ProgramOp::BrightnessContrast { .. }
        | ProgramOp::Saturate { .. }
        | ProgramOp::HueShift { .. }
        | ProgramOp::Blur { .. }
        | ProgramOp::Emboss { .. }
        | ProgramOp::NormalMap { .. }
        | ProgramOp::Bevel { .. } => unreachable!("colour op evaluated as a scalar"),
    }
}

fn eval_color_uncached(
    program: &Program,
    memo: &mut Memo,
    index: usize,
    u: i32,
    v: i32,
) -> [u8; 3] {
    let node = &program.nodes[index];
    match &node.op {
        ProgramOp::Ramp { stops } => {
            let value = scalar_input(program, memo, node, 0, u, v);
            sample_ramp(stops, value)
        }
        ProgramOp::Palette { colors } => {
            let value = scalar_input(program, memo, node, 0, u, v) as i64;
            let last = colors.len() as i64 - 1;
            let pick = ((value * last + MAX as i64 / 2) / MAX as i64) as usize;
            colors[pick.min(colors.len() - 1)]
        }
        ProgramOp::Multiply => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            [
                (a[0] as u16 * b[0] as u16 / 255) as u8,
                (a[1] as u16 * b[1] as u16 / 255) as u8,
                (a[2] as u16 * b[2] as u16 / 255) as u8,
            ]
        }
        ProgramOp::Overlay => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            [
                overlay_channel(a[0], b[0]),
                overlay_channel(a[1], b[1]),
                overlay_channel(a[2], b[2]),
            ]
        }
        ProgramOp::MixColor => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            let mask = scalar_input(program, memo, node, 2, u, v);
            [
                lerp_byte(a[0], b[0], mask),
                lerp_byte(a[1], b[1], mask),
                lerp_byte(a[2], b[2], mask),
            ]
        }
        ProgramOp::Rotate { angle } => {
            let (dx, dy) = centre_delta(u, v);
            let (rx, ry) = rotate_point(dx, dy, *angle as i32);
            eval_color(
                program,
                memo,
                node.inputs[0],
                wrap(32_768 + rx),
                wrap(32_768 + ry),
            )
        }
        ProgramOp::Mirror { axis } => {
            let (su, sv) = match axis {
                Axis::X => (MAX - u, v),
                Axis::Y => (u, MAX - v),
            };
            eval_color(program, memo, node.inputs[0], wrap(su), wrap(sv))
        }
        ProgramOp::Tile { x, y } => {
            let su = (u as i64 * *x as i64).rem_euclid(ONE) as i32;
            let sv = (v as i64 * *y as i64).rem_euclid(ONE) as i32;
            eval_color(program, memo, node.inputs[0], su, sv)
        }
        ProgramOp::Offset { x, y } => eval_color(
            program,
            memo,
            node.inputs[0],
            wrap(u + *x as i32),
            wrap(v + *y as i32),
        ),
        ProgramOp::Polar => {
            let angle = u;
            let radius = v as i64 * 46_340 / MAX as i64;
            let (dx, dy) = direction(angle, radius as i32);
            eval_color(
                program,
                memo,
                node.inputs[0],
                wrap(32_768 + dx),
                wrap(32_768 + dy),
            )
        }
        ProgramOp::Cartesian => {
            let (dx, dy) = centre_delta(u, v);
            let angle = angle_of(dx, dy);
            let radius = scale_distance(toroidal_distance(u, v) as i64);
            eval_color(program, memo, node.inputs[0], angle, radius)
        }
        ProgramOp::Screen => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            [
                screen_channel(a[0], b[0]),
                screen_channel(a[1], b[1]),
                screen_channel(a[2], b[2]),
            ]
        }
        ProgramOp::AddColor => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            [
                a[0].saturating_add(b[0]),
                a[1].saturating_add(b[1]),
                a[2].saturating_add(b[2]),
            ]
        }
        ProgramOp::Subtract => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            [
                a[0].saturating_sub(b[0]),
                a[1].saturating_sub(b[1]),
                a[2].saturating_sub(b[2]),
            ]
        }
        ProgramOp::Difference => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            [
                a[0].abs_diff(b[0]),
                a[1].abs_diff(b[1]),
                a[2].abs_diff(b[2]),
            ]
        }
        ProgramOp::Lighten => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])]
        }
        ProgramOp::Darken => {
            let a = eval_color(program, memo, node.inputs[0], u, v);
            let b = eval_color(program, memo, node.inputs[1], u, v);
            [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])]
        }
        ProgramOp::BrightnessContrast {
            brightness,
            contrast,
        } => {
            let color = eval_color(program, memo, node.inputs[0], u, v);
            [
                adjust_channel(color[0], *brightness, *contrast),
                adjust_channel(color[1], *brightness, *contrast),
                adjust_channel(color[2], *brightness, *contrast),
            ]
        }
        ProgramOp::Saturate { amount } => {
            let color = eval_color(program, memo, node.inputs[0], u, v);
            let luma = luma(color) as u8;
            [
                lerp_byte(luma, color[0], *amount as i32),
                lerp_byte(luma, color[1], *amount as i32),
                lerp_byte(luma, color[2], *amount as i32),
            ]
        }
        ProgramOp::HueShift { angle } => {
            let color = eval_color(program, memo, node.inputs[0], u, v);
            hue_shift(color, *angle as i32)
        }
        ProgramOp::Blur { radius } => {
            let radius = *radius as i32;
            if radius == 0 {
                return eval_color(program, memo, node.inputs[0], u, v);
            }
            let mut sums = [0u32; 3];
            let mut count = 0u32;
            for oy in -radius..=radius {
                for ox in -radius..=radius {
                    let color = eval_color(
                        program,
                        memo,
                        node.inputs[0],
                        wrap(u + ox * TEXEL),
                        wrap(v + oy * TEXEL),
                    );
                    for (sum, channel) in sums.iter_mut().zip(color) {
                        *sum += channel as u32;
                    }
                    count += 1;
                }
            }
            [
                ((sums[0] + count / 2) / count) as u8,
                ((sums[1] + count / 2) / count) as u8,
                ((sums[2] + count / 2) / count) as u8,
            ]
        }
        ProgramOp::Emboss { strength } => {
            let plus = eval_color(
                program,
                memo,
                node.inputs[0],
                wrap(u + TEXEL),
                wrap(v + TEXEL),
            );
            let minus = eval_color(
                program,
                memo,
                node.inputs[0],
                wrap(u - TEXEL),
                wrap(v - TEXEL),
            );
            let difference = (luma(plus) - luma(minus)) * *strength as i64 / MAX as i64;
            let value = (128 + difference).clamp(0, 255) as u8;
            [value, value, value]
        }
        ProgramOp::NormalMap { strength } => {
            let left = luma(eval_color(
                program,
                memo,
                node.inputs[0],
                wrap(u - TEXEL),
                v,
            ));
            let right = luma(eval_color(
                program,
                memo,
                node.inputs[0],
                wrap(u + TEXEL),
                v,
            ));
            let top = luma(eval_color(
                program,
                memo,
                node.inputs[0],
                u,
                wrap(v - TEXEL),
            ));
            let bottom = luma(eval_color(
                program,
                memo,
                node.inputs[0],
                u,
                wrap(v + TEXEL),
            ));
            let scale = *strength as i64;
            let nx = -(right - left) * scale / MAX as i64;
            let ny = -(bottom - top) * scale / MAX as i64;
            let nz = 255i64;
            let length = isqrt((nx * nx + ny * ny + nz * nz) as u64).max(1) as i64;
            [
                (128 + nx * 127 / length).clamp(0, 255) as u8,
                (128 + ny * 127 / length).clamp(0, 255) as u8,
                (nz * 255 / length).clamp(0, 255) as u8,
            ]
        }
        ProgramOp::Bevel { radius, strength } => {
            let offset = *radius as i32 * TEXEL;
            let top = luma(eval_color(
                program,
                memo,
                node.inputs[0],
                u,
                wrap(v - offset),
            ));
            let left = luma(eval_color(
                program,
                memo,
                node.inputs[0],
                wrap(u - offset),
                v,
            ));
            let bottom = luma(eval_color(
                program,
                memo,
                node.inputs[0],
                u,
                wrap(v + offset),
            ));
            let right = luma(eval_color(
                program,
                memo,
                node.inputs[0],
                wrap(u + offset),
                v,
            ));
            let light = (top + left) - (bottom + right);
            let value = (128 + light * *strength as i64 / (MAX as i64 * 4)).clamp(0, 255) as u8;
            [value, value, value]
        }
        _ => unreachable!("scalar op evaluated as a colour"),
    }
}

fn overlay_channel(a: u8, b: u8) -> u8 {
    if a < 128 {
        (2 * a as u16 * b as u16 / 255) as u8
    } else {
        (255 - 2 * (255 - a as u16) * (255 - b as u16) / 255) as u8
    }
}

fn sample_ramp(stops: &[(u16, [u8; 3])], value: i32) -> [u8; 3] {
    let value = value.clamp(0, MAX);
    if value <= stops[0].0 as i32 {
        return stops[0].1;
    }
    let last = stops[stops.len() - 1];
    if value >= last.0 as i32 {
        return last.1;
    }
    for pair in stops.windows(2) {
        let (lo, lo_color) = pair[0];
        let (hi, hi_color) = pair[1];
        if value >= lo as i32 && value <= hi as i32 {
            let span = (hi - lo) as i64;
            if span == 0 {
                return hi_color;
            }
            let t = ((value - lo as i32) as i64 * MAX as i64 / span) as i32;
            return [
                lerp_byte(lo_color[0], hi_color[0], t),
                lerp_byte(lo_color[1], hi_color[1], t),
                lerp_byte(lo_color[2], hi_color[2], t),
            ];
        }
    }
    last.1
}

fn lerp_byte(a: u8, b: u8, t: i32) -> u8 {
    (a as i64 + ((b as i64 - a as i64) * t as i64 / MAX as i64)) as u8
}

fn qmul(a: i32, b: i32) -> i32 {
    (((a as i64 * b as i64) + MAX as i64 / 2) / MAX as i64).clamp(0, MAX as i64) as i32
}

fn lerp(a: i32, b: i32, t: i32) -> i32 {
    (a as i64 + ((b as i64 - a as i64) * t as i64 / MAX as i64)).clamp(0, MAX as i64) as i32
}

fn remap(value: i32, from_lo: u16, from_hi: u16, to_lo: u16, to_hi: u16) -> i32 {
    let from_lo = from_lo as i32;
    let from_hi = from_hi as i32;
    if from_lo == from_hi {
        return to_lo as i32;
    }
    let t = if from_lo < from_hi {
        ((value - from_lo) as i64 * MAX as i64 / (from_hi - from_lo) as i64).clamp(0, MAX as i64)
            as i32
    } else {
        ((from_lo - value) as i64 * MAX as i64 / (from_lo - from_hi) as i64).clamp(0, MAX as i64)
            as i32
    };
    lerp(to_lo as i32, to_hi as i32, t)
}

fn smoothstep(edge0: i32, edge1: i32, value: i32) -> i32 {
    if value <= edge0 {
        return 0;
    }
    if value >= edge1 {
        return MAX;
    }
    let t = (value - edge0) as i64 * MAX as i64 / (edge1 - edge0) as i64;
    let t2 = t * t;
    let three_minus_two_t = 3 * MAX as i64 - 2 * t;
    (t2 * three_minus_two_t / (MAX as i64 * MAX as i64)) as i32
}

fn wrap(value: i32) -> i32 {
    value.rem_euclid(ONE as i32)
}

fn sine(phase: i32) -> i32 {
    SINE[(phase.rem_euclid(65_536) >> 6) as usize] as i32
}

fn cosine(phase: i32) -> i32 {
    sine(phase.wrapping_add(16_384))
}

fn signed_sine(phase: i32) -> i32 {
    sine(phase) - 32_768
}

fn signed_cosine(phase: i32) -> i32 {
    cosine(phase) - 32_768
}

fn triangle(phase: i32) -> i32 {
    let phase = phase.rem_euclid(65_536);
    if phase < 32_768 {
        phase * 2
    } else {
        (65_536 - phase) * 2
    }
}

fn centre_delta(u: i32, v: i32) -> (i32, i32) {
    (u - 32_768, v - 32_768)
}

fn toroidal_distance(u: i32, v: i32) -> i32 {
    let (dx, dy) = centre_delta(u, v);
    let dx = dx.abs().min(ONE as i32 - dx.abs());
    let dy = dy.abs().min(ONE as i32 - dy.abs());
    isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i32
}

fn scale_distance(distance: i64) -> i32 {
    (distance * MAX as i64 / 46_340).clamp(0, MAX as i64) as i32
}

fn angle_of(dx: i32, dy: i32) -> i32 {
    let ax = dx.abs();
    let ay = dy.abs();
    if ax == 0 && ay == 0 {
        return 0;
    }
    let (max, min) = if ax >= ay { (ax, ay) } else { (ay, ax) };
    let ratio = (min as i64 * 256 / max as i64) as usize;
    let base = ATAN[ratio.min(256)] as i32;
    let angle = if ax >= ay { base } else { 16_384 - base };
    let angle = match (dx >= 0, dy >= 0) {
        (true, true) => angle,
        (false, true) => 32_768 - angle,
        (false, false) => 32_768 + angle,
        (true, false) => 65_536 - angle,
    };
    angle.rem_euclid(65_536)
}

fn soft_mask(distance: i32, radius: i32, softness: i32) -> i32 {
    if softness <= 0 {
        return if distance <= radius { MAX } else { 0 };
    }
    let inner = (radius - softness).max(0);
    let outer = radius.saturating_add(softness);
    if distance <= inner {
        return MAX;
    }
    if distance >= outer {
        return 0;
    }
    let t = ((distance - inner) as i64 * MAX as i64 / (outer - inner) as i64) as i32;
    MAX - smoothstep(0, MAX, t)
}

fn rotate_point(dx: i32, dy: i32, angle: i32) -> (i32, i32) {
    let c = signed_cosine(angle) as i64;
    let s = signed_sine(angle) as i64;
    (
        ((dx as i64 * c - dy as i64 * s) >> 15) as i32,
        ((dx as i64 * s + dy as i64 * c) >> 15) as i32,
    )
}

fn direction(angle: i32, radius: i32) -> (i32, i32) {
    let c = signed_cosine(angle) as i64;
    let s = signed_sine(angle) as i64;
    (
        (radius as i64 * c / 32_768) as i32,
        (radius as i64 * s / 32_768) as i32,
    )
}

fn luma(color: [u8; 3]) -> i64 {
    (77 * color[0] as i64 + 150 * color[1] as i64 + 29 * color[2] as i64) >> 8
}

fn screen_channel(a: u8, b: u8) -> u8 {
    (255 - (255 - a as u16) * (255 - b as u16) / 255) as u8
}

fn adjust_channel(channel: u8, brightness: i16, contrast: u16) -> u8 {
    let value = (channel as i64 - 128) * contrast as i64 / 32_768
        + 128
        + brightness as i64 * 255 / MAX as i64;
    value.clamp(0, 255) as u8
}

fn hue_shift(color: [u8; 3], angle: i32) -> [u8; 3] {
    let r = color[0] as i64;
    let g = color[1] as i64;
    let b = color[2] as i64;
    let y = (77 * r + 150 * g + 29 * b) >> 8;
    let i = (596 * r - 274 * g - 322 * b) >> 10;
    let q = (211 * r - 523 * g + 312 * b) >> 10;
    let c = signed_cosine(angle) as i64;
    let s = signed_sine(angle) as i64;
    let i2 = (i * c - q * s) >> 15;
    let q2 = (i * s + q * c) >> 15;
    let red = y + (956 * i2 + 621 * q2) / 1000;
    let green = y + (-272 * i2 - 647 * q2) / 1000;
    let blue = y + (-1106 * i2 + 1703 * q2) / 1000;
    [
        red.clamp(0, 255) as u8,
        green.clamp(0, 255) as u8,
        blue.clamp(0, 255) as u8,
    ]
}

fn value_noise(u: i32, v: i32, period: u32, octaves: u8, stream: u64) -> i32 {
    let mut total = 0i64;
    let mut weight = 0i64;
    let mut amplitude = 1i64 << (octaves.saturating_sub(1));
    let mut cell_period = period as i64;
    for octave in 0..octaves {
        total += amplitude * noise_octave(u, v, cell_period, stream, octave as u64) as i64;
        weight += amplitude;
        amplitude = (amplitude / 2).max(1);
        cell_period *= 2;
    }
    (total / weight.max(1)) as i32
}

fn noise_octave(u: i32, v: i32, period: i64, stream: u64, octave: u64) -> i32 {
    let period = period.max(1);
    let px = u as i64 * period;
    let py = v as i64 * period;
    let x = px >> 16;
    let y = py >> 16;
    let fx = (px & 0xFFFF) as i32;
    let fy = (py & 0xFFFF) as i32;
    let x0 = x.rem_euclid(period);
    let y0 = y.rem_euclid(period);
    let x1 = (x0 + 1) % period;
    let y1 = (y0 + 1) % period;

    let a = hash16(stream, x0, y0, octave) as i32;
    let b = hash16(stream, x1, y0, octave) as i32;
    let c = hash16(stream, x0, y1, octave) as i32;
    let d = hash16(stream, x1, y1, octave) as i32;

    let sx = smoothstep(0, MAX, fx);
    let sy = smoothstep(0, MAX, fy);
    let top = lerp(a, b, sx);
    let bottom = lerp(c, d, sx);
    lerp(top, bottom, sy)
}

fn worley(u: i32, v: i32, period: u32, mode: WorleyMode, stream: u64) -> i32 {
    let period = period.max(1) as i64;
    let px = u as i64 * period;
    let py = v as i64 * period;
    let cell_x = px >> 16;
    let cell_y = py >> 16;

    let mut nearest = u64::MAX;
    let mut second = u64::MAX;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let nx = cell_x + dx;
            let ny = cell_y + dy;
            let wrapped_x = nx.rem_euclid(period);
            let wrapped_y = ny.rem_euclid(period);
            let feature_x = hash16(stream, wrapped_x, wrapped_y, 1) as i64;
            let feature_y = hash16(stream, wrapped_x, wrapped_y, 2) as i64;
            let ddx = nx * 65_536 + feature_x - px;
            let ddy = ny * 65_536 + feature_y - py;
            let distance = isqrt((ddx * ddx + ddy * ddy) as u64);
            if distance < nearest {
                second = nearest;
                nearest = distance;
            } else if distance < second {
                second = distance;
            }
        }
    }
    let scale = |distance: u64| -> i32 { ((distance * 2 / 3).min(MAX as u64)) as i32 };
    match mode {
        WorleyMode::F1 => scale(nearest),
        WorleyMode::F2 => scale(second),
        WorleyMode::Edge => (scale(second) - scale(nearest)).clamp(0, MAX),
    }
}

fn rings(u: i32, v: i32, scale: u32) -> i32 {
    let dx = (u - 32_768).abs();
    let dx = dx.min(ONE as i32 - dx);
    let dy = (v - 32_768).abs();
    let dy = dy.min(ONE as i32 - dy);
    let distance = isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i64;
    let phase = (distance * scale as i64) % ONE;
    let triangle = if phase < ONE / 2 {
        phase * 2
    } else {
        (ONE - phase) * 2
    };
    triangle.min(MAX as i64) as i32
}

fn stripes(axis: Axis, count: u32, wobble: u16, u: i32, v: i32, stream: u64) -> i32 {
    let mut coordinate = match axis {
        Axis::X => u as i64,
        Axis::Y => v as i64,
    };
    if wobble != 0 {
        // The wobble field is periodic at the stripe frequency, so it cannot
        // itself create a seam.
        let noise = value_noise(u, v, count, 1, splitmix64(stream ^ 0x5EED_5EED)) as i64;
        coordinate += (noise - 32_768) * wobble as i64 / MAX as i64;
    }
    let phase = (coordinate * count as i64).rem_euclid(ONE);
    let triangle = if phase < ONE / 2 {
        phase * 2
    } else {
        (ONE - phase) * 2
    };
    triangle.min(MAX as i64) as i32
}

fn hash16(stream: u64, x: i64, y: i64, key: u64) -> u32 {
    let mixed = stream
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ key.wrapping_mul(0x1656_67B1_9E37_79F9);
    (splitmix64(mixed) >> 48) as u32
}

/// Integer square root (Newton iteration).
fn isqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut x = 1u64 << (64 - value.leading_zeros() as u64).div_ceil(2);
    loop {
        let next = (x + value / x) / 2;
        if next >= x {
            return x;
        }
        x = next;
    }
}

/// Re-exported so the manifest writer and tests can derive an alpha node
/// without duplicating the source enum.
pub fn alpha_source_value(source: &AlphaSource) -> Option<u16> {
    match source {
        AlphaSource::Const(value) => Some(*value),
        AlphaSource::Node(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_mapping_is_round_half_up() {
        assert_eq!(to_byte(0), 0);
        assert_eq!(to_byte(32_768), 128);
        assert_eq!(to_byte(65_535), 255);
    }

    #[test]
    fn integer_sqrt_matches_squares() {
        for value in 0u64..2_000 {
            let root = isqrt(value);
            assert!(root * root <= value);
            assert!((root + 1) * (root + 1) > value, "{value}");
        }
    }

    #[test]
    fn smoothstep_is_monotonic_and_clamped() {
        assert_eq!(smoothstep(0, MAX, 0), 0);
        assert_eq!(smoothstep(0, MAX, MAX), MAX);
        let mut previous = 0;
        for value in (0..=MAX).step_by(257) {
            let current = smoothstep(0, MAX, value);
            assert!(current >= previous);
            previous = current;
        }
    }

    /// A chain where every node feeds the next one twice (`Add([i-1, i-1])`)
    /// evaluates to 2^n calls per texel without a memo.
    #[test]
    fn repeated_inputs_are_evaluated_once_per_pixel() {
        use std::sync::atomic::Ordering;

        use crate::graph::ProgramOutput;
        use voxel_block::texture_manifest::{AlphaClass, Tiling};

        const DEPTH: usize = 22;
        let mut nodes = vec![ProgramNode {
            id: 0,
            op: ProgramOp::Const(1),
            inputs: Vec::new(),
            stream: 0,
        }];
        for index in 1..=DEPTH {
            nodes.push(ProgramNode {
                id: index as u32,
                op: ProgramOp::Add,
                inputs: vec![index - 1, index - 1],
                stream: 0,
            });
        }
        let color = DEPTH + 1;
        nodes.push(ProgramNode {
            id: color as u32,
            op: ProgramOp::Ramp {
                stops: vec![(0, [0, 0, 0]), (u16::MAX, [255, 255, 255])],
            },
            inputs: vec![DEPTH],
            stream: 0,
        });
        let program = Program {
            key: "diamond".to_owned(),
            size: 8,
            tiling: Tiling::Both,
            alpha: AlphaClass::Opaque,
            seed: 0,
            nodes,
            output: ProgramOutput {
                color,
                alpha: ProgramAlpha::Const(255),
            },
            breaks_h: false,
            breaks_v: false,
        };

        EVALS.store(0, Ordering::Relaxed);
        let pixels = evaluate(&program);
        assert_eq!(pixels.len(), 8 * 8 * 4);
        let evals = EVALS.load(Ordering::Relaxed);
        assert_eq!(evals, 64 * (color + 1), "one evaluation per node per pixel");
    }
}
