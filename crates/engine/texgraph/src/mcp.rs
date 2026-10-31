//! Model Context Protocol server for Texgen.
//!
//! The server speaks JSON-RPC over stdio (MCP `legacy` transport) and exposes
//! read, preview, generate and check tools. It deliberately does **not** write
//! graph RON: an agent can propose a graph with `validate_graph`, but edits stay
//! explicit file changes.
//!
//! OpenCode configuration:
//!
//! ```jsonc
//! {
//!   "mcp": {
//!     "servers": {
//!       "texgen": {
//!         "type": "local",
//!         "command": ["cargo", "run", "--quiet", "-p", "voxel-texgraph", "--bin", "texgen-mcp"],
//!         "cwd": "voxel-approved",
//!         "timeout": { "startup": 120000 }
//!       }
//!     }
//!   }
//! }
//! ```

use std::error::Error;
use std::fs;
use std::io::{self, BufRead, BufWriter, Write};
use std::path::PathBuf;

use base64::Engine;
use serde_json::{Value, json};
use voxel_block::texture_manifest::TextureManifest;

use crate::bake;
use crate::eval;
use crate::graph::{Graph, Type, op_type};
use crate::pipeline::{self, Workspace};
use crate::png;

/// Runs the MCP server until stdin closes.
pub fn run() -> Result<(), Box<dyn Error>> {
    let workspace = match workspace_from_args() {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("texgen-mcp: {error}");
            return Err(error.into());
        }
    };
    let server = Server { workspace };
    let stdin = io::stdin();
    let mut stdout = BufWriter::new(io::stdout());
    /// Largest request line the server will buffer (a graph RON is well under).
    const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if line.len() > MAX_REQUEST_BYTES {
            let response = error_response(
                Value::Null,
                -32600,
                format!(
                    "request is {} bytes, above the {MAX_REQUEST_BYTES}-byte limit",
                    line.len()
                ),
            );
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => server.handle(&request),
            Err(error) => Some(error_response(
                Value::Null,
                -32700,
                format!("parse error: {error}"),
            )),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn workspace_from_args() -> Result<Workspace, pipeline::PipelineError> {
    let mut args = std::env::args().skip(1);
    let mut root: Option<PathBuf> = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => {
                root = Some(PathBuf::from(args.next().ok_or_else(|| {
                    pipeline::PipelineError::Invalid("--root needs a path".to_owned())
                })?));
            }
            "--help" | "-h" => {
                println!(
                    "texgen-mcp: MCP stdio server for Voxel Texgen\n\
                     \n\
                     usage: texgen-mcp [--root PATH]\n\
                     \n\
                     The server reads JSON-RPC from stdin and writes it to stdout.\n\
                     Diagnostics go to stderr."
                );
                std::process::exit(0);
            }
            other => {
                return Err(pipeline::PipelineError::Invalid(format!(
                    "unknown argument `{other}`"
                )));
            }
        }
    }
    match root {
        Some(root) => Workspace::at(root),
        None => Workspace::locate(),
    }
}

struct Server {
    workspace: Workspace,
}

struct ToolResult {
    content: Vec<Value>,
    is_error: bool,
}

impl ToolResult {
    fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![json!({ "type": "text", "text": text.into() })],
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![json!({ "type": "text", "text": text.into() })],
            is_error: true,
        }
    }

    fn image(png: &[u8]) -> Self {
        Self {
            content: vec![json!({
                "type": "image",
                "data": base64::engine::general_purpose::STANDARD.encode(png),
                "mimeType": "image/png",
            })],
            is_error: false,
        }
    }

    fn push(mut self, value: Value) -> Self {
        self.content.push(value);
        self
    }

    fn into_value(self) -> Value {
        json!({ "content": self.content, "isError": self.is_error })
    }
}

