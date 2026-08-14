# Procedural Tree Generation & Rendering — Implementation Spec

**Status:** design agreed, gated on the measurements in §13
**Target:** Rust voxel engine, orthographic isometric renderer
**Audience:** implementing agent with codebase access

---

## Revision note — v2

v1 was written without codebase access and said so: *"Where the spec conflicts
with existing systems, the codebase wins."* It has now been read against the
tree. Three things changed, each because code contradicted an assumption.

| # | v1 | v2 | Why |
|---|---|---|---|
| 1 | Per-block spine words baked into a persisted chunk array (§4) | Skeletons re-derived per chunk in the margin band; segment instances in a **generated, non-persisted** layer | The engine already achieves cross-chunk continuity without communication by re-deriving features world-absolutely (`feature.rs`, design §5 v1.10). Every benefit v1 §4.2 claimed already holds, at zero persisted bytes. Gated on §13 |
| 2 | "Wood **and leaves** remain real voxels" (§2 invariant) | Wood is voxels; leaves are instances; canopy occupancy is a specified query with no implementation | At 8 px/voxel a leaf voxel is an enormous unit of canopy. Quantizing canopy silhouette to the grid reproduces Eco Machina's "the leaves still look like cubes", which is the exact failure the ellipsoid clump field exists to avoid |
| 3 | ~11 px/voxel, treated as a constant | `k ∈ {16, 8, 4}` on an octave ladder, **8 at default zoom** | `compute_render_dimensions` (`main.rs:122`). Every pixel figure in v1 was calibrated against a screenshot estimate |

Consequences of (3) reach further than the number: radius quantization becomes
`f(k)` and must not be persisted, the §11 budget halves, the LOD ladder is
rekeyed on screen-space radius, and the octave transition is a new event that
anything pixel-snapped must invalidate against.

Two arguments used to reach (2) were **withdrawn as illegitimate** and are
recorded so they are not re-used: that design §6 says foliage does not occupy
voxel cells, and that `ShapeId` cannot express present-but-not-solid before D1.
A design document describing the current state is an input to a revision, not a
constraint on it, and a pending decision is what this work should inform. If
leaf voxels were right, "D1 must land first" would be an argument for D1, not
against leaves. The conclusion survives on the pixel-density argument alone.

---

## 0. How to read this document

This is opinionated and most decisions have been argued through. **§14 lists
approaches that were considered and rejected, with reasons.** Read it before
proposing an alternative; several obvious-looking options were rejected for
non-obvious reasons specific to this renderer.

Everything here remains subordinate to the codebase. §15 lists what is genuinely
uncertain and wants prototyping. §13 lists the measurements that gate §4 — if
they come back bad, §4 reverts toward v1 and this document is amended again.

---

## 1. Engine constraints that drive every decision

### 1.1 Constraints

| Constraint | Consequence |
|---|---|
| **Orthographic projection** | All view rays are parallel. Ray direction is a shader *uniform*, not a per-fragment computation. Analytic ray-primitive intersection becomes very cheap. Billboard facing is also a uniform, not per-instance |
| **Isometric, camera rotates in 90° increments** | Everything view-dependent is one of four cases and can be precomputed |
| **Rotation is smoothed, never an instant snap** | The camera occupies arbitrary intermediate yaw for a few hundred ms. **This kills any approach that bakes 4 discrete view variants.** Geometry must be correct at continuous yaw |
| **The camera already carries a snapped target alongside the eased current** | `target_rotation` versus `rotation`. The blueprint preview (Substep 16g) deliberately reads the snapped one so it steps with the key rather than spinning through the ease. Stepped yaw (§8.2) is therefore a smaller departure than it looks |
| **World renders to a low-res buffer, then upscales** | **`k` texels per voxel edge, where `k` is 4, 8 or 16 depending on zoom.** See §1.2. Precision budget is render pixels, not voxels — and the budget moves |
| **Camera translation snaps to whole low-res pixels** (with smoothing) | Anything animated must snap to the same grid or it shimmers against a stable background |
| **Volumetric effects: fog, rain, fire, dynamic lighting** | Foliage needs a real depth buffer contribution and real normals. Alpha-blended cards are unusable as volumetric occluders |
| **A shadow map already exists** | `shadow_pass.rs` renders chunk geometry to a depth map over a ~256-unit cube; `terrain.wgsl::compute_shadow` samples it with a comparison sampler and normal-offset bias. Canopy shadow rides this (§6.3) rather than needing a light bake |
| **Player must never be lost behind geometry** | Occlusion handling is a hard requirement, not polish |
| **Existing system: flood-fill sky-exposure check around player** | `room_detection_system`. It is a *rendering* system driving the enclosure cutaway, and canopy must not feed it. See §9.3 |
| **Existing system: cross-chunk feature derivation** | `derive_features` / `stamp_features`, with the seam property asserted in CI. §4.3 is "verify, don't build" |
| **Existing system: foliage scatter storage and render path** | `ScatterBucket` / `ScatterInstance` / the scatter pass. Leaves reuse this. The *distribution* system is surface-only and is not reused — see §7.1 |

### 1.2 Pixel density is a function of zoom, not a constant

`compute_render_dimensions` (`main.rs:122`) runs an octave ladder:
`world_pixel_density` defaults to 16, and `k` halves while the upscale factor
`s` would fall below 0.85.

At 1080p:

| zoom | k (texels per voxel) | note |
|---|---|---|
| 20 | 16 | near |
| **40** | **8** | **default (`initial_zoom`)** |
| 100 | 4 | `zoom_max` |

**Calibrate everything to k = 8 and express it as `f(k)`.** A figure stated in
pixels without a `k` is a bug waiting for a zoom change.

### 1.3 Scale math at k = 8

- Trunk of radius 0.4 voxels → **~6.4 render pixels wide** (~3.2 at k = 4). A
  hexagonal cross-section is *more* visibly faceted at 6 px than at 9, so the
  analytic capsule decision in §5 gets stronger under the correction, not weaker.
- An 8-voxel tree is **~64 px tall** with a **~80 px canopy**. Trees are large
  on-screen objects; silhouette and internal structure both carry.
- Radius quantization floor is one render pixel, so useful radius steps are
  roughly `1.5k`: **~23 at k = 16, ~12 at k = 8, ~6 at k = 4.** Computed at
  render time from the current `k`. **Never persisted** — see §14.
- A 6 px leaf sprite covers **~0.56 voxel²** (0.75 voxels across), against v1's
  0.3. This halves the leaf budget; see §11.3.

### 1.4 The octave step

When `k` halves, `s` doubles, so on-screen voxel size `k·s` is **continuous
across the transition**. Nothing changes size. What changes is the texel grid
that everything snaps *to*, so the visible event is a simultaneous sub-voxel
reposition across the whole screen — a one-frame global pop, not a scale jump.

Two consequences:

- Acceptable as a hard cut. Zoom is player-initiated and the pop lands on a
  frame the player caused. The decision here is to **know about it**, not to
  build a masked transition.
- **Anything caching a pixel-snapped value across frames must invalidate when
  `k` changes.** `k` is a field of the `RenderDims` that
  `compute_render_dimensions` already returns, and the `alloc_w`/`alloc_h`
  bucketing establishes the precedent for treating a dims change as an event.

---

## 2. Architecture overview

