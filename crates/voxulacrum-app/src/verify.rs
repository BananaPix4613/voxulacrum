//! Headless generation verification (roadmap §4.5 Category D).
//!
//! Generates N chunks twice and compares them bit-for-bit, then prints an
//! aggregate hash. This is P1's CI enforcement: *same seed + graphs +
//! coordinates -> bit-identical output*.
//!
//! It runs from the normal binary behind `--verify-generation`, parsed before
//! the event loop exists, so nothing here touches `wgpu` or `winit`. A second
//! `[[bin]]` would have needed a `lib` target to reach these modules; a flag
//! needs neither.
//!
//! Generation runs on rayon's global pool rather than sequentially, which is the
//! point rather than a speed-up: §12's determinism rules include "no
//! thread-order-dependent generation," and running in parallel is what actually
//! tests it. It is also outside the frame schedule, so the
//! `nodegraph_eval::guard` schedule-thread assertion does not apply.
//!
//! **No golden hash is committed.** A reference file would fail on every
//! intentional graph edit during worldgen work, and a check people learn to
//! re-baseline reflexively is worse than no check. The hash is printed for
//! humans to compare across commits; the *pass/fail* condition is
//! self-consistency, which no intentional change should every break.

use glam::IVec3;
use rayon::prelude::*;
use crate::world::chunk::CHUNK_VOLUME;
use crate::world::storage::ChunkStorage;
use crate::world::world_generator::{world_manifest_path, WorldGenerator};

/// Chunks verified when no count is given.
const DEFAULT_COUNT: usize = 64;
/// Chunk-Y layers to cover, matching the engine's default streaming range.
const LAYERS: i32 = 4;

/// Handle `--verify-generation [N] [--seed S]`.
///
/// Returns `Some(exit_code)` when this was a verification run and the process
/// should exit, or `None` to fall through and start the engine normally.
pub fn run_from_args(args: &[String]) -> Option<i32> {
    let pos = args.iter().position(|a| a == "--verify-generation")?;

    // An immediately-following bare number is the count.
    let count = args
        .get(pos + 1)
        .filter(|a| !a.starts_with("--"))
        .and_then(|a| a.parse::<usize>().ok())
        .unwrap_or(DEFAULT_COUNT);

    let seed = args
        .iter()
        .position(|a| a == "--seed")
        .and_then(|i| args.get(i + 1))
        .and_then(|a| a.parse::<u64>().ok())
        .unwrap_or(0);

    Some(verify_generation(count, seed))
}

/// A deterministic block of chunk coordinates: a square XZ patch repeated over
/// every chunk-Y layer, so the sample covers surface, subsurface and air rather
/// than a strip of one.
fn positions(count: usize) -> Vec<IVec3> {
    let per_layer = count.div_ceil(LAYERS as usize);
    let side = (per_layer as f64).sqrt().ceil() as i32;
    let half = side / 2;

    let mut out = Vec::with_capacity(count);
    'outer: for cy in 0..LAYERS {
        for cz in -half..(side - half) {
            for cx in -half..(side - half) {
                out.push(IVec3::new(cx, cy, cz));
                if out.len() == count {
                    break 'outer;
                }
            }
        }
    }
    out
}

fn hash_storage(storage: &ChunkStorage) -> u64 {
    use seahash::SeaHasher;
    use std::hash::Hasher;

    let mut h = SeaHasher::new();
    for i in 0..CHUNK_VOLUME {
        h.write_u32(storage.voxel(i).pack());
    }
    h.finish()
}

fn verify_generation(count: usize, seed: u64) -> i32 {
    let manifest = world_manifest_path();
    println!("verify-generation: {count} chunks, seed {seed}");
    println!("  manifest: {}", manifest.display());

    let generator = match WorldGenerator::from_manifest(&manifest, seed) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("verify-generation: FAILED to load world manifest: {e}");
            return 2;
        }
    };

    let coords = positions(count);
    // `collect` preserves order, so the aggregate hash below is stable.
    let results: Vec<(IVec3, u64, bool)> = coords
        .par_iter()
        .map(|&pos| {
            let a = generator.generate_chunk(pos);
            let b = generator.generate_chunk(pos);
            let ha = hash_storage(&a.storage);
            let hb = hash_storage(&b.storage);
            (pos, ha, ha == hb)
        })
        .collect();

    let mut diverged = 0usize;
    for (pos, _, ok) in &results {
        if !ok {
            diverged += 1;
            eprintln!("  DIVERGED: chunk {pos:?} generated differently on two runs");
        }
    }

    let aggregate = {
        use seahash::SeaHasher;
        use std::hash::Hasher;
        let mut h = SeaHasher::new();
        for (_, chunk_hash, _) in &results {
            h.write_u64(*chunk_hash);
        }
        h.finish()
    };

    if diverged > 0 {
        eprintln!(
            "verify-generation: FAILED — {diverged}/{} chunks are not reproducible",
            results.len(),
        );
        return 1;
    }

    println!("  all {} chunks reproduce bit-identically", results.len());
    println!("  aggregate hash: {aggregate:#018x}");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_deterministic_and_cover_every_layer() {
        let a = positions(64);
        let b = positions(64);
        assert_eq!(a, b, "the sample must not depend on iteration order");
        assert_eq!(a.len(), 64);
        for cy in 0..LAYERS {
            assert!(
                a.iter().any(|p| p.y == cy),
                "sample must cover chunk-Y layer {cy}, not just the surface",
            );
        }
    }

    #[test]
    fn args_are_parsed_and_ignored_when_absent() {
        let none: Vec<String> = vec!["--some-other-flag".into()];
        assert!(
            run_from_args(&none).is_none(),
            "a non-verify invocation must fall through to the engine",
        );
    }
}
