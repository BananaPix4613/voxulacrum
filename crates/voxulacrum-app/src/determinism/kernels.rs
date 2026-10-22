//! Kernel results the determinism manifest hashes directly (R7): tree
//! skeletons and Poisson-disk placement, the generation code that uses
//! transcendental math.
//!
//! The generated base sees both only through what they write into chunks, and
//! that hides most of a difference: a skeleton node that moves by a fraction of
//! a voxel changes no voxel, and no shipped graph places by Poisson disk. Hashing
//! their output here compares the math itself.

use nodegraph_eval::poisson_placement;
use nodegraph_eval::skeleton::Skeleton;
use nodegraph_ir::TreeSpecies;

use super::{LayerHash, Record};

/// Seeds each skeleton variant and each Poisson-disk setting is derived with.
const SEEDS: u64 = 8;

/// Poisson-disk settings: radius in voxels, and candidates tried per point.
const POISSON: [(f32, u32); 2] = [(2.0, 30), (5.0, 8)];

/// Skeletons of the default species with and without space colonization, and
/// Poisson-disk point sets, each hashed over the little-endian bits of every
/// field.
///
/// Fails if a skeleton grew no canopy anywhere, if colonization added nothing,
/// or if a point set holds fewer than two points: each would leave math
/// unexercised while still hashing identically everywhere.
pub(super) fn records() -> Result<Vec<Record>, String> {
    let turtle = TreeSpecies::default();
    let crowned = TreeSpecies { crown_attractors: 400, ..TreeSpecies::default() };

    let mut records = Vec::new();
    let mut skeletons = 0usize;
    let mut lobed = 0usize;
    for seed in 0..SEEDS {
        let bare = Skeleton::derive(seed, &turtle, 1.0);
        let full = Skeleton::derive(seed, &crowned, 1.0);
        if full.nodes.len() <= bare.nodes.len() {
            return Err(format!(
                "kernels: colonization added no nodes to seed {seed} ({} against {}). Check \
                 TreeSpecies' crown defaults.",
                full.nodes.len(),
                bare.nodes.len(),
            ));
        }
        for (variant, s) in [("turtle", &bare), ("crowned", &full)] {
            if !s.lobes.is_empty() {
                lobed += 1;
            }
            records.push(skeleton_record(variant, seed, s));
            skeletons += 1;
        }
    }
    if lobed == 0 {
        return Err("kernels: no skeleton grew a canopy lobe. Check TreeSpecies' lobe defaults."
            .to_string());
    }

    let mut point_sets = 0usize;
    for (radius, k) in POISSON {
        for seed in 0..SEEDS {
            let points = poisson_placement(seed, radius, k);
            if points.len() < 2 {
                return Err(format!(
                    "kernels: Poisson disk at radius {radius}, k {k}, seed {seed} placed {} \
                     point(s); it must place at least two",
                    points.len(),
                ));
            }
            records.push(Record {
                at: format!("poisson radius {radius}, k {k}, seed {seed}"),
                kinds: Vec::new(),
                layers: vec![LayerHash::of("points", |b| {
                    for &(x, z) in &points {
                        put_f32s(b, &[x, z]);
                    }
                })],
            });
            point_sets += 1;
        }
    }

    println!("  kernels: {skeletons} skeletons, {point_sets} point sets");
    Ok(records)
}

/// One skeleton's nodes, lobes and solved values.
fn skeleton_record(variant: &str, seed: u64, s: &Skeleton) -> Record {
    Record {
        at: format!("skeleton {variant}, seed {seed}"),
        kinds: Vec::new(),
        layers: vec![
            LayerHash::of("nodes", |b| {
                for n in &s.nodes {
                    put_f32s(b, &[n.pos.x, n.pos.y, n.pos.z, n.radius, n.weight]);
                    for v in [n.parent, n.chain, n.chain_idx] {
                        b.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }),
            LayerHash::of("lobes", |b| {
                for l in &s.lobes {
                    put_f32s(
                        b,
                        &[l.center.x, l.center.y, l.center.z, l.radius, l.aniso.x, l.aniso.y, l.aniso.z],
                    );
                    b.extend_from_slice(&l.anchor_node.to_le_bytes());
                }
            }),
            LayerHash::of("solved", |b| {
                b.extend_from_slice(&s.chains.to_le_bytes());
                put_f32s(b, &[s.tip_radius, s.pipe_exp, s.reach, s.height]);
            }),
        ],
    }
}

/// Append each value's bits, little-endian, so equal floats hash equally on
/// every platform.
fn put_f32s(b: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        b.extend_from_slice(&v.to_bits().to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kernel_record_is_produced_and_its_checks_pass() {
        // The checks also run in every manifest run; here they run with the
        // ordinary tests, so a change to the species defaults that empties a
        // check fails before the determinism job does.
        let records = records().expect("the kernel checks hold");
        assert_eq!(records.len(), 2 * SEEDS as usize + POISSON.len() * SEEDS as usize);
    }
}