```
┌─────────────────────────────────────────────────────────────┐
│ 1. SKELETON GENERATION      (worldgen, CPU, per chunk in band)│
│    Parametric recursion + space colonization                 │
│    → pipe model (radii) → heavy path decomposition           │
├─────────────────────────────────────────────────────────────┤
│ 2. RESIDENCY                (generated, non-persisted)       │
│    Wood voxels in the grid (persisted like any voxel).       │
│    Branch segment instances + clump ellipsoids in a          │
│    generated per-chunk layer, rebuilt on load and on remesh. │
│    NO baked spine words. NO persistent per-tree graph.       │
├─────────────────────────────────────────────────────────────┤
│ 3. RENDERING                (GPU, instanced)                 │
│    Wood:    analytic tapered capsule impostors               │
│    Foliage: ellipsoid depth core + scattered leaf cards      │
│    Shadow:  depth cores drawn into the existing shadow map   │
├─────────────────────────────────────────────────────────────┤
│ 4. OCCLUSION                (independent of 1–3)             │
│    Visual: proximity-depth mask + Bayer dither               │
│    Logic:  DDA against voxels (wood) + ray-ellipsoid (canopy)│
└─────────────────────────────────────────────────────────────┘
```

### 2.1 The invariant

**Wood is voxels. Leaves are instances. Canopy occupancy is a specified query
with no implementation yet (§10).**

Collision, mining, drops and the wood half of the DDA query read the voxel grid
and know nothing about impostors or instancing. Rendering is a pure view over
the data. That property is unchanged from v1; what narrowed is its scope.

Wood stays in the grid because voxel occupancy *is* collision — `player/sim.rs`
resolves movement purely through `solid_interval(voxel)`, so nothing outside the
grid can collide without a new physics path. Leaves leave the grid because at
8 px/voxel, quantizing canopy silhouette to voxels throws away the sub-voxel
resolution that the whole clump field exists to provide.

### 2.1a Occupancy is not rendering — and the mesher must be told

**No wood voxel is ever meshed.** Wood occupies cells for collision, mining,
lighting and the DDA query; it is *drawn* by §5's analytic capsules and by
nothing else. These are two layers and collapsing them is the single easiest
mistake to make here, because the default behaviour of the existing engine makes
it for you: `cube_mesher.rs` emits faces for every solid voxel
(`if !v.is_solid() { continue; }`) with no material discrimination anywhere.

Wire wood into the grid without changing that and every tree renders as blocks —
grid-aligned, maximally visible on exactly the thick trunk where the eye goes,
and on the wrong side of the Eco Machina baseline this design exists to beat.
That mod fights an engine not built for this, on data it does not control. We
bake the skeleton, know every radius, and own the render path; a blockier trunk
than theirs would be a planning failure, not a constraint.

**Therefore, as an ordering constraint rather than a preference: wood must not
enter the voxel grid on any live path until a render-delegated material trait
exists and the mesher honours it.** The check is a test asserting the mesher
emits zero faces for such a material - an invariant, not a scheduled cleanup.
There is no interval during which cubic wood is acceptable, so this is not a
placeholder and does not get a removal trigger.

Note the two thresholds pull opposite ways, which is what makes the mistake
plausible. `voxelize` fills a cell whose centre lies inside a capsule, so thin
branches occupy nothing and thick ones occupy cells - correct for occupancy, a
twig has no room to be solid in. Cubic rendering is harmless on a 1px twig and
worst on a 24px trunk. **Occupancy should follow thickness; rendering must not
follow occupancy.**

**Consequence for §15.1.** The shimmer prototype must not run against
cube-rendered wood. Grid-aligned edges are near-stationary under pixel snapping,
so the result would be a guaranteed false pass on the highest-priority open
question in this document. If cubes are anywhere in the pipeline, the prototype
waits.

### 2.2 What lives where

| Thing | Form | Persisted? |
|---|---|---|
| Trunk and branch wood | Voxels, `wood` material | Yes, as any voxel. Player edits are overrides |
| Branch centerline segments | Instance records in a generated per-chunk layer | **No.** Rebuilt from the skeleton on load and on remesh |
| Clump ellipsoids | Same generated layer | **No** |
| Leaf placements | `FoliageInstance` in a `ScatterBucket` | **No.** Same lifetime as scattered props today |
| Skeleton graph | Transient. Consumed by the two emitters, then dropped | No |
| Canopy occupancy | A query (§10) | No |
| Tree age | A skeleton parameter (§3.6) | Not in 0.4.0. See §3.6 |

### 2.3 Prior art

Mirrors the Eco Machina mod for Vintage Story — render-only tapered trees over
an unmodified voxel world, which validated the approach in production. Two
divergences:

- That mod must **infer** topology from raw blocks at load, and that inference
  is the documented source of nearly all its performance problems: cluster
  separation in dense forests, multi-second chunk stalls, allocation churn
  during tessellation. We generate the trees, so we re-derive rather than infer.
- Its canopy is voxel leaves, and "the leaves still look like cubes" is its most
  common complaint, which the author declined to fix. §6 is the answer to that.

---

## 3. Skeleton generation

Runs per tree, in every chunk of the tree's margin band that can see it. Output
is a node graph consumed by the two emitters in §4.4, then **discarded**.

### 3.1 Branch structure

1. **Parametric recursion** for trunk and primary limbs — full artistic control
   via a per-species params struct.
2. **Space colonization** (Runions et al.) seeded from limb tips for fine
   branching — natural secondary structure that fills a crown volume you define.

Notes:

- Explicit `Vec<Node>` work queue, **not** recursion. Hard-cap node count; the
  cap is also what bounds the §13 derivation cost.
- Carry a full orientation frame (`Quat` or 3×3 basis) through the turtle, not
  just a direction vector, or branches lose roll and twist oddly.
- Azimuth around the parent axis uses **phyllotactic increments (~137.5°)**, not
  uniform random. Large, cheap realism win.
- Apply gravitropism: lerp each child direction toward or away from world-up per
  species.

### 3.2 Data structure

```rust
use glam::IVec3;
use smallvec::SmallVec;

pub struct Node {
    pub pos:       IVec3,
    pub parent:    Option<u32>,
    pub children:  SmallVec<[u32; 4]>,
    pub radius:    f32,
    pub weight:    f32,   // subtree mass, filled by solve_radii
    pub chain:     u32,   // heavy path id, filled by decompose
    pub chain_idx: u32,
}
```

### 3.3 Radii — the pipe model (Shinozaki 1964)

Cross-sectional area is conserved across a fork: parent area = sum of child
areas. Solved bottom-up. This is what prevents conical trunks and blocky
saplings.

### 3.4 Trunk selection — heavy path decomposition

At each fork the child with the greatest subtree mass continues the current
chain; others start new chains. Standard heavy-light decomposition applied to a
physical tree, producing a readable trunk → main-branch → side-branch hierarchy.

