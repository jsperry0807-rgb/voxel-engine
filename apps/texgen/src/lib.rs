//! `voxel-texgen`: the standalone desktop editor for Voxel-Stuff textures.
//!
//! The deterministic core lives in `voxel-texgraph`. This crate adds the
//! Material Maker-style egui application and the `texgen` entry point.

pub mod app;

pub use voxel_texgraph::*;
