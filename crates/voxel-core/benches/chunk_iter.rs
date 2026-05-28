//! Phase 1 chunk iteration benchmark.
//!
//! Targets (per the design review):
//! - `iter_uniform_32` - Uniform variant full iteration
//! - `iter_palette_32` - Palette variant full iteration
//! - `iter_dense_32`   - Dense variant full iteration
//! - `scatter_writes`  - 1024 random sets
//! - `promote_to_palette` - first divergent write cost
//! - `try_collapse`    - collapse pass on a uniform-able buffer

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use voxel_core::{ChunkBuffer, MaterialId, Voxel};

type Buf32 = ChunkBuffer<Voxel, 32>;

fn bench_iter_variants(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunk_iter");

    // Uniform
    let uniform: Buf32 = ChunkBuffer::uniform(Voxel::EMPTY);
    group.bench_function("iter_uniform_32", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            uniform.for_each(|_, _, _, v| acc = acc.wrapping_add(v.pack()));
            black_box(acc);
        });
    });

    // Palette with -8 distinct values
    let mut palette = Buf32::uniform(Voxel::EMPTY);
    for i in 0..8 {
        palette.set_index(i * 4096, Voxel::cube(MaterialId(i as u16 + 1)));
    }
    group.bench_function("iter_palette_32", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            palette.for_each(|_, _, _, v| acc = acc.wrapping_add(v.pack()));
            black_box(acc);
        });
    });

    // Dense
    let mut dense = palette.clone();
    dense.make_dense();
    group.bench_function("iter_dense_32", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            dense.for_each(|_, _, _, v| acc = acc.wrapping_add(v.pack()));
            black_box(acc);
        });
    });

    group.finish();
}

fn bench_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunk_writes");

    group.bench_function("promote_to_palette_first_write", |b| {
        b.iter_batched(
            || Buf32::uniform(Voxel::EMPTY),
            |mut buf| {
                buf.set(0, 0, 0, Voxel::cube(MaterialId(1)));
                black_box(buf);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("scatter_1024_writes", |b| {
        b.iter_batched(
            || Buf32::uniform(Voxel::EMPTY),
            |mut buf| {
                // Deterministic linear-congruential walk; covers -1024 cells.
                let mut x = 1u32;
                for _ in 0..1024 {
                    x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                    let i = (x as usize) % Buf32::VOLUME;
                    buf.set_index(i, Voxel::cube(MaterialId((x & 0xF) as u16 + 1)));
                }
                black_box(buf);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_collapse(c: &mut Criterion) {
    c.bench_function("try_collapse_uniformable", |b| {
        b.iter_batched(
            || {
                // Buffer that *can* collapse: every entry is identical even
                // though it's been promoted out of Uniform.
                let mut buf = Buf32::uniform(Voxel::EMPTY);
                buf.set(0, 0, 0, Voxel::cube(MaterialId(1))); // promote
                buf.set(0, 0, 0, Voxel::EMPTY);               // back to all-empty
                buf
            },
            |mut buf| {
                let collapsed = buf.try_collapse();
                black_box(collapsed);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_iter_variants, bench_writes, bench_collapse);
criterion_main!(benches);