```rust
const TIP_RADIUS: f32 = 0.06;   // voxel units
const PIPE_EXP:   f32 = 2.3;    // 2.0 = strict da Vinci; 2.0..2.5 for real trees

/// Iterative post-order: leaves first, root last.
fn post_order(nodes: &[Node], root: u32) -> Vec<u32> {
    let mut stack = vec![root];
    let mut out = Vec::with_capacity(nodes.len());
    while let Some(n) = stack.pop() {
        out.push(n);
        stack.extend_from_slice(&nodes[n as usize].children);
    }
    out.reverse();
    out
}

/// Pipe model + subtree weight accumulation, in one bottom-up pass.
fn solve_radii(nodes: &mut [Node], order: &[u32]) {
    for &i in order {
        let (r, w) = {
            let n = &nodes[i as usize];
            if n.children.is_empty() {
                (TIP_RADIUS, 1.0)
            } else {
                let mut sum = 0.0;
                let mut w = 1.0;
                for &c in &n.children {
                    sum += nodes[c as usize].radius.powf(PIPE_EXP);
                    w   += nodes[c as usize].weight;
                }
                (sum.powf(1.0 / PIPE_EXP), w)
            }
        };
        nodes[i as usize].radius = r;
        nodes[i as usize].weight = w;
    }
}

/// Heavy path decomposition. Heaviest child inherits the parent's chain.
fn decompose(nodes: &mut [Node], root: u32) -> u32 {
    let mut next_chain = 0u32;
    let mut stack = vec![(root, u32::MAX, 0u32)];
    while let Some((i, inherited, idx)) = stack.pop() {
        let chain = if inherited == u32::MAX {
            let c = next_chain; next_chain += 1; c
        } else { inherited };
        nodes[i as usize].chain = chain;
        nodes[i as usize].chain_idx = idx;

        let kids = nodes[i as usize].children.clone();
        if kids.is_empty() { continue; }
        let heavy = *kids.iter()
            .max_by(|a, b| nodes[**a as usize].weight
                .total_cmp(&nodes[**b as usize].weight))
            .unwrap();
        for &c in &kids {
            if c == heavy { stack.push((c, chain, idx + 1)); }
            else          { stack.push((c, u32::MAX, 0)); }
        }
    }
    next_chain
}
```

### 3.5 Determinism — already guaranteed, verify rather than build

Generation must be a pure function of `(world_seed, tree_world_pos, age)`. No
global RNG, no `thread_rng`.

**The engine already provides this and already tests it.**
`EvalContext::world_cell_seed` and `SplitMix64` are the existing primitives, and
determinism is asserted in CI by `--verify-generation` plus the reverse-order,
sequential, fresh-generator pass added at Substep 18. Use those primitives and
the property is inherited.

### 3.6 Age

`age` is a parameter of the skeleton function from the first line, so a stand of
trees is not a field of clones and a growth system later is a parameter change
rather than a representation change.

**0.4.0 ships age as a generation-time value only.** A tick that mutates it is
deferred, because mutating age at runtime changes generated geometry underneath
persisted overrides, which design §9 explicitly excludes as a goal. **Trigger:
the first system needing per-instance mutable state that survives a chunk
round-trip.** Palubicki et al. (§17) is the right foundation when that arrives.

Note for §13: space colonization is iterative, so derivation cost rises with
age. The node cap bounds it.

### 3.7 Every source declares `max_reach`

A skeleton's extent is not known until it is derived, so the margin band cannot
be sized from it the way `ResolvedBlueprint::horizontal_reach()` sizes a
blueprint's.

**The feature source declares a `max_reach` bound and the derivation asserts it
stays inside.** One bound covering every case, in the spirit of decision K3, and
a violated bound is a test failure rather than a seam artifact discovered in
content. Declare a vertical extent alongside it — §4.4 needs it.

---

## 4. Derivation and residency

**This section replaces v1 §4 (per-block spine words) entirely.**

### 4.1 Decision: re-derive, do not bake

v1 weighed two options — a persistent per-tree registry, or baked per-block
records — and rejected the registry. There is a third, and the engine already
runs on it: **every chunk in the margin band re-derives the identical feature
world-absolutely and keeps only what falls in its own window** (design §5 v1.10,
`feature.rs`, Substep 17a).

Read v1 §4.2's four claimed benefits against re-derivation:

| v1 §4.2 claim | Under re-derivation |
|---|---|
| C0-continuous across block *and* chunk boundaries | Holds — both chunks computed the same skeleton, so there is nothing to reconcile |
| Meshing is a pure local function, fully parallel | Holds — no apron, no registry, no locks |
| No cross-chunk communication | Holds |
| No "which tree owns this block" question | Holds — there is no ownership question to ask |

All four, at **zero persisted bytes**. Spine words would cost a fifth per-chunk
layer, a `BLOB_VERSION` bump and a save wipe, against a recorded defect where
saves already grow with every chunk *visited* rather than edited (Substep 12).
They would also freeze a radius precision against a `k` that §1.2 shows is not
constant.

What v1 §4 got right and this keeps: **one record decodes to one instance, and
chopping is a buffer write.** That is a render-side property, and §4.4 preserves
it without persisting anything.

**This decision was gated on §13, and §13 has been taken: the gate is closed
for the shipped path.** Derivation costs 0.015–0.068 ms per chunk against an
8.5 ms generate. Baking would buy that back at the price of a fifth persisted
chunk layer, a `BLOB_VERSION` bump and a save wipe. Not close.

### 4.2 Chunk straddling — already built

v1 §4.3 describes deriving neighbors' tree placements and writing only the
blocks landing in the current chunk. That is `derive_features` plus the margin
band, shipped at Substep 17a, with the straddling property asserted end-to-end
at 17e (a structure crossing a chunk boundary is present in whichever chunk owns
each cell) and order independence asserted at 18.

**Verify, do not build.** What it needs from this work is §3.7's `max_reach`.

v1's suggestion to generate into a local dense buffer sized to the tree AABB and
then blit is compatible and worth keeping as an implementation detail — derive
into the buffer, copy the intersection with the chunk window.

### 4.3 Gate derivation on vertical overlap

Features derive in **every** vertical chunk of the band, not just the ones the
tree reaches. Left ungated, a tree spanning two chunk-Y layers is re-derived by
every chunk-Y layer in its column, and the skeleton is the expensive part.

Gate it: the anchor Y plus the source's declared vertical extent give a Y span,
and a chunk whose window does not intersect it skips the skeleton and pays only
the candidate roll. This mirrors Substep 17a's roll-before-probe ordering, which
is what keeps stage 8 inside the frontier budget today and **must not be
reordered**.

### 4.4 The generated segment layer

Each chunk holds, in a generated non-persisted layer with the same lifetime as
`ScatterStore`:

- **Branch segment instances** — one per capsule, decoded straight into the §5
  instance buffer.
- **Clump ellipsoids** — consumed by §6, §9.2 and §10.

Both are emitted from the skeleton at chunk build time, clipped to the chunk
window by the same ownership rule `stamp_features` applies to voxels.

**Segments are `skeleton ∩ current voxel occupancy.`** A segment is emitted only
where its wood voxels still exist. Chopping therefore drops the impostor on the
next rebuild, through the existing mesh-dirty path, with no special case — and
v1's "chopping is a single instance-buffer write" survives intact.

### 4.5 Player-modified wood

Player-placed logs and chopped stumps have no skeleton. Use a **3D Euclidean
distance transform** over the affected AABB: distance-to-nearest-air *is* the
radius, and ridge-following the DT gives the centerline. One pass yields both,
and it handles arbitrary player-built shapes gracefully — a 3×3 pillar comes out
correctly thick — where the pipe model cannot.

Limitation: the DT cannot invent taper the blocks do not contain. A 1-wide
branch is radius 0.5 everywhere. Acceptable for player edits.

**Rule: pipe model for generated wood, DT for player-touched regions.**
Recompute only the affected AABB, never globally.

---

## 5. Wood rendering — analytic tapered capsules

