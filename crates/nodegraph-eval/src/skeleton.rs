//! Tree skeletons: a branching path derived from an anchor.
//!
//! A skeleton is a set of connected segments rooted at an anchor, derived as a
//! pure function of `(seed, species, age)`. It is consumed twice and then
//! dropped: once to rasterize wood into the voxel grid, once to distribute
//! foliage over it. The skeleton knows about neither consumer, which is what
//! makes the mechanism reusable for anything that splits from a point and
//! distributes something along a path.
//!
//! **Nothing here is stored.** Every chunk in a tree's margin band re-derives
//! the identical skeleton and keeps only what falls in its own window, the way
//! `feature.rs` already treats structures. That is why there is no per-tree
//! registry and no baked per-block record: continuity across a chunk border is
//! not reconciled, it is recomputed.
//!
//! **A node's parent always has a lower index**, because growth appends from a
//! queue whose buds reference nodes that already exist. Both passes in [`solve`]
//! depend on it and neither needs a children list: the reverse pass pushes each
//! node's contribution into its parent rather than pulling from its children,
//! and the forward pass reads a parent that is already assigned. Substep 20c
//! measured a per-node `Vec` losing to a linear scan at every size in this
//! codebase, so avoiding one is the local idiom rather than a micro-optimization.
//!
//! **Age is a node budget, not a scale.** Growth extends the frontier and
//! leaves every existing node's index, parent and position untouched, so a
//! growth tick adds to a tree rather than replacing it. Radii legitimately move,
//! because the pipe model thickens a node once it has children. That property
//! rests on each node drawing its randomness from its own index rather than
//! from a shared sequential stream - a shared stream would reshuffle the whole
//! tree the moment the node count changed.
//!
//! Positions are **continuous**, relative to the anchor, in voxels. The
//! centerline is not grid-aligned: the renderer solves tapered capsules
//! analytically, and quantizing to voxels would discard the sub-voxel precision
//! that decision exists for.
//!
//! **Two generators, one budget.** Parametric recursion produces the trunk and
//! primary limbs; space colonization then fills a crown volume with fine
//! branching, which is what stops a tree reading as self-similar. The turtle
//! either completes or spends the whole budget - it cannot do neither - so the
//! crown is positioned against a height that is final whenever colonization
//! runs at all.
//!
//! [`TreeSpecies`] lives in this crate because nothing authors a species yet.
//! It moves when one does - to `nodegraph-ir` if a node kind carries it, or to
//! a registry beside materials and props if it is authored as content. That
//! fork is open; see `voxel-tree-rendering-spec.md` §12.

use std::collections::VecDeque;

use glam::{IVec3, Quat, Vec3};
use nodegraph_ir::TreeSpecies;

use crate::context::mix64;
use crate::scatter::SplitMix64;

/// Pipe-model exponent. 2.0 is strict da Vinci; real trees measure 2.0 to 2.5.
pub const PIPE_EXP: f32 = 2.3;

/// Hard ceiling on nodes in one skeleton, reached at age 1.0. Derivation cost
/// is linear in node count, so this is the constant that makes it bounded
/// rather than parameter-dependent - and it is what the derivation measurement
/// is taken against.
pub const MAX_NODES: usize = 512;

/// [`Node::parent`] value marking the root.
pub const NO_PARENT: u32 = u32::MAX;

/// Hard ceiling on space-colonization rounds. The budget normally stops growth
/// first; this bounds the case where attractors are placed such that nothing
/// ever reaches them, which would otherwise spin until the budget filled one
/// node at a time.
const MAX_COLONIZE_ITERS: usize = 64;

/// Phyllotactic increment between successive laterals on one axis, in radians
/// (the golden angle, ~137.5 degrees). Uniform random azimuth reads as noise;
/// this reads as a plant, and costs the same.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// One node: a point on a branch centerline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    /// Position relative to the anchor, in voxels. Continuous, not grid-aligned.
    pub pos: Vec3,
    /// Parent node index, or [`NO_PARENT`] for the root. Always lower than this
    /// node's own index.
    pub parent: u32,
    /// Centerline radius here, in voxels. Filled by [`solve`].
    pub radius: f32,
    /// Node count in this node's subtree, including itself. Filled by [`solve`].
    pub weight: f32,
    /// Heavy-path chain this node belongs to.
    pub chain: u32,
    /// Position along that chain, counting from its first node.
    pub chain_idx: u32,
}

/// One capsule of the centerline: a node paired with its parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    /// Parent end.
    pub a: Vec3,
    /// Child end.
    pub b: Vec3,
    /// Radius at `a`.
    pub ra: f32,
    /// Radius at `b`.
    pub rb: f32,
    /// Heavy-path chain the child belongs to.
    pub chain: u32,
}

/// One canopy lobe: an ellipsoid of foliage carried by a limb.
///
/// This is the design doc's canopy "clump", and it is *derived* rather than
/// authored. A limb terminal is where it hangs, and the pipe model sizes it:
/// cross-sectional area is biologically a statement about the foliage mass a
/// limb supplies, so lobe volume tracks that area and the dominant limb gets
/// the dominant lobe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lobe {
    /// Center, relative to the skeleton's anchor, in voxels.
    pub center: Vec3,
    /// Mean radius, in voxels.
    pub radius: f32,
    /// Per-axis multipliers about `radius`.
    pub aniso: Vec3,
    /// The limb terminal this lobe hangs from. Its voxel is the anchor every
    /// leaf instance in this lobe reports to, so destroying the limb destroys
    /// the lobe's foliage.
    pub anchor_node: u32,
}

