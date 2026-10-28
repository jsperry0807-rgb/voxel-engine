//! `texgen-mcp`: MCP stdio server for the texture generator.

use std::process::ExitCode;

fn main() -> ExitCode {
    match voxel_texgraph::mcp::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("texgen-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}