### 5.1 Decision

**Do not tessellate tubes.** Emit one instanced quad per branch segment and
solve the tapered capsule analytically in the fragment shader.

A 6–8-gon cross-section on a ~6 px trunk shows visibly flat sides, and
silhouette does most of the visual work in a pixel-art renderer. Analytic gives
a mathematically exact round silhouette at any radius, correct per-fragment
depth and correct normals, with no vertex allocation and no meshing pass.

### 5.2 Shader (ortho fast path)

Under ortho the ray direction is constant, so `m2 = dot(rd, ba)` is per-instance
rather than per-fragment.

```glsl
// Per-instance: capsule endpoints a, b in view space; radii ra, rb.
vec3 ro = vec3(v_viewXY, 0.0);   // fragment position on the near plane
const vec3 rd = vec3(0.0, 0.0, -1.0);

vec3  ba = b - a;
vec3  oa = ro - a;
float m0 = dot(ba, ba);
float m1 = dot(oa, ba);
float m2 = dot(rd, ba);          // constant per instance under ortho
float m3 = dot(oa, rd);
float m5 = dot(oa, oa);

float rr = ra - rb;
float hy = m0 + rr * rr;
float k2 = m0 * m0 - m2 * m2 * hy;
float k1 = m0 * m0 * m3 - m1 * m2 * hy + m0 * ra * (rr * m2);
float k0 = m0 * m0 * m5 - m1 * m1 * hy + m0 * ra * (rr * m1 * 2.0 - m0 * ra);

float h = k1 * k1 - k2 * k0;
if (h < 0.0) discard;
float t = (-k1 - sqrt(h)) / k2;
// ... then cap tests against the two end spheres ...

vec3 p = ro + rd * t;
vec3 n = normalize(m0 * (m0 * (oa + t*rd) + rr*ba*ra) - ba*hy*(t*m2 + m1));
gl_FragDepth = encodeOrtho(t);
```

Fragment cost is negligible — the low-res target means shading a small fraction
of the pixels a full-res renderer would.

### 5.3 Details

- **Radius quantization: `f(k)`,** roughly `1.5k` steps (§1.3). Computed at
  render time from the current `k`, never baked into an asset or a save.
- **Polyline smoothing:** one Chaikin pass before instancing. Two is wasted at
  this resolution.
- **Fork connectors:** start a child segment *inside* the parent's radius —
  offset the first endpoint backward along the child direction by about
  `parent_radius` — and let surfaces interpenetrate. Cheaper than junction
  geometry and reads fine.
- **Z-fighting:** bias child radius down by a fraction of a step where a branch
  matches its parent's thickness exactly. This was a real shipped bug in Eco
  Machina.
- **Bark texturing:** triplanar or view-space projection in the impostor shader.
  No swept mesh means no UVs to unwrap and no rotation-minimizing frames.

---

## 6. Foliage — ellipsoid clump field

### 6.1 Structure

Each canopy is **4–8 ellipsoid clumps**, each rendered as two things: a shrunken
depth-only core (§6.2) and a scatter field of leaf cards (§7).

Clump placement rules — these determine whether the canopy reads as volume under
rotation:

- Genuinely different depths **along all three axes**. Clumps at similar depths
  average into a sphere under rotation.
- **Uneven sizes** — one dominant, several secondary.
- **Deliberate gaps between clumps.** The gaps are what sell depth; a sealed
  canopy reads flat from every angle, and branches must be visible through them.

### 6.2 The depth-only core

Render each clump ellipsoid at **~0.75 scale, depth-only, no color**, before the
leaves. Solve analytically, same pattern as the capsules:

```glsl
// Per-instance: center c, radii r (vec3).
vec3  ro_l = (ro - c) / r;
vec3  rd_l = rd / r;
float a = dot(rd_l, rd_l);
float b = dot(ro_l, rd_l);
float k = dot(ro_l, ro_l) - 1.0;
float h = b*b - a*k;
if (h < 0.0) discard;
float t = (-b - sqrt(h)) / a;
gl_FragDepth = encodeOrtho(t);
```

Four things this buys, all load-bearing:

1. **Interior culling for free.** Leaves on the far side fail the depth test —
   roughly halves fragment load, with no CPU hemisphere culling and no popping
   during rotation, because it is continuous.
2. **A watertight volume for volumetrics.** Fog, rain and light shafts need
   something solid to intersect. A cloud of alpha-tested cards is a poor
   occluder for that.
3. **A guaranteed fallback silhouette.** If leaf density drops under LOD or a
   budget cap, the canopy still occludes correctly instead of going
   see-through.
4. **Canopy shadow**, via §6.3.

Shrink factor 0.7–0.8. Too large eats the leaves' silhouette; too small loses
the culling.

### 6.3 Depth cores go into the existing shadow map

The engine already has a shadow map: `shadow_pass.rs` renders chunk geometry to
a depth target over a ~256-unit cube, and `terrain.wgsl::compute_shadow` samples
it with a comparison sampler and normal-offset bias.

**Draw the depth cores into that pass.** They are already analytic
depth-writing primitives, so this is one more instanced draw with the same
shader. Ground under a tree is then shadowed by the identical mechanism that
shadows it from a cliff: no light bake, no voxel occupancy field, and no
dependency on D2.

It is also better than a voxel-quantized alternative would be, for the same
reason leaves are not voxels — the shadow follows the ellipsoid's continuous
silhouette.

Two riders:

- The shadow frustum is ~256 units, so distant forests will not cast. Distant
  cliffs already do not; this is the existing limitation, not a new one.
- Cores-only gives a smooth elliptical shadow where a real canopy is dappled. At
  k = 8 dapple is 1–2 px. Ship cores-only and treat "also render leaf cards into
  the shadow map" as a tuning item, not a design question.

---

## 7. Leaf scatter

### 7.1 Where this lives — alongside the existing system, not inside it

v1 §7 said to reuse the engine's foliage distribution, and §12.6 asked whether
that system accepts an arbitrary volume predicate. **It does not, and not
closely.** In `detail_eval.rs`:

- `PoissonDistribution` is a 2D world-XZ jittered grid.
- `SurfaceFilter` resolves `surface_y` by scanning a column top-down.
- `ScatterPlace` anchors at `[lx, surface_y, lz]`.
- `Candidate` is `{ x, z, surface_y }` — there is no 3D position anywhere in the
  detail domain.

So the ellipsoid shell sampler is a **new emitter beside the detail evaluator**,
not a node inside it. What the two share is the output type and everything
downstream: emit `FoliageInstance` into a `ScatterBucket` and the storage
boundary, the render pass, stable ids and the anchor interaction model are all
inherited unchanged.

Use the feature's `stable_instance_id` as the identity the sampler hashes
against, so leaf positions derive from the same identity everything else in the
tree does.

### 7.2 Sampling

