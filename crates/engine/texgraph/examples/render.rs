//! Render one graph RON to a PNG without touching the committed set.
//!
//! ```sh
//! cargo run -p voxel-texgraph --example render -- graph.ron out.png [size]
//! ```

use std::process::ExitCode;

use voxel_texgraph::{eval, graph::Graph, png};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        eprintln!("usage: render GRAPH.ron OUT.png [SIZE]");
        return ExitCode::FAILURE;
    };
    let size = args
        .get(2)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(256);
    if size == 0 || size > 1024 || !size.is_multiple_of(16) {
        eprintln!("size must be a multiple of 16 in 16..=1024");
        return ExitCode::FAILURE;
    }
    let graph = match Graph::load(std::path::Path::new(input)) {
        Ok(graph) => graph,
        Err(error) => {
            eprintln!("texgen render: {error}");
            return ExitCode::FAILURE;
        }
    };
    let program = match graph.compile() {
        Ok(program) => program,
        Err(error) => {
            eprintln!("texgen render: {error}");
            return ExitCode::FAILURE;
        }
    };
    let pixels = eval::evaluate_at(&program, size);
    let encoded = match png::encode_rgba(size, size, &pixels) {
        Ok(encoded) => encoded,
        Err(error) => {
            eprintln!("texgen render: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = std::fs::write(output, encoded) {
        eprintln!("texgen render: {output}: {error}");
        return ExitCode::FAILURE;
    }
    println!("{} -> {output} ({size}x{size})", graph.key);
    ExitCode::SUCCESS
}
