//! Voxel viewer library.
//!
//! The binary in `main.rs` is a thin shell around these modules. Keeping the
//! atlas, camera and GPU types in a library also gives integration tests a
//! stable target (`cargo test -p voxel-viewer --test gpu`).

pub mod atlas;
pub mod atlas_texture;
pub mod bake;
pub mod camera;
pub mod chunk_content;
pub mod chunk_lighting;
pub mod chunk_mesh;
pub mod chunk_render;
pub mod cull;
pub mod depth;
pub mod error;
pub mod frametime;
pub mod gpu;
pub mod hitch;
pub mod input;
pub mod mesh_pool;
pub mod naive_mesh;
pub mod pose;
pub mod quads;
pub mod scene;
pub mod screenshot;
pub mod stats;
pub mod streaming;
pub mod tracker;
pub mod upload;