impl Lobe {
    /// Ellipsoid radii, for the depth core and the leaf scatter.
    pub fn radii(&self) -> Vec3 {
        self.aniso * self.radius
    }
}

/// A derived skeleton, anchor-relative.
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    /// Nodes in creation order; index 0 is the root at the anchor.
    pub nodes: Vec<Node>,
    /// Number of heavy-path chains.
    pub chains: u32,
    /// Radius every leaf node was given - the species' authored value, except on
    /// an unbranched skeleton where the trunk wins.
    pub tip_radius: f32,
    /// The profile exponent solved for this tree, so both authored radii land.
    /// `PIPE_EXP` only when the two are too close together to solve for.
    pub pipe_exp: f32,
    /// The canopy, derived from the limb structure.
    pub lobes: Vec<Lobe>,
    /// Greatest horizontal distance from the anchor to any node, plus that
    /// node's radius. What a feature source's reach bound is checked against.
    /// Rotation-invariant by construction: a quarter turn about the anchor
    /// leaves a horizontal distance from it unchanged.
    pub reach: f32,
    /// Greatest height above the anchor, plus radius.
    pub height: f32,
}

impl Skeleton {
    /// Derive a skeleton. Pure in `(seed, species, age)`; `age` clamps to
    /// `[0, 1]` and selects a node budget.
    pub fn derive(seed: u64, species: &TreeSpecies, age: f32) -> Self {
        let budget = node_budget(age);
        let mut nodes = grow(seed, species, budget);
        colonize(&mut nodes, seed, species, budget);
        let (chains, tip_radius, pipe_exp) =
            solve(&mut nodes, species.trunk_radius, species.tip_radius);
        let lobes = derive_lobes(&nodes, chains, seed, species, pipe_exp);
        let (reach, height) = extent(&nodes, &lobes);
        Self { nodes, chains, tip_radius, pipe_exp, lobes, reach, height }
    }

    /// Visit every voxel cell this skeleton's wood occupies, in world cells.
    ///
    /// `root` is the world-continuous position of node 0 - normally the top
    /// center of the voxel the tree stands on. Working in world-continuous
    /// space rather than anchor-relative integers keeps the half-voxel offsets
    /// in one place instead of at every call site.
    ///
    /// A cell is wood when its center lies inside a segment's capsule. That is
    /// the rule with no magic number in it: a thick trunk fills a solid
    /// cylinder, and a branch thinner than half a voxel fills nothing, which is
    /// correct rather than a limitation - it has no room to be solid in. Those
    /// branches still exist in the skeleton and still render; they are simply
    /// not things you can stand on.
    ///
    /// **`f` may be called more than once for the same cell.** Adjacent
    /// segments overlap by construction, and de-duplicating would mean an
    /// allocation on a path whose only caller writes idempotently.
    pub fn voxelize(&self, root: Vec3, mut f: impl FnMut(IVec3)) {
        for seg in self.segments() {
            let (a, b) = (root + seg.a, root + seg.b);
            let r = seg.ra.max(seg.rb);
            let lo = (a.min(b) - Vec3::splat(r)).floor().as_ivec3();
            let hi = (a.max(b) + Vec3::splat(r)).ceil().as_ivec3();
            for z in lo.z..=hi.z {
                for y in lo.y..=hi.y {
                    for x in lo.x..=hi.x {
                        let cell = IVec3::new(x, y, z);
                        let center = cell.as_vec3() + Vec3::splat(0.5);
                        if in_capsule(center, a, b, seg.ra, seg.rb) {
                            f(cell);
                        }
                    }
                }
            }
        }
    }

    /// One segment per non-root node. This is what the renderer instances and
    /// what a voxelizer rasterizes.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        let nodes = &self.nodes;
        nodes.iter().filter(|n| n.parent != NO_PARENT).map(move |n| {
            let p = &nodes[n.parent as usize];
            Segment { a: p.pos, b: n.pos, ra: p.radius, rb: n.radius, chain: n.chain }
        })
    }
}

/// Is `p` inside the tapered capsule from `a` (radius `ra`) to `b` (`rb`)?
///
/// The union of the two end spheres with a linearly-tapering cylinder. That is
/// marginally larger than the true round cone, which is tangent to both spheres
/// rather than joining their centers - deliberately so, because for
/// voxelization erring outward costs a cell and erring inward costs a hole.
fn in_capsule(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32) -> bool {
    if p.distance_squared(a) <= ra * ra || p.distance_squared(b) <= rb * rb {
        return true;
    }
    let ba = b - a;
    let len2 = ba.length_squared();
    if len2 <= f32::EPSILON {
        return false;
    }
    let t = ((p - a).dot(ba) / len2).clamp(0.0, 1.0);
    let r = ra + (rb - ra) * t;
    (p - (a + ba * t)).length_squared() <= r * r
}

/// Nodes permitted at an age. Always at least the root.
fn node_budget(age: f32) -> usize {
    ((MAX_NODES as f32 * age.clamp(0.0, 1.0)).ceil() as usize).max(1)
}

/// An axis waiting to extend by one internode.
struct Bud {
    parent: u32,
    frame: Quat,
    length: f32,
    order: u8,
    step: u8,
}

