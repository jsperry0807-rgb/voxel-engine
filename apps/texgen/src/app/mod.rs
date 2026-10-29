//! The egui editor: a Material Maker-style node canvas for this engine's
//! fixed-point texture graphs.
//!
//! The editor is intentionally a client of [`crate::graph`] and
//! [`crate::eval`]. A saved graph is the canonical input; the CPU evaluator is
//! the authority; PNG, manifest, material-table and atlas writes all go
//! through [`crate::pipeline`].

mod canvas;
mod params;
mod preview;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, Pos2, RichText, TextureHandle, Vec2};
use voxel_block::texture_manifest::{AlphaClass, TextureManifest, Tiling};

use crate::bake::{self, AtlasBuild};
use crate::graph::{Graph, Op, UiPosition};
use crate::pipeline::{self, MaterialTable, Workspace};

use canvas::CanvasState;

/// How long the editor waits after an edit before re-evaluating a preview.
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(90);
/// Undo history limit.
const HISTORY_LIMIT: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Inspect,
    Atlas,
    Materials,
    Build,
}

/// One graph in the library.
pub(crate) struct LibraryEntry {
    pub key: String,
    pub slot: u16,
    pub path: PathBuf,
    pub alpha: Option<AlphaClass>,
    pub tiling: Option<Tiling>,
}

/// A rendered live preview.
pub(crate) struct Preview {
    pub source: TextureHandle,
    pub tile: TextureHandle,
    pub edge: u32,
}

/// The atlas preview tab.
pub(crate) struct AtlasPreview {
    pub texture: Option<TextureHandle>,
    pub build: Option<AtlasBuild>,
    pub error: Option<String>,
}

/// The material browser tab.
pub(crate) struct MaterialPreview {
    pub manifest: Option<TextureManifest>,
    pub table: Option<MaterialTable>,
    pub error: Option<String>,
}

/// A pipeline build running on a worker thread.
pub(crate) struct BuildJob {
    label: String,
    receiver: Receiver<Result<String, String>>,
}

/// Dialog state for saving a selection as a reusable subgraph.
pub(crate) struct SubgraphDialog {
    pub open: bool,
    pub name: String,
    pub error: Option<String>,
}

/// The application.
pub struct TexgenApp {
    pub(crate) workspace: Workspace,
    pub(crate) graph: Graph,
    pub(crate) graph_path: Option<PathBuf>,
    pub(crate) library: Vec<LibraryEntry>,
    pub(crate) library_filter: String,
    pub(crate) node_filter: String,
    pub(crate) selected: Option<u32>,
    pub(crate) tab: Tab,
    pub(crate) status: String,
    pub(crate) error: Option<String>,
    pub(crate) build_log: Vec<String>,
    pub(crate) build_job: Option<BuildJob>,

    pub(crate) history: Vec<Graph>,
    pub(crate) redo: Vec<Graph>,
    pub(crate) dirty: bool,
    pub(crate) generate_on_save: bool,

    pub(crate) preview: Option<Preview>,
    pub(crate) preview_size: u32,
    pub(crate) show_tiled: bool,
    pub(crate) show_checker: bool,
    pub(crate) preview_due: Option<Instant>,
    pub(crate) preview_error: Option<String>,
    pub(crate) revision: u64,

    pub(crate) canvas: CanvasState,
    pub(crate) pending_open: Option<PathBuf>,
    pub(crate) save_as_open: bool,
    pub(crate) save_as_path: String,
    pub(crate) subgraph: SubgraphDialog,
    pub(crate) insert_open: bool,
    pub(crate) insert_filter: String,
    pub(crate) about_open: bool,

    pub(crate) atlas: AtlasPreview,
    pub(crate) materials: MaterialPreview,
}

