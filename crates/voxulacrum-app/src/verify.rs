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
///
/// **512, not 64, because of what a sampled hash can and cannot see.** The
/// sample is a square XZ patch about the origin repeated over every chunk-Y
/// layer; 64 chunks is a 128-voxel footprint, and content placed on a coarse
/// world grid is observed only if an instance happens to land inside it. The
/// first structure source shipped (cell size 96, density 0.35) expected well
/// under one instance in that box and in fact produced none, so the anchor was
/// bit-identical with structures on and off - a regression check that could not
/// have caught a structure regression. 512 covers a 384-voxel footprint, about
/// five or six instances, and costs under a second.
///
/// **Raising the count buys probability, not a guarantee.** No sample size makes
/// this instrument reliable for sparse content; it samples a fixed box, and
/// anything placed sparsely is seen by luck. A new *sparse* content system needs
/// a targeted test asserting its property directly - `feature.rs`'s derivation
/// tests and `world_eval`'s seam test are that for stage 8 - and whoever adds
/// one should check that the anchor actually moves when the content is disabled,
/// rather than assuming coverage.
const DEFAULT_COUNT: usize = 512;
/// Chunk-Y layers to cover, matching the engine's default streaming range.
const LAYERS: i32 = 4;
/// Coordinates re-generated in reverse for the order-independence check. A
/// subset, because accumulated state shows on any coordinate and a full
/// sequential sweep would cost several times the parallel pass.
const ORDER_SAMPLE: usize = 64;

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

    // No `--seed` means the manifest's seed - the world as it actually
    // generates. Previously this defaulted to 0, so CI was verifying
    // determinism at a seed nothing ever plays at.
    let seed = args
        .iter()
        .position(|a| a == "--seed")
        .and_then(|i| args.get(i + 1))
        .and_then(|a| a.parse::<u64>().ok());

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

fn verify_generation(count: usize, seed: Option<u64>) -> i32 {
    let manifest = world_manifest_path();
    match seed {
        Some(s) => println!("verify-generation: {count} chunks, seed {s} (override)"),
        None => println!("verify-generation: {count} chunks, seed from manifest"),
    }
    println!("  manifest: {}", manifest.display());

    let loaded = match seed {
        Some(s) => WorldGenerator::from_manifest_seeded(&manifest, s),
        None => WorldGenerator::from_manifest(&manifest),
    };
    let generator = match loaded {
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

    // Order independence (P1). The pass above proves repeatability *within* one
    // generator and under parallelism; it cannot see state that accumulates
    // across different coordinates - chunk A generated after B differing from A
    // generated before it. That is exactly what a feature-cell memoization
    // introduces the moment it stops being purely an optimization, which design
    // §5 warns about by name, so it is worth a standing check rather than a
    // reading of the code.
    //
    // Reverse catches ordering, sequential removes the parallelism that would
    // mask it, and a second generator instance catches anything cached in the
    // first. A strided subset rather than the whole sample: accumulated state
    // shows on any coordinate, so one mismatch fails, and a full sequential
    // sweep would quadruple this command's runtime to prove the same thing.
    let mut order_dependent = 0usize;
    let reloaded = match seed {
        Some(s) => WorldGenerator::from_manifest_seeded(&manifest, s),
        None => WorldGenerator::from_manifest(&manifest),
    };
    match reloaded {
        Ok(fresh) => {
            let stride = (results.len() / ORDER_SAMPLE).max(1);
            let mut checked = 0usize;
            for (pos, expected, _) in results.iter().rev().step_by(stride) {
                let actual = hash_storage(&fresh.generate_chunk(*pos).storage);
                checked += 1;
                if actual != *expected {
                    order_dependent += 1;
                    eprintln!(
                        "  ORDER-DEPENDENT: chunk {pos:?} differs when generated in \
                         reverse order by a fresh generator",
                    );
                }
            }
            println!("  {checked} chunks reproduce in reverse order on a fresh generator");
        }
        Err(e) => {
            eprintln!("verify-generation: FAILED to reload manifest for the order check: {e}");
            return 2;
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

    if diverged > 0 || order_dependent > 0 {
        if diverged > 0 {
            eprintln!(
                "verify-generation: FAILED — {diverged}/{} chunks are not reproducible",
                results.len(),
            );
        }
        if order_dependent > 0 {
            eprintln!(
                "verify-generation: FAILED — {order_dependent} chunks depend on generation order",
            );
        }
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