/// Grow nodes breadth-first until the budget is spent or the queue empties.
///
/// Breadth-first rather than depth-first because the budget truncates: FIFO
/// removes the finest tips evenly across the tree, LIFO would remove one whole
/// limb. It is also what makes age a prefix relation rather than a reshuffle.
fn grow(seed: u64, sp: &TreeSpecies, budget: usize) -> Vec<Node> {
    let mut nodes = Vec::with_capacity(budget.min(MAX_NODES));
    nodes.push(Node {
        pos: Vec3::ZERO,
        parent: NO_PARENT,
        radius: 0.0,
        weight: 1.0,
        chain: 0,
        chain_idx: 0,
    });

    // Drawn once per tree, before any node exists, so two trees of one species
    // differ in more than jitter. Without these, every oak grows laterals at the
    // same azimuths from the same upright trunk and a stand reads as one tree
    // copied with noise on top - the per-node wobble varies detail, never shape.
    let mut tree_rng = SplitMix64::new(mix64(seed ^ 0x5EED_0F17_A11E_5EED));
    let root_yaw = tree_rng.next_f32() * std::f32::consts::TAU;
    let phyllotaxis_phase = tree_rng.next_f32() * std::f32::consts::TAU;
    let v = sp.variance.max(0.0);
    let length_scale = 1.0 + (tree_rng.next_f32() - 0.5) * 2.0 * v;
    let angle_scale = 1.0 + (tree_rng.next_f32() - 0.5) * 2.0 * v;

    let mut queue: VecDeque<Bud> = VecDeque::new();
    queue.push_back(Bud {
        parent: 0,
        frame: Quat::from_rotation_y(root_yaw),
        length: sp.internode * length_scale,
        order: 0,
        step: 0,
    });

    while nodes.len() < budget {
        let Some(bud) = queue.pop_front() else { break };

        // Seeded from the node's own index, never from a shared sequential
        // stream: growing must add nodes without perturbing the ones already
        // placed, and a shared stream shifts under any change in node count.
        let index = nodes.len() as u32;
        let mut rng =
            SplitMix64::new(mix64(seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)));

        let frame = bend(bud.frame, sp.gravitropism, sp.wobble, &mut rng);
        let pos = nodes[bud.parent as usize].pos + frame * (Vec3::Y * bud.length);
        nodes.push(Node {
            pos,
            parent: bud.parent,
            radius: 0.0,
            weight: 1.0,
            chain: 0,
            chain_idx: 0,
        });

        // Continue this axis.
        if bud.step.saturating_add(1) < sp.axis_steps {
            queue.push_back(Bud {
                parent: index,
                frame,
                length: bud.length * sp.taper,
                order: bud.order,
                step: bud.step + 1,
            });
        }

        // Laterals, phyllotactic about the parent axis.
        if bud.order < sp.max_order {
            for k in 0..sp.laterals {
                let azimuth = phyllotaxis_phase
                    + GOLDEN_ANGLE * (bud.step as f32 * sp.laterals as f32 + k as f32);
                queue.push_back(Bud {
                    parent: index,
                    frame: frame
                        * Quat::from_rotation_y(azimuth)
                        * Quat::from_rotation_x(sp.branch_angle * angle_scale),
                    length: bud.length * sp.child_scale,
                    order: bud.order + 1,
                    step: 0,
                });
            }
        }
    }
    nodes
}

/// Bend a frame toward world up (or away), then add a random wobble. The
/// frame's local `+Y` is the growth direction, so an identity frame grows
/// straight up.
fn bend(frame: Quat, gravitropism: f32, wobble: f32, rng: &mut SplitMix64) -> Quat {
    let mut out = frame;
    let g = gravitropism.clamp(-1.0, 1.0);
    if g != 0.0 {
        let fwd = (out * Vec3::Y).normalize_or_zero();
        let target = if g >= 0.0 { Vec3::Y } else { Vec3::NEG_Y };
        // `from_rotation_arc` has no defined plane for antiparallel inputs, and
        // a branch pointing exactly away from the target has no preferred bend.
        // Leave it and let the wobble break the tie on a later step.
        if fwd != Vec3::ZERO && fwd.dot(target) > -0.999 {
            out = Quat::IDENTITY.slerp(Quat::from_rotation_arc(fwd, target), g.abs()) * out;
        }
    }
    if wobble > 0.0 {
        let a = (rng.next_f32() - 0.5) * 2.0 * wobble;
        let b = (rng.next_f32() - 0.5) * 2.0 * wobble;
        out *= Quat::from_rotation_x(a) * Quat::from_rotation_z(b);
    }
    out.normalize()
}

/// Attraction points filling the crown ellipsoid, rejection-sampled so the
/// distribution is uniform in the volume rather than bunched toward the poles.
///
/// The try count is bounded: a degenerate radius would otherwise reject
/// forever, and a crown that comes out short is a visible tuning problem
/// rather than a hang.
fn crown_attractors(seed: u64, sp: &TreeSpecies, turtle_height: f32) -> Vec<Vec3> {
    let want = sp.crown_attractors as usize;
    let mut rng = SplitMix64::new(mix64(seed ^ 0xA771_AC70_1205_EED5));
    let center = Vec3::new(0.0, turtle_height * sp.crown_center, 0.0);
    let radii = Vec3::new(sp.crown_radius, sp.crown_height, sp.crown_radius);
    let mut out = Vec::with_capacity(want);
    let mut tries = 0usize;
    while out.len() < want && tries < want * 8 + 64 {
        tries += 1;
        let p = Vec3::new(
            rng.next_f32() * 2.0 - 1.0,
            rng.next_f32() * 2.0 - 1.0,
            rng.next_f32() * 2.0 - 1.0,
        );
        if p.length_squared() > 1.0 {
            continue;
        }
        out.push(center + p * radii);
    }
    out
}

