//! Inspector parameter editors for every op.

use eframe::egui::{self, RichText};

use crate::graph::{Axis, Op, WorleyMode};

use super::TexgenApp;

impl TexgenApp {
    pub(super) fn params_ui(&mut self, ui: &mut egui::Ui, id: u32) {
        let Some(index) = self.graph.nodes.iter().position(|node| node.id == id) else {
            ui.label("Node is gone.");
            return;
        };
        let inputs = self.graph.nodes[index].inputs.clone();
        let node = &mut self.graph.nodes[index];
        ui.label(RichText::new(node.op.description()).weak());
        ui.add_space(4.0);

        match &mut node.op {
            Op::Const { value } => {
                scalar(ui, "Value", value);
            }
            Op::Uv { axis } => {
                axis_combo(ui, "Axis", axis, "uv-axis");
            }
            Op::ValueNoise { period, octaves } | Op::Fbm { period, octaves } => {
                ui.horizontal(|ui| {
                    ui.label("Period");
                    ui.add(egui::DragValue::new(period).range(1..=1024));
                });
                ui.horizontal(|ui| {
                    ui.label("Octaves");
                    ui.add(egui::DragValue::new(octaves).range(1..=8));
                });
            }
            Op::Worley { period, mode } => {
                ui.horizontal(|ui| {
                    ui.label("Period");
                    ui.add(egui::DragValue::new(period).range(1..=1024));
                });
                egui::ComboBox::from_id_salt("worley-mode")
                    .selected_text(format!("{mode:?}"))
                    .show_ui(ui, |ui| {
                        for value in [WorleyMode::F1, WorleyMode::F2, WorleyMode::Edge] {
                            ui.selectable_value(mode, value, format!("{value:?}"));
                        }
                    });
            }
            Op::Rings { scale } => {
                ui.horizontal(|ui| {
                    ui.label("Scale");
                    ui.add(egui::DragValue::new(scale).range(1..=64));
                });
            }
            Op::Stripes {
                axis,
                count,
                wobble,
            } => {
                axis_combo(ui, "Axis", axis, "stripe-axis");
                ui.horizontal(|ui| {
                    ui.label("Count");
                    ui.add(egui::DragValue::new(count).range(1..=128));
                });
                scalar(ui, "Wobble", wobble);
            }
            Op::Rect { x0, y0, x1, y1 } => {
                scalar(ui, "X0", x0);
                scalar(ui, "Y0", y0);
                scalar(ui, "X1", x1);
                scalar(ui, "Y1", y1);
            }
            Op::HBand { y0, y1 } => {
                scalar(ui, "Y0", y0);
                scalar(ui, "Y1", y1);
            }
            Op::Checker { period } => {
                ui.horizontal(|ui| {
                    ui.label("Period");
                    ui.add(egui::DragValue::new(period).range(1..=1024));
                });
            }
            Op::Bricks {
                rows,
                columns,
                mortar,
                offset,
            } => {
                ui.horizontal(|ui| {
                    ui.label("Rows");
                    ui.add(egui::DragValue::new(rows).range(1..=64));
                });
                ui.horizontal(|ui| {
                    ui.label("Columns");
                    ui.add(egui::DragValue::new(columns).range(1..=64));
                });
                scalar(ui, "Mortar", mortar);
                scalar(ui, "Row offset", offset);
            }
            Op::Wave { axis, count, phase } => {
                axis_combo(ui, "Axis", axis, "wave-axis");
                ui.horizontal(|ui| {
                    ui.label("Count");
                    ui.add(egui::DragValue::new(count).range(1..=64));
                });
                scalar(ui, "Phase", phase);
            }
            Op::Spiral { turns, arms } => {
                ui.horizontal(|ui| {
                    ui.label("Turns");
                    ui.add(egui::DragValue::new(turns).range(1..=64));
                });
                ui.horizontal(|ui| {
                    ui.label("Arms");
                    ui.add(egui::DragValue::new(arms).range(1..=64));
                });
            }
            Op::RadialGradient { radius } => scalar(ui, "Radius", radius),
            Op::Circle { radius, softness } => {
                scalar(ui, "Radius", radius);
                scalar(ui, "Softness", softness);
            }
            Op::Star {
                points,
                inner,
                rotation,
                softness,
            } => {
                ui.horizontal(|ui| {
                    ui.label("Points");
                    ui.add(egui::DragValue::new(points).range(3..=64));
                });
                scalar(ui, "Inner", inner);
                scalar(ui, "Rotation", rotation);
                scalar(ui, "Softness", softness);
            }
            Op::Polygon {
                sides,
                radius,
                rotation,
                softness,
            } => {
                ui.horizontal(|ui| {
                    ui.label("Sides");
                    ui.add(egui::DragValue::new(sides).range(3..=64));
                });
                scalar(ui, "Radius", radius);
                scalar(ui, "Rotation", rotation);
                scalar(ui, "Softness", softness);
            }
            Op::Add | Op::Mul | Op::Min | Op::Max => {}
            Op::Invert => {}
            Op::Remap {
                from_lo,
                from_hi,
                to_lo,
                to_hi,
            } => {
                scalar(ui, "From low", from_lo);
                scalar(ui, "From high", from_hi);
                scalar(ui, "To low", to_lo);
                scalar(ui, "To high", to_hi);
            }
            Op::Smoothstep { edge0, edge1 } => {
                scalar(ui, "Edge 0", edge0);
                scalar(ui, "Edge 1", edge1);
            }
            Op::Threshold { at } => scalar(ui, "At", at),
            Op::Quantize { levels } => {
                ui.horizontal(|ui| {
                    ui.label("Levels");
                    ui.add(egui::DragValue::new(levels).range(2..=32));
                });
            }
            Op::Mix | Op::MixColor => {}
            Op::Warp { amount } => scalar(ui, "Amount", amount),
            Op::Ramp { stops } => {
                let mut remove: Option<usize> = None;
                for (index, (position, color)) in stops.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(position).range(0..=65_535).speed(64.0));
                        let mut rgb = parse_hex(color);
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            *color = to_hex(rgb);
                        }
                        if ui.small_button("−").clicked() {
                            remove = Some(index);
                        }
                    });
                }
                if let Some(index) = remove
                    && stops.len() > 1
                {
                    stops.remove(index);
                }
                if ui.small_button("+ stop").clicked() {
                    let position = stops
                        .last()
                        .map(|(position, _)| position.saturating_add(4096))
                        .unwrap_or(0);
                    stops.push((position, "#808080".to_owned()));
                }
            }
            Op::Palette { colors } => {
                let mut remove: Option<usize> = None;
                for (index, color) in colors.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let mut rgb = parse_hex(color);
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            *color = to_hex(rgb);
                        }
                        ui.monospace(color.clone());
                        if ui.small_button("−").clicked() {
                            remove = Some(index);
                        }
                    });
                }
                if let Some(index) = remove
                    && colors.len() > 1
                {
                    colors.remove(index);
                }
                if ui.small_button("+ colour").clicked() {
                    colors.push("#808080".to_owned());
                }
            }
            Op::Multiply | Op::Overlay => {}
            Op::Rotate { angle } => scalar(ui, "Angle", angle),
            Op::Mirror { axis } => axis_combo(ui, "Axis", axis, "mirror-axis"),
            Op::Tile { x, y } => {
                ui.horizontal(|ui| {
                    ui.label("Repeat X");
                    ui.add(egui::DragValue::new(x).range(1..=64));
                });
                ui.horizontal(|ui| {
                    ui.label("Repeat Y");
                    ui.add(egui::DragValue::new(y).range(1..=64));
                });
            }
            Op::Offset { x, y } => {
                scalar(ui, "Offset X", x);
                scalar(ui, "Offset Y", y);
            }
            Op::Polar | Op::Cartesian | Op::Luminance => {}
            Op::Screen
            | Op::AddColor
            | Op::Subtract
            | Op::Difference
            | Op::Lighten
            | Op::Darken => {}
            Op::BrightnessContrast {
                brightness,
                contrast,
            } => {
                ui.horizontal(|ui| {
                    ui.label("Brightness");
                    ui.add(egui::DragValue::new(brightness).range(-32_768..=32_767));
                });
                scalar(ui, "Contrast", contrast);
            }
            Op::Saturate { amount } => scalar(ui, "Amount", amount),
            Op::HueShift { angle } => scalar(ui, "Angle", angle),
            Op::Blur { radius } => {
                ui.horizontal(|ui| {
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(radius).range(0..=4));
                });
            }
            Op::Emboss { strength } => scalar(ui, "Strength", strength),
            Op::NormalMap { strength } => scalar(ui, "Strength", strength),
            Op::Bevel { radius, strength } => {
                ui.horizontal(|ui| {
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(radius).range(1..=8));
                });
                scalar(ui, "Strength", strength);
            }
            Op::AlphaFromMask => {}
            Op::AlphaBinary { at } => scalar(ui, "Threshold", at),
            Op::AlphaBand { min, max } => {
                scalar(ui, "Min", min);
                scalar(ui, "Max", max);
            }
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Inputs").strong());
        if inputs.is_empty() {
            ui.label(RichText::new("none").weak());
        }
        for (slot, input) in inputs.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.monospace(format!("{slot}"));
                ui.label("←");
                ui.label(format!("#{input}"));
            });
        }
        if matches!(node.op, Op::Warp { .. }) {
            ui.label(
                RichText::new("Warp accepts two or three inputs; the third is optional.")
                    .weak()
                    .small(),
            );
        }
    }
}

fn scalar(ui: &mut egui::Ui, label: &str, value: &mut u16) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(value)
                .range(0..=65_535)
                .speed(64.0)
                .custom_formatter(|value, _| format!("{:.3}", value / 65_535.0)),
        );
    });
}

fn axis_combo(ui: &mut egui::Ui, label: &str, axis: &mut Axis, id: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(format!("{axis:?}"))
            .show_ui(ui, |ui| {
                ui.selectable_value(axis, Axis::X, "X");
                ui.selectable_value(axis, Axis::Y, "Y");
            });
    });
}

fn parse_hex(color: &str) -> [u8; 3] {
    let hex = color.trim_start_matches('#');
    // `is_ascii` makes the byte length a character length, so the channel
    // slices below can never land inside a multi-byte character.
    if hex.len() >= 6 && hex.is_ascii() {
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(128);
        [channel(0), channel(2), channel(4)]
    } else {
        [128, 128, 128]
    }
}

fn to_hex(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}
