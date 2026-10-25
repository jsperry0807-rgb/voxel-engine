//! Shared fixtures: registries and access sequences.
//!
//! The same registry shape and the same id sequences are used by the memory,
//! CPU, latency, Criterion and Callgrind lenses, so their numbers describe the
//! same workload.

use voxel_block::{
    BlockFlags, BlockProperties, BlockRegistry, BlockRegistryBuilder, FaceTextures, MaterialId,
    NamespacedKey, RenderType,
};

/// How many distinct textures the generated registries share.
///
/// Sharing keeps a 65 536-block registry well under the 65 536-texture cap
/// while still exercising distinct texture sets.
pub const SHARED_TEXTURES: usize = 64;

/// A frozen registry with `blocks` entries, including air at id 0.
///
/// Block `i` uses one of [`SHARED_TEXTURES`] six-face sets, alternates render
/// type and light, and carries solid/opaque flags, so hot-record packing sees
/// every field change.
pub fn registry(blocks: usize) -> BlockRegistry {
    let mut builder = BlockRegistryBuilder::with_capacity(blocks);
    for i in 1..blocks {
        let texture = NamespacedKey::builtin(&format!("block/t{}", i % SHARED_TEXTURES));
        let render = if i % 7 == 0 {
            RenderType::Cutout
        } else if i % 13 == 0 {
            RenderType::Translucent
        } else {
            RenderType::Solid
        };
        let properties = BlockProperties::new()
            .with_render_type(render)
            .with_textures(FaceTextures::all(texture))
            .with_flags(BlockFlags::SOLID | BlockFlags::OPAQUE)
            .with_light((i % 16) as u8, ((i / 16) % 16) as u8);
        builder
            .register(NamespacedKey::builtin(&format!("b{i}")), properties)
            .expect("fixture keys are unique and the table has room");
    }
    builder
        .freeze()
        .expect("shared textures stay under the cap")
}

fn rng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// `count` material ids drawn uniformly from the registry (air included).
pub fn random_ids(registry: &BlockRegistry, count: usize, seed: u64) -> Vec<MaterialId> {
    if registry.len() <= 1 {
        return vec![MaterialId::from_raw(0); count];
    }
    let mut state = seed | 1;
    (0..count)
        .map(|_| MaterialId::from_raw((rng(&mut state) as usize % registry.len()) as u16))
        .collect()
}

/// `count` material ids in short runs, like a ray crossing a few materials.
///
/// Every 16th draw picks a new id; the rest repeat it.
pub fn coherent_ids(registry: &BlockRegistry, count: usize, seed: u64) -> Vec<MaterialId> {
    if registry.len() <= 1 {
        return vec![MaterialId::from_raw(0); count];
    }
    let mut state = seed | 1;
    let mut current = 1u16;
    (0..count)
        .map(|i| {
            if i % 16 == 0 {
                current = (rng(&mut state) as usize % registry.len()) as u16;
            }
            MaterialId::from_raw(current)
        })
        .collect()
}