/// Space colonization (Runions et al.), appending fine branching to whatever
/// the turtle produced.
///
/// **Two-phase per round.** Every node that attracted something grows, and only
/// then are attractors consumed. That makes a round independent of the order
/// nodes are visited within it, and combined with appending in index order it
/// is what keeps a lower budget a strict prefix of a higher one - the property
/// the age model rests on.
///
/// The nearest-node query is a linear scan, so a round costs
/// `attractors * nodes`. Isolated here deliberately: if the derivation
/// measurement names this, a uniform grid over the crown replaces one loop.
fn colonize(nodes: &mut Vec<Node>, seed: u64, sp: &TreeSpecies, budget: usize) {
    if sp.crown_attractors == 0 || nodes.len() >= budget {
        return;
    }
    // Safe to read as final: `grow` exits either with the budget spent - in
    // which case the early return above already fired - or with its queue
    // empty and the turtle complete.
    let turtle_height = nodes.iter().fold(0.0f32, |h, n| h.max(n.pos.y));
    let mut attractors = crown_attractors(seed, sp, turtle_height);
    let kill2 = sp.kill_radius * sp.kill_radius;
    let influence2 = sp.influence * sp.influence;

    // Attractors already inside the turtle skeleton pull nothing useful.
    attractors.retain(|a| nodes.iter().all(|n| n.pos.distance_squared(*a) > kill2));

    let mut pull: Vec<Vec3> = Vec::new();
    for _ in 0..MAX_COLONIZE_ITERS {
        if nodes.len() >= budget || attractors.is_empty() {
            break;
        }
        pull.clear();
        pull.resize(nodes.len(), Vec3::ZERO);

        // Phase 1: every attractor pulls on its nearest node within influence.
        for a in &attractors {
            let mut best = usize::MAX;
            let mut best_d2 = influence2;
            for (i, n) in nodes.iter().enumerate() {
                let d2 = n.pos.distance_squared(*a);
                if d2 < best_d2 {
                    best_d2 = d2;
                    best = i;
                }
            }
            if best != usize::MAX {
                pull[best] += (*a - nodes[best].pos).normalize_or_zero();
            }
        }

        // Phase 2: every pulled node grows one child, in index order.
        let before = nodes.len();
        for i in 0..before {
            if nodes.len() >= budget {
                break;
            }
            let dir = pull[i].normalize_or_zero();
            if dir == Vec3::ZERO {
                continue;
            }
            let pos = nodes[i].pos + dir * sp.colonize_step;
            nodes.push(Node {
                pos,
                parent: i as u32,
                radius: 0.0,
                weight: 1.0,
                chain: 0,
                chain_idx: 0,
            });
        }
        if nodes.len() == before {
            break;
        }

        // Phase 3: consume the attractors that new growth reached.
        let new = &nodes[before..];
        attractors.retain(|a| new.iter().all(|n| n.pos.distance_squared(*a) > kill2));
    }
}

/// Per-node scratch for [`solve`]'s reverse pass.
#[derive(Clone, Copy)]
struct Acc {
    /// Summed `radius^exp` over this node's children, at the tree's solved
    /// profile exponent.
    area: f32,
    /// Weight of the heaviest child seen.
    heaviest: f32,
    /// Index of that child.
    heavy: u32,
}