impl Server {
    fn handle(&self, request: &Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                -32600,
                "invalid request: missing method".to_owned(),
            ));
        };
        let is_notification = id.is_none();
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Some(self.initialize(&params)),
            "notifications/initialized" | "notifications/cancelled" => None,
            "ping" => Some(Ok(json!({}))),
            "tools/list" => Some(Ok(json!({ "tools": tools() }))),
            "tools/call" => Some(self.call_tool(&params)),
            "resources/list" => Some(Ok(json!({ "resources": resources() }))),
            "resources/templates/list" => Some(Ok(json!({ "resourceTemplates": [] }))),
            "resources/read" => Some(self.read_resource(&params)),
            "prompts/list" => Some(Ok(json!({ "prompts": [] }))),
            _ => {
                if is_notification {
                    return None;
                }
                return Some(error_response(
                    id.unwrap_or(Value::Null),
                    -32601,
                    format!("method not found: {method}"),
                ));
            }
        };
        if is_notification {
            return None;
        }
        Some(match result {
            Some(Ok(value)) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Some(Err(message)) => error_response(id.unwrap_or(Value::Null), -32602, message),
            None => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": ToolResult::error("notification has no result").into_value()
            }),
        })
    }

    fn initialize(&self, params: &Value) -> Result<Value, String> {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("2025-06-18");
        let version = match requested {
            "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25" => requested,
            _ => "2025-06-18",
        };
        Ok(json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "listChanged": false, "subscribe": false },
            },
            "serverInfo": {
                "name": "texgen",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": instructions(),
        }))
    }

    fn call_tool(&self, params: &Value) -> Result<Value, String> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "tools/call needs params.name".to_owned())?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let result = self.dispatch(name, &arguments);
        Ok(result.into_value())
    }

    fn dispatch(&self, name: &str, arguments: &Value) -> ToolResult {
        match name {
            "list_faces" => self.list_faces(),
            "read_graph" => self.read_graph(arguments),
            "validate_graph" => self.validate_graph(arguments),
            "render_preview" => self.render_preview(arguments),
            "render_tile" => self.render_tile(arguments),
            "render_atlas" => self.render_atlas(),
            "list_materials" => self.list_materials(),
            "read_manifest" => self.read_manifest(),
            "check" => self.check(),
            "generate" => self.generate(),
            "bake_atlas" => self.bake_atlas(),
            other => ToolResult::error(format!("unknown tool `{other}`")),
        }
    }

    fn list_faces(&self) -> ToolResult {
        let manifest = TextureManifest::load(&self.workspace.manifest_path()).ok();
        let mut faces = Vec::new();
        for (key, slot) in pipeline::SLICES {
            let graph_path = self.workspace.graph_path(key);
            let png_path = self.workspace.texture_path(key);
            let graph_hash = fs::read(&graph_path)
                .ok()
                .map(|bytes| blake3::hash(&bytes).to_hex().to_string());
            let png_hash = fs::read(&png_path)
                .ok()
                .map(|bytes| blake3::hash(&bytes).to_hex().to_string());
            let entry = manifest.as_ref().and_then(|manifest| manifest.entry(key));
            let mut stale = Vec::new();
            if entry.is_none() {
                stale.push("no manifest entry");
            } else if let Some(entry) = entry {
                if entry.graph_hash != graph_hash {
                    stale.push("graph changed since manifest");
                }
                if Some(entry.hash.as_str()) != png_hash.as_deref() {
                    stale.push("PNG changed since manifest");
                }
            }
            if png_hash.is_none() {
                stale.push("PNG missing");
            }
            faces.push(json!({
                "key": key,
                "slot": slot,
                "alpha": entry.map(|entry| format!("{:?}", entry.alpha)),
                "tiling": entry.map(|entry| format!("{:?}", entry.tiling)),
                "graph": relative(&self.workspace, &graph_path),
                "source": relative(&self.workspace, &png_path),
                "graph_blake3": graph_hash,
                "png_blake3": png_hash,
                "stale": !stale.is_empty(),
                "stale_reasons": stale,
            }));
        }
        ToolResult::text(
            serde_json::to_string_pretty(&json!({
                "workspace": self.workspace.root().display().to_string(),
                "faces": faces,
            }))
            .unwrap_or_else(|_| "could not serialize face list".to_owned()),
        )
    }

    fn read_graph(&self, arguments: &Value) -> ToolResult {
        let Some(key) = arguments.get("key").and_then(Value::as_str) else {
            return ToolResult::error("read_graph requires `key`");
        };
        let key = match normalize_key(key) {
            Ok(key) => key,
            Err(error) => return ToolResult::error(error),
        };
        let path = self.workspace.graph_path(&key);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => return ToolResult::error(format!("{}: {error}", path.display())),
        };
        let status = match ron::from_str::<Graph>(&text) {
            Ok(graph) => match graph.compile() {
                Ok(program) => format!(
                    "ok: {} nodes, tiling {:?}, alpha {:?}, breaks_h={}, breaks_v={}",
                    program.nodes.len(),
                    program.tiling,
                    program.alpha,
                    program.breaks_h,
                    program.breaks_v
                ),
                Err(error) => format!("compile error: {error}"),
            },
            Err(error) => format!("parse error: {error}"),
        };
        ToolResult::text(format!(
            "path: {}\nstatus: {status}\n\n{text}",
            relative(&self.workspace, &path)
        ))
    }

    fn validate_graph(&self, arguments: &Value) -> ToolResult {
        let (source, text) = if let Some(key) = arguments.get("key").and_then(Value::as_str) {
            let key = match normalize_key(key) {
                Ok(key) => key,
                Err(error) => return ToolResult::error(error),
            };
            let path = self.workspace.graph_path(&key);
            match fs::read_to_string(&path) {
                Ok(text) => (relative(&self.workspace, &path), text),
                Err(error) => return ToolResult::error(format!("{}: {error}", path.display())),
            }
        } else if let Some(text) = arguments.get("ron").and_then(Value::as_str) {
            ("inline".to_owned(), text.to_owned())
        } else {
            return ToolResult::error("validate_graph requires `key` or `ron`");
        };

        let graph: Graph = match ron::from_str(&text) {
            Ok(graph) => graph,
            Err(error) => {
                return ToolResult::text(
                    serde_json::to_string_pretty(&json!({
                        "source": source,
                        "ok": false,
                        "error": format!("parse error: {error}"),
                    }))
                    .unwrap(),
                );
            }
        };
        match graph.compile() {
            Ok(program) => ToolResult::text(
                serde_json::to_string_pretty(&json!({
                    "source": source,
                    "ok": true,
                    "key": graph.key,
                    "nodes": program.nodes.len(),
                    "tiling": format!("{:?}", program.tiling),
                    "alpha": format!("{:?}", program.alpha),
                    "breaks_h": program.breaks_h,
                    "breaks_v": program.breaks_v,
                    "output": {
                        "color": program.output.color,
                        "type": format!("{:?}", op_type(&graph.nodes[program.output.color].op)),
                    },
                    "node_ids": graph.nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
                }))
                .unwrap(),
            ),
            Err(error) => ToolResult::text(
                serde_json::to_string_pretty(&json!({
                    "source": source,
                    "ok": false,
                    "error": error.to_string(),
                }))
                .unwrap(),
            ),
        }
    }

    fn render_preview(&self, arguments: &Value) -> ToolResult {
        let Some(key) = arguments.get("key").and_then(Value::as_str) else {
            return ToolResult::error("render_preview requires `key`");
        };
        let key = match normalize_key(key) {
            Ok(key) => key,
            Err(error) => return ToolResult::error(error),
        };
        let size = arguments.get("size").and_then(Value::as_u64).unwrap_or(128);
        if !matches!(size, 64 | 128 | 256) {
            return ToolResult::error("render_preview `size` must be 64, 128 or 256");
        }
        let path = self.workspace.graph_path(&key);
        let graph = match Graph::load(&path) {
            Ok(graph) => graph,
            Err(error) => return ToolResult::error(format!("{}: {error}", path.display())),
        };
        let program = match graph.compile() {
            Ok(program) => program,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let edge = size as u32;
        let pixels = eval::evaluate_at(&program, edge);
        let png = match png::encode_rgba(edge, edge, &pixels) {
            Ok(png) => png,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let tile = match bake::downsample_edge(&pixels, edge, graph.alpha) {
            Ok(tile) => tile,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let tile_png = match png::encode_rgba(128, 128, &upscale(&tile, 16, 128)) {
            Ok(png) => png,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let tile_note = if size == 256 {
            "tile: 128x128 preview of the committed 16² downsample"
        } else {
            "tile: 128x128 preview of this render's 16² downsample (differs from the committed 256² tile)"
        };
        let text = format!(
            "{} slot {} · {:?} · {:?} · preview {}²\nsource: {}x{} RGBA8\n{tile_note}",
            key,
            pipeline::slot_of(&key).unwrap_or(0),
            graph.alpha,
            graph.tiling,
            size,
            edge,
            edge
        );
        ToolResult::text(text)
            .push(ToolResult::image(&png).content.remove(0))
            .push(ToolResult::image(&tile_png).content.remove(0))
    }

    fn render_tile(&self, arguments: &Value) -> ToolResult {
        let Some(key) = arguments.get("key").and_then(Value::as_str) else {
            return ToolResult::error("render_tile requires `key`");
        };
        let key = match normalize_key(key) {
            Ok(key) => key,
            Err(error) => return ToolResult::error(error),
        };
        let manifest = match TextureManifest::load(&self.workspace.manifest_path()) {
            Ok(manifest) => manifest,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let Some(entry) = manifest.entry(&key) else {
            return ToolResult::error(format!("no manifest entry for {key}"));
        };
        let image = match voxel_block::texture_manifest::load_source(
            &self.workspace.block_crate(),
            entry,
        ) {
            Ok(image) => image,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let tile = match bake::downsample(&image.pixels, entry.alpha) {
            Ok(tile) => tile,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let png = match png::encode_rgba(128, 128, &upscale(&tile, 16, 128)) {
            Ok(png) => png,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let text = format!(
            "{} slot {} · {:?} · 16² tile shown at 128² nearest\nsource: {}",
            key,
            entry.slot,
            entry.alpha,
            relative(&self.workspace, &self.workspace.texture_path(&key))
        );
        ToolResult::text(text).push(ToolResult::image(&png).content.remove(0))
    }

    fn render_atlas(&self) -> ToolResult {
        match bake::build_atlas(&self.workspace) {
            Ok(build) => ToolResult::text(format!(
                "atlas: {} tiles · filter {} · png {} · mips {}",
                build.tile_count,
                bake::FILTER,
                build.atlas_png_blake3,
                build.mip_chain_blake3
            ))
            .push(ToolResult::image(&build.png).content.remove(0)),
            Err(error) => ToolResult::error(error.to_string()),
        }
    }

    fn list_materials(&self) -> ToolResult {
        let table = match voxel_block::material_table::generate(&self.workspace.block_crate()) {
            Ok(table) => table,
            Err(error) => return ToolResult::error(error.to_string()),
        };
        let expected = voxel_block::material_table::to_ron(&table).ok();
        let committed = fs::read_to_string(self.workspace.materials_path()).ok();
        let stale = committed.as_deref() != expected.as_deref();
        ToolResult::text(
            serde_json::to_string_pretty(&json!({
                "path": relative(&self.workspace, &self.workspace.materials_path()),
                "stale": stale,
                "manifest_blake3": table.manifest_blake3,
                "blocks_blake3": table.blocks_blake3,
                "materials": table.materials,
                "faces": table.faces,
            }))
            .unwrap_or_else(|_| "could not serialize material table".to_owned()),
        )
    }

    fn read_manifest(&self) -> ToolResult {
        let path = self.workspace.manifest_path();
        match fs::read_to_string(&path) {
            Ok(text) => ToolResult::text(format!(
                "path: {}\n\n{text}",
                relative(&self.workspace, &path)
            )),
            Err(error) => ToolResult::error(format!("{}: {error}", path.display())),
        }
    }

    fn check(&self) -> ToolResult {
        match pipeline::check_all(&self.workspace) {
            Ok(message) => ToolResult::text(message),
            Err(error) => ToolResult::error(error.to_string()),
        }
    }

    fn generate(&self) -> ToolResult {
        match pipeline::update_all(&self.workspace) {
            Ok(message) => ToolResult::text(message),
            Err(error) => ToolResult::error(error.to_string()),
        }
    }

    fn bake_atlas(&self) -> ToolResult {
        match bake::bake_atlas(&self.workspace, false) {
            Ok(message) => ToolResult::text(message),
            Err(error) => ToolResult::error(error.to_string()),
        }
    }

    fn read_resource(&self, params: &Value) -> Result<Value, String> {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| "resources/read needs `uri`".to_owned())?;
        let text = if uri == "texgen://faces" {
            self.list_faces().content[0]
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        } else if uri == "texgen://manifest" {
            fs::read_to_string(self.workspace.manifest_path())
                .map_err(|error| format!("{}: {error}", self.workspace.manifest_path().display()))?
        } else if uri == "texgen://materials" {
            fs::read_to_string(self.workspace.materials_path()).map_err(|error| {
                format!("{}: {error}", self.workspace.materials_path().display())
            })?
        } else if uri == "texgen://atlas/hash" {
            fs::read_to_string(self.workspace.atlas_hash_path()).map_err(|error| {
                format!("{}: {error}", self.workspace.atlas_hash_path().display())
            })?
        } else if let Some(name) = uri.strip_prefix("texgen://graph/") {
            let key = normalize_key(name)?;
            fs::read_to_string(self.workspace.graph_path(&key)).map_err(|error| {
                format!("{}: {error}", self.workspace.graph_path(&key).display())
            })?
        } else {
            return Err(format!("unknown resource `{uri}`"));
        };
        Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": if uri.ends_with(".ron") || uri == "texgen://manifest" || uri == "texgen://materials" || uri.starts_with("texgen://graph/") { "application/ron" } else { "text/plain" },
                "text": text,
            }]
        }))
    }
}

fn instructions() -> &'static str {
    "Voxel Texgen is the authoritative generator for this engine's material textures. \
     Start with list_faces to see the committed faces and whether their outputs are stale. \
     read_graph and validate_graph inspect a fixed-point node graph; render_preview and render_tile show the result; \
     render_atlas shows the packed material atlas. check verifies every committed byte without writing. \
     generate writes all source PNGs, textures.ron, materials.gen.ron and the atlas; bake_atlas writes only the atlas. \
     Graph RON is not writable through MCP: edit textures/graph/*.ron with normal file tools, validate it, then generate. \
     The evaluator is integer-only; previews and committed PNGs come from the same code."
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "list_faces",
            "List every committed face with slot, alpha class, tiling, paths, hashes and staleness.",
            empty_schema(),
            true,
        ),
        tool(
            "read_graph",
            "Read a face's graph RON and report whether it parses and compiles.",
            key_schema(),
            true,
        ),
        tool(
            "validate_graph",
            "Validate a graph by face key or by inline RON without writing anything. Returns node count, tiling, alpha, output type and compile errors.",
            json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Face key, e.g. voxel:block/stone or stone" },
                    "ron": { "type": "string", "description": "Inline graph RON instead of a key" }
                },
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            "render_preview",
            "Render a face preview at 64², 128² or 256² plus a 128² view of its 16² atlas tile. Returns PNG images.",
            json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Face key, e.g. voxel:block/stone or stone" },
                    "size": { "type": "integer", "enum": [64, 128, 256], "description": "Preview edge in texels (default 128)" }
                },
                "required": ["key"],
                "additionalProperties": false
            }),
            true,
        ),
        tool(
            "render_tile",
            "Render the committed 16² atlas tile as a 128² nearest-neighbour PNG.",
            key_schema(),
            true,
        ),
        tool(
            "render_atlas",
            "Build and return the 256² material atlas plus its hashes. Does not write.",
            empty_schema(),
            true,
        ),
        tool(
            "list_materials",
            "List the generated material table: one row per material and one six-face row per textured block, with staleness.",
            empty_schema(),
            true,
        ),
        tool(
            "read_manifest",
            "Read the committed textures.ron manifest exactly as stored.",
            empty_schema(),
            true,
        ),
        tool(
            "check",
            "Regenerate everything in memory and fail if any committed PNG, manifest byte, material row or atlas byte differs.",
            empty_schema(),
            true,
        ),
        tool(
            "generate",
            "Write all committed source PNGs, textures.ron, materials.gen.ron and the atlas. Use check first.",
            empty_schema(),
            false,
        ),
        tool(
            "bake_atlas",
            "Write only atlas.png and atlas.hash.ron from the committed manifest and sources.",
            empty_schema(),
            false,
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value, read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": false,
            "idempotentHint": true,
        }
    })
}