impl TexgenApp {
    /// Builds the editor for a workspace.
    pub fn new(cc: &eframe::CreationContext<'_>, workspace: Workspace) -> Self {
        cc.egui_ctx.set_visuals(dark_visuals());
        let library = load_library(&workspace);
        let (graph, graph_path, status) = match library.first() {
            Some(entry) => match Graph::load(&entry.path) {
                Ok(graph) => (
                    graph,
                    Some(entry.path.clone()),
                    format!("opened {}", entry.key),
                ),
                Err(error) => (
                    blank_graph(),
                    None,
                    format!("could not open {}: {error}", entry.key),
                ),
            },
            None => (
                blank_graph(),
                None,
                "no graphs found under crates/engine/block/textures/graph".to_owned(),
            ),
        };
        let mut app = Self {
            workspace,
            graph,
            graph_path,
            library,
            library_filter: String::new(),
            node_filter: String::new(),
            selected: None,
            tab: Tab::Inspect,
            status,
            error: None,
            build_log: Vec::new(),
            build_job: None,
            history: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            generate_on_save: true,
            preview: None,
            preview_size: 128,
            show_tiled: true,
            show_checker: true,
            preview_due: Some(Instant::now()),
            preview_error: None,
            revision: 0,
            canvas: CanvasState::default(),
            pending_open: None,
            save_as_open: false,
            save_as_path: String::new(),
            subgraph: SubgraphDialog {
                open: false,
                name: String::new(),
                error: None,
            },
            insert_open: false,
            insert_filter: String::new(),
            about_open: false,
            atlas: AtlasPreview {
                texture: None,
                build: None,
                error: None,
            },
            materials: MaterialPreview {
                manifest: None,
                table: None,
                error: None,
            },
        };
        app.graph.ensure_layout();
        app.selected = Some(app.graph.output.color);
        app.save_as_path = app
            .graph_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| {
                app.workspace
                    .graph_path(&app.graph.key)
                    .display()
                    .to_string()
            });
        app
    }

    pub(crate) fn color_nodes(&self) -> Vec<(u32, String)> {
        self.graph
            .nodes
            .iter()
            .filter(|node| crate::graph::op_type(&node.op) == crate::graph::Type::Color)
            .map(|node| (node.id, format!("{} #{}", node.op.label(), node.id)))
            .collect()
    }

    pub(crate) fn scalar_nodes(&self) -> Vec<(u32, String)> {
        self.graph
            .nodes
            .iter()
            .filter(|node| crate::graph::op_type(&node.op) == crate::graph::Type::Scalar)
            .map(|node| (node.id, format!("{} #{}", node.op.label(), node.id)))
            .collect()
    }

    pub(crate) fn node(&self, id: u32) -> Option<&crate::graph::Node> {
        self.graph.nodes.iter().find(|node| node.id == id)
    }

    pub(crate) fn node_mut(&mut self, id: u32) -> Option<&mut crate::graph::Node> {
        self.graph.nodes.iter_mut().find(|node| node.id == id)
    }

    /// Records a pre-edit snapshot for undo.
    pub(crate) fn snapshot(&mut self) {
        if self.history.last().is_none_or(|last| *last != self.graph) {
            self.history.push(self.graph.clone());
            if self.history.len() > HISTORY_LIMIT {
                self.history.remove(0);
            }
        }
        self.redo.clear();
    }

    /// Marks the document changed and schedules a preview.
    pub(crate) fn edited(&mut self) {
        self.dirty = true;
        self.invalidate_preview();
    }

    pub(crate) fn invalidate_preview(&mut self) {
        self.preview_due = Some(Instant::now() + PREVIEW_DEBOUNCE);
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn undo(&mut self) {
        if let Some(previous) = self.history.pop() {
            self.redo.push(std::mem::replace(&mut self.graph, previous));
            self.selected = Some(self.graph.output.color);
            self.dirty = true;
            self.invalidate_preview();
            self.status = "undo".to_owned();
        }
    }

    pub(crate) fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.history.push(std::mem::replace(&mut self.graph, next));
            self.selected = Some(self.graph.output.color);
            self.dirty = true;
            self.invalidate_preview();
            self.status = "redo".to_owned();
        }
    }

    pub(crate) fn open_graph(&mut self, path: PathBuf) {
        match Graph::load(&path) {
            Ok(mut graph) => {
                graph.ensure_layout();
                self.graph = graph;
                self.graph_path = Some(path.clone());
                self.history.clear();
                self.redo.clear();
                self.selected = Some(self.graph.output.color);
                self.dirty = false;
                self.preview = None;
                self.preview_error = None;
                self.invalidate_preview();
                self.save_as_path = path.display().to_string();
                self.status = format!("opened {}", path.display());
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("could not open {}: {error}", path.display()));
            }
        }
    }

    pub(crate) fn request_open(&mut self, path: PathBuf) {
        if self.dirty {
            self.pending_open = Some(path);
        } else {
            self.open_graph(path);
        }
    }

    pub(crate) fn save_current(&mut self) -> bool {
        let Some(path) = self.graph_path.clone() else {
            self.save_as_open = true;
            return false;
        };
        match self.graph.save(&path) {
            Ok(()) => {
                self.dirty = false;
                self.status = format!("saved {}", path.display());
                self.error = None;
                self.library = load_library(&self.workspace);
                if self.generate_on_save {
                    let workspace = self.workspace.clone();
                    self.run_pipeline("update-all", move || pipeline::update_all(&workspace));
                }
                true
            }
            Err(error) => {
                self.error = Some(format!("save failed: {error}"));
                false
            }
        }
    }

    pub(crate) fn save_as(&mut self, path: PathBuf) {
        match self.graph.save(&path) {
            Ok(()) => {
                self.graph_path = Some(path.clone());
                self.dirty = false;
                self.save_as_open = false;
                self.save_as_path = path.display().to_string();
                self.status = format!("saved {}", path.display());
                self.library = load_library(&self.workspace);
                if self.generate_on_save {
                    let workspace = self.workspace.clone();
                    self.run_pipeline("update-all", move || pipeline::update_all(&workspace));
                }
            }
            Err(error) => self.error = Some(format!("save failed: {error}")),
        }
    }

    pub(crate) fn run_pipeline(
        &mut self,
        label: &str,
        action: impl FnOnce() -> Result<String, pipeline::PipelineError> + Send + 'static,
    ) {
        if let Some(job) = &self.build_job {
            self.status = format!("{} is already running", job.label);
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let label_owned = label.to_owned();
        std::thread::spawn(move || {
            let _ = sender.send(action().map_err(|error| error.to_string()));
        });
        self.build_job = Some(BuildJob {
            label: label_owned.clone(),
            receiver,
        });
        self.status = format!("{label_owned}: running…");
    }

    fn poll_build(&mut self) {
        let Some(job) = self.build_job.take() else {
            return;
        };
        match job.receiver.try_recv() {
            Ok(Ok(message)) => {
                self.status = message.clone();
                self.error = None;
                self.build_log.push(format!("{}: {message}", job.label));
                self.atlas.texture = None;
                self.atlas.build = None;
                self.materials.manifest = None;
                self.materials.table = None;
            }
            Ok(Err(error)) => {
                self.error = Some(format!("{}: {error}", job.label));
                self.build_log.push(format!("{}: {error}", job.label));
            }
            Err(TryRecvError::Empty) => {
                self.build_job = Some(job);
            }
            Err(TryRecvError::Disconnected) => {
                self.error = Some(format!("{}: worker stopped without a result", job.label));
            }
        }
        if self.build_log.len() > 100 {
            self.build_log.remove(0);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let save = ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
        let undo = ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
        let redo = ctx.input_mut(|input| {
            input.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Z,
            )
        });
        if save {
            self.save_current();
        }
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if !ctx.wants_keyboard_input() && ctx.input(|input| input.key_pressed(egui::Key::Delete)) {
            self.delete_selected();
        }
        if ctx.input(|input| input.key_pressed(egui::Key::D) && input.modifiers.command) {
            self.duplicate_selected();
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Save (Ctrl+S)").clicked() {
                        self.save_current();
                        ui.close();
                    }
                    if ui.button("Save As…").clicked() {
                        self.save_as_open = true;
                        ui.close();
                    }
                    if ui.button("Reload").clicked() {
                        if let Some(path) = self.graph_path.clone() {
                            self.open_graph(path);
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Insert subgraph…").clicked() {
                        self.insert_open = true;
                        ui.close();
                    }
                    if ui.button("Save selection as subgraph…").clicked() {
                        self.subgraph.open = true;
                        self.subgraph.name = format!("subgraph_{}", self.revision);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Generate textures + manifest").clicked() {
                        let workspace = self.workspace.clone();
                        self.run_pipeline("generate", move || pipeline::generate_all(&workspace));
                        ui.close();
                    }
                    if ui.button("Generate materials").clicked() {
                        let workspace = self.workspace.clone();
                        self.run_pipeline("materials", move || {
                            pipeline::write_materials(&workspace)
                        });
                        ui.close();
                    }
                    if ui.button("Bake atlas").clicked() {
                        let workspace = self.workspace.clone();
                        self.run_pipeline("bake-atlas", move || {
                            bake::bake_atlas(&workspace, false)
                                .map_err(pipeline::PipelineError::Bake)
                        });
                        ui.close();
                    }
                    if ui.button("Update everything").clicked() {
                        let workspace = self.workspace.clone();
                        self.run_pipeline("update-all", move || pipeline::update_all(&workspace));
                        ui.close();
                    }
                    if ui.button("Check committed files").clicked() {
                        let workspace = self.workspace.clone();
                        self.run_pipeline("check", move || pipeline::check_all(&workspace));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui
                        .add_enabled(!self.history.is_empty(), egui::Button::new("Undo"))
                        .clicked()
                    {
                        self.undo();
                        ui.close();
                    }
                    if ui
                        .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                        .clicked()
                    {
                        self.redo();
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(self.selected.is_some(), egui::Button::new("Duplicate"))
                        .clicked()
                    {
                        self.duplicate_selected();
                        ui.close();
                    }
                    if ui
                        .add_enabled(self.selected.is_some(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        self.delete_selected();
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.label("Preview size");
                    for size in [64, 128, 256] {
                        if ui
                            .selectable_label(self.preview_size == size, format!("{size}x{size}"))
                            .clicked()
                        {
                            self.preview_size = size;
                            self.preview = None;
                            self.invalidate_preview();
                            ui.close();
                        }
                    }
                    ui.separator();
                    ui.checkbox(&mut self.show_tiled, "Tiled preview");
                    ui.checkbox(&mut self.show_checker, "Alpha checkerboard");
                    if ui.button("Reset canvas view").clicked() {
                        self.canvas = CanvasState::default();
                        ui.close();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                });
                ui.separator();
                let title = format!(
                    "{}  ·  slot {}  ·  {}{}",
                    self.graph.key,
                    pipeline::slot_of(&self.graph.key)
                        .map(|slot| slot.to_string())
                        .unwrap_or_else(|| "—".to_owned()),
                    match (self.graph.tiling, self.graph.alpha) {
                        (Tiling::Both, AlphaClass::Opaque) => "Both / Opaque",
                        (Tiling::Both, AlphaClass::Cutout) => "Both / Cutout",
                        (Tiling::Both, AlphaClass::Translucent) => "Both / Translucent",
                        (Tiling::Horizontal, AlphaClass::Opaque) => "Horizontal / Opaque",
                        (Tiling::Horizontal, AlphaClass::Cutout) => "Horizontal / Cutout",
                        (Tiling::Horizontal, AlphaClass::Translucent) => "Horizontal / Translucent",
                        (Tiling::None, AlphaClass::Opaque) => "None / Opaque",
                        (Tiling::None, AlphaClass::Cutout) => "None / Cutout",
                        (Tiling::None, AlphaClass::Translucent) => "None / Translucent",
                    },
                    if self.dirty { "  ·  unsaved" } else { "" }
                );
                ui.label(RichText::new(title).weak());
            });
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(job) = &self.build_job {
                    ui.spinner();
                    ui.label(RichText::new(&job.label).weak());
                } else if let Some(error) = &self.error {
                    ui.colored_label(Color32::from_rgb(235, 104, 104), error);
                } else {
                    ui.label(RichText::new(&self.status).weak());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} nodes · {} graph files",
                            self.graph.nodes.len(),
                            self.library.len()
                        ))
                        .weak(),
                    );
                });
            });
        });
    }

    fn library_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("library")
            .resizable(true)
            .default_width(250.0)
            .show(ctx, |ui| {
                ui.heading("Textures");
                ui.label(
                    RichText::new(format!("committed face set · {} faces", self.library.len()))
                        .weak(),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.library_filter)
                        .hint_text("Filter…")
                        .desired_width(f32::INFINITY),
                );
                ui.separator();
                let mut open: Option<PathBuf> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let filter = self.library_filter.to_ascii_lowercase();
                    for entry in &self.library {
                        if !filter.is_empty() && !entry.key.to_ascii_lowercase().contains(&filter) {
                            continue;
                        }
                        let selected = self.graph_path.as_deref() == Some(entry.path.as_path());
                        let label = format!("{:>2}  {}", entry.slot, pipeline::name_of(&entry.key));
                        let response = ui.selectable_label(selected, label);
                        if response.clicked() {
                            open = Some(entry.path.clone());
                        }
                        response.on_hover_text(format!(
                            "{}\n{:?} · {:?}",
                            entry.key,
                            entry.alpha.unwrap_or(AlphaClass::Opaque),
                            entry.tiling.unwrap_or(Tiling::Both)
                        ));
                    }
                    ui.separator();
                    ui.label(RichText::new("Reusable graphs").strong());
                    let dir = self.workspace.graph_dir().join("_subgraphs");
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        let mut paths: Vec<PathBuf> = entries
                            .filter_map(|entry| entry.ok())
                            .map(|entry| entry.path())
                            .filter(|path| path.extension().is_some_and(|ext| ext == "ron"))
                            .collect();
                        paths.sort();
                        for path in paths {
                            if ui
                                .button(
                                    path.file_stem()
                                        .map(|stem| stem.to_string_lossy().to_string())
                                        .unwrap_or_default(),
                                )
                                .clicked()
                            {
                                open = Some(path);
                            }
                        }
                    }
                    ui.separator();
                    ui.label(RichText::new("Node library").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.node_filter)
                            .hint_text("Search nodes…")
                            .desired_width(f32::INFINITY),
                    );
                    let needle = self.node_filter.to_ascii_lowercase();
                    let mut add = None;
                    for op in crate::graph::catalog() {
                        let haystack =
                            format!("{} {} {}", op.label(), op.category(), op.description())
                                .to_ascii_lowercase();
                        if !needle.is_empty() && !haystack.contains(&needle) {
                            continue;
                        }
                        if ui
                            .button(format!("{}  ·  {}", op.label(), op.category()))
                            .on_hover_text(op.description())
                            .clicked()
                        {
                            add = Some(op);
                        }
                    }
                    if let Some(op) = add {
                        self.add_node(op);
                    }
                });
                if let Some(path) = open {
                    self.request_open(path);
                }
            });
    }

    fn inspector_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(380.0)
            .show(ctx, |ui| {
                self.preview_ui(ui);
                ui.separator();
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (Tab::Inspect, "Inspect"),
                        (Tab::Atlas, "Atlas"),
                        (Tab::Materials, "Materials"),
                        (Tab::Build, "Build"),
                    ] {
                        if ui.selectable_label(self.tab == tab, label).clicked() {
                            self.tab = tab;
                            match tab {
                                Tab::Atlas => self.ensure_atlas(ctx),
                                Tab::Materials => self.ensure_materials(),
                                _ => {}
                            }
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                    Tab::Inspect => self.inspect_ui(ui),
                    Tab::Atlas => self.atlas_ui(ui),
                    Tab::Materials => self.materials_ui(ui),
                    Tab::Build => self.build_ui(ui),
                });
            });
    }

    fn inspect_ui(&mut self, ui: &mut egui::Ui) {
        let before = self.graph.clone();
        ui.heading("Graph");
        egui::Grid::new("graph-properties")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Key");
                ui.add(egui::TextEdit::singleline(&mut self.graph.key));
                ui.end_row();
                ui.label("Seed");
                ui.add(egui::DragValue::new(&mut self.graph.seed).speed(1));
                ui.end_row();
                ui.label("Size");
                ui.label(format!("{}x{}", self.graph.size, self.graph.size));
                ui.end_row();
                ui.label("Tiling");
                egui::ComboBox::from_id_salt("tiling")
                    .selected_text(format!("{:?}", self.graph.tiling))
                    .show_ui(ui, |ui| {
                        for value in [Tiling::Both, Tiling::Horizontal, Tiling::None] {
                            ui.selectable_value(
                                &mut self.graph.tiling,
                                value,
                                format!("{value:?}"),
                            );
                        }
                    });
                ui.end_row();
                ui.label("Alpha");
                egui::ComboBox::from_id_salt("alpha")
                    .selected_text(format!("{:?}", self.graph.alpha))
                    .show_ui(ui, |ui| {
                        for value in [
                            AlphaClass::Opaque,
                            AlphaClass::Cutout,
                            AlphaClass::Translucent,
                        ] {
                            ui.selectable_value(&mut self.graph.alpha, value, format!("{value:?}"));
                        }
                    });
                ui.end_row();
            });

        ui.separator();
        ui.heading("Output");
        egui::Grid::new("output-properties")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Colour");
                let colors = self.color_nodes();
                egui::ComboBox::from_id_salt("output-color")
                    .selected_text(
                        colors
                            .iter()
                            .find(|(id, _)| *id == self.graph.output.color)
                            .map(|(_, label)| label.clone())
                            .unwrap_or_else(|| "none".to_owned()),
                    )
                    .show_ui(ui, |ui| {
                        for (id, label) in &colors {
                            ui.selectable_value(&mut self.graph.output.color, *id, label);
                        }
                    });
                ui.end_row();
                ui.label("Alpha");
                let scalars = self.scalar_nodes();
                let mut use_node =
                    matches!(self.graph.output.alpha, crate::graph::AlphaSource::Node(_));
                if ui.checkbox(&mut use_node, "use node").changed() {
                    self.graph.output.alpha = if use_node {
                        scalars
                            .first()
                            .map(|(id, _)| crate::graph::AlphaSource::Node(*id))
                            .unwrap_or(crate::graph::AlphaSource::Const(65_535))
                    } else {
                        crate::graph::AlphaSource::Const(65_535)
                    };
                }
                if use_node {
                    let current = match self.graph.output.alpha {
                        crate::graph::AlphaSource::Node(id) => id,
                        crate::graph::AlphaSource::Const(_) => 0,
                    };
                    egui::ComboBox::from_id_salt("output-alpha")
                        .selected_text(
                            scalars
                                .iter()
                                .find(|(id, _)| *id == current)
                                .map(|(_, label)| label.clone())
                                .unwrap_or_else(|| "none".to_owned()),
                        )
                        .show_ui(ui, |ui| {
                            for (id, label) in &scalars {
                                if ui.selectable_label(current == *id, label).clicked() {
                                    self.graph.output.alpha = crate::graph::AlphaSource::Node(*id);
                                }
                            }
                        });
                } else {
                    ui.label("constant 65535 (opaque)");
                }
                ui.end_row();
            });

        ui.separator();
        if let Some(id) = self.selected {
            ui.heading(format!("Node #{id}"));
            self.params_ui(ui, id);
        } else {
            ui.label(RichText::new("Select a node on the canvas.").weak());
        }

        if self.graph != before {
            self.snapshot_with(before);
            self.edited();
        }
    }

    fn build_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Build");
        ui.label(RichText::new("All commands use the same pipeline as CI.").weak());
        if let Some(job) = &self.build_job {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("{} running…", job.label));
            });
        }
        let busy = self.build_job.is_some();
        ui.checkbox(&mut self.generate_on_save, "Generate on save (Ctrl+S)");
        ui.separator();
        if ui
            .add_enabled(
                !busy,
                egui::Button::new("Generate textures + manifest + materials"),
            )
            .clicked()
        {
            let workspace = self.workspace.clone();
            self.run_pipeline("generate", move || pipeline::generate_all(&workspace));
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Bake atlas"))
            .clicked()
        {
            let workspace = self.workspace.clone();
            self.run_pipeline("bake-atlas", move || {
                bake::bake_atlas(&workspace, false).map_err(pipeline::PipelineError::Bake)
            });
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Update everything"))
            .clicked()
        {
            let workspace = self.workspace.clone();
            self.run_pipeline("update-all", move || pipeline::update_all(&workspace));
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Check committed files"))
            .clicked()
        {
            let workspace = self.workspace.clone();
            self.run_pipeline("check", move || pipeline::check_all(&workspace));
        }
        ui.separator();
        ui.label(RichText::new("Workspace").strong());
        ui.monospace(self.workspace.root().display().to_string());
        ui.label(RichText::new("Atlas output").strong());
        ui.monospace(self.workspace.atlas_png_path().display().to_string());
        ui.separator();
        ui.label(RichText::new("Log").strong());
        if self.build_log.is_empty() {
            ui.label(RichText::new("No builds yet.").weak());
        }
        for line in self.build_log.iter().rev() {
            ui.monospace(line);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.pending_open.is_some() {
            let mut open = self.pending_open.clone();
            let mut close = false;
            egui::Window::new("Unsaved changes")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("The current graph has unsaved changes.");
                    ui.horizontal(|ui| {
                        if ui.button("Save and open").clicked() && self.save_current() {
                            close = true;
                        }
                        if ui.button("Discard and open").clicked() {
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            open = None;
                            close = true;
                        }
                    });
                });
            if close {
                if let Some(path) = open.take()
                    && self.pending_open.as_ref() == Some(&path)
                {
                    self.open_graph(path);
                }
                self.pending_open = None;
            }
        }

        if self.save_as_open {
            let mut open = self.save_as_open;
            egui::Window::new("Save graph as")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.save_as_path).desired_width(520.0));
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            let path = PathBuf::from(self.save_as_path.clone());
                            self.save_as(path);
                            open = false;
                        }
                        if ui.button("Cancel").clicked() {
                            open = false;
                        }
                    });
                });
            self.save_as_open = open;
        }

        if self.insert_open {
            self.insert_dialog(ctx);
        }

        if self.subgraph.open {
            self.subgraph_dialog(ctx);
        }

        if self.about_open {
            let mut open = self.about_open;
            egui::Window::new("About Voxel Texgen")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.heading("Voxel Texgen");
                    ui.label(
                        "Standalone node-graph texture and material generator for Voxel-Stuff.",
                    );
                    ui.label(
                        "CPU evaluator is authoritative; fixed-point only; no GPU float preview.",
                    );
                    if ui.button("Close").clicked() {
                        open = false;
                    }
                });
            self.about_open = open;
        }
    }

    fn insert_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.insert_open;
        let mut pick: Option<PathBuf> = None;
        egui::Window::new("Insert subgraph")
            .default_size([460.0, 520.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Nodes are inlined. The included graph's output colour becomes a node.");
                ui.add(
                    egui::TextEdit::singleline(&mut self.insert_filter)
                        .hint_text("Filter…")
                        .desired_width(f32::INFINITY),
                );
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut paths = Vec::new();
                    for dir in [
                        self.workspace.graph_dir(),
                        self.workspace.graph_dir().join("_subgraphs"),
                    ] {
                        if let Ok(entries) = std::fs::read_dir(dir) {
                            for entry in entries.filter_map(|entry| entry.ok()) {
                                let path = entry.path();
                                if path.extension().is_some_and(|ext| ext == "ron") {
                                    paths.push(path);
                                }
                            }
                        }
                    }
                    paths.sort();
                    let filter = self.insert_filter.to_ascii_lowercase();
                    for path in paths {
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if !filter.is_empty() && !name.to_ascii_lowercase().contains(&filter) {
                            continue;
                        }
                        if ui.selectable_label(false, name).clicked() {
                            pick = Some(path);
                        }
                    }
                });
            });
        self.insert_open = open;
        if let Some(path) = pick {
            self.insert_graph(&path);
            self.insert_open = false;
        }
    }

    fn subgraph_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.subgraph.open;
        let mut create = false;
        egui::Window::new("Save selection as subgraph")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("Selected nodes and their dependencies are written to textures/graph/_subgraphs.");
                ui.add(
                    egui::TextEdit::singleline(&mut self.subgraph.name)
                        .hint_text("name")
                        .desired_width(320.0),
                );
                if let Some(error) = &self.subgraph.error {
                    ui.colored_label(Color32::from_rgb(235, 104, 104), error);
                }
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        open = false;
                    }
                });
            });
        self.subgraph.open = open;
        if create {
            if let Err(error) = self.save_selection_as_subgraph() {
                self.subgraph.error = Some(error);
            } else {
                self.subgraph.open = false;
                self.subgraph.error = None;
            }
        }
    }

    fn snapshot_with(&mut self, before: Graph) {
        if self.history.last().is_none_or(|last| *last != before) {
            self.history.push(before);
            if self.history.len() > HISTORY_LIMIT {
                self.history.remove(0);
            }
        }
        self.redo.clear();
    }

    fn ensure_atlas(&mut self, ctx: &egui::Context) {
        if self.atlas.texture.is_some() {
            return;
        }
        self.ensure_materials();
        self.atlas.error = None;
        match bake::build_atlas(&self.workspace) {
            Ok(build) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [
                        build.levels[0].width as usize,
                        build.levels[0].height as usize,
                    ],
                    &build.levels[0].pixels,
                );
                self.atlas.texture =
                    Some(ctx.load_texture("atlas-preview", image, egui::TextureOptions::NEAREST));
                self.atlas.build = Some(build);
            }
            Err(error) => self.atlas.error = Some(error.to_string()),
        }
    }

    fn ensure_materials(&mut self) {
        if self.materials.table.is_some() {
            return;
        }
        self.materials.error = None;
        match TextureManifest::load(&self.workspace.manifest_path()) {
            Ok(manifest) => match pipeline::material_table(&self.workspace) {
                Ok(table) => {
                    self.materials.manifest = Some(manifest);
                    self.materials.table = Some(table);
                }
                Err(error) => self.materials.error = Some(error.to_string()),
            },
            Err(error) => self.materials.error = Some(error.to_string()),
        }
    }

    fn atlas_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Atlas");
        if let Some(error) = &self.atlas.error {
            ui.colored_label(Color32::from_rgb(235, 104, 104), error);
            if ui.button("Rebuild").clicked() {
                self.atlas.texture = None;
            }
            return;
        }
        let Some(texture) = &self.atlas.texture else {
            ui.label("Building…");
            return;
        };
        let size = ui.available_width().clamp(128.0, 256.0);
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        // Grid lines.
        for index in 1..16 {
            let x = rect.left() + rect.width() * index as f32 / 16.0;
            let y = rect.top() + rect.height() * index as f32 / 16.0;
            ui.painter().line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                egui::Stroke::new(0.5_f32, Color32::from_black_alpha(110)),
            );
            ui.painter().line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                egui::Stroke::new(0.5_f32, Color32::from_black_alpha(110)),
            );
        }
        if let Some(pointer) = response.hover_pos() {
            let u = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 0.999);
            let v = ((pointer.y - rect.top()) / rect.height()).clamp(0.0, 0.999);
            let slot = ((v * 16.0) as u16) * 16 + (u * 16.0) as u16;
            if let Some(entry) = self
                .materials
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.textures.iter().find(|entry| entry.slot == slot))
            {
                response
                    .clone()
                    .on_hover_text(format!("{slot}: {}", entry.key));
            } else {
                response.clone().on_hover_text(format!("{slot}: empty"));
            }
        }
        if let Some(build) = &self.atlas.build {
            ui.label(format!(
                "{} tiles · {} · {}",
                build.tile_count, build.atlas_png_blake3, build.mip_chain_blake3
            ));
        }
        if ui.button("Rebuild atlas preview").clicked() {
            self.atlas.texture = None;
            self.ensure_atlas(ui.ctx());
        }
    }

    fn materials_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Materials");
        if let Some(error) = &self.materials.error {
            ui.colored_label(Color32::from_rgb(235, 104, 104), error);
            if ui.button("Reload").clicked() {
                self.materials.table = None;
            }
            return;
        }
        let Some(table) = &self.materials.table else {
            ui.label("Loading…");
            return;
        };
        if let Some(manifest) = &self.materials.manifest {
            ui.label(RichText::new("Slots").strong());
            egui::Grid::new("materials-slots")
                .num_columns(3)
                .spacing([8.0, 2.0])
                .show(ui, |ui| {
                    for entry in manifest.sorted() {
                        ui.monospace(format!("{:>2}", entry.slot));
                        ui.label(entry.key.trim_start_matches("voxel:block/"));
                        ui.label(format!("{:?}", entry.alpha));
                        ui.end_row();
                    }
                });
        }
        ui.separator();
        ui.label(RichText::new("Blocks").strong());
        egui::Grid::new("materials-blocks")
            .num_columns(7)
            .spacing([6.0, 2.0])
            .show(ui, |ui| {
                for header in ["block", "top", "bot", "n", "s", "e", "w"] {
                    ui.label(RichText::new(header).strong());
                }
                ui.end_row();
                for row in &table.faces {
                    ui.label(row.block.trim_start_matches("voxel:"));
                    for slot in [
                        row.top, row.bottom, row.north, row.south, row.east, row.west,
                    ] {
                        ui.monospace(slot.to_string());
                    }
                    ui.end_row();
                }
            });
        if ui.button("Reload materials").clicked() {
            self.materials.table = None;
            self.materials.manifest = None;
            self.ensure_materials();
        }
    }
}