/// Pipe model (Shinozaki 1964) and heavy path decomposition, in two linear
/// passes over the node array.
///
/// Cross-sectional area is conserved across a fork, which is what stops trunks
/// looking conical and saplings looking blocky. The heaviest child then
/// inherits its parent's chain, giving a readable chunk -> main branch -> side
/// branch hierarchy. Returns the chain count.
///
/// Ties go to the highest-index child, because the reverse pass reaches it
/// first. Arbitrary but deterministic, which is all that is required.
fn solve(nodes: &mut [Node], trunk_radius: f32, species_tip: f32) -> (u32, f32, f32) {
    let n = nodes.len();
    if n == 0 {
        return (0, trunk_radius, PIPE_EXP);
    }

    let mut has_child = vec![false; n];
    for node in nodes.iter() {
        if node.parent != NO_PARENT {
            has_child[node.parent as usize] = true;
        }
    }
    let tips = has_child.iter().filter(|c| !**c).count().max(1);

    // A single-tip skeleton is an unbranched chain, and no exponent can make its
    // root differ from its tip - the recursion has nothing to sum. Honor the
    // trunk in that case, since a bare trunk's whole point is its thickness.
    let tip_radius = if tips <= 1 { trunk_radius } else { species_tip };

    // Both radii are authored, so the exponent is what gives: solve
    // `tip * tips^(1/e) = trunk` for `e`. Area conservation (e = PIPE_EXP)
    // cannot honor both ends on a skeleton with tens of terminals rather than
    // the tens of thousands a real tree carries - it makes every tip a club.
    let ratio = trunk_radius / tip_radius.max(1e-4);
    let exp = if tips > 1 && ratio > 1.001 {
        ((tips as f32).ln() / ratio.ln()).clamp(0.5, 6.0)
    } else {
        PIPE_EXP
    };

    let mut acc = vec![Acc { area: 0.0, heaviest: 0.0, heavy: NO_PARENT }; n];

    // Reverse: every child has a higher index, so by the time a node is reached
    // all of them have already pushed into it and its radius is final.
    for i in (0..n).rev() {
        nodes[i].radius = if acc[i].area > 0.0 {
            acc[i].area.powf(1.0 / exp)
        } else {
            tip_radius
        };
        let parent = nodes[i].parent;
        if parent == NO_PARENT {
            continue;
        }
        let p = parent as usize;
        acc[p].area += nodes[i].radius.powf(exp);
        nodes[p].weight += nodes[i].weight;
        if nodes[i].weight > acc[p].heaviest {
            acc[p].heaviest = nodes[i].weight;
            acc[p].heavy = i as u32;
        }
    }

    // Forward: every parent has a lower index, so it is already assigned.
    let mut next_chain = 1u32;
    nodes[0].chain = 0;
    nodes[0].chain_idx = 0;
    for i in 1..n {
        let p = nodes[i].parent as usize;
        if acc[p].heavy == i as u32 {
            nodes[i].chain = nodes[p].chain;
            nodes[i].chain_idx = nodes[p].chain_idx + 1;
        } else {
            nodes[i].chain = next_chain;
            nodes[i].chain_idx = 0;
            next_chain += 1;
        }
    }
    (next_chain, tip_radius, exp)
}

/// Derive the canopy from the limb structure.
///
/// Each heavy-path chain is a limb. Its **base** radius is the cross-section
/// supplying it, and its **terminal** is where that limb's foliage hangs - so a
/// lobe is placed at the terminal and sized from the base. Candidates are taken
/// largest first, and the design doc's arrangement rules are applied as
/// constraints on that set rather than as authored offsets. Varied depth on all
/// three axes comes free: phyllotaxis has already spread the limbs.
///
/// Sizes are jittered because a uniform recursion gives every limb of one order
/// the identical base radius. The pipe model alone would produce tiers of equal
/// lobes, not the uneven set the arrangement rules ask for.
fn derive_lobes(
    nodes: &[Node],
    chains: u32,
    seed: u64,
    sp: &TreeSpecies,
    pipe_exp: f32,
) -> Vec<Lobe> {
    if sp.max_lobes == 0 || nodes.is_empty() {
        return Vec::new();
    }
    let mut base = vec![0.0f32; chains as usize];
    let mut term = vec![(0u32, 0u32); chains as usize];
    for (i, n) in nodes.iter().enumerate() {
        let c = n.chain as usize;
        if n.chain_idx == 0 {
            base[c] = n.radius;
        }
        if n.chain_idx >= term[c].0 {
            term[c] = (n.chain_idx, i as u32);
        }
    }

    let mut cand: Vec<(f32, u32)> =
        (0..chains as usize).map(|c| (base[c], term[c].1)).collect();
    // Largest supply first, node index as the tiebreak. Equal-radius limbs are
    // the common case rather than a rarity, so resting the order on the float
    // comparison alone would make the canopy depend on sort stability.
    cand.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    // Build every candidate first, then reconcile. Merging grows the lobe it
    // merges into, which can push it inside a lobe that was clear a moment ago,
    // so the rule cannot be enforced in one pass.
    let mut out: Vec<Lobe> = Vec::new();
    for (supply, node) in cand {
        let mut rng = SplitMix64::new(mix64(
            seed ^ (node as u64).wrapping_mul(0xD1B5_4A32_D192_ED03),
        ));
        // Volume tracks supplying area, so radius tracks it to the 1/3 power.
        // Written against PIPE_EXT rather than a literal 2/3 so the two cannot
        // drift apart when the exponent is tuned per species.
        let mut radius = sp.lobe_scale * supply.powf(pipe_exp / 3.0);
        radius *= 1.0 + (rng.next_f32() - 0.5) * 2.0 * sp.lobe_size_jitter;
        if radius < sp.lobe_min_radius {
            continue;
        }
        let j = sp.lobe_anisotropy;
        out.push(Lobe {
            center: nodes[node as usize].pos,
            radius,
            aniso: Vec3::new(
                1.0 + (rng.next_f32() - 0.5) * 2.0 * j,
                1.0 + (rng.next_f32() - 0.5) * 2.0 * j,
                1.0 + (rng.next_f32() - 0.5) * 2.0 * j,
            ),
            anchor_node: node,
        });
    }

    // Merge crowded pairs to a fixpoint. Merging rather than rejecting because
    // rejecting silently loses crown mass on exactly the species where
    // gravitropism pulls terminals together - the case least able to spare it.
    // The surviving lobe keeps the larger limb's anchor, so foliage stays
    // attached to the branch most likely to still be standing.
    loop {
        let mut hit = None;
        'scan: for i in 0..out.len() {
            for k in (i + 1)..out.len() {
                let gap = (out[i].center - out[k].center).length();
                if gap < sp.lobe_separation * (out[i].radius + out[k].radius) {
                    hit = Some((i, k));
                    break 'scan;
                }
            }
        }
        let Some((i, k)) = hit else { break };
        let (a3, b3) = (out[i].radius.powi(3), out[k].radius.powi(3));
        let toward = out[k].center;
        out[i].center = out[i].center.lerp(toward, b3 / (a3 + b3));
        out[i].radius = (a3 + b3).cbrt();
        out.remove(k);
    }

    // Over the cap, drop the smallest rather than merging across a gap: a lobe
    // spanning empty space to satisfy a count is worse than one fewer lobe, and
    // `lobe_min_radius` has already removed anything visually load-bearing.
    while out.len() > sp.max_lobes as usize {
        let (worst, _) = out
            .iter()
            .enumerate()
            .map(|(i, l)| (i, (l.radius, std::cmp::Reverse(l.anchor_node))))
            .min_by(|a, b| {
                a.1 .0.total_cmp(&b.1 .0).then(a.1 .1.cmp(&b.1 .1))
            })
            .expect("non-empty above the cap");
        out.remove(worst);
    }
    out
}