```rust
/// Deterministic scatter over a clump's shell. Stable across reloads
/// and identical on every client - position derives only from ids.
fn scatter_clump(tree: StableId, clump: u32, count: u32, e: &Ellipsoid) -> Vec<LeafInstance> {
    let mut out = Vec::with_capacity(count as usize);
    let max_r = e.radii.max_element();
    let mut i = 0u32;
    let mut emitted = 0u32;

    while emitted < count {
        let h = hash3(tree.0, clump, i); i += 1;
        let d = uniform_sphere(h.xy());

        // Areal-density correction: reject proportional to the surface
        // Jacobian, so a flattened lobe does not bunch at the poles.
        // Without this, normalize(gaussian) * radii clusters samples
        // toward the short axes - visible as a density band.
        let j = (d * e.radii.yzx() * e.radii.zxy()).length() / (max_r * max_r);
        if h.z > j { continue; }

        // Shell thickness: most leaves at the surface, cubic tail inward.
        // The inward tail closes gaps that would otherwise open to sky
        // as the camera rotates past a lobe's silhouette.
        let depth = 1.0 - h.w.powf(3.0) * SHELL_FRAC;      // SHELL_FRAC ~ 0.35

        out.push(LeafInstance {
            pos:    e.center + d * e.radii * depth,
            normal: pack_snorm((d / e.radii).normalize()),  // FIELD normal
            roll:   hash1(tree.0, clump, i) * TAU,
            sprite: (hash1(tree.0, clump, i ^ 0x9E37) * VARIANTS as f32) as u8,
            phase:  hash1(tree.0, clump, i ^ 0x51ED),
        });
        emitted += 1;
    }
    out
}
```

### 7.3 Billboard facing — do not compute per-instance

Under ortho **every card shares one facing basis**. Pass `view_right` /
`view_up` as uniforms and expand the quad in the vertex shader. Per-instance
data is only position, hashed roll, sprite index, packed field normal and wind
phase.

Consequence worth understanding: each card's silhouette never changes during
rotation. That is fine — all apparent volume comes from instance *positions*
moving relative to each other, which is genuine 3D parallax. Flat cards in a
well-distributed field read as volume.

### 7.4 Lighting — field normals, not per-card normals

This is the difference between a canopy and confetti. Shade using the
**ellipsoid's normal at the leaf's anchor point** plus a depth-into-canopy term:

```glsl
float ndl   = dot(v_fieldNormal, u_lightDir) * 0.5 + 0.5;
float depth = v_shellDepth;                    // 1.0 surface → ~0.65 inward
float lum   = ndl * mix(0.55, 1.0, depth);

// Quantize hard - palette lookup, NOT a gradient.
int   step  = int(clamp(lum, 0.0, 0.999) * 4.0);
vec3  col   = u_canopyPalette[v_speciesBase + step];
```

Per-card normals produce high-frequency value noise that destroys the read at
this resolution. Field normals give coherent form shading — an obvious lit side
and shadow side across a whole lobe — from thousands of independent sprites.

**Constrain the palette hard: 3–4 values per canopy layer.** This matters more
to the final look than any geometry decision in this document.

### 7.5 Alpha

**Alpha test, never alpha blend.** `discard` below threshold, write depth.
Blending forces sorting, which under ortho iso is a permanent bug source, and
hard-edged cutouts are what make it read as pixel art anyway.

### 7.6 Building into a canopy

A player can build inside the canopy volume. **Cull a leaf instance when the
voxel containing its position is solid** — its position, not its anchor cell; a
leaf sits far from the branch it hangs off.

This is the same rule §4.4 applies to segments, evaluated at the same rebuild,
so building a platform in a tree carves the canopy around it as a behavior
rather than as a placement restriction. No rule to author and nothing to
validate.

---

## 8. Animation and the shimmer problem

### 8.1 The risk

Thousands of tiny sprites whose projected centers land at arbitrary sub-pixel
positions. Under smooth camera rotation each crawls independently — a boiling
canopy. **This is the single most likely way the whole approach fails.**

Snapping each instance to the pixel grid in the vertex shader fixes the crawl
but replaces it with independent one-pixel pops at arbitrary times, which reads
as noise rather than animation.

### 8.2 Recommended fix — quantize the rotation itself

Step yaw through **8–12 discrete sub-angles** over the transition, with a slight
ease, instead of interpolating continuously. Every leaf then resolves its snap
on the same frames, so pops become *coherent*: the whole canopy shifts together
and reads as hand-authored sprite animation. It also lets rotation steps, wind
cadence and sprite animation frames share one clock.

This still satisfies "never an instant snap" — it is a transition with frames,
just a deliberately framed one. And it is a smaller change than it looks: the
camera already carries a snapped `target_rotation` beside the eased `rotation`,
and the blueprint preview already prefers the snapped one (Substep 16g).

**Fallback if continuous yaw must be preserved:** snap to a 2 px grid — fewer,
larger, less noisy pops — and disable snapping during transitions. Expected to
look worse.

### 8.3 Wind — two levels

Per-leaf phase alone looks like static. Sum a coherent clump sway with a
decorrelated per-leaf flutter:

```rust
// Clump sway - slow, large, coherent. All leaves in a lobe move together.
let sway = wind_dir * (t * 0.6 + clump_phase).sin() * clump_amp * height_falloff;

// Leaf flutter - fast, small, decorrelated.
let flutter = leaf_tangent * (t * 4.0 + inst.phase * TAU).sin() * 0.12;

let offset = snap_to_pixels(sway + flutter, view_basis);
```

```rust
// Snap in SCREEN space, then reproject back to world.
// px_per_unit is k * s and changes on the octave ladder - see 8.4.
fn snap_to_pixels(raw: Vec3, view_right: Vec3, view_up: Vec3, px_per_unit: f32) -> Vec3 {
    let sx = raw.dot(view_right) * px_per_unit;
    let sy = raw.dot(view_up)    * px_per_unit;
    view_right * (sx.round() / px_per_unit)
  + view_up    * (sy.round() / px_per_unit)
}
```

Two gotchas:

- **Sample `t` at a fixed low rate (12–15 Hz), held between updates.**
  Continuous sampling makes clumps near a rounding threshold flicker between
  adjacent pixels. The low rate also reads as hand-animated.
- **During rotation the view basis is changing**, so the snapping basis moves.
  Freezing wind offsets for the duration of the transition is acceptable and
  usually invisible — the rotation is short and the eye is busy.

Sprite animation frames index off the same clock with per-leaf phase as offset,
so no two leaves flip on the same frame but all flips land on the grid.

### 8.4 Invalidate on the octave step

`k` changes discretely with zoom (§1.4), and every snapped value is expressed in
its grid. **Any cached snapped position must be recomputed when `k` changes.**
Treat a `RenderDims` change as an event, as the allocation bucketing already
does.

---

## 9. Occlusion

Two independent halves. **Neither requires any change to the trees**, so this
can be built after the tree work lands.

### 9.1 Visual — proximity depth mask + Bayer dither

Render the proximity volume — a capsule or box around the player, a few voxels
of radius — to a small offscreen target, depth only. Then a full-screen pass
over the main buffer:

```glsl
float maskDepth = texture(u_proximityDepth, uv).r;
if (maskDepth < 1.0) {                        // pixel is inside the volume footprint
    float sceneDepth = texture(u_sceneDepth, uv).r;
    if (sceneDepth < maskDepth - u_bias) {    // scene geometry is in front of it
        float th = bayer4x4(ivec2(gl_FragCoord.xy) & 3);
        if (u_strength > th) { /* composite the behind-layer instead */ }
    }
}
```

Fully general — catches trees, cliffs, buildings, anything. Needs a second color
target holding what is *behind* the occluder; the simplest source is re-rendering
the proximity volume unlit, which costs nothing at this resolution.

**Ordered dither, not alpha blending.** At this pixel density a 4×4 Bayer stipple
reads as a deliberate shader effect rather than broken transparency.