impl eframe::App for TexgenApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_preview(ctx);
        self.poll_build();
        self.handle_shortcuts(ctx);
        self.menu_bar(ctx);
        self.status_bar(ctx);
        self.library_panel(ctx);
        self.inspector_panel(ctx);
        self.canvas_panel(ctx);
        self.dialogs(ctx);
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}

fn load_library(workspace: &Workspace) -> Vec<LibraryEntry> {
    let mut entries = Vec::new();
    for (key, slot) in pipeline::SLICES {
        let path = workspace.graph_path(key);
        let graph = Graph::load(&path).ok();
        entries.push(LibraryEntry {
            key: (*key).to_owned(),
            slot: *slot,
            path,
            alpha: graph.as_ref().map(|graph| graph.alpha),
            tiling: graph.as_ref().map(|graph| graph.tiling),
        });
    }
    entries
}

fn blank_graph() -> Graph {
    Graph {
        schema: 1,
        key: "voxel:block/new_texture".to_owned(),
        size: crate::graph::GRAPH_SIZE,
        tiling: Tiling::Both,
        alpha: AlphaClass::Opaque,
        seed: 0,
        nodes: vec![
            crate::graph::Node {
                id: 0,
                op: Op::Fbm {
                    period: 16,
                    octaves: 4,
                },
                inputs: Vec::new(),
                ui: Some(UiPosition::new(40.0, 40.0)),
            },
            crate::graph::Node {
                id: 1,
                op: Op::Ramp {
                    stops: vec![(0, "#3a3a3a".to_owned()), (65_535, "#a0a0a0".to_owned())],
                },
                inputs: vec![0],
                ui: Some(UiPosition::new(280.0, 40.0)),
            },
        ],
        output: crate::graph::Output {
            color: 1,
            alpha: crate::graph::AlphaSource::Const(65_535),
        },
    }
}

fn dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(24, 26, 30);
    visuals.window_fill = Color32::from_rgb(30, 33, 38);
    visuals.extreme_bg_color = Color32::from_rgb(16, 18, 21);
    visuals.selection.bg_fill = Color32::from_rgb(54, 112, 158);
    visuals.hyperlink_color = Color32::from_rgb(111, 186, 230);
    visuals
}

impl TexgenApp {
    pub(crate) fn node_types(&self) -> HashMap<u32, crate::graph::Type> {
        self.graph
            .nodes
            .iter()
            .map(|node| (node.id, crate::graph::op_type(&node.op)))
            .collect()
    }
}
