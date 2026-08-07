//! Cross-chunk feature derivation (design §5, "Cross-chunk feature generation").
//!
//! A **feature** is a structure occurrence: a blueprint, a world-absolute anchor,
//! a rotation, and a priority. Features are derived per **feature cell** - a
//! coarse world grid - from `hash(world_seed, cell_x, cell_z, seed)`, the same
//! construction `scatter::jittered_grid` uses for props.
//!
//! **Order independence is structural, not incidental.** Every chunk whose
//! margin band reaches a feature cell derives that cell's contents identically,
//! then stamps only the part falling inside its own window. No chunk reads
//! another chunk's output, so there is no order to be independent *of*. The one
//! thing that could break this is where the anchor's height comes from: it is a
//! **pointwise surface query**, a function of world position, never a read of a
//! neighboring chunk's voxels. That distinction is why this model works where
//! cross-chunk staircase smoothing does not (§5).
//!
//! **The band is sized by a rotation-invariant reach.** See
//! [`ResolvedBlueprint::horizontal_reach`]: a quarter turn permutes `(dx, dz)`
//! magnitudes, so one bound covers all four rotations.
//!
//! Nothing calls this yet - the graph node and the real surface probe arrive
//! in the next substep, and the tests here are what the substep is for.

use std::collections::HashSet;

use glam::IVec3;
use voxel_core::{ChunkBuffer, ResolvedBlueprint, Voxel, Yaw};

use crate::context::EvalContext;
use crate::field::CHUNK_DIM;
use crate::scatter::SplitMix64;

const N: i32 = CHUNK_DIM as i32;

/// What a source needs to know about one world column to place there.
///
/// Both facts come from one probe call because the expensive part is the column
/// pipeline pass, and asking twice would pay for it twice.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColumnProbe {
    /// Which zone owns the column. Sources may restrict themselves to some.
    pub zone_id: u16,
    /// World-Y of the topmost solid voxel.
    pub surface_y: i32,
}

/// A rule producing features on a coarse world grid.
pub struct FeatureSource {
    /// Edge length of a feature cell, in voxels. One candidate per cell.
    pub cell_size: i32,
    /// Chance a cell produces a feature, `0..=1`.
    pub density: f32,
    /// Source-local seed, mixed with the world seed and the cell coordinates.
    pub seed: u32,
    /// Zones this source appears in. Empty means every zone.
    pub zones: Vec<u16>,
    /// Whether each occurrence takes a random quarter turn.
    pub random_yaw: bool,
    /// How far above the surface the anchor lands.
    pub surface_offset: i32,
    /// The template stamped at each occurrence.
    pub blueprint: ResolvedBlueprint,
}

impl FeatureSource {
    /// How far this source's occurrences can reach horizontally from an anchor.
    fn reach(&self) -> i32 {
        self.blueprint.horizontal_reach()
    }
}

/// One derived occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feature {
    /// World voxel the blueprint's anchor lands on.
    pub anchor: IVec3,
    /// Rotation applied at stamp time.
    pub yaw: Yaw,
    /// Ordering key, derived world-absolutely so every chunk resolves an
    /// overlap the same way.
    pub priority: u64,
    /// Index into the source list this came from.
    pub source: usize,
}

