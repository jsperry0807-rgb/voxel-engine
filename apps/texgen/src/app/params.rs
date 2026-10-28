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
    if hex.len() >= 6 {
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(128);
        [channel(0), channel(2), channel(4)]
    } else {
        [128, 128, 128]
    }
}

fn to_hex(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}
