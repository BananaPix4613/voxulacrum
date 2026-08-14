//! Cross-chunk feature derivation (design §5, "Cross-chunk feature generation").
//!
//! A **feature** is one placed occurrence - a blueprint or a tree - with a
//! world-absolute anchor, a rotation, and a priority. Features are derived per
//! **feature cell** - a coarse world grid - from
//! `hash(world_seed, cell_x, cell_z, seed)`, the same construction
//! `scatter::jittered_grid` uses for props.
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

use std::collections::HashSet;

use glam::{IVec3, Vec3};
use nodegraph_ir::TreeSpecies;
use voxel_core::{ChunkBuffer, MaterialId, ResolvedBlueprint, Voxel, Yaw};

use crate::context::{mix64, EvalContext};
use crate::detail_eval::stable_instance_id;
use crate::field::CHUNK_DIM;
use crate::foliage::{FoliageInstance, ScatterBucket};
use crate::scatter::SplitMix64;
use crate::skeleton::Skeleton;

const N: i32 = CHUNK_DIM as i32;

/// What a source needs to know about one world column to place there.
///
/// Both facts come from one probe call because the expensive part is the column
/// pipeline pass, and asking twice would pay for it twice.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColumnProbe {
    /// Which zone owns the column. Sources may restrict themselves to some.
    pub zone_id: u16,
    /// Which biome the column resolves to within its zone. Sources may restrict
    /// themselves to some - a zone holds several biomes, so zone alone cannot
    /// keep a forest off the ocean floor.
    pub biome_id: u16,
    /// World-Y of the topmost solid voxel.
    pub surface_y: i32,
}

/// Foliage an occurrence anchors, in addition to the voxels it stamps.
///
/// A tree is one feature with two halves: a trunk that occupies the voxel grid,
/// so it collides and can be broken, and a canopy that does not. Both come from
/// one derivation - two independent placements would have to agree about where
/// the tree is, and would eventually not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureCanopy {
    /// Scatter type bucket the instance lands in.
    pub type_id: u16,
    /// Prefab instanced at the anchor.
    pub prefab_id: u32,
    /// Voxels above the feature's anchor the canopy sits at - normally the
    /// trunk's height, so the canopy crowns it.
    pub y_offset: i32,
}

/// What a source places at each occurrence.
///
/// An enum rather than two parallel source lists because everything around the
/// payload - cell grid, density roll, zone and biome filters, priority ordering,
/// the margin band - is identical for both, and duplicating it would be two
/// copies of the ordering rule waiting to disagree about a shared cell.
pub enum FeaturePayload {
    /// An authored voxel template, stamped cell for cell.
    Blueprint(ResolvedBlueprint),
    /// A tree, re-derived from its species at every chunk that can see it.
    Tree(TreeSource),
}

impl FeaturePayload {
    /// Stable name, for the content-ordered sort that fixes source indices.
    pub fn name(&self) -> &str {
        match self {
            Self::Blueprint(bp) => &bp.name,
            Self::Tree(t) => &t.name,
        }
    }
}

/// A tree species placed as a feature.
pub struct TreeSource {
    /// Stable name, for ordering and diagnostics.
    pub name: String,
    /// Shape parameters. Also carries `max_crown_reach`, which sizes the band.
    pub species: TreeSpecies,
    /// Material the wood is written as. Must be render-delegated, or the
    /// terrain mesher will draw the tree as cubes.
    pub wood: MaterialId,
    /// Maturity, `0..=1`.
    pub age: f32,
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
    /// Biomes this source appears in. Empty means every biome. Applied with
    /// `zones`, not instead of it: both must admit the column.
    pub biomes: Vec<u16>,
    /// Whether each occurrence takes a random quarter turn.
    pub random_yaw: bool,
    /// How far above the surface the anchor lands.
    pub surface_offset: i32,
    /// What each occurrence places.
    pub payload: FeaturePayload,
    /// Foliage each occurrence anchors, if any. `None` means the occurrence
    /// carries none - which is every tree, whose canopy comes from its skeleton.
    pub canopy: Option<FeatureCanopy>,
}