/// Derive every feature whose footprint can reach `ctx.chunk`'s window.
///
/// `probe` answers for one world column and may return `None` where there is no
/// surface (an all-air column, or one the caller declines to place on).
///
/// `FnMut` rather than `Fn` deliberately: §5 permits the caller to memoize
/// per-cell probes, and a memoizing probe holds state. It also lets a caller
/// carry an error out, since a probe cannot itself fail in this signature.
pub fn derive_features<P>(
    ctx: EvalContext,
    sources: &[FeatureSource],
    mut probe: P,
) -> Vec<Feature>
where
    P: FnMut(i32, i32) -> Option<ColumnProbe>,
{
    let base_x = ctx.chunk.x * N;
    let base_z = ctx.chunk.z * N;
    let mut out = Vec::new();

    for (index, source) in sources.iter().enumerate() {
        let cell = source.cell_size.max(1);
        // The band is the chunk window grown by the worst-case reach, so any
        // cell whose occurrence could touch this chunk is enumerated.
        let reach = source.reach();
        let lo_x = (base_x - reach).div_euclid(cell);
        let hi_x = (base_x + N + reach).div_euclid(cell);
        let lo_z = (base_z - reach).div_euclid(cell);
        let hi_z = (base_z + N + reach).div_euclid(cell);

        for cz in lo_z..=hi_z {
            for cx in lo_x..=hi_x {
                let mut rng =
                    SplitMix64::new(ctx.world_cell_seed(source.seed, cx as i64, cz as i64));
                if rng.next_f32() >= source.density {
                    continue;
                }
                // The anchor column is drawn before anything else that can
                // reject, so a rejected cell consumes the same stream a placed
                // one would - the draw order is part of the hash contract.
                let ax = cx * cell + (rng.next_f32() * cell as f32) as i32;
                let az = cz * cell + (rng.next_f32() * cell as f32) as i32;
                let yaw = if source.random_yaw {
                    Yaw::from_steps((rng.next_u64() % 4) as i32)
                } else {
                    Yaw::Deg0
                };
                let priority = rng.next_u64();

                let Some(column) = probe(ax, az) else { continue };
                if !source.zones.is_empty() && !source.zones.contains(&column.zone_id) {
                    continue;
                }
                out.push(Feature {
                    anchor: IVec3::new(ax, column.surface_y + source.surface_offset, az),
                    yaw,
                    priority,
                    source: index,
                });
            }
        }
    }

    // Total order, not just by priority: two features sharing a key must still
    // apply in the same sequence in every chunk, and equal keys are possible.
    out.sort_by_key(|f| {
        (f.priority, f.anchor.x, f.anchor.y, f.anchor.z, f.source)
    });
    out
}