Generalize the focus point to a small SSBO of 2–4 points: player, important NPCs,
interactables in reach.

**Escalation if needed:** dithering an entire trunk away can look chewed. Since
branches are already instanced, the same test can run CPU-side per-instance
against projected bounds and the instance simply skipped — whole branches vanish
cleanly. Keep per-fragment dither for leaves, where stipple looks best.

**Add a silhouette pass regardless.** Re-render the player with
`depth_compare: Greater`, unlit, one palette color. About 20 lines, and it means
the player is never actually lost even if a dither path misses. That is the
difference between "occasionally frustrating" and "never frustrating".

### 9.2 Logic — DDA for wood, ray-ellipsoid for canopy

For decisions rather than visuals: camera pull-in, UI hints, AI visibility.
Deterministic, no GPU readback.

v1 marched a single DDA and read `Block::Leaves` as a partial occluder. With
leaves out of the grid that term moves to the clump ellipsoids, which is both
cheaper and more accurate — 4–8 analytic tests replace the eight voxel steps a
canopy would have contributed, and the coverage term follows the real silhouette
instead of a voxel approximation.

```rust
/// Ortho: view_dir is constant across the whole screen, even mid-rotation.
/// Returns accumulated occlusion weight (canopy partial, wood full).
fn occlusion_along_view(world: &World, start: Vec3, view_dir: Vec3, max_v: i32) -> f32 {
    let dir = -view_dir;                        // march toward the camera
    let mut v = start.floor().as_ivec3();
    let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
    let delta = dir.recip().abs();              // t per voxel crossing, per axis
    let mut t = ((v + step.max(IVec3::ZERO)).as_vec3() - start) / dir;
    let mut acc = 0.0;

    for _ in 0..max_v {
        if t.x < t.y && t.x < t.z { v.x += step.x; t.x += delta.x; }
        else if t.y < t.z         { v.y += step.y; t.y += delta.y; }
        else                      { v.z += step.z; t.z += delta.z; }

        if world.get(v) != Block::Air { return 1.0; }
    }

    // Canopy: analytic, against the clump ellipsoids in the generated layer.
    // Partial by construction - one lobe should not trigger a cutaway, several
    // should - so accumulate chord length rather than a hit test.
    acc += canopy_coverage_along(start, dir, max_v as f32);
    acc.min(1.0)
}
```

Fire about 8 rays — head, feet, six points on the proximity boundary — and
average for a smooth 0–1 signal. Works unchanged at arbitrary yaw, so it stays
valid throughout a rotation transition.

### 9.3 Sky exposure must ignore canopy

`room_detection_system` (`ecs/systems.rs:666`) is a **rendering** system, not a
world-state query. It runs `overhead_coverage` as a pre-filter at 0.3, floods
through air with a budget of 22, and sets `room.enclosed` at `u >= 0.6`, which
drives the enclosure cutaway and the visibility mask.

Feed canopy into it and standing under a tree raises overhead coverage, runs the
flood, and can push undergroundness past the threshold — **treating a forest as
an interior and switching on the cutaway.** That is concretely wrong.

So sky exposure carries two different questions and only one of them exists
today:

| Question | Consumer | Canopy-aware? |
|---|---|---|
| "Am I in a cave?" (`undergroundness`) | Enclosure cutaway, visibility mask | **No** |
| "Is there something over me?" (shelter) | Weather, rain, spawning | Yes — see §10 |

One number cannot serve both. Do not collapse them.

---

## 10. Canopy occupancy — specified, not built

Canopy is absent from the voxel grid, so a system that wants to know "is there
canopy at this point" has nothing to read. Three candidate consumers were
considered, and **two of them dissolved**:

- **Lighting** — answered by §6.3. The shadow map handles it, better.
- **Placement** — answered by §7.6. Instance culling handles it, with no rule.
- **Weather shelter and canopy-aware spawning** — genuinely need it, and neither
  system exists.

So the field has no live consumer, and this project's own repeated rule applies:
an abstraction lands with its consumer, and a deferral gets a named trigger.
**Specify the contract, do not build it.**

```rust
/// Canopy density at a world position, 0 = open, 255 = fully enclosed.
/// Derived from the clump ellipsoids in the generated layer, over the same
/// margin band as the segments, so it is continuous across chunk borders by
/// the same construction the seam test already covers.
fn canopy_density(pos: Vec3) -> u8;
```

**Trigger:** the first system needing weather shelter or canopy-aware spawning.

**Representation, decided in advance because it is cheap now:**

- If the consumer sweeps every voxel (a light bake would), **rasterize once into
  a sparse per-chunk array.**
- If the consumer is pointwise, **query the ellipsoids analytically** and store
  nothing.

Either way it must be **sparse or absent** for chunks containing no tree, which
is the overwhelming majority. A dense `u8` per voxel is 32 KB per chunk, against
a fluid layer already measured as the largest resident CPU layer at 25.6 MB.
Same fast-path shape as `FluidFillMode::Empty`.

Whichever form it takes, it is derived and never persisted, and the seam test
covers it.

---

## 11. LOD and budgets

### 11.1 Key the ladder on screen-space radius, not distance

At k = 4 the camera sees roughly four times the world area **and** every trunk
is half as wide, so a fixed world-distance ladder is wrong at both ends.

**One discriminant: `radius_px = world_radius * k`. Every tier threshold is
stated in pixels.** The ladder is then zoom-invariant for free, and it absorbs
what would otherwise be a second set of k-dependent constants — leaf size is
fixed in world units, and the tier decides count and upscale.

### 11.2 Foliage ladder

Every tier renders the same volume with the same palette and the same field
normals, so transitions are nearly invisible. Cross-fade with the Bayer mask if
popping appears.

| `radius_px` | Foliage |
|---|---|
| Large | Full scatter + depth core |
| Medium | Half the leaves, sprites upscaled to keep coverage constant |
| Small | Analytic ellipsoid with world-lattice noise cutout (§11.4) |
| Tiny | Solid ellipsoid, 2-value palette |

### 11.3 Budget at k = 8

Recalibrated from v1's 11 px assumption. A 6 px sprite covers ~0.56 voxel², not
0.3, so the leaf count roughly halves:

- Clump shell ≈ 50 voxel²; sprite covers ~0.56 voxel² → **~200 leaves per
  clump** at 2.5× overlap (v1 said 400).
- 6 clumps → **~1,200 instances per tree** (v1 said 2,500), roughly half
  surviving the depth core.
- 50 visible trees → **~65 k instances**.

Comfortable — **but only if instance culling is per-clump** (CPU, or a compute
pass writing indirect args), never per-leaf.

Note that visible tree count itself rises as `k` falls, since the camera sees
more world. The `radius_px` ladder is what absorbs that: the extra trees arrive
already in the cheap tiers.

### 11.4 Far-tier noise cutout

Cut the ellipsoid silhouette with **world-space quantized noise** so it keeps
ragged pixel-art edges:

```glsl
vec3 q = floor(p_world * u_detailLattice) / u_detailLattice;  // ~3-4 samples/voxel
float mask = noise3(q * u_noiseFreq + u_clumpSeed);
if (mask < u_cutoff) discard;
```

Quantizing to a **world-space lattice** is essential — it anchors detail to the
world so it does not swim under rotation. Screen-space noise would shimmer.

### 11.5 Wood LOD

There is no tessellation, so there are no ring counts to reduce. LOD is fewer
segments: below a `radius_px` threshold a whole tree collapses to one capsule
plus one ellipsoid, still on the analytic path.