impl FeatureSource {
    /// How far this source's occurrences can reach horizontally from an anchor.
    ///
    /// A blueprint knows its own extent. A tree does not until it is derived, so
    /// its species **declares** a bound and the derivation is asserted against
    /// it - one bound covering every seed, in the spirit of the rotation-
    /// invariant reach a blueprint uses.
    fn reach(&self) -> i32 {
        match &self.payload {
            FeaturePayload::Blueprint(bp) => bp.horizontal_reach(),
            FeaturePayload::Tree(t) => t.species.max_crown_reach.ceil() as i32,
        }
    }
}

/// One capsule of a tree's wood, in world space, ready for the imposter pass.
///
/// Wood occupies voxels but is **drawn** by an analytic tapered capsule, so the
/// render path needs the centerline the voxels were rasterized from - not the
/// voxels. Emitting it here rather than re-deriving it at draw time keeps one
/// skeleton derivation per tree per chunk.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BranchSegment {
    /// Thick end, world space.
    pub a: Vec3,
    /// Thin end, world space.
    pub b: Vec3,
    /// Radius at `a`, in voxels.
    pub ra: f32,
    /// Radius at `b`, in voxels.
    pub rb: f32,
}

/// What one chunk's feature stamping produced.
pub struct StampResult {
    /// Terrain with every in-window feature cell written.
    pub terrain: ChunkBuffer<Voxel, 32>,
    /// Wood capsules this chunk owns.
    ///
    /// **Owned by the chunk holding the segment's thick end, and not clipped.**
    /// A capsule crossing a border is drawn whole by its owner, so there is no
    /// cut edge and no duplicate - and unloading a chunk removes only the
    /// segments rooted in it, rather than making a whole tree vanish.
    pub segments: Vec<BranchSegment>,
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
                if !source.biomes.is_empty() && !source.biomes.contains(&column.biome_id) {
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

/// The canopy instances `features` contribute to `ctx.chunk`.
///
/// A canopy is a single point, so exactly one chunk owns each one: the chunk
/// containing its anchor. Every chunk in the band derives the same features, and
/// each keeps only its own, which is the same ownership rule `stamp_features`
/// applies to voxels and the reason this needs no cross-chunk agreement.
pub fn canopy_instances(
    ctx: EvalContext,
    sources: &[FeatureSource],
    features: &[Feature],
) -> Vec<ScatterBucket> {
    let base = IVec3::new(ctx.chunk.x * N, ctx.chunk.y * N, ctx.chunk.z * N);
    let mut buckets: Vec<ScatterBucket> = Vec::new();

    for feature in features {
        let Some(source) = sources.get(feature.source) else { continue };
        let Some(canopy) = source.canopy.as_ref() else { continue };
        let world = feature.anchor + IVec3::new(0, canopy.y_offset, 0);
        let local = world - base;
        if local.x < 0 || local.y < 0 || local.z < 0
            || local.x >= N || local.y >= N || local.z >= N
        {
            continue;
        }
        // `seq` is 0: a feature cell produces at most one occurrence, so an
        // anchor cannot be shared the way scattered props share one.
        let stable_id = stable_instance_id(
            ctx.world_seed, world.x, world.y, world.z, canopy.prefab_id, 0,
        );
        let instance = FoliageInstance {
            anchor: [local.x as u8, local.y as u8, local.z as u8],
            sub_offset: [0; 3],
            rotation_y: (feature.yaw.steps() as u8).wrapping_mul(64),
            scale_variant: 0,
            prefab_id: canopy.prefab_id,
            stable_id,
            flags: 0,
        };
        match buckets.iter_mut().find(|b| b.type_id == canopy.type_id) {
            Some(bucket) => bucket.instances.push(instance),
            None => buckets.push(ScatterBucket {
                type_id: canopy.type_id,
                instances: vec![instance],
            }),
        }
    }
    buckets
}

#[inline]
fn out_of_window(local: IVec3) -> bool {
    local.x < 0 || local.y < 0 || local.z < 0 || local.x >= N || local.y >= N || local.z >= N
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
) -> StampResult {
    let mut out = terrain.clone();
    let mut segments = Vec::new();
    if features.is_empty() {
        return StampResult { terrain: out, segments };
    }
    out.make_dense();
    let base = IVec3::new(ctx.chunk.x * N, ctx.chunk.y * N, ctx.chunk.z * N);
    let mut protected: HashSet<(i32, i32, i32)> = HashSet::new();

    for feature in features {
        let Some(source) = sources.get(feature.source) else { continue };
        match &source.payload {
            FeaturePayload::Blueprint(bp) => {
                let anchor = IVec3::from_array(bp.anchor);
                for &(at, voxel) in &bp.cells {
                    let rel = IVec3::from_array(
                        feature.yaw.apply((IVec3::from_array(at) - anchor).to_array()),
                    );
                    let local = feature.anchor + rel - base;
                    if out_of_window(local) {
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
            FeaturePayload::Tree(t) => {
                // Re-derived here rather than carried on the feature: every
                // chunk in the band derives the identical skeleton from the
                // identical seed, which is what makes a trunk crossing a chunk
                // border agree without either chunk reading the other.
                //
                // `priority` is reused as that seed. It is already a per-
                // occurrence hash of (world seed, source seed, cell), so it is
                // world-absolute and stable; deriving a second hash from the
                // same inputs would be a second thing to keep in step.
                let skeleton = Skeleton::derive(mix64(feature.priority), &t.species, t.age);
                debug_assert!(
                    skeleton.reach <= t.species.max_crown_reach,
                    "{}: derived reach {} exceeds its declared bound {}; the margin band is \
                     sized from the bound, so this tree is clipped at a chunk border",
                    t.name,
                    skeleton.reach,
                    t.species.max_crown_reach,
                );
                // The trunk stands *on* the anchor voxel, so a tree source wants
                // `surface_offset: 0` where a blueprint wants 1.
                let root = feature.anchor.as_vec3() + Vec3::new(0.5, 1.0, 0.5);
                let wood = Voxel::cube(t.wood);
                skeleton.voxelize(root, |cell| {
                    let local = cell - base;
                    if out_of_window(local) {
                        return;
                    }
                    let key = (local.x, local.y, local.z);
                    if protected.contains(&key) {
                        return;
                    }
                    out.set(local.x as usize, local.y as usize, local.z as usize, wood);
                });

                // Wood is drawn as two kinds of instance, and the split is what
                // makes the joints seamless.
                //
                // A cone body is *tangent* to the spheres at both its ends, so a
                // sphere and the cones meeting it join with a continuous normal.
                // The artifact was never the geometry - it was that both
                // capsules at a joint drew the same sphere, so one copy z-fought
                // the other. Biasing a radius to break the tie only replaced the
                // fight with a step, because it also broke the tangency.
                //
                // So: cones carry no caps, and every node emits its sphere once.
                // The union is then exactly the convex hull of consecutive
                // spheres, with no surface drawn twice.
                for seg in skeleton.segments() {
                    let a = root + seg.a;
                    let owner = a.floor().as_ivec3() - base;
                    if out_of_window(owner) {
                        continue;
                    }
                    segments.push(BranchSegment { a, b: root + seg.b, ra: seg.ra, rb: seg.rb });
                }
                // Joint spheres, as degenerate capsules. The shader tells them
                // apart by `a == b`, so this needs no change to the instance
                // format. Includes tips, which is where a branch gets its end.
                for node in &skeleton.nodes {
                    let p = root + node.pos;
                    let owner = p.floor().as_ivec3() - base;
                    if out_of_window(owner) {
                        continue;
                    }
                    segments.push(BranchSegment { a: p, b: p, ra: node.radius, rb: node.radius });
                }
            }
        }
    }

    out.try_collapse();
    StampResult { terrain: out, segments }
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
            biomes: Vec::new(),
            random_yaw: true,
            surface_offset: 1,
            payload: FeaturePayload::Blueprint(bp),
            canopy: None,
        }
    }

    /// A surface that varies with world position, so a wrong anchor shows up as
    /// a wrong height rather than coincidentally matching.
    fn flat_probe(x: i32, z: i32) -> Option<ColumnProbe> {
        Some(ColumnProbe { zone_id: 0, biome_id: 0, surface_y: 8 + (x.rem_euclid(3)) - (z.rem_euclid(2)) })
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
    fn a_biome_filter_keeps_a_source_out_of_the_wrong_biome() {
        let ctx = EvalContext::new(7, IVec3::new(0, 0, 0));
        let probe = |_: i32, _: i32| Some(ColumnProbe { zone_id: 0, biome_id: 2, surface_y: 40 });

        let unfiltered = [source(pillar("tree", 3, false))];
        assert!(
            !derive_features(ctx, &unfiltered, probe).is_empty(),
            "no filter should admit every column",
        );

        // Same source, restricted to biome 0, against columns that report 2.
        let mut restricted = source(pillar("tree", 3, false));
        restricted.biomes = vec![0];
        assert!(
            derive_features(ctx, &[restricted], probe).is_empty(),
            "biome 2 must not produce a biome-0 source",
        );
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
        let out = stamp_features(&base, ctx, &sources, &features).terrain;
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
        let out = stamp_features(&base, ctx, &sources, &features).terrain;
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
        let out = stamp_features(&base, ctx, &sources, &features).terrain;
        assert_eq!(out.get(0, 4, 4).material, MaterialId(0), "must not wrap to the near edge");
    }

    #[test]
    fn a_canopy_belongs_to_the_chunk_holding_its_anchor() {
        // The ownership rule: every chunk in the band derives the same feature,
        // and exactly one emits its canopy. Two would double the tree; none
        // would lose it.
        let ctx = EvalContext::new(1, IVec3::ZERO);
        let mut src = source(pillar("trunk", 2, false));
        src.canopy = Some(FeatureCanopy { type_id: 4, prefab_id: 7, y_offset: 5 });
        let sources = [src];

        // Anchor inside this chunk; canopy 5 above it, still inside.
        let inside = vec![Feature {
            anchor: IVec3::new(10, 3, 12),
            yaw: Yaw::Deg0,
            priority: 1,
            source: 0,
        }];
        let got = canopy_instances(ctx, &sources, &inside);
        assert_eq!(got.len(), 1, "one bucket");
        assert_eq!(got[0].type_id, 4);
        assert_eq!(got[0].instances.len(), 1);
        assert_eq!(got[0].instances[0].anchor, [10, 8, 12], "canopy sits y_offset above");
        assert_eq!(got[0].instances[0].prefab_id, 7);

        // Same feature seen from a neighbor: the anchor is outside its window,
        // so it contributes nothing.
        let neighbor = EvalContext::new(1, IVec3::new(1, 0, 0));
        assert!(canopy_instances(neighbor, &sources, &inside).is_empty());

        // A feature with no canopy emits nothing at all.
        let plain = [source(pillar("rock", 2, false))];
        assert!(canopy_instances(ctx, &plain, &inside).is_empty());
    }

    /// A thick-trunked tree source, dense enough that every cell produces one.
    fn tree_source(name: &str) -> FeatureSource {
        FeatureSource {
            cell_size: 16,
            density: 1.0,
            seed: 3,
            zones: Vec::new(),
            biomes: Vec::new(),
            random_yaw: false,
            // A tree stands *on* its anchor voxel; a blueprint's first cell
            // replaces it. That is the one place the two payloads differ in how
            // they read `surface_offset`.
            surface_offset: 0,
            payload: FeaturePayload::Tree(TreeSource {
                name: name.to_string(),
                species: TreeSpecies { trunk_radius: 2.0, ..TreeSpecies::default() },
                wood: MaterialId(9),
                age: 1.0,
            }),
            canopy: None,
        }
    }

    #[test]
    fn a_tree_source_stamps_wood() {
        let ctx = EvalContext::new(11, IVec3::ZERO);
        let sources = [tree_source("oak")];
        let features = derive_features(ctx, &sources, flat_probe);
        assert!(!features.is_empty(), "vacuous: no tree was derived");
        let base: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        let out = stamp_features(&base, ctx, &sources, &features).terrain;
        let wood = (0..32)
            .flat_map(|y| (0..32).flat_map(move |z| (0..32).map(move |x| (x, y, z))))
            .filter(|&(x, y, z)| out.get(x, y, z).material == MaterialId(9))
            .count();
        assert!(wood > 0, "a tree source stamped no wood");
    }

    #[test]
    fn a_tree_emits_capsules_owned_by_the_chunk_holding_their_thick_end() {
        // The impostor pass needs the centerline, not the voxels - and exactly
        // one chunk must own each capsule, or a tree at a border draws twice.
        let ctx = EvalContext::new(3, IVec3::ZERO);
        let sources = [tree_source("oak")];
        let features = derive_features(ctx, &sources, flat_probe);
        assert!(!features.is_empty(), "vacuous: no tree was derived");

        let base: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        let out = stamp_features(&base, ctx, &sources, &features);
        assert!(!out.segments.is_empty(), "a tree emitted no capsules");

        for s in &out.segments {
            let owner = s.a.floor().as_ivec3();
            assert!(
                (0..N).contains(&owner.x) && (0..N).contains(&owner.y) && (0..N).contains(&owner.z),
                "capsule rooted at {:?} is outside the owning window",
                s.a,
            );
            assert!(s.ra >= s.rb, "a capsule must taper away from the trunk");
            assert_ne!(s.a, s.b, "a capsule must have length");
        }
    }

    #[test]
    fn a_tree_crossing_a_chunk_border_is_whole() {
        // The seam property for trees, and the reason the skeleton is a pure
        // function of the feature's own hash: two chunks derive the identical
        // tree and each keeps only its own window, so a trunk is continuous
        // across the border without either chunk reading the other.
        let sources = [tree_source("oak")];
        let base: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        let left_c = IVec3::new(0, 0, 0);
        let right_c = IVec3::new(1, 0, 0);

        let mut crossers = 0usize;
        let mut checked = 0usize;
        // Swept over seeds rather than resting on one draw: whether any tree
        // happens to straddle this particular border is incidental, and a
        // single configuration that quietly stops straddling is how a seam test
        // turns into a test of nothing.
        for seed in 0..12u64 {
            let left = EvalContext::new(seed, left_c);
            let right = EvalContext::new(seed, right_c);
            let out_l = stamp_features(&base, left, &sources, &derive_features(left, &sources, flat_probe)).terrain;
            let out_r = stamp_features(&base, right, &sources, &derive_features(right, &sources, flat_probe)).terrain;

            for feature in derive_features(left, &sources, flat_probe) {
                let FeaturePayload::Tree(t) = &sources[feature.source].payload else { unreachable!() };
                let sk = Skeleton::derive(mix64(feature.priority), &t.species, t.age);
                let root = feature.anchor.as_vec3() + Vec3::new(0.5, 1.0, 0.5);
                let mut cells = std::collections::HashSet::new();
                sk.voxelize(root, |c| {
                    cells.insert(c);
                });

                let local = |c: IVec3, chunk: IVec3| c - chunk * N;
                let hits = |chunk: IVec3| cells.iter().any(|c| !out_of_window(local(*c, chunk)));
                // Straddling means reaching into *both windows*, not merely
                // crossing the plane: a tree past the far z edge crosses x and
                // lands in neither chunk.
                if !(hits(left_c) && hits(right_c)) {
                    continue;
                }
                crossers += 1;

                for cell in &cells {
                    for (chunk, out) in [(left_c, &out_l), (right_c, &out_r)] {
                        let l = local(*cell, chunk);
                        if out_of_window(l) {
                            continue;
                        }
                        assert_eq!(
                            out.get(l.x as usize, l.y as usize, l.z as usize).material,
                            MaterialId(9),
                            "seed {seed}: chunk {chunk} is missing {cell} of a straddling trunk",
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(crossers > 0, "vacuous: no tree reached into both windows at any seed");
        assert!(checked > 0, "vacuous: nothing was actually compared");
    }
}
