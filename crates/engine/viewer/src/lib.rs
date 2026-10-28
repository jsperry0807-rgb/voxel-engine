//! Voxel viewer library.
//!
//! The binary in `main.rs` is a thin shell around these modules. Keeping the
//! atlas, camera and GPU types in a library also gives integration tests a
//! stable target (`cargo test -p voxel-viewer --test gpu`).

pub mod atlas;
pub mod atlas_texture;
pub mod bake;
pub mod camera;
pub mod chunk_mesh;
pub mod depth;
pub mod error;
pub mod gpu;
pub mod naive_mesh;
pub mod quads;