**Do not bake per-orientation impostors.** v1 §10.3 proposed it, and it
contradicts v1 §1's own rejection of baked view variants: smoothed rotation
means there is nothing to show at intermediate yaw. The analytic path is already
cheap enough that the baked version buys nothing.

---

## 12. Interfaces this work depends on

These belong to the wider foliage rework rather than to trees, and are recorded
here because the tree work consumes them. They should move to
`engine-design.md` §6 when that section is revised.

**Material traits replace `supports_flora`.** The bool is a one-sided encoding
of a two-sided fact: a material can only carry "does foliage grow here" because
foliage has no entity to carry the rule instead. Replace it with a small named
trait bitflag set authored in `materials.ron` — `soil`, `stone`, `sand`, `wood`,
`snow`, `ice`, `organic`, `porous` — named for what the material *is*, never for
the consequence. Fixed set rather than open strings, so a typo is a compile
error rather than a rule that silently matches nothing.

**Prop definitions carry the placement rule.** `PrefabDef` matures into the
definition every placement path reads: substrate trait mask, admissible faces,
size class, destruction policy, sway, and later the sprite atlas reference.
Explicit material overrides **by name**, per decision B1 and D5. This is the
registry `supports_flora` was standing in for.

**One admission function.** `admits(prop, voxel, face) -> bool`, called from
`eval_paint`, `eval_scatter` and the leaf emitter. Today the substrate test
exists twice and disagrees — `material.supports_flora` is authored and read by
nothing, while `SurfaceFilter.materials` is read and numeric. Two copies of a
lookup is this engine's recurring seam defect; they collapse here, with
`SurfaceFilter` becoming a graph-level refinement rather than a parallel answer.

**An anchor is a (voxel, face).** This is what admits moss on a wall, vines
under an overhang and hanging roots. Grass is the +Y case and does not change.
`ScatterInstance.flags` has five free bits; a face needs three.

**Paint stays +Y only.** A `PaintTexel` map is indexed by XZ and structurally
cannot hold a face. Anything face-oriented is scatter. Trigger for revisiting: a
species wanting continuous density over a non-top surface at a density where
instancing measurably costs too much.

**Retired by this work:** `FeatureCanopy`, `StructureCanopy` and `PlaceTree`.
The first two are a single-blob canopy at a fixed `y_offset`, which cannot
express leaves distributed through branches. Note that `PlaceStructureParams.canopy`
is currently `#[serde(skip)]`, so it is `None` on every load and no canopy has
ever been emitted — that is the cause of the handoff's open item 4.4, and it is
code to delete rather than a bug to diagnose.

---

## 13. The measurements that gate §4 — taken; gate closed

§4 trades storage for CPU: skeletons are re-derived by every chunk in the band
instead of read from disk. That trade is right if the derivation is cheap and
wrong if it is not.

**Outcome.** Release build, 400 derivations per configuration, after the lobe
work landed:

| species | nodes | mean | max single | per chunk, 98-chunk band |
|---|---|---|---|---|
| default | 156 | 15.5 µs | 159.9 µs | **0.015 ms** |
| largest (node cap, 12 lobes) | 512 | 67.8 µs | 265.2 µs | **0.068 ms** |
| small turtle + colonization | 512 | ~620 µs | — | ~0.6 ms |

Against a `generate` mean of 8.5–9.0 ms, the shipped path adds **0.2–0.8 %**.
A chunk seeing 20 trees at once pays 0.31 ms (default) or 1.36 ms (largest).
Baking would recover that at the price of a fifth persisted chunk layer, a
`BLOB_VERSION` bump and a save wipe. **Gate closed; spine words stay dropped.**

Three things the measurement found that the plan did not anticipate:

- **The largest species is not the worst case.** Colonization on a maxed-out
  turtle costs nothing extra, because the turtle fills the node budget and the
  pass early-returns. The expensive shape is the opposite - a *small* turtle
  leaving a large colonization budget, at ~620 µs and up to 1368 µs with 1000
  attractors. **Colonization is the only thing that can reopen this gate**, and
  it is opt-in and off by default. A species author enabling it is taking a
  10-40x cost multiplier and should be told so.
- **Spike ratio is 4-10x** (160-265 µs max against a 15-68 µs mean). That is
  exactly the shape point 3 below warns about, at a magnitude too small to
  matter. Recorded so it is recognised rather than rediscovered if the mean
  ever grows.
- Points **3 and 4 could not be taken** and are carried, not waived - see below.

### The six, and what happened to each

1. **Largest species, not average** - taken. 67.8 µs. Superseded as the worst
   case by the colonization finding above.
2. **Include the vertical axis** - taken. The 98-derivation figure is
   7x7 chunks x 2 chunk-Y layers.
3. **Latency on the chunk-load critical path, not throughput** - **carried.**
   Nothing derives a tree during generation yet, so there is no chunk-load path
   to measure on. Re-take when segment emission lands. The bound above is what
   stands in the meantime: a spike cannot exceed the total, and the total is
   1.5-6.6 ms spread across 98 chunk generations.
4. **Per-span distributions, not max-of-one-frame** - **carried**, same reason.
   The max-single-derivation column above is the microbenchmark stand-in.
5. **Memoization keying** - not needed. Nothing memoizes and nothing needs to.
6. **Guard vacuity** - moot while (5) holds.

Carrying 3 and 4 rather than declaring them met: the question they ask is real,
and a three-order-of-magnitude margin is a reason to stop blocking on it, not a
reason to pretend it was answered.

1. **Measure the largest species, not the average.** Eco Machina's stalls were
   specifically kapok and redwood. A band that is one chunk for an oak may be
   three for a redwood, and 7×7 chunks re-deriving one large skeleton is where
   this holds or does not.
2. **Include the vertical axis.** Features derive in every vertical chunk of the
   band, so 7×7 is really 7×7×(chunk-Y layers spanned) — roughly 98 derivations,
   not 49. If that is what breaks it, §4.3's Y-extent gate is the fix.
3. **Measure latency on the chunk-load critical path, not throughput.** It is a
   pure function and parallelizes beautifully, which will make throughput look
   fine while individual loads stall.
4. **Per-span distributions, not a max-of-one-frame capture.** Substeps 2b and
   2c produced contradictory attributions from one-frame captures, and 2d had to
   introduce max *and* mean across a long run to resolve it. Same instrument,
   same failure available.
5. **If memoization lands, key it on `(seed, anchor, age)`,** not seed alone, or
   the first grown tree poisons the cache. Treat the cache as derived state;
   Substep 18's reverse-order test is then the guard, which is what makes
   memoizing safe here at all.
6. **Check that guard is not vacuous.** S18 only catches a bad cache if two
   sampled coordinates actually share a tree, and `ORDER_SAMPLE = 64` is strided
   across 512 — the odds are poor. Without a deliberately adjacent pair inside
   one tree's band it is Substep 17f again: a guard that cannot see the thing it
   guards.

Had the numbers gone the other way, the fallback was a **cache** of the
derivation, not a return to baking as the source of truth. It is not needed.

---

## 14. Rejected approaches — do not re-litigate without new information