/// Horizontal reach and height, over branches **and** lobes.
///
/// Crown extent is emergent - limb reach plus lobe radius - which is why there
/// is no authored crown volume to keep in step with the recursion.
fn extent(nodes: &[Node], lobes: &[Lobe]) -> (f32, f32) {
    let mut reach: f32 = 0.0;
    let mut height: f32 = 0.0;
    for n in nodes {
        reach = reach.max((n.pos.x * n.pos.x + n.pos.z * n.pos.z).sqrt() + n.radius);
        height = height.max(n.pos.y + n.radius);
    }
    for l in lobes {
        let r = l.radii();
        let c = l.center;
        reach = reach.max((c.x * c.x + c.z * c.z).sqrt() + r.x.max(r.z));
        height = height.max(c.y + r.y);
    }
    (reach, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oak() -> TreeSpecies {
        TreeSpecies::default()
    }

    /// An oak with the optional colonization stage switched on.
    ///
    /// Colonization is off by default - lobes anchored to limb terminals
    /// replaced it - so a test about the crown, or about the node budget
    /// binding at all, has to ask for it. A bare turtle terminates at 56 nodes
    /// and never reaches any budget.
    fn crowned() -> TreeSpecies {
        TreeSpecies { crown_attractors: 400, ..TreeSpecies::default() }
    }

    #[test]
    fn a_child_never_precedes_its_parent() {
        // The invariant both solver passes rest on. It holds by construction -
        // a bud always references a node that exists - but a future generator
        // that broke it would make the reverse pass read unfinished children
        // and produce quietly wrong radii rather than failing.
        let s = Skeleton::derive(7, &oak(), 1.0);
        assert!(s.nodes.len() > 1);
        for (i, n) in s.nodes.iter().enumerate() {
            if n.parent != NO_PARENT {
                assert!((n.parent as usize) < i, "node {i} has parent {}", n.parent);
            }
        }
    }

    #[test]
    fn the_pipe_model_conserves_cross_section_at_every_fork() {
        let s = Skeleton::derive(11, &oak(), 1.0);
        let mut area = vec![0.0f32; s.nodes.len()];
        for n in &s.nodes {
            if n.parent != NO_PARENT {
                area[n.parent as usize] += n.radius.powf(s.pipe_exp);
            }
        }
        let mut interior = 0;
        for (i, n) in s.nodes.iter().enumerate() {
            if area[i] == 0.0 {
                assert_eq!(n.radius, s.tip_radius, "tip {i} is not at tip radius");
                continue;
            }
            interior += 1;
            let expected = area[i].powf(1.0 / s.pipe_exp);
            assert!((n.radius - expected).abs() < 1e-4, "node {i}: {} vs {expected}", n.radius);
        }
        assert!(interior > 0, "vacuous: the skeleton has no interior nodes");
    }

    #[test]
    fn the_heavy_chain_follows_the_greatest_subtree_mass() {
        // Walk chain 0 from the root. At each step the node it continued into
        // must be no lighter than any of its siblings.
        let s = Skeleton::derive(3, &oak(), 1.0);
        let mut children = vec![Vec::new(); s.nodes.len()];
        for (i, n) in s.nodes.iter().enumerate() {
            if n.parent != NO_PARENT {
                children[n.parent as usize].push(i);
            }
        }
        let mut cur = 0usize;
        let mut steps = 0;
        while let Some(&next) =
            children[cur].iter().find(|&&c| s.nodes[c].chain == s.nodes[cur].chain)
        {
            for &sib in &children[cur] {
                assert!(
                    s.nodes[sib].weight <= s.nodes[next].weight,
                    "chain left {cur} for {next} while sibling {sib} is heavier",
                );
            }
            cur = next;
            steps += 1;
        }
        assert!(steps > 0, "vacuous: chain 0 has no second node");
    }

    #[test]
    fn derivation_is_a_pure_function_of_its_inputs() {
        let a = Skeleton::derive(42, &oak(), 1.0);
        assert_eq!(a, Skeleton::derive(42, &oak(), 1.0));
        assert_ne!(a, Skeleton::derive(43, &oak(), 1.0), "a new seed must give a new tree");
    }

    #[test]
    fn growing_extends_the_frontier_and_moves_nothing() {
        // Age is a node budget, not a scale: a younger tree's nodes are the
        // older tree's first N, at the same indices, with the same parents and
        // positions. That is what lets a growth tick add to a tree instead of
        // replacing it, and it is why each node seeds from its own index.
        // Radii legitimately differ - a node that was a tip has children now.
        //
        // Needs `crowned()`: the property is about the budget truncating a
        // generator, and a bare turtle terminates long before any budget.
        let young = Skeleton::derive(5, &crowned(), 0.4);
        let old = Skeleton::derive(5, &crowned(), 1.0);
        assert!(young.nodes.len() < old.nodes.len(), "age must change the node count");
        for (i, (y, o)) in young.nodes.iter().zip(old.nodes.iter()).enumerate() {
            assert_eq!(y.parent, o.parent, "node {i} was reparented by growth");
            assert_eq!(y.pos, o.pos, "node {i} moved during growth");
        }
        assert!(
            young.nodes.iter().zip(old.nodes.iter()).any(|(y, o)| y.radius != o.radius),
            "growth must thicken something",
        );
    }

    #[test]
    fn the_node_cap_bounds_every_age() {
        for age in [0.0, 0.25, 0.5, 1.0, 2.0] {
            let s = Skeleton::derive(1, &oak(), age);
            assert!(!s.nodes.is_empty(), "age {age} produced no root");
            assert!(s.nodes.len() <= MAX_NODES, "age {age} exceeded the cap");
        }
    }

    #[test]
    fn reach_and_height_bound_every_node() {
        let s = Skeleton::derive(9, &oak(), 1.0);
        for (i, n) in s.nodes.iter().enumerate() {
            let horiz = (n.pos.x * n.pos.x + n.pos.z * n.pos.z).sqrt();
            assert!(horiz <= s.reach + 1e-4, "node {i} lies outside reach");
            assert!(n.pos.y <= s.height + 1e-4, "node {i} lies above height");
        }
        assert!(s.reach > 0.0 && s.height > 0.0);
    }

    #[test]
    fn colonization_fills_the_crown_past_what_the_turtle_builds() {
        let turtle = Skeleton::derive(13, &oak(), 1.0);
        let full = Skeleton::derive(13, &crowned(), 1.0);

        // The turtle must terminate on its own, or the crown gets no budget and
        // this test would pass by measuring the cap twice.
        assert!(turtle.nodes.len() < MAX_NODES, "the turtle spent the whole budget");
        assert!(full.nodes.len() > turtle.nodes.len(), "colonization added nothing");

        // Colonized nodes append after the turtle's, so the turtle is a prefix.
        for (i, (a, b)) in turtle.nodes.iter().zip(full.nodes.iter()).enumerate() {
            assert_eq!(a.pos, b.pos, "colonization moved turtle node {i}");
            assert_eq!(a.parent, b.parent, "colonization reparented turtle node {i}");
        }
        // And they keep the invariant both solver passes depend on.
        for (i, n) in full.nodes.iter().enumerate().skip(turtle.nodes.len()) {
            assert!((n.parent as usize) < i, "colonized node {i} has parent {}", n.parent);
        }
    }

    #[test]
    fn colonization_reaches_into_the_crown() {
        // Without it every node sits on a turtle axis, so the crown's outer
        // shell is empty. The point of the pass is that it is not.
        let turtle_reach = Skeleton::derive(21, &oak(), 1.0).reach;
        let full_reach = Skeleton::derive(21, &crowned(), 1.0).reach;
        assert!(full_reach > turtle_reach, "{full_reach} vs {turtle_reach}");
    }

    #[test]
    fn the_authored_trunk_radius_is_what_the_root_gets() {
        // The pipe model inverts exactly, so this holds for any branch
        // structure - which is the point. Before this, changing `max_order`
        // silently changed how thick every tree in the world was.
        for (attractors, order) in [(0u16, 1u8), (0, 2), (400, 1)] {
            let mut sp = oak();
            sp.crown_attractors = attractors;
            sp.max_order = order;
            sp.trunk_radius = 1.4;
            let s = Skeleton::derive(3, &sp, 1.0);
            assert!(
                (s.nodes[0].radius - 1.4).abs() < 1e-3,
                "attractors {attractors}, order {order}: root is {} not 1.4",
                s.nodes[0].radius,
            );
            assert!(s.tip_radius < s.nodes[0].radius, "tips must be thinner than the trunk");
        }
    }

    #[test]
    fn every_lobe_hangs_from_a_limb_terminal() {
        let s = Skeleton::derive(1, &oak(), 1.0);
        assert!(!s.lobes.is_empty(), "vacuous: the default species grew no canopy");
        for l in &s.lobes {
            let node = &s.nodes[l.anchor_node as usize];
            let deeper = s
                .nodes
                .iter()
                .any(|n| n.chain == node.chain && n.chain_idx > node.chain_idx);
            assert!(!deeper, "lobe anchored mid-limb rather than at its terminal");
        }
    }

    #[test]
    fn lobes_keep_the_separation_that_merging_enforces() {
        // The merge pass is the only thing standing between the arrangement
        // rules and a canopy of overlapping balls, and a merge that ran on a
        // stale radius would leave violations behind it.
        let sp = oak();
        for seed in 0..8u64 {
            let s = Skeleton::derive(seed, &sp, 1.0);
            for (i, a) in s.lobes.iter().enumerate() {
                for b in s.lobes.iter().skip(i + 1) {
                    let sep = (a.center - b.center).length();
                    assert!(
                        sep >= sp.lobe_separation * (a.radius + b.radius) - 1e-4,
                        "seed {seed}: lobes {sep:.3} apart, closer than the rule allows",
                    );
                }
            }
        }
    }

    #[test]
    fn the_canopy_has_uneven_sizes() {
        // A uniform recursion gives every limb of one order the identical base
        // radius, so without the jitter the pipe model produces tiers of equal
        // lobes - which is the one thing the arrangement rules rule out.
        let s = Skeleton::derive(4, &oak(), 1.0);
        assert!(s.lobes.len() >= 3, "only {} lobes", s.lobes.len());
        let big = s.lobes.iter().map(|l| l.radius).fold(0.0f32, f32::max);
        let small = s.lobes.iter().map(|l| l.radius).fold(f32::MAX, f32::min);
        assert!(big > small * 1.2, "lobe radii {small:.2}..{big:.2} are near-uniform");
    }

    #[test]
    fn crown_extent_covers_the_lobes_not_just_the_branches() {
        let s = Skeleton::derive(6, &oak(), 1.0);
        for l in &s.lobes {
            let r = l.radii();
            let horiz = (l.center.x * l.center.x + l.center.z * l.center.z).sqrt();
            assert!(horiz + r.x.max(r.z) <= s.reach + 1e-4, "a lobe lies outside reach");
            assert!(l.center.y + r.y <= s.height + 1e-4, "a lobe lies above height");
        }
    }

    #[test]
    fn a_species_with_no_lobes_is_a_bare_skeleton() {
        let bare = TreeSpecies { max_lobes: 0, ..oak() };
        let s = Skeleton::derive(1, &bare, 1.0);
        assert!(s.lobes.is_empty());
        // And the crown no longer inflates the extent the margin band is sized from.
        assert!(s.reach < Skeleton::derive(1, &oak(), 1.0).reach);
    }

    /// World cells a species occupies, standing on `anchor`.
    fn wood_cells(sp: &TreeSpecies, seed: u64, anchor: IVec3) -> std::collections::HashSet<IVec3> {
        let s = Skeleton::derive(seed, sp, 1.0);
        let root = anchor.as_vec3() + Vec3::new(0.5, 1.0, 0.5);
        let mut out = std::collections::HashSet::new();
        s.voxelize(root, |c| {
            out.insert(c);
        });
        out
    }

    #[test]
    fn the_trunk_voxelizes_and_the_twigs_do_not() {
        let cells = wood_cells(&oak(), 1, IVec3::ZERO);
        assert!(!cells.is_empty(), "the tree occupies nothing at all");
        // Far fewer cells than nodes: most of the skeleton is thinner than half
        // a voxel and correctly has no room to be solid.
        let nodes = Skeleton::derive(1, &oak(), 1.0).nodes.len();
        assert!(cells.len() < nodes, "{} cells for {nodes} nodes", cells.len());
        // The trunk stands on the anchor, so the cell directly above it is wood.
        assert!(cells.contains(&IVec3::new(0, 1, 0)), "nothing sits on the anchor");
    }

    #[test]
    fn a_thicker_trunk_occupies_more() {
        let thin = wood_cells(&TreeSpecies { trunk_radius: 0.4, ..oak() }, 1, IVec3::ZERO);
        let thick = wood_cells(&TreeSpecies { trunk_radius: 2.0, ..oak() }, 1, IVec3::ZERO);
        assert!(thick.len() > thin.len() * 2, "{} vs {}", thick.len(), thin.len());
    }

    #[test]
    fn voxelization_is_exactly_translation_invariant() {
        // The seam property in miniature. Two chunks derive the same tree and
        // must agree cell for cell about where its wood is; if rasterization
        // drifted with position, a trunk would gain or lose a cell at a chunk
        // border and the two chunks would disagree about a shared column.
        let at_origin = wood_cells(&oak(), 5, IVec3::ZERO);
        let offset = IVec3::new(37, -11, 204);
        let moved = wood_cells(&oak(), 5, offset);
        let expected: std::collections::HashSet<IVec3> =
            at_origin.iter().map(|c| *c + offset).collect();
        assert_eq!(moved, expected, "wood moved by more than the translation");
    }

    #[test]
    fn every_wood_cell_lies_inside_the_reported_extent() {
        // What the margin band is sized from. A cell outside it would be
        // stamped by a chunk that never derived the tree.
        let s = Skeleton::derive(9, &oak(), 1.0);
        for c in &wood_cells(&oak(), 9, IVec3::ZERO) {
            let d = c.as_vec3() + Vec3::splat(0.5) - Vec3::new(0.5, 1.0, 0.5);
            let horiz = (d.x * d.x + d.z * d.z).sqrt();
            assert!(horiz <= s.reach + 1.0, "cell {c} at {horiz:.2} outside reach {:.2}", s.reach);
            assert!(d.y <= s.height + 1.0, "cell {c} above height");
        }
    }

    #[test]
    fn segments_pair_every_node_with_its_parent() {
        let s = Skeleton::derive(2, &oak(), 1.0);
        let segs: Vec<_> = s.segments().collect();
        assert_eq!(segs.len(), s.nodes.len() - 1, "one segment per non-root node");
        assert!(segs.iter().all(|g| g.a != g.b), "a segment must have length");
    }
}
