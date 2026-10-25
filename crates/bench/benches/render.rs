//! Criterion lens for the GPU upload preparation and the storage flush path.
//!
//! These bench CPU-side packing and the sink contract with an in-memory sink.
//! A real wgpu upload benchmark runs only with `VOXEL_GPU_BENCH=1`, because CI
//! machines usually have no adapter and driver teardown varies wildly.

use std::sync::OnceLock;

use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};
use voxel_bench::{fixtures, patterns};
use voxel_storage::ChunkPos;
use voxel_storage::dag::Svdag;
use voxel_storage::gpu::{GpuStore, MemSink};

const BLOCKS: usize = 65_536;

fn bench_material_words(c: &mut Criterion) {
    let registry = fixtures::registry(BLOCKS);
    let mut group = c.benchmark_group("render");
    group.throughput(Throughput::Elements(BLOCKS as u64));
    group.bench_function("hot_words/65536", |b| {
        b.iter(|| black_box(registry.gpu_hot_words()))
    });
    group.throughput(Throughput::Elements(registry.texture_sets().len() as u64));
    group.bench_function("texture_set_words/65536", |b| {
        b.iter(|| black_box(registry.gpu_texture_set_words()))
    });
    group.finish();
}

fn bench_flush(c: &mut Criterion) {
    let chunks: Vec<(ChunkPos, Svdag)> = (0..64i32)
        .map(|i| {
            let (x, z) = (i % 8, i / 8);
            (
                ChunkPos::new(x, 0, z),
                Svdag::from_dense(&patterns::terrain(x, z)),
            )
        })
        .collect();

    let mut group = c.benchmark_group("gpu_flush");
    group.sample_size(20);
    group.throughput(Throughput::Elements(1));
    group.bench_function("import_64_chunks", |b| {
        b.iter_batched(
            GpuStore::new,
            |mut store| {
                for (pos, dag) in &chunks {
                    store.set_chunk(*pos, dag);
                }
                black_box(store.unique_nodes())
            },
            BatchSize::LargeInput,
        )
    });
    group.bench_function("flush_after_edits", |b| {
        let mut store = GpuStore::new();
        for (pos, dag) in &chunks {
            store.set_chunk(*pos, dag);
        }
        let mut sink = MemSink::default();
        let mut index = 0u32;
        b.iter(|| {
            index = index.wrapping_add(1);
            for (pos, _) in &chunks {
                store.apply_edits(*pos, [(index % 262_144, 4)]);
            }
            store.flush(&mut sink);
            let tag = store.end_batch();
            store.reclaim(tag);
            black_box(sink.bytes_written)
        })
    });
    group.finish();
}

struct Gpu {
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<&'static Gpu> {
    static GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("voxel-bench"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .ok()?;
        Some(Gpu {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
        })
    })
    .as_ref()
}

fn bench_gpu_upload(c: &mut Criterion) {
    if std::env::var_os("VOXEL_GPU_BENCH").is_none() {
        eprintln!("VOXEL_GPU_BENCH not set; skipping real wgpu upload benchmarks");
        return;
    }
    let Some(gpu) = gpu() else {
        eprintln!("no wgpu adapter; skipping real upload benchmarks");
        return;
    };
    let registry = fixtures::registry(BLOCKS);
    let mut group = c.benchmark_group("render_wgpu");
    group.sample_size(10);
    group.throughput(Throughput::Elements(BLOCKS as u64));
    group.bench_function("upload_material_tables", |b| {
        b.iter(|| {
            black_box(voxel_render::MaterialBuffers::upload(
                &gpu.device,
                &gpu.queue,
                &registry,
            ))
        })
    });
    group.finish();
}

fn bench_overlay(c: &mut Criterion) {
    let dag = Svdag::from_dense(&patterns::terrain(0, 0));
    let pos = ChunkPos::new(0, 0, 0);

    let mut group = c.benchmark_group("overlay_gpu");
    group.bench_function("set_one", |b| {
        let mut store = GpuStore::new();
        store.set_chunk(pos, &dag);
        store.set_overlay(pos, 0, 4); // allocate table + slot outside the samples
        let mut idx = 0u32;
        b.iter(|| {
            idx = (idx + 1) & 0xFFF;
            store.set_overlay(pos, idx, 4);
        })
    });
    group.bench_function("lookup_hit", |b| {
        let mut store = GpuStore::new();
        store.set_chunk(pos, &dag);
        for within in 0..4096u32 {
            let x = within & 15;
            let y = (within >> 4) & 15;
            let z = (within >> 8) & 15;
            store.set_overlay(pos, x | (y << 6) | (z << 12), 4);
        }
        b.iter(|| black_box(store.lookup(pos, 5, 5, 5)))
    });
    group.bench_function("flush_64", |b| {
        let mut store = GpuStore::new();
        for i in 0..64i32 {
            let (x, z) = (i % 8, i / 8);
            store.set_chunk(
                ChunkPos::new(x, 0, z),
                &Svdag::from_dense(&patterns::terrain(x, z)),
            );
            store.end_batch();
        }
        let mut sink = MemSink::default();
        let mut idx = 0u32;
        b.iter(|| {
            idx = idx.wrapping_add(1);
            for i in 0..64i32 {
                let (x, z) = (i % 8, i / 8);
                store.set_overlay(ChunkPos::new(x, 0, z), idx & 0xFFF, 4);
            }
            store.flush(&mut sink);
            let tag = store.end_batch();
            store.reclaim(tag);
            black_box(sink.bytes_written)
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_material_words,
    bench_flush,
    bench_overlay,
    bench_gpu_upload
);
criterion_main!(benches);
