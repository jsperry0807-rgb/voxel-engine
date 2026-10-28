//! Node canvas: pan, zoom, drag nodes, drag wires, add/delete nodes.

use std::collections::HashSet;

use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};

use crate::graph::{
    AlphaSource, Graph, Node, Op, Type, UiPosition, catalog, expected_inputs, op_type,
};

use super::TexgenApp;

const NODE_WIDTH: f32 = 196.0;
const HEADER_HEIGHT: f32 = 26.0;
const PORT_TOP: f32 = 38.0;
const PORT_GAP: f32 = 18.0;
const PORT_RADIUS: f32 = 4.5;

struct NodeDrag {
    id: u32,
    last_pointer: Pos2,
}

/// Canvas view state.
pub struct CanvasState {
    pub pan: Vec2,
    pub zoom: f32,
    drag: Option<NodeDrag>,
    link: Option<u32>,
    panning: Option<Pos2>,
    spawn: Pos2,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            pan: Vec2::new(40.0, 20.0),
            zoom: 1.0,
            drag: None,
            link: None,
            panning: None,
            spawn: Pos2::new(80.0, 80.0),
        }
    }
}

impl TexgenApp {
    pub(super) fn canvas_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let rect = ui.max_rect();
            let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            paint_grid(ui, rect, &self.canvas);
            self.canvas_input(ui, rect, &response);
            self.paint_connections(ui, rect);
            self.paint_nodes(ui, rect);
            self.paint_link(ui, rect);
            self.canvas_context_menu(&response);
        });
    }

    fn canvas_context_menu(&mut self, response: &egui::Response) {
        let mut add = None;
        response.context_menu(|ui| {
            ui.set_min_width(220.0);
            ui.label(RichText::new("Add node").strong());
            let categories = [
                "Source",
                "Pattern",
                "Shape",
                "Math",
                "Transform",
                "Color",
                "Blend",
                "Effect",
                "Alpha",
            ];
            for category in categories {
                ui.menu_button(category, |ui| {
                    for op in catalog().into_iter().filter(|op| op.category() == category) {
                        if ui
                            .button(op.label())
                            .on_hover_text(op.description())
                            .clicked()
                        {
                            add = Some(op);
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
            if ui
                .button(format!("Fit {} nodes", self.graph.nodes.len()))
                .clicked()
            {
                self.canvas = CanvasState::default();
                ui.close();
            }
        });
        if let Some(op) = add {
            self.add_node(op);
        }
    }

    fn canvas_input(&mut self, ui: &mut egui::Ui, rect: Rect, response: &egui::Response) {
        if response.hovered() {
            let scroll = ui.input(|input| input.raw_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let pointer = ui
                    .input(|input| input.pointer.hover_pos())
                    .unwrap_or(rect.center());
                let before = self.screen_to_graph(rect, pointer);
                let factor = (scroll * 0.0015).exp();
                self.canvas.zoom = (self.canvas.zoom * factor).clamp(0.25, 2.5);
                let after = self.graph_to_screen(rect, before);
                self.canvas.pan += pointer - after;
            }
        }

        let pointer = ui.input(|input| input.pointer.interact_pos());
        let primary_pressed =
            ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
        let primary_released =
            ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary));
        let middle_down = ui.input(|input| input.pointer.button_down(egui::PointerButton::Middle));
        let middle_pressed =
            ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Middle));

        if let Some(pointer) = pointer {
            if primary_pressed && response.hovered() {
                self.canvas.spawn = self.screen_to_graph(rect, pointer);
                if let Some(node) = self.port_at(rect, pointer) {
                    match node {
                        PortHit::Output(id) => {
                            self.canvas.link = Some(id);
                        }
                        PortHit::Input { .. } => {}
                    }
                } else if let Some(id) = self.node_at(rect, pointer) {
                    self.selected = Some(id);
                    self.snapshot();
                    self.canvas.drag = Some(NodeDrag {
                        id,
                        last_pointer: pointer,
                    });
                } else {
                    self.selected = None;
                    self.canvas.panning = Some(pointer);
                }
            }

            if middle_pressed && response.hovered() {
                self.canvas.panning = Some(pointer);
            }

            if let Some(drag) = &mut self.canvas.drag {
                let delta = (pointer - drag.last_pointer) / self.canvas.zoom;
                drag.last_pointer = pointer;
                let id = drag.id;
                if let Some(node) = self.node_mut(id) {
                    let position = node.ui.unwrap_or(UiPosition::new(0.0, 0.0));
                    node.ui = Some(UiPosition::new(position.x + delta.x, position.y + delta.y));
                    self.dirty = true;
                }
            } else if let Some(last) = self.canvas.panning {
                self.canvas.pan += pointer - last;
                self.canvas.panning = Some(pointer);
            }
        }

        if primary_released {
            if let Some(from) = self.canvas.link.take()
                && let Some(pointer) = pointer
                && let Some(PortHit::Input { node, slot }) = self.port_at(rect, pointer)
            {
                let want = self
                    .node(node)
                    .and_then(|target| expected_inputs(&target.op).get(slot).copied())
                    .unwrap_or(Type::Scalar);
                let have = self
                    .node(from)
                    .map(|source| op_type(&source.op))
                    .unwrap_or(Type::Scalar);
                if want == have {
                    self.snapshot();
                    if let Some(target) = self.node_mut(node) {
                        if slot < target.inputs.len() {
                            target.inputs[slot] = from;
                        } else {
                            target.inputs.push(from);
                        }
                    }
                    self.edited();
                } else {
                    self.error = Some(format!(
                        "cannot connect {have:?} output to a {want:?} input"
                    ));
                }
            }
            self.canvas.drag = None;
            self.canvas.panning = None;
        }
        if !middle_down && self.canvas.drag.is_none() && self.canvas.link.is_none() {
            self.canvas.panning = None;
        }
    }

    fn paint_nodes(&self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter();
        let types = self.node_types();
        for node in &self.graph.nodes {
            let node_rect = self.node_rect(rect, node);
            let selected = self.selected == Some(node.id);
            let category = node.op.category();
            let header = category_color(category);
            painter.rect_filled(node_rect, 6.0, Color32::from_rgb(31, 34, 39));
            painter.rect_filled(
                Rect::from_min_size(node_rect.min, Vec2::new(node_rect.width(), HEADER_HEIGHT)),
                6.0,
                header,
            );
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(node_rect.left(), node_rect.top() + HEADER_HEIGHT - 6.0),
                    Pos2::new(node_rect.right(), node_rect.top() + HEADER_HEIGHT),
                ),
                0.0,
                header,
            );
            let stroke = if selected {
                Stroke::new(2.0_f32, Color32::from_rgb(120, 200, 255))
            } else if node.inputs.iter().any(|input| !types.contains_key(input)) {
                Stroke::new(1.5_f32, Color32::from_rgb(230, 96, 96))
            } else {
                Stroke::new(1.0_f32, Color32::from_rgb(66, 71, 80))
            };
            painter.rect_stroke(node_rect, 6.0, stroke, egui::StrokeKind::Inside);
            let font = FontId::proportional((11.5 * self.canvas.zoom).clamp(8.0, 15.0));
            painter.text(
                Pos2::new(node_rect.left() + 8.0, node_rect.top() + 7.0),
                egui::Align2::LEFT_TOP,
                format!("{}  #{}", node.op.label(), node.id),
                font.clone(),
                Color32::from_rgb(235, 238, 244),
            );

            let input_types = expected_inputs(&node.op);
            for (slot, input) in node.inputs.iter().enumerate() {
                let position = self.input_port(rect, node, slot);
                let color = port_color(input_types.get(slot).copied().unwrap_or(Type::Scalar));
                painter.circle_filled(position, PORT_RADIUS * self.canvas.zoom.max(0.6), color);
                painter.text(
                    Pos2::new(position.x + 10.0, position.y - 6.0),
                    egui::Align2::LEFT_TOP,
                    format!("#{input}"),
                    FontId::proportional((9.5 * self.canvas.zoom).clamp(7.0, 12.0)),
                    Color32::from_rgb(150, 156, 166),
                );
            }
            if node.inputs.is_empty() {
                painter.text(
                    Pos2::new(node_rect.left() + 8.0, node_rect.top() + PORT_TOP - 8.0),
                    egui::Align2::LEFT_TOP,
                    "source",
                    FontId::proportional((9.5 * self.canvas.zoom).clamp(7.0, 12.0)),
                    Color32::from_rgb(130, 136, 146),
                );
            }
            let output = self.output_port(rect, node);
            painter.circle_filled(
                output,
                PORT_RADIUS * self.canvas.zoom.max(0.6),
                port_color(op_type(&node.op)),
            );
        }
    }

    fn paint_connections(&self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter();
        let types = self.node_types();
        for node in &self.graph.nodes {
            let input_types = expected_inputs(&node.op);
            for (slot, input) in node.inputs.iter().enumerate() {
                let Some(source) = self.node(*input) else {
                    continue;
                };
                let from = self.output_port(rect, source);
                let to = self.input_port(rect, node, slot);
                let color = port_color(input_types.get(slot).copied().unwrap_or(Type::Scalar));
                paint_wire(painter, from, to, color, self.canvas.zoom);
            }
        }
        let _ = types;
    }

    fn paint_link(&self, ui: &mut egui::Ui, rect: Rect) {
        let Some(from) = self.canvas.link else {
            return;
        };
        let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) else {
            return;
        };
        let Some(node) = self.node(from) else {
            return;
        };
        let start = self.output_port(rect, node);
        paint_wire(
            ui.painter(),
            start,
            pointer,
            port_color(op_type(&node.op)),
            self.canvas.zoom,
        );
    }

    fn node_at(&self, rect: Rect, pointer: Pos2) -> Option<u32> {
        self.graph
            .nodes
            .iter()
            .rev()
            .find(|node| self.node_rect(rect, node).contains(pointer))
            .map(|node| node.id)
    }

    fn port_at(&self, rect: Rect, pointer: Pos2) -> Option<PortHit> {
        let radius = (PORT_RADIUS * self.canvas.zoom.max(0.7) + 4.0).max(7.0);
        for node in self.graph.nodes.iter().rev() {
            let output = self.output_port(rect, node);
            if output.distance(pointer) <= radius {
                return Some(PortHit::Output(node.id));
            }
            for slot in 0..node.inputs.len() {
                let input = self.input_port(rect, node, slot);
                if input.distance(pointer) <= radius {
                    return Some(PortHit::Input {
                        node: node.id,
                        slot,
                    });
                }
            }
        }
        None
    }

    fn node_rect(&self, rect: Rect, node: &Node) -> Rect {
        let position = node.ui.unwrap_or(UiPosition::new(0.0, 0.0));
        let screen = self.graph_to_screen(rect, Pos2::new(position.x, position.y));
        let height = HEADER_HEIGHT + PORT_TOP + node.inputs.len().max(1) as f32 * PORT_GAP + 8.0;
        Rect::from_min_size(screen, Vec2::new(NODE_WIDTH, height) * self.canvas.zoom)
    }

    fn input_port(&self, rect: Rect, node: &Node, slot: usize) -> Pos2 {
        let node_rect = self.node_rect(rect, node);
        Pos2::new(
            node_rect.left(),
            node_rect.top() + (PORT_TOP + slot as f32 * PORT_GAP) * self.canvas.zoom,
        )
    }

    fn output_port(&self, rect: Rect, node: &Node) -> Pos2 {
        let node_rect = self.node_rect(rect, node);
        Pos2::new(
            node_rect.right(),
            node_rect.top() + PORT_TOP * self.canvas.zoom,
        )
    }

    fn graph_to_screen(&self, rect: Rect, position: Pos2) -> Pos2 {
        rect.min + self.canvas.pan + position.to_vec2() * self.canvas.zoom
    }

    fn screen_to_graph(&self, rect: Rect, position: Pos2) -> Pos2 {
        ((position - rect.min - self.canvas.pan) / self.canvas.zoom).to_pos2()
    }

    pub(super) fn add_node(&mut self, op: Op) {
        self.snapshot();
        let mut next_id = self
            .graph
            .nodes
            .iter()
            .map(|node| node.id)
            .max()
            .unwrap_or(0)
            + 1;
        let spawn = self.canvas.spawn;
        let expected = expected_inputs(&op);
        let mut inputs = Vec::with_capacity(expected.len());
        let mut fallbacks = Vec::new();
        for (slot, wanted) in expected.into_iter().enumerate() {
            let existing = self
                .graph
                .nodes
                .iter()
                .rev()
                .find(|node| op_type(&node.op) == wanted)
                .map(|node| node.id);
            if let Some(id) = existing {
                inputs.push(id);
                continue;
            }
            // Create a neutral source so the graph stays valid while the user
            // wires the real input.
            let position = UiPosition::new(spawn.x - 260.0, spawn.y + slot as f32 * 110.0);
            let id = next_id;
            next_id += 1;
            if wanted == Type::Color {
                let scalar_id = next_id;
                next_id += 1;
                fallbacks.push(Node {
                    id: scalar_id,
                    op: Op::Const { value: 32_768 },
                    inputs: Vec::new(),
                    ui: Some(position),
                });
                fallbacks.push(Node {
                    id,
                    op: Op::Palette {
                        colors: vec!["#8a8a8a".to_owned(), "#b0b0b0".to_owned()],
                    },
                    inputs: vec![scalar_id],
                    ui: Some(UiPosition::new(position.x + 210.0, position.y)),
                });
            } else {
                fallbacks.push(Node {
                    id,
                    op: Op::Const { value: 32_768 },
                    inputs: Vec::new(),
                    ui: Some(position),
                });
            }
            inputs.push(id);
        }
        self.graph.nodes.extend(fallbacks);
        let id = next_id;
        self.graph.nodes.push(Node {
            id,
            op,
            inputs,
            ui: Some(UiPosition::new(
                spawn.x,
                spawn.y + self.graph.nodes.len() as f32 % 5.0 * 20.0,
            )),
        });
        self.selected = Some(id);
        self.edited();
    }

    pub(super) fn delete_selected(&mut self) {
        let Some(id) = self.selected else {
            return;
        };
        self.snapshot();
        let mut doomed = HashSet::from([id]);
        loop {
            let mut grew = false;
            for node in &self.graph.nodes {
                if !doomed.contains(&node.id)
                    && node.inputs.iter().any(|input| doomed.contains(input))
                {
                    doomed.insert(node.id);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        self.graph.nodes.retain(|node| !doomed.contains(&node.id));
        if doomed.contains(&self.graph.output.color) {
            if let Some(color) = self
                .graph
                .nodes
                .iter()
                .rev()
                .find(|node| op_type(&node.op) == Type::Color)
                .map(|node| node.id)
            {
                self.graph.output.color = color;
            } else {
                let scalar = self
                    .graph
                    .nodes
                    .iter()
                    .map(|node| node.id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let color = scalar + 1;
                self.graph.nodes.push(Node {
                    id: scalar,
                    op: Op::Const { value: 32_768 },
                    inputs: Vec::new(),
                    ui: Some(UiPosition::new(40.0, 40.0)),
                });
                self.graph.nodes.push(Node {
                    id: color,
                    op: Op::Ramp {
                        stops: vec![(0, "#303030".to_owned()), (65_535, "#c0c0c0".to_owned())],
                    },
                    inputs: vec![scalar],
                    ui: Some(UiPosition::new(280.0, 40.0)),
                });
                self.graph.output.color = color;
            }
        }
        if let AlphaSource::Node(alpha) = self.graph.output.alpha
            && doomed.contains(&alpha)
        {
            self.graph.output.alpha = AlphaSource::Const(65_535);
        }
        self.selected = None;
        self.edited();
    }

    pub(super) fn duplicate_selected(&mut self) {
        let Some(node) = self.selected.and_then(|id| self.node(id)).cloned() else {
            return;
        };
        self.snapshot();
        let id = self
            .graph
            .nodes
            .iter()
            .map(|node| node.id)
            .max()
            .unwrap_or(0)
            + 1;
        let position = node.ui.unwrap_or(UiPosition::new(40.0, 40.0));
        self.graph.nodes.push(Node {
            id,
            op: node.op,
            inputs: node.inputs,
            ui: Some(UiPosition::new(position.x + 32.0, position.y + 32.0)),
        });
        self.selected = Some(id);
        self.edited();
    }

    pub(super) fn insert_graph(&mut self, path: &std::path::Path) {
        let mut other = match Graph::load(path) {
            Ok(graph) => graph,
            Err(error) => {
                self.error = Some(format!("insert failed: {error}"));
                return;
            }
        };
        other.ensure_layout();
        self.snapshot();
        let offset = self
            .graph
            .nodes
            .iter()
            .map(|node| node.id)
            .max()
            .unwrap_or(0)
            + 1;
        let spawn = self.canvas.spawn;
        let mut mapped = Vec::with_capacity(other.nodes.len());
        for mut node in other.nodes {
            node.id += offset;
            node.inputs = node
                .inputs
                .into_iter()
                .map(|input| input + offset)
                .collect();
            if let Some(ui) = node.ui {
                node.ui = Some(UiPosition::new(ui.x + spawn.x, ui.y + spawn.y));
            }
            mapped.push(node);
        }
        let output = other.output.color + offset;
        self.graph.nodes.extend(mapped);
        self.selected = Some(output);
        self.status = format!("inserted {} from {}", other.key, path.display());
        self.edited();
    }

    pub(super) fn save_selection_as_subgraph(&mut self) -> Result<(), String> {
        let Some(selected) = self.selected else {
            return Err("select a node first".to_owned());
        };
        let name = self.subgraph.name.trim().to_owned();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err("name must contain only letters, digits, `_` or `-`".to_owned());
        }

        let mut keep = HashSet::from([selected]);
        loop {
            let mut grew = false;
            for node in &self.graph.nodes {
                if keep.contains(&node.id) && node.inputs.iter().any(|input| keep.insert(*input)) {
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        let mut nodes: Vec<Node> = self
            .graph
            .nodes
            .iter()
            .filter(|node| keep.contains(&node.id))
            .cloned()
            .collect();
        if nodes.iter().all(|node| op_type(&node.op) != Type::Color) {
            return Err("selection has no colour node to use as output".to_owned());
        }
        let color_source = nodes
            .iter()
            .rev()
            .find(|node| node.id == selected && op_type(&node.op) == Type::Color)
            .map(|node| node.id)
            .or_else(|| {
                nodes
                    .iter()
                    .rev()
                    .find(|node| op_type(&node.op) == Type::Color)
                    .map(|node| node.id)
            })
            .unwrap();
        let alpha_source = match self.graph.output.alpha {
            AlphaSource::Node(id) if keep.contains(&id) => Some(id),
            _ => None,
        };

        let mut map = std::collections::HashMap::new();
        for (index, node) in nodes.iter().enumerate() {
            map.insert(node.id, index as u32);
        }
        let min_x = nodes
            .iter()
            .filter_map(|node| node.ui.map(|ui| ui.x))
            .fold(f32::MAX, f32::min);
        let min_y = nodes
            .iter()
            .filter_map(|node| node.ui.map(|ui| ui.y))
            .fold(f32::MAX, f32::min);
        for node in &mut nodes {
            node.id = map[&node.id];
            node.inputs = node
                .inputs
                .iter()
                .filter_map(|input| map.get(input).copied())
                .collect();
            if let Some(ui) = node.ui {
                node.ui = Some(UiPosition::new(ui.x - min_x + 40.0, ui.y - min_y + 40.0));
            }
        }
        let graph = Graph {
            schema: 1,
            key: format!("voxel:block/_subgraph/{name}"),
            size: crate::graph::GRAPH_SIZE,
            tiling: self.graph.tiling,
            alpha: self.graph.alpha,
            seed: 0,
            nodes,
            output: crate::graph::Output {
                color: map[&color_source],
                alpha: alpha_source
                    .map(|id| AlphaSource::Node(map[&id]))
                    .unwrap_or(AlphaSource::Const(65_535)),
            },
        };
        let path = self
            .workspace
            .graph_dir()
            .join("_subgraphs")
            .join(format!("{name}.ron"));
        graph.save(&path).map_err(|error| error.to_string())?;
        self.status = format!("saved subgraph {}", path.display());
        Ok(())
    }
}

enum PortHit {
    Output(u32),
    Input { node: u32, slot: usize },
}

fn paint_grid(ui: &egui::Ui, rect: Rect, canvas: &CanvasState) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, Color32::from_rgb(20, 22, 26));
    let step = 40.0 * canvas.zoom;
    if step < 12.0 {
        return;
    }
    let start_x = rect.left() + canvas.pan.x.rem_euclid(step);
    let start_y = rect.top() + canvas.pan.y.rem_euclid(step);
    let stroke = Stroke::new(0.5_f32, Color32::from_rgb(30, 33, 38));
    let mut x = start_x;
    while x < rect.right() {
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            stroke,
        );
        x += step;
    }
    let mut y = start_y;
    while y < rect.bottom() {
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            stroke,
        );
        y += step;
    }
}

fn paint_wire(painter: &egui::Painter, from: Pos2, to: Pos2, color: Color32, zoom: f32) {
    let pull = (60.0 * zoom).max(24.0);
    let points = [
        from,
        Pos2::new(from.x + pull, from.y),
        Pos2::new(to.x - pull, to.y),
        to,
    ];
    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
        points,
        false,
        Color32::TRANSPARENT,
        Stroke::new(2.0 * zoom.clamp(0.7, 1.4), color),
    ));
}

fn port_color(ty: Type) -> Color32 {
    match ty {
        Type::Scalar => Color32::from_rgb(224, 178, 88),
        Type::Color => Color32::from_rgb(104, 198, 218),
    }
}

fn category_color(category: &str) -> Color32 {
    match category {
        "Source" => Color32::from_rgb(58, 92, 130),
        "Pattern" => Color32::from_rgb(74, 104, 68),
        "Shape" => Color32::from_rgb(118, 96, 58),
        "Math" => Color32::from_rgb(98, 80, 130),
        "Transform" => Color32::from_rgb(62, 96, 110),
        "Color" => Color32::from_rgb(120, 76, 68),
        "Blend" => Color32::from_rgb(132, 84, 98),
        "Effect" => Color32::from_rgb(84, 88, 128),
        "Alpha" => Color32::from_rgb(92, 92, 96),
        _ => Color32::from_rgb(72, 74, 80),
    }
}