| Approach | Why rejected |
|---|---|
| **Per-block spine words, persisted** (v1 §4) | Solves cross-chunk continuity, which `derive_features` already solves by re-derivation at zero persisted bytes. Costs a fifth chunk layer, a `BLOB_VERSION` bump and a save wipe, against a recorded defect where saves already grow per chunk *visited*. Would also freeze a radius precision against a `k` that is not constant. Gated on §13 — a derivation *cache* remains available |
| **Leaves as voxels** (v1 §2 invariant) | At 8 px/voxel a leaf voxel is an enormous unit of canopy, and quantizing canopy silhouette to the grid reproduces Eco Machina's "the leaves still look like cubes". Sub-voxel instance positions are the entire point of the clump field. Rejected on pixel density, **not** on `ShapeId` or on design §6 |
| **`Block::Leaves` as a DDA occlusion term** | Follows from the above. Ray-ellipsoid against the clump volumes is cheaper and follows the real silhouette (§9.2) |
| **A canopy density field built now** | No live consumer: lighting goes to the shadow map, placement to instance culling, and shelter/spawning do not exist. Specified with a trigger instead (§10) |
| **Canopy feeding `room_detection_system`** | It drives the enclosure cutaway, so a forest would read as an interior (§9.3) |
| **Baked per-orientation wood impostors** (v1 §10.3) | Contradicts v1 §1's own rejection of baked view variants under smoothed rotation |
| **Full voxelization of tree geometry** (capsule/SDF stamping) | The original plan; superseded. Correct for a full-res voxel game, but here the render layer decouples entirely, giving smooth silhouettes at no data cost |
| **Tessellated tube meshes** (swept K-gon rings) | Faceted silhouette at ~6 px trunk width — worse under the corrected pixel density than v1 assumed. Analytic impostors are exact, cheaper, and need no meshing pass |
| **Rotation-minimizing frames** (double-reflection) | Was needed to avoid bark twist on swept meshes. No swept mesh exists — triplanar projection instead |
| **SDF + surface nets / dual contouring** | Genuinely better silhouettes (root flare, blended collars, burls), but the extra fidelity is sub-pixel at this scale. Reconsider only if an isosurface path already exists for other reasons |
| **4-view baked sprite atlas** for leaf clumps | Assumes discrete camera orientations. **Smoothed rotation kills it** — nothing to show at intermediate yaw. Also 4× atlas memory for a worse result |
| **Per-instance billboard facing** | Redundant under ortho — facing is a uniform |
| **Alpha blending on foliage** | Forces sorting; permanent bug source under ortho iso; hard cutouts suit the art direction better anyway |
| **Per-card normals on leaves** | High-frequency value noise destroys the read at this resolution. Field normals instead |
| **Screen-space noise for canopy detail** | Swims under camera rotation. Must be a world-space lattice |
| **Persistent per-tree graph + registry** | Forces cross-chunk mesher dependencies, locking, and "which tree owns this block" bookkeeping. Precisely the design behind Eco Machina's documented stalls |
| **Flood-fill tree detection at load** | Merged clusters in dense forests are expensive to separate and a known source of multi-second stalls. We generate the trees |
| **Strahler ordering** instead of HPD | Cheaper, and radius falls out of the order directly, but tie-breaks on topological depth rather than mass — picks a less convincing trunk on lopsided trees. Viable fallback if HPD proves too costly |
| **Trees as entities, not blocks** (Valheim-style) | Cheapest option and trivially supports felling physics, but forfeits per-block mining, building into trunks, and voxel-world interaction. Wrong trade unless trees become pure scenery |
| **GPU mesh shaders** for chain expansion | Better than instanced quads in principle, but `wgpu` does not expose them |

---

## 15. Open questions — prototype, do not implement

Closed since v1: **§12.5** (spine word bit layout) is moot, nothing is
persisted. **§12.6** (does the existing distribution accept a volume predicate)
is answered no, see §7.1.

1. **Shimmer verdict (highest priority).** Build one tree, orbit continuously
   with pixel snapping on, and judge whether the canopy boils. **This decides
   stepped versus continuous yaw (§8.2), which several systems hang off.**
   Resolve first. Partly pre-answered: the camera already keeps a snapped target,
   so stepped yaw is a small change.
2. **Leaf scatter tuning.** Expose `SHELL_FRAC`, leaf count per clump and
   depth-core shrink as live sliders. Tune against a static camera, *then* enable
   rotation. Target: the canopy holds its read through a full 360°.
3. **Depth-core shrink factor.** Stated 0.7–0.8. Needs visual confirmation it
   does not eat the leaf silhouette.
4. **`PIPE_EXP` per species.** 2.3 is a starting point; range 2.0–2.5.
5. **Does the octave step read as a defect?** §1.4 argues it should be
   acceptable as a hard cut, since `k·s` is continuous and only the snap grid
   moves. Confirm by zooming across a boundary with a dense canopy on screen.
6. **Does cores-only canopy shadow read as a blob?** §6.3. If so, leaf cards go
   into the shadow map too.
7. **Leaf sprite authoring.** Number of variants, whether animation frames are
   per-sprite or shared. Art-dependent.

---

## 16. Implementation order

Each step is independently verifiable.

1. **Skeleton generation + pipe model + HPD**, with a debug visualizer drawing
   the node graph as lines. `DebugLinePass` (Substep 16c) is the existing hook.
   No rendering integration yet.
2. **§13's measurements**, against the largest species. Cheap here, because step
   1 is the thing being measured and nothing depends on it yet. **This is the
   gate on §4** — take it before building the emitters, not after.
3. **Segment emission into the generated layer**, with a debug view decoding
   instances back to line segments. Verify continuity across chunk borders
   explicitly, and add `max_reach` (§3.7) with its assertion.
4. **Analytic capsule impostors** for wood. This alone should look like a
   substantial visual upgrade.
5. **Ellipsoid depth cores**, rendered visibly rather than depth-only, to confirm
   clump placement and distribution.
6. **Depth cores into the shadow map** (§6.3).
7. **Leaf scatter**, emitting into `ScatterBucket`. Static camera only.
8. **Prototype #1 from §15** — rotation, shimmer verdict, yaw decision.
9. **Wind**, on whatever clock the yaw decision produced.
10. **LOD ladder**, keyed on `radius_px`.
11. **Occlusion**, both halves. Independent of everything above and can be
    parallelized with other work.
12. **DT fallback** for player-modified wood.

`max_reach`, the Y-extent gate (§4.3) and the leaf/segment occupancy culling
(§4.4, §7.6) are not separate steps — they land with the step that introduces
the thing they bound.

---

## 17. References

- Shinozaki et al. (1964) — pipe model theory. Radii via cross-sectional area
  conservation.
- Runions et al. — space colonization for branching structure.
- Weber & Penn — parametric tree generation; the recursive method here is a
  stripped-down version.
- Wang et al. — double-reflection rotation-minimizing frames. *Not needed in the
  current design; listed in case swept meshes return.*
- Palubicki et al., SIGGRAPH 2009 — self-organizing tree models. Buds compete for
  light; trees lean away from neighbors and fill canopy gaps on their own. **Not
  part of this spec**, but the right foundation for the growth system §3.6
  defers. Expensive per step; run on a seasonal schedule, not per tick.
- Eco Machina (Vintage Story mod, `mods.vintagestory.at/ecomachina`) — production
  prior art for render-only tapered trees over an unmodified voxel world. Uses
  HPD + pipe model + 16 thickness steps. Its public comment thread is a useful
  catalogue of the failure modes this design avoids, including the voxel-leaf
  complaint that §2.1 exists to answer.