fn key_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "description": "Face key, e.g. voxel:block/stone or stone" }
        },
        "required": ["key"],
        "additionalProperties": false
    })
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn resources() -> Vec<Value> {
    let mut resources = vec![
        json!({
            "uri": "texgen://faces",
            "name": "faces",
            "description": "Every committed face, its slot, hashes and staleness.",
            "mimeType": "application/json",
        }),
        json!({
            "uri": "texgen://manifest",
            "name": "textures.ron",
            "description": "The committed texture manifest.",
            "mimeType": "application/ron",
        }),
        json!({
            "uri": "texgen://materials",
            "name": "materials.gen.ron",
            "description": "The generated material table.",
            "mimeType": "application/ron",
        }),
        json!({
            "uri": "texgen://atlas/hash",
            "name": "atlas.hash.ron",
            "description": "The committed atlas verification data.",
            "mimeType": "application/ron",
        }),
    ];
    for (key, _) in pipeline::SLICES {
        let name = pipeline::name_of(key);
        resources.push(json!({
            "uri": format!("texgen://graph/{name}"),
            "name": format!("{name}.ron"),
            "description": format!("Node graph for {key}."),
            "mimeType": "application/ron",
        }));
    }
    resources
}

fn normalize_key(input: &str) -> Result<String, String> {
    let key = if input.starts_with("voxel:block/") {
        input.to_owned()
    } else {
        format!(
            "voxel:block/{}",
            input.trim_start_matches("voxel:").trim_start_matches('/')
        )
    };
    if pipeline::slot_of(&key).is_none() {
        return Err(format!(
            "unknown face `{input}`; expected one of: {}",
            pipeline::SLICES
                .iter()
                .map(|(key, _)| pipeline::name_of(key))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(key)
}

fn relative(workspace: &Workspace, path: &std::path::Path) -> String {
    path.strip_prefix(workspace.root())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn upscale(source: &[u8], source_edge: usize, target_edge: usize) -> Vec<u8> {
    let mut out = vec![0u8; target_edge * target_edge * 4];
    for y in 0..target_edge {
        let sy = y * source_edge / target_edge;
        for x in 0..target_edge {
            let sx = x * source_edge / target_edge;
            let source_offset = (sy * source_edge + sx) * 4;
            let target_offset = (y * target_edge + x) * 4;
            out[target_offset..target_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }
    out
}

fn error_response(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

/// Small helper used by tests and the `validate_graph` output.
pub fn graph_node_type(graph: &Graph, index: usize) -> Option<Type> {
    graph.nodes.get(index).map(|node| op_type(&node.op))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_normalization_accepts_short_and_full_keys() {
        assert_eq!(
            normalize_key("stone").unwrap(),
            "voxel:block/stone".to_owned()
        );
        assert_eq!(
            normalize_key("voxel:block/stone").unwrap(),
            "voxel:block/stone".to_owned()
        );
        assert!(normalize_key("not_a_face").is_err());
    }

    #[test]
    fn upscale_repeats_pixels() {
        let source = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let out = upscale(&source, 2, 4);
        assert_eq!(&out[0..4], &[1, 2, 3, 4]);
        assert_eq!(&out[12..16], &[5, 6, 7, 8]);
        assert_eq!(&out[48..52], &[9, 10, 11, 12]);
        assert_eq!(&out[60..64], &[13, 14, 15, 16]);
    }

    fn server() -> Server {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("repository root")
            .to_owned();
        Server {
            workspace: Workspace::at(root).expect("workspace"),
        }
    }

    #[test]
    fn initialize_and_tools_list_are_protocol_shaped() {
        let server = server();
        let init = server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" }
                }
            }))
            .expect("initialize response");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["serverInfo"]["name"], "texgen");
        assert_eq!(
            init["result"]["capabilities"]["tools"]["listChanged"],
            false
        );

        let list = server
            .handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .expect("tools response");
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"list_faces"));
        assert!(names.contains(&"render_preview"));
        assert!(names.contains(&"generate"));
        assert!(!names.contains(&"write_graph"));
    }

    #[test]
    fn preview_tool_returns_text_and_images() {
        let server = server();
        let response = server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "render_preview",
                    "arguments": { "key": "stone", "size": 64 }
                }
            }))
            .expect("preview response");
        let content = response["result"]["content"].as_array().unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[2]["type"], "image");
        assert_eq!(content[1]["mimeType"], "image/png");
    }

    #[test]
    fn graph_resource_reads_the_committed_ron() {
        let server = server();
        let response = server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "resources/read",
                "params": { "uri": "texgen://graph/stone" }
            }))
            .expect("resource response");
        let text = response["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("voxel:block/stone"));
    }

    #[test]
    fn tool_catalog_has_unique_names() {
        let tools = tools();
        let names: std::collections::BTreeSet<_> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), tools.len());
    }
}
