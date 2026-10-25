//! Material-table layout: old 40-byte `BlockRecord` array vs the 8-byte hot table
//! (+ deduplicated texture sets). Same registry, same id sequence, same work.
//!
//! "old" = `Vec<BlockRecord>`, which is exactly the layout the registry used to store.
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use voxel_block::{
    BlockProperties, BlockRecord, BlockRegistry, BlockRegistryBuilder, FaceTextures, MaterialId,
    NamespacedKey, RenderType,
};
use voxel_core::Face;

fn rng(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// `n` blocks over `textures` distinct textures (most blocks share sets).
fn registry(n: usize, textures: usize) -> BlockRegistry {
    let mut b = BlockRegistryBuilder::with_capacity(n);
    for i in 1..n {
        let tex = |k: usize| NamespacedKey::builtin(&format!("block/t{}", k % textures));
        let faces = if i % 3 == 0 {
            FaceTextures::column(tex(i), tex(i + 1), tex(i + 2))
        } else {
            FaceTextures::all(tex(i))
        };
        b.register(
            NamespacedKey::builtin(&format!("b{i}")),
            BlockProperties::new()
                .with_render_type(if i % 7 == 0 {
                    RenderType::Cutout
                } else {
                    RenderType::Solid
                })
                .with_textures(faces)
                .with_flags(voxel_block::BlockFlags::SOLID | voxel_block::BlockFlags::OPAQUE)
                .with_light((i % 16) as u8, ((i / 16) % 16) as u8),
        )
        .unwrap();
    }
    b.freeze().unwrap()
}

fn ids(n: usize, count: usize, coherent: bool) -> Vec<MaterialId> {
    let mut s = 0x1234_5678_9ABC_DEF1u64;
    let mut last = 1usize;
    (0..count)
        .map(|_| {
            let r = rng(&mut s);
            // coherent: long runs of the same few ids (like a ray through one material)
            if coherent && r & 15 != 0 {
                MaterialId::from_raw(last as u16)
            } else {
                last = 1 + (r >> 8) as usize % (n - 1);
                MaterialId::from_raw(last as u16)
            }
        })
        .collect()
}

fn bench(c: &mut Criterion) {
    const LOOKUPS: usize = 1 << 18;
    for (n, textures) in [(16usize, 16usize), (512, 256), (65_536, 4096)] {
        let reg = registry(n, textures);
        let old: Vec<BlockRecord> = (0..n)
            .map(|i| reg.resolved(MaterialId::from_raw(i as u16)))
            .collect();
        println!(
            "n={n}: old table {} KiB | hot table {} KiB + {} texture sets ({} B)",
            old.len() * size_of::<BlockRecord>() / 1024,
            reg.hot_table().len() * 8 / 1024,
            reg.texture_sets().len(),
            reg.texture_sets().len() * 12,
        );

        for coherent in [false, true] {
            let seq = ids(n, LOOKUPS, coherent);
            let label = format!("n{n}/{}", if coherent { "coherent" } else { "random" });
            let mut g = c.benchmark_group(format!("material/{label}"));
            g.throughput(Throughput::Elements(LOOKUPS as u64));
            g.sample_size(10);
            g.warm_up_time(std::time::Duration::from_millis(500));
            g.measurement_time(std::time::Duration::from_secs(2));

            // Light/opacity/flags only: what a lighting or meshing loop reads.
            g.bench_function("meta/old40", |b| {
                b.iter(|| {
                    let mut acc = 0u32;
                    for &id in &seq {
                        let r = &old[id.index()];
                        acc = acc.wrapping_add(
                            r.light_opacity as u32 + r.flags.as_bits() + r.render_type as u32,
                        );
                    }
                    black_box(acc)
                })
            });
            g.bench_function("meta/hot8", |b| {
                let hot = reg.hot_table();
                b.iter(|| {
                    let mut acc = 0u32;
                    for &id in &seq {
                        let r = hot[id.index()];
                        acc = acc.wrapping_add(
                            r.light_opacity() as u32 + r.flags().as_bits() + r.render_type() as u32,
                        );
                    }
                    black_box(acc)
                })
            });

            // Full surface hit: meta + the texture for one face.
            g.bench_function("hit/old40", |b| {
                b.iter(|| {
                    let mut acc = 0u32;
                    for (i, &id) in seq.iter().enumerate() {
                        let r = &old[id.index()];
                        let face = Face::ALL[i % 6];
                        let t = r.textures.map_or(0, |t| t.get(face).as_raw() as u32);
                        acc = acc.wrapping_add(t + r.light_opacity as u32 + r.render_type as u32);
                    }
                    black_box(acc)
                })
            });
            g.bench_function("hit/hot8+sets", |b| {
                b.iter(|| {
                    let mut acc = 0u32;
                    for (i, &id) in seq.iter().enumerate() {
                        let r = reg.hot(id);
                        let face = Face::ALL[i % 6];
                        let t = r
                            .texture_set()
                            .map_or(0, |s| reg.texture_set(s).get(face).as_raw() as u32);
                        acc =
                            acc.wrapping_add(t + r.light_opacity() as u32 + r.render_type() as u32);
                    }
                    black_box(acc)
                })
            });
            g.finish();
        }
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