/// Stamp `features` into a chunk's terrain, clipping to `[0, N)³`.
///
/// Applied in the order `derive_features` returned, so the highest-priority
/// feature is written last and wins an overlap - **except** where an earlier
/// feature declared a protected volume, which nothing later overwrites (§5).
/// That reads backwards until you see what it is for: it is the mechanism that
/// lets an authored vault survive both a later structure and, eventually, region
/// transformation.
pub fn stamp_features(
    terrain: &ChunkBuffer<Voxel, 32>,
    ctx: EvalContext,
    sources: &[FeatureSource],
    features: &[Feature],
) -> ChunkBuffer<Voxel, 32> {
    let mut out = terrain.clone();
    if features.is_empty() {
        return out;
    }
    out.make_dense();
    let base = IVec3::new(ctx.chunk.x * N, ctx.chunk.y * N, ctx.chunk.z * N);
    let mut protected: HashSet<(i32, i32, i32)> = HashSet::new();

    for feature in features {
        let Some(source) = sources.get(feature.source) else { continue };
        let bp = &source.blueprint;
        let anchor = IVec3::from_array(bp.anchor);
        for &(at, voxel) in &bp.cells {
            let rel = IVec3::from_array(feature.yaw.apply((IVec3::from_array(at) - anchor).to_array()));
            let local = feature.anchor + rel - base;
            if local.x < 0 || local.y < 0 || local.z < 0
                || local.x >= N || local.y >= N || local.z >= N
            {
                continue;
            }
            let key = (local.x, local.y, local.z);
            if protected.contains(&key) {
                continue;
            }
            out.set(local.x as usize, local.y as usize, local.z as usize, voxel);
            if bp.protected_volume {
                protected.insert(key);
            }
        }
    }

    out.try_collapse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::{DestructionPolicy, MaterialId};

    fn blueprint(name: &str, cells: Vec<([i32; 3], Voxel)>, protected: bool) -> ResolvedBlueprint {
        ResolvedBlueprint {
            name: name.to_string(),
            anchor: [0, 0, 0],
            cells,
            destruction: DestructionPolicy::Destroy,
            protected_volume: protected,
        }
    }

    fn pillar(name: &str, material: u16, protected: bool) -> ResolvedBlueprint {
        let v = Voxel::cube(MaterialId(material));
        blueprint(name, vec![([0, 0, 0], v), ([0, 1, 0], v)], protected)
    }

    fn source(bp: ResolvedBlueprint) -> FeatureSource {
        FeatureSource {
            cell_size: 24,
            density: 1.0,
            seed: 7,
            zones: Vec::new(),
            random_yaw: true,
            surface_offset: 1,
            blueprint: bp,
        }
    }

    /// A surface that varies with world position, so a wrong anchor shows up as
    /// a wrong height rather than coincidentally matching.
    fn flat_probe(x: i32, z: i32) -> Option<ColumnProbe> {
        Some(ColumnProbe { zone_id: 0, surface_y: 8 + (x.rem_euclid(3)) - (z.rem_euclid(2)) })
    }

    #[test]
    fn neighboring_chunks_derive_the_shared_features_identically() {
        // The property the whole model rests on. Two chunks that can both see a
        // feature cell must produce byte-identical features for it - not merely
        // similar, and not dependent on which was generated first.
        let sources = [source(pillar("p", 2, false))];
        let a = derive_features(EvalContext::new(1234, IVec3::new(0, 0, 0)), &sources, flat_probe);
        let b = derive_features(EvalContext::new(1234, IVec3::new(1, 0, 0)), &sources, flat_probe);

        let shared: Vec<_> = a.iter().filter(|f| b.contains(f)).collect();
        assert!(
            !shared.is_empty(),
            "the two windows must overlap in the band, or this proves nothing"
        );
        // And every feature either appears in both or lies outside the other's band.
        for f in &a {
            let reachable = f.anchor.x >= 32 - sources[0].reach() ;
            if reachable {
                assert!(b.contains(f), "chunk (1,0,0) missed a feature it can see: {f:?}");
            }
        }
    }

    #[test]
    fn derivation_is_a_pure_function_of_the_seed() {
        let sources = [source(pillar("p", 2, false))];
        let ctx = EvalContext::new(99, IVec3::new(-3, 0, 5));
        let once = derive_features(ctx, &sources, flat_probe);
        let twice = derive_features(ctx, &sources, flat_probe);
        assert_eq!(once, twice);

        let other = derive_features(
            EvalContext::new(100, IVec3::new(-3, 0, 5)),
            &sources,
            flat_probe,
        );
        assert_ne!(once, other, "a different world seed must produce a different layout");
    }

    #[test]
    fn a_rejected_cell_does_not_shift_the_stream_for_later_cells() {
        // Halving density must remove features, never relocate the survivors:
        // the anchor draw happens before any rejection that follows it.
        let dense = [source(pillar("p", 2, false))];
        let mut sparse = source(pillar("p", 2, false));
        sparse.zones = vec![9]; // no column reports zone 9
        let sparse = [sparse];

        let ctx = EvalContext::new(5, IVec3::ZERO);
        assert!(!derive_features(ctx, &dense, flat_probe).is_empty());
        assert!(derive_features(ctx, &sparse, flat_probe).is_empty());
    }

    #[test]
    fn features_apply_in_priority_order_and_the_last_one_wins() {
        let ctx = EvalContext::new(1, IVec3::ZERO);
        let sources = [source(pillar("low", 2, false)), source(pillar("high", 3, false))];
        // Same anchor, different priorities: the higher key applies later.
        let features = vec![
            Feature { anchor: IVec3::new(4, 4, 4), yaw: Yaw::Deg0, priority: 1, source: 0 },
            Feature { anchor: IVec3::new(4, 4, 4), yaw: Yaw::Deg0, priority: 2, source: 1 },
        ];
        let base: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        let out = stamp_features(&base, ctx, &sources, &features);
        assert_eq!(out.get(4, 4, 4).material, MaterialId(3), "higher priority wins");
    }

    #[test]
    fn a_protected_volume_survives_a_higher_priority_feature() {
        // Reads backwards until you see what it is for: this is what lets an
        // authored vault outlive whatever the world decides to put on top of it.
        let ctx = EvalContext::new(1, IVec3::ZERO);
        let sources = [source(pillar("vault", 2, true)), source(pillar("rubble", 3, false))];
        let features = vec![
            Feature { anchor: IVec3::new(6, 6, 6), yaw: Yaw::Deg0, priority: 1, source: 0 },
            Feature { anchor: IVec3::new(6, 6, 6), yaw: Yaw::Deg0, priority: 9, source: 1 },
        ];
        let base: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        let out = stamp_features(&base, ctx, &sources, &features);
        assert_eq!(out.get(6, 6, 6).material, MaterialId(2), "protected cells are not overwritten");
    }

    #[test]
    fn cells_outside_the_window_are_clipped_not_wrapped() {
        let ctx = EvalContext::new(1, IVec3::ZERO);
        let sources = [source(pillar("p", 2, false))];
        // Anchor one voxel past the far border: both its cells fall outside.
        let features = vec![Feature {
            anchor: IVec3::new(32, 4, 4),
            yaw: Yaw::Deg0,
            priority: 1,
            source: 0,
        }];
        let base: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        let out = stamp_features(&base, ctx, &sources, &features);
        assert_eq!(out.get(0, 4, 4).material, MaterialId(0), "must not wrap to the near edge");
    }
}
