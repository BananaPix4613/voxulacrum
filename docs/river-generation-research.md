# River generation: research findings and proposal

Written in response to `river-research-brief.md`, at the pause after Phase 11
Substep 19c. The brief asked for options evaluated against real engine limits
rather than general best practice. This document does that, and reaches a
recommendation.

**Two findings drive everything below.**

1. **The brief's open question resolves in the affirmative, and the mechanism
   already ships.** `ColumnEvaluator::sample_column(id, world_x, world_z)` is a
   world-absolute pointwise query over a per-column graph channel, valid at
   arbitrary world XZ, chunk-independent by construction. Bed *and* flow
   topology can come from one wired input. The problem the brief left open is
   not open.

2. **The current model's defect is not the bed, it is the direction of
   dependence.** Every technique that produces rivers which read as rivers -
   without exception in the literature or in shipped implementations - has the
   *terrain derive from the river network*, not the reverse. The current model
   has an independent network incised into an independent terrain, and no
   amount of reconciling two fields fixes a model whose two fields were never
   one. The fix is to make the network's elevation *be* the macro elevation the
   biome builds on.

The recommendation is therefore closest to the brief's Direction 2, but arrives
at it by a different route than "author both from the same potential", and it
answers the brief's objection to Direction 2 ("does this over-constrain terrain
authoring?") with a concrete no.

---

## 1. What the codebase actually permits

The brief lists constraints as non-negotiable. They are, but three of them are
narrower than stated, and one is wider. Verified by reading the code, not by
inference.

### 1.1 Pointwise column sampling is already world-absolute

`crates/nodegraph-eval/src/column_eval.rs`:

```rust
pub fn sample_column(&self, id: NodeId, world_x: i32, world_z: i32)
    -> EvalResult<ColumnSample>
```

Its own doc comment states the property: *"Two chunks that share a border
therefore sample the same world column identically - the basis for cross-chunk
biome-boundary blending."* It takes world coordinates, not chunk-local ones. It
recurses through `sample_input_surface`, which resolves `GraphRef` boundary
imports pointwise. `world_eval.rs::sample_zone_and_biome_at(world_seed, wx, wz)`
already uses it at arbitrary world XZ with `EvalContext::new(world_seed,
IVec3::ZERO)` - a context whose chunk coordinate is *ignored* for this path.

The same is true of the density-domain sampler. `Evaluator::sample_density` is
fully pull-based: every leaf reads `ctx.world_pos(x, y, z)` and evaluates noise
there. It touches no cached field and performs no array indexing. Its `usize`
parameters are a signature detail, not a window. `EvalContext::noise_seed`
deliberately excludes chunk coordinates, and says so, so every noise field is
globally continuous.

**Consequence.** "A pointwise surface query is expensive" is true of the *full
biome-blended surface*, because that requires resolving zone and biome identity
per column and blending neighbours. It is **not** true of a single named
per-column channel on the WorldGraph. Sampling one `GraphOutput` chain at an
arbitrary world XZ costs one noise evaluation per `SurfaceNoise` node in that
chain - the same order as `SurfaceNoise` itself, which is what the design
document already accepts as the unit of affordable cost.

This is the single fact that changes the answer. The brief's constraint was
correct about the surface; it generalised to a channel, and the generalisation
does not hold.

**Two caveats, both concrete work items.**

*The edge scan.* `sample_input_surface` and `sample_input` locate an incoming
edge by linear scan over `graph.edges`. A pointwise probe therefore costs
`O(chain_depth × |edges|)`. At 200 edges and a depth-5 chain that is ~1,000
comparisons per probe — and §6 measures this as **93 % of the proposal's probe
cost**, dwarfing the noise evaluation it exists to reach. A precomputed
`(node, pin) -> edge` map removes it outright and benefits every pointwise
consumer, not just rivers. This is the single highest-leverage fix in this
document and it is unrelated to rivers.

*The column domain has no arithmetic.* `ColumnEvaluator::fill_node` and
`sample_column` both handle exactly four node kinds — `SurfaceNoise`,
`WorldOutput`, `ZoneOutput`, `GraphOutput` — and error `WrongGraphDomain` on
everything else. There is no `Add`, `Multiply`, `Remap` or `Clamp` in the column
domain. A control channel can therefore only be a *raw* `SurfaceNoise` in
`[-1, 1]` today; it cannot be scaled into world-Y units or composed from
octaves. **Prerequisite: promote the arithmetic and curve nodes into the column
domain.** The implementations already exist in the density domain and are
elementwise, so this is a port rather than a design, but nothing below works
without it.

### 1.2 Rivers are in the wrong graph domain

`RiverParams` lives in the **density** domain (`NodeCategory::Density`,
`DENSITY_IN`/`DENSITY_OUT`) and is evaluated per *voxel*:

```rust
NodeKind::River(p) => {
    let terrain = self.input_scalar(id, 0)?;
    let net = self.river_net(id, p);
    for i in 0..ScalarField::VOLUME { /* 32,768 iterations */ }
}
```

`RiverNetwork::sample` is called once per **voxel**, 32,768 times per chunk, to
answer a question that is per **column** - 1,024 distinct answers. The result is
recomputed 32× per column. This is invisible today only because the segment
count is small.

A river is a per-column fact. It belongs in the column domain, where the engine
already has a cache (`ColumnCache`), a pointwise sampler, and a stage (1) whose
outputs every later stage reads.

### 1.3 The stage-ordering constraint is satisfied more cheaply than assumed

The design document places rivers at stages 3-4 "so every downstream stage sees
the carved surface", and specifically so scatter does not anchor over a channel
lowered afterwards. That reasoning is correct and is preserved by the proposal
below - but it does not require the *network* to be resolved at stage 3. It
requires the *carve* to happen at stage 3. Resolving the network at stage 1 and
carving at stage 3-4 satisfies both, and is strictly cheaper.

### 1.4 What genuinely does not move

- **P1 (bit-identical regeneration).** Every construction below is a pure
  function of `(world_seed, cell coordinates)` via `world_cell_seed`, and of
  world position via `noise_seed`. No proposal here accumulates cross-chunk
  state.
- **No chunk reads another chunk's output.** Held throughout. The proposal
  reads *graph channels* at foreign world positions, which is a re-derivation,
  not a read - the same distinction the cross-chunk feature model already
  makes.
- **Shapes are Cube/SlabBottom/SlabTop.** Banks stay terraced at half-voxel
  granularity until D1. This is a visual ceiling the proposal cannot raise, and
  it argues below (§5.4) that it is less damaging than it sounds.
- **Fluid is a CA, uncoupled from generation.** Generation authors a static
  per-column level. Flow is 0.5.0.

---

## 2. The taxonomy that makes the option space legible

AlcatrazEscapee (TerraFirmaCraft) states the cleanest available framing for
exactly this problem, in exactly this setting. Methods divide by how much
context they need:

- **Context-free** - evaluable at a position with no knowledge of anything
  else. All noise-derived rivers. Perfect fit for parallel chunk generation;
  cannot know terrain height, so cannot flow downhill.
- **Bounded** - needs context, but from a distance knowable *a priori*. The
  chunk fetches what it needs and re-derives. This is where structures,
  scatter, caves and the cross-chunk feature model all live.
- **Unbounded** - needs context from a distance not knowable in advance
  ("traverse until you reach an ocean"). Cannot work in an infinite,
  parallel-generated, order-independent world. Full stop.

The engine's P1 and its parallel worker pool make this taxonomy binding rather
than advisory. Restated in engine terms:

| Category | Engine meaning | Verdict |
|---|---|---|
| Context-free | Pure noise; no terrain knowledge | Allowed, insufficient |
| Bounded | Margin-band re-derivation, radius fixed by construction | **The target** |
| Unbounded | Traversal to an unknown-distance outlet; flow accumulation over the real surface | Forbidden by P1 |

The current implementation is context-free with respect to terrain: the
potential field knows nothing about the generated surface. That is precisely
why the bed cannot follow terrain, and it is the same failure mode vanilla
Minecraft rivers have (all at sea level, no direction, cycles common).

**The proposal is a bounded method.** Its bound is stated explicitly in §5.5
and is checkable in CI.

---

## 3. Survey

### 3.1 Streams Reflowing - the reference the brief cites

Closed-source (ARR), and the author states it is "not affiliated with or in any
way derived from delvr's original Streams mod". Its own documentation, however,
describes its mechanism and its cost clearly enough to be decisive:

> "Streams Reflowing needs to query terrain height across the spawn area before
> it can place streams. During this process, **the loading bar may appear stuck
> at 0% for a minute**." … "Players moving quickly through unvisited regions may
> encounter unrendered chunks while the mod catches up with terrain queries." …
> "we strongly recommend pregenerating your world with Chunky." … quality
> presets "control how accurately streams follow natural terrain waterflows and
> how far watersheds extend across the landscape."

This is flow accumulation over the real surface, with watershed extent as a
tunable radius - i.e. an *unbounded* method, bounded by a quality dial, paid for
with a minute of stall and a pregeneration recommendation.

**The finding that matters:** the visual result the brief admires is produced by
exactly the cost profile the engine's frontier budget forbids, and the mod's own
documentation is where that is admitted. This is not an argument that the look
is unreachable. It is an argument that it will not be reached by copying this
approach, and that the parts of it worth copying are the *outputs* (lakes at
varying elevations, banks skinned in three zones, cascades at drops) rather than
the method.

Two of its outputs are directly adoptable and cheap (§7).

### 3.2 delvr's Streams - the honest version of the same trick

Open source (Scala). Its technique, in the author's own words:

> "it creates a copy of the world's chunk generator and uses it to generate the
> checked chunks in memory and not in the world, with only the raw stone terrain
> and no structures that would cause recursive generation."

Shadow-generation of terrain to answer height queries, then a drain-to-source
walk. Bounded by partitioning the world into 16×16-chunk (256-block) zones, one
river per zone, with an outlet required on the zone edge. It also generates into
*raw* terrain before ground replacement - the same reasoning as this engine's
"carve at stages 3-4, before material at 5".

Meets all six of AlcatrazEscapee's realism criteria. The price is that rivers
are small and rare, and the partition boundary is a hard wall.

**Transferable:** the ordering argument (already held), and the demonstration
that shadow-evaluating terrain is the mechanism everyone reaches for. This
engine can do the shadow evaluation *without* the shadow, because
`sample_column` already exists (§1.1) - which is a strictly better position than
Streams was in.

**Not transferable:** the partition. A 256-block partition in a world where
`cell_size` defaults to 128 would put a wall every two cells.

### 3.3 TFC-TNG - the scale trade, stated plainly

The world is partitioned into watersheds emergent from an earlier generation
stage; rivers are built drain-to-source within one watershed with explicit
intersection avoidance; segments are then refined by **midpoint bisection
fractal** - split a segment at its midpoint, displace the midpoint, recurse.

Result: excellent self-similarity, real directionality, no cycles, a proper
graph. **Height was infeasible** because the scale (thousands of blocks) made
per-node terrain probes unaffordable.

**Transferable:** midpoint bisection is the cheapest known way to turn straight
segments into convincing windings, it is a pure function of the segment
endpoints and a seed, and it is exactly what the current implementation lacks.
Its rivers are straight because nothing bends them.

**The instructive part:** TFC-TNG could not afford height *because it probed the
terrain*. The proposal below never probes the terrain - it probes a dedicated
low-frequency channel - which is why it can afford what TFC-TNG could not.

### 3.4 Dendry (Gaillard et al., I3D 2019) - the load-bearing reference

`Dendry: A Procedural Model for Dendritic Patterns`. A **locally computable**
procedural function returning the distance to a dendritic tree that is
constructed on the fly around the query point.

The construction, in the paper's terms:

1. Overlay the domain with a grid `G₀`. Each cell holds a jittered key-point
   `q₀(i,j)`.
2. Sample a **control function** `c: R² -> [0,1]` at each key-point.
3. Connect each key-point to the key-point of minimal `c` in its Moore
   neighbourhood. This is `B₀`, the coarse tree.
4. `f(p) = d(p, T)`.

**The engine's `river.rs` is this algorithm.** `node()` is the jittered
key-point; `potential_frequency` noise is the control function; the eight-way
steepest-descent link is step 3; `sample()` is step 4. The implementation
independently arrived at Dendry's level 0. That is a good sign, and it means the
gap between what ships and what the literature says works is enumerable rather
than open-ended.

What ships is missing four things Dendry specifies:

| Dendry component | Purpose | Present in `river.rs` |
|---|---|---|
| Multi-level subdivision `B₁…Bₙ₋₁`, grid ×2 per level, key-points inherited | Self-similarity; tributaries at every scale | No - single level |
| Cubic-spline segment smoothing + displacement `Δ` | Windings; removes the polyline look | No - straight segments |
| Flint's-law minimum slope `S = ρ(2μ−1)^β`, `β = −0.6` | Real vertical fall along the profile | No - linear interp of two potentials |
| **Terrain height derived from the tree** (Eq. 4-7): `h(q) = h_T(q) + tanθ · f(q)`, inverse-distance blended over a 5×5 primitive neighbourhood | **The terrain is the river network's valley system** | **No - and this is the defect** |

The fourth is the whole answer to "it looks wrong". In Dendry the elevation
around a river is *defined* as the river's elevation plus a slope times the
distance to it. A flat floor through varying ground cannot occur, because the
ground near the floor is defined relative to the floor.

**The measured result** (paper Table 1, depressions computed with Barnes et
al.'s Priority-Flood, drainage with D-Infinity):

| Method | Depression surface | # depressions |
|---|---|---|
| Multifractal noise | 35.9 % | 1,352 |
| Ridged noise | 24.5 % | 2,058 |
| **Dendry (mean of 10 seeds)** | **0.8 %** | **52** |
| Real DEM, Alps 30×30 km | 3.3 % | 798 |

Dendry drains *better than a real mountain range*. This is the strongest
available evidence that a locally computable, chunk-parallel, order-independent
river model can produce hydrologically consistent terrain - which is exactly the
claim the brief needs to be true.

**The cost, stated honestly.** Table 2 reports 47 s for 512×512 at `n=1`, and
269 s for 1024×1024 at `n=3`, on an i5-6300U; the abstract quotes ~10 s for
512×512 on four cores. Taking the fastest reading, that is ~38 µs per point.
1,024 columns per chunk would be ~39 ms - **four times the entire per-chunk
generation budget.** Dendry as published is unaffordable here.

It is unaffordable for a reason that does not apply: the paper reconstructs the
tree from scratch at *every point*, deliberately, to keep the memory footprint
near zero. It says so, and lists caching as future work: *"many local branches
are repeatedly calculated. It would be possible to increase the memory footprint
by caching some of the already calculated structures."*

A chunk is a natural cache scope. `river.rs` already resolves once per chunk and
samples per column. Applying that to a multi-level Dendry is the missing
optimisation the paper names, and §6 prices it.

### 3.5 Génevaux et al. (SIGGRAPH 2013) - where the valley shape comes from

`Terrain Generation Using Procedural Models Based on Hydrology`. Builds a
hierarchical drainage network by grammar-like growth, derives watersheds,
classifies river types, then generates terrain by **combining procedural terrain
patches and river patches** in a construction tree.

Dendry's height model (Eq. 4-6) is explicitly "inspired from [Génevaux et al.
2013]" and is its locally computable reduction. The transferable idea is the one
Dendry inherits: **terrain near a river is a primitive anchored to the river,
blended with inverse-distance weights.** Not "terrain, then subtract a channel".

### 3.6 Peytavie et al. (Pacific Graphics 2019) - what happens after the bed

`Procedural Riverscapes`. Takes a bare-earth heightfield, derives river
trajectories, **carves riverbeds by combining compactly supported elevation
modifiers over the river course**, then builds a blend-flow tree for the
animated surface. Riverbed width, depth and shape, and the fluid surface
elevation and flow, are derived from terrain and **river type** - and river type
is a function of slope and Strahler order, spanning meanders through rapids to
waterfalls.

Two transferable ideas:

- **Compactly supported modifiers.** The carve is a sum of local, bounded-support
  primitives along the course. Bounded support is what makes a carve a *bounded*
  method. This is what the engine's `sample()` already approximates with its
  `dist > width` early-out; making it explicit is what lets the margin be proven
  rather than guessed.
- **River type by slope.** One classification, driven by local bed gradient,
  selects the channel profile - a flat wide meander bed, a stepped rapid, a
  vertical drop. This is the mechanism that produces waterfalls, and it is
  purely local: it needs the gradient of one segment.

### 3.7 Kinoshita / sine-generated curves - meanders in closed form

Langbein & Leopold's sine-generated curve models a meander as *heading varying
sinusoidally with arc length*: `ψ(s) = ψ₀ sin(2πs/λ)`. The Kinoshita extension
adds skewness and flatness terms at the third harmonic:

```
ψ(s) = ψ₀ sin(2πs/λ) + ψ₀³ (J_s cos(6πs/λ) − J_f sin(6πs/λ))
```

This is closed-form, local, cheap, and correct-by-construction: it is the
standard idealisation of real meander planform. It is a better source of
windings than midpoint displacement for the *lowland* portion of a river,
because the curvature is physically motivated rather than random - though it
needs a low-slope gate, since real rivers only meander where the gradient is
low.

Recommendation: use **both**. Midpoint bisection at high slope (mountain
sinuosity), sine-generated at low slope (lowland meanders), selected by the same
gradient that selects river type (§3.6). Both are pure functions of the segment
and a seed.

### 3.8 Techniques considered and rejected

| Technique | Why rejected |
|---|---|
| Flow accumulation / D8 over the generated surface | Unbounded. Requires the real surface at unbounded distance. Already ruled out in the brief, and the Streams Reflowing evidence (§3.1) confirms the cost is real, not theoretical. |
| Priority-Flood depression filling (Barnes et al. 2014) | Global algorithm over a bounded DEM. No bounded formulation. Useful only as a *verification* tool - see §9. |
| Stream-power fluvial erosion (Cordonnier et al. 2016) | Iterative simulation over a global grid; converges over many steps. Fundamentally not a pure function of position. Belongs to a precomputed-asset world (§8.2), never to per-chunk generation. |
| Particle-based hydraulic erosion | Same objection, plus non-determinism under parallel scheduling - a direct P1 violation. |
| Partitioned drain-to-source (Streams, TFC-TNG) | Works, but imposes a hard wall at partition boundaries. In an engine whose whole cross-chunk story is "no walls, re-derive world-absolutely", introducing a wall is a regression in kind, not just in quality. |
| Rivers as cross-chunk features (§5 of the design doc) | Already correctly excluded: `((extent + chunk_dim)/cell_dim)²` does not survive a feature spanning hundreds of chunks. Unchanged by anything here. |

---

## 4. Diagnosis: why the current model looks wrong

The brief states the symptom (zero vertical variance, reads as ravines) and the
proximate cause (bed maps potential onto a fixed `[bed_low, bed_high]` band with
no relationship to the terrain). Both are right. The root cause is one level
further down, and naming it correctly is what determines which fix works.

**The model has two elevation fields and privileges the wrong one.**

- The terrain's elevation comes from the biome density chain.
- The river's elevation comes from `elevation(p, potential)`, a linear remap of
  a dedicated noise field into `[14, 46]`.

These are unrelated functions of position. Over a 200-voxel stretch a
0.004-frequency OpenSimplex field varies by well under one period, so the bed is
near-flat; the terrain over the same stretch swings tens of voxels. Subtracting
a near-flat solid from a varying solid produces a trench of varying depth. That
is a ravine, and the design document predicted it verbatim: *"a river cannot
follow terrain it did not shape."*

**Why reconciling the two fields is not enough.** Direction 1 of the brief (bed
from a wired elevation input, topology from the potential) fixes the *height* of
the bed but not the *shape of the ground beside it*. A bed that tracks macro
elevation, cut into terrain whose detail noise still swings ±15 voxels
independently, produces a channel whose banks are sometimes 2 voxels high and
sometimes 25, and whose water surface passes through hillsides. Better than
today. Still not a river.

The clause "a river cannot follow terrain it did not shape" is not a limitation
to be worked around. It is a **specification**. Let the river shape the terrain.

---

## 5. Proposal: hydrological elevation as a WorldGraph channel

### 5.1 The one-sentence version

Move the network to the column domain, make its control function a wired
WorldGraph elevation channel, and have it emit a **reconciled macro elevation**
that the biome density chain builds on - so the valley exists in the terrain
before anything is carved, and the carve only cuts the trench in a valley floor
already at the right height.

### 5.2 Shape

Three nodes, replacing the single density-domain `River`:

**`HydroNetwork`** *(WorldGraph, column domain)*

| Pin | Dir | Type | Meaning |
|---|---|---|---|
| `control` | in | SurfaceField | Macro elevation, world units. **The single source of truth.** |
| `elevation` | out | SurfaceField | `control`, reconciled with the network: valleys carved in, ridges preserved |
| `channel` | out | SurfaceField | Signed distance to the nearest centreline, negative inside the channel |
| `bed` | out | SurfaceField | World-Y of the channel floor where `channel < 0` |
| `water` | out | SurfaceField | World-Y of the water surface where `channel < 0`; `NaN`/sentinel elsewhere |
| `order` | out | SurfaceField | Stream order proxy (§5.6), drives width and material |

Exported as named `GraphOutput`s so Zone and Biome graphs import them by
`GraphRef` - the mechanism §4 of the design document already specifies for
cross-graph dataflow, and which `sample_input_surface` already resolves
pointwise.

**`RiverCarve`** *(BiomeGraph, density domain, stages 3-4)*

Consumes `channel` + `bed` via `GraphRef`, subtracts density above the floor.
Same position in the pipeline the design document already argues for, same
reasoning: before material (5), walkability (6) and slab smoothing (7), so
scatter never anchors over a channel lowered afterwards.

**`RiverFluid`** *(BiomeGraph, stage 9)*

Consumes `channel` + `water`, authors the per-column level through the existing
`FluidOutput` mechanism. Unchanged in kind from today.

**Why the split is the point.** The bed and the terrain agree because they are
the same function evaluated twice, not two functions kept in sync. The brief
correctly rejected "two fields that must agree, kept in sync by an author
remembering to". This is the version where the question does not arise.

### 5.3 The construction

Per chunk, once, at stage 1.

**Step 1 - resolve the network over the chunk's cell window, at `n` levels.**

Level 0: grid of `cell_size`. Each cell gets a jittered key-point from
`world_cell_seed(seed, cx, cz)`. Sample `control` at the key-point via
`sample_column` - **this is the change**: the control function is the authored
elevation channel, evaluated at the node's world position, not a private noise
field. Link each node to the Moore-neighbour of lowest control value.

Levels 1..n-1: grid of `cell_size / 2^k`. Key-points inherited from lower levels
where they coincide (Dendry §4.1.2, Fig. 7). New key-points connect to the
nearest segment of any lower level.

**Step 2 - assign bed elevations, downstream-monotone.**

`bed(q) = control(q)`, then clamped so each node sits at least a minimum fall
below the node upstream of it:

```
S = ρ · (2μ − 1)^(−0.6)          // Flint's law, as Dendry uses it
bed(q) = min(control(q), bed(downstream) + S · |q − downstream|)
```

with `μ` the order proxy (§5.6) and `ρ` an authored scalar. Purely local - it
reads one downstream node, which is already resolved. **Acyclicity remains
structural**: links strictly decrease `control`, so a cycle would need a node
below itself. This property is inherited unchanged from the current
implementation and from Dendry, and its existing test
(`every_link_flows_downhill_so_the_network_cannot_cycle`) continues to hold with
`control` substituted for `potential`.

**Step 3 - bend the segments.**

Local bed gradient `g = Δbed / length` selects the planform:

- `g` above `meander_max_slope`: midpoint bisection, `d` iterations, displacement
  seeded from the segment's endpoint cell coordinates. Mountain sinuosity.
- `g` below it: sine-generated curve, `ψ(s) = ψ₀ sin(2πs/λ)`, amplitude scaled by
  width. Lowland meanders.

Both are pure functions of `(endpoints, seed)`, so both chunks that see a
segment bend it identically.

**Step 4 - reconcile elevation (this is the part that fixes the look).**

Dendry Eq. 4-6, adapted. Place height primitives on a lattice at `cell_size /
2^(n+2)`. For each primitive `q`:

```
h(q) = bed(nearest point on T to q) + tanθ · d(q, T)
```

`tanθ` is the valley wall slope, a smooth function of the nearest bed elevation
(Dendry Eq. 7): wide shallow valleys near the network's minima, narrow steep
ones near its maxima, controlled by an authored exponent `β`.

Then blend the primitive against the authored `control`, weighted by distance to
the network, over a corridor of `valley_width`:

```
w = smoothstep(valley_width, 0, d(p, T))
elevation(p) = lerp(control(p), h̄(p), w)
```

where `h̄(p)` is the inverse-distance-weighted mean over the 5×5 nearest
primitives (Dendry Eq. 5-6, `g(r) = (1 − r²)³`).

Outside the corridor, `elevation == control` exactly, and the author's terrain is
untouched. Inside it, the terrain is a valley whose floor is the bed. **This is
the direct answer to the brief's objection to Direction 2 - "does this
over-constrain terrain authoring?"** No: the authored field remains the control
function, the network is *seeded* by it, and the network only overrides
elevation within a bounded corridor around channels it placed there. Away from
water, authoring is unaffected. The constraint is exactly as wide as the valleys
are, which is the constraint reality imposes too.

**Step 5 - the biome builds on `elevation`.**

The biome density chain takes `elevation` as its base rather than deriving
macro height itself, and adds detail noise on top - **damped by `channel`**, so
detail amplitude falls to near zero on the valley floor. A biome that ignores
`elevation` still generates; it simply gets no rivers, which is the correct
behaviour for a floating-island biome.

**Step 6 - carve, at stage 3-4.**

Unchanged from today in mechanism. `RiverCarve` removes density above `bed`
within the width profile.

### 5.4 Waterfalls, without a simulation

The brief asks for "waterfalls from high sources of water down to sea level".
They fall out of Step 2 plus one rule.

Where `control` drops sharply between adjacent nodes, Flint's minimum-fall clamp
does not bind and the segment carries a large `Δbed` over a short length. Rather
than interpolating that fall linearly (a ramp), quantise it:

- If `|Δbed| > waterfall_threshold`, place the drop at `t*`, derived by hash
  from the segment's endpoint cell coordinates, and make the along-segment bed
  profile a **step** at `t*` rather than a ramp.
- Below the step, widen and deepen locally by `plunge_factor` - a plunge pool,
  which also gives the fluid CA somewhere to land in 0.5.0.

Pure function of the segment. Deterministic. Costs one comparison and one hash
per segment.

**On the half-voxel ceiling.** The brief notes banks cannot be smooth until D1.
For waterfalls this is close to irrelevant, and for channels it is less damaging
than it reads: the failure mode of a terraced bank is a staircase, and the
failure mode the engine actually has today is a vertical-walled trench. A
staircase down a valley wall is a recognisable landform. Where it will show is
on *shallow* channels crossing *flat* ground, and the mitigation is the corridor
blend of Step 4 - a wide, low-slope valley reads as terrain, not as terracing.

### 5.5 The correctness condition, stated so it can be tested

This is the one place the proposal can silently break P1, so it is stated as a
rule rather than left to the margin constant.

**A column's answer must not depend on which chunk asks.** A column's answer
depends on the set of segments within `max_influence` of it. That set is
identical across chunks **iff** every chunk's resolution window includes every
cell whose key-point could produce a segment reaching the chunk.

Dendry gives the neighbourhood recurrence (Eq. 3): `N_k ≥ 2⌈(1 + N_{k−1})/4⌉ +
3`, satisfied by `N_k = 5` at every level. A level-`k` segment therefore reaches
at most `(N_k / 2) · cell_size_k = 2.5 · cell_size / 2^k`. The coarsest level
dominates. Adding the bend displacement and the influence radius:

```
margin = 2.5 · cell_size
       + max_bend_displacement            // Δ · cell_size, or meander amplitude
       + max(valley_width, max_width)
```

The current implementation uses `margin = max_width + cell_size`. **For what it
does today, that is correct and provably sufficient**: a level-0 link reaches
exactly one cell (Moore neighbourhood), a node lies within its own cell, and
`sample()` rejects beyond `width`, so `cell_size + max_width` bounds the reach
exactly. The `2.5` factor is Dendry's `N_k = 5` neighbourhood for levels *above*
zero, where a new key-point connects to a segment up to two cells away.

**The margin becomes insufficient the moment either multi-level refinement or
segment bending lands**, and both are proposed above. It must be widened in the
same change, not after.

**CI test.** The existing
`two_chunks_agree_about_a_column_on_their_shared_border` is the right shape and
should be extended: sweep cell sizes × levels × bend amplitudes × seeds, assert
agreement on every border column, and assert a non-zero count of columns
actually inside a channel (the existing test already guards against comparing
only `None`s - keep that; it is the part most tests of this kind get wrong).

### 5.6 Stream order without unbounded traversal

The design document rejects Strahler order because it needs unbounded upstream
traversal, and substitutes potential-derived width. That substitution is
defensible but it makes width a function of *absolute elevation* rather than of
*accumulated flow*, so two headwaters at the same height are the same width even
if one drains ten times the area.

A bounded alternative: **count inbound links within a fixed radius.** A node's
inbound degree is knowable by testing whether each of the 8 neighbours links to
it - which requires resolving those neighbours, which the chunk already does. A
depth-`d` reverse traversal is bounded by the `(2d+1)²` cell window, so cost is
`O(d²)` cells rather than exponential.

At `d = 3` the window is 7×7 = 49 cells, giving an order proxy that distinguishes
a trunk from a tributary from a headwater - which is all the width function
needs. Deeper is available but not obviously worth it. Combine with Dendry's
level-based multiplier (`μ` ×3 per level) for the cross-scale component.

This is a genuine improvement over both the current model and the design
document's stated reasoning, and it is bounded, so it is permitted.

### 5.7 Lakes, and why they are a feature

Terminus nodes - no lower neighbour - are the network's local minima. Dendry
measures 0.8 % depression surface, so they are rare, but they are not zero, and
zero is not the goal: a real Alps DEM is 3.3 %.

A terminus **above** sea level becomes a lake. The basin already exists, because
Step 4 built a valley converging on that terminus, and `water` is authored at
the terminus elevation. A terminus **below** sea level is the ocean, and stage
9's existing ocean fill handles it with no special case.

This is Streams Reflowing's advertised "lakes and ponds at varying elevations",
obtained as a consequence of the construction rather than as a feature. Its
dryness scaling - "in drier biomes a lake fills to a lower level… a
dryness-scaled share of basins stay dry altogether" - is a one-line addition
once the climate vector is available per column, which it already is
(`ColumnCache::climate`).

---

## 6. Cost

Anchored to the measured figure, not to the frame budget: `perf-baseline.md`
records the **generate job at 8.5-9.0 ms mean, flat across every radius**, with
a global concurrency budget of 20. Generation is off the main thread; the
frontier's 33 ms budget is dominated by `mesh` (42.4 of 59.1 ms in the worst
reading, and `stream`/`upload` in the attributed 2d run). A river pass does not
land on the frame budget directly; it lands on **generation throughput**, and
the honest question is what fraction of 8.75 ms it consumes.

**Target: ≤ 1 ms/chunk (≈ 11 %).** Rationale: streaming fill at r=15 is 177 ms
for 111 chunks with 11× margin against the 2 s bound, so an 11 % generation
regression is comfortably absorbed; a 50 % one is not.

Assumptions: 1,024 columns/chunk; OpenSimplex2 2D ≈ 30 ns; point-segment
distance ≈ 5 ns; control chain depth 5 over a 200-edge graph; `cell_size = 256`,
`n = 3` levels.

The resolution window at level `k` is `chunk_dim + 2 · reach_k · cell_size_k`,
and because `reach_k` is measured *in cells*, the window is roughly **constant in
cell count across levels** — 16 cells at level 0 (reach 1, Moore), ~49 at each
level above (reach 2.5, Dendry `N_k = 5`). This is the single most important
correction to the intuition that finer levels cost `4×` more: they do not, because
their margin shrinks with their cell size. Total ≈ **114 segments per chunk**, not
the several hundred a naive reading suggests.

| Step | Work | Cost |
|---|---|---|
| Level-0 resolve | 16 cells, 36 distinct probes | **77 µs** |
| Level-1 resolve | 49 cells, 81 probes + 49×16 link tests | **178 µs** |
| Level-2 resolve | 49 cells, 81 probes + 49×65 link tests | **190 µs** |
| Segment bending | 114 segments × ~8 subdivisions | **3 µs** |
| Bucket index build | 114 segments × ~9 buckets | **5 µs** |
| Height primitives | 121 primitives (8-voxel lattice) × ~8 candidates | **5 µs** |
| **Per column** | 1,024 × (8 dist + 25 weighted blend) | **92 µs** |
| **Total** | | **≈ 0.55 ms/chunk — 6.3 % of the generate job** |

Within target, with headroom.

**Where the cost actually is, and it is not where it looks.** Of the 445 µs in
the three resolve rows, **426 µs is the linear edge scan** in
`sample_input_surface` (§1.1) — 5 chain levels × 200 edges × 2 ns × 198 probes.
The noise evaluation those scans exist to reach is 32 µs. With a precomputed
`(node, pin) -> edge` map the total falls to **≈ 0.16 ms/chunk, 1.8 %**.

That reframes the work: the expensive part of this proposal is a graph-plumbing
inefficiency that predates rivers, affects every pointwise consumer, and is
cheap to remove. Fix it first and the river model is nearly free.

**The bucket index is worth building but is not load-bearing.** At 114 segments
the naive per-column scan is `1,024 × 114 × 5 ns ≈ 0.58 ms` — over target on its
own, but only by a factor of two, and it degrades gracefully. It is a within-chunk
memoization exactly as the design document sanctions ("memoization is an
optimisation, never a correctness mechanism"): dropping the index must change
speed and nothing else, which is testable and should be tested.

**Tuning dials, measured.** `n = 2` gives **0.36 ms** (4.1 %). `cell_size = 128,
n = 3` gives **0.60 ms** (6.9 %) — cell size barely matters, because the margin
scales with it. Level count is the dial that moves the number.

**Against today's cost.** The current implementation resolves 16 cells (~144
noise samples per chunk) and runs ~16 distance tests per *voxel* — 32,768
`sample()` calls for 1,024 distinct answers (§1.2). Moving to the column domain
recovers a 32× redundancy that is already being paid. The proposal spends the
recovered budget plus roughly 0.4 ms, or plus almost nothing once the edge map
lands.

**These are estimates.** Substep-scoped measurement should replace them before
the design is committed, and the number to instrument is the generate job's
mean, at the frontier, in a release build, read at rest - per the procedure
`perf-baseline.md` establishes and per the lesson it records about debug builds
and wall-clock frame time.

---

## 7. Two cheap wins from Streams Reflowing

Independent of everything above, and worth taking whatever else is decided.

**Three-zone bank materials.** Streams Reflowing skins each channel in bed /
waterline / cut-bank zones with biome-appropriate materials - gravel beds and
grassy banks by default, sand in deserts, raw stone in mountains. The `channel`
signed-distance output makes this a threshold on one value at stage 5, and it is
most of the visual difference between "a trench" and "a river". It costs
essentially nothing and it does not depend on the network being any good.

**Whitewater at drops.** Spray and foam scale with drop height. The waterfall
step of §5.4 already knows the drop height. This is a 0.5.0 particle concern,
but the data to drive it exists as soon as the step exists, and recording that
now avoids re-deriving it later.

---

## 8. What belongs to later versions

### 8.1 Flow (0.5.0)

Generation authors a static level; the CA makes it move. Two things the
proposal owes the CA:

- **Vertical fall.** Flint's-law minimum slope guarantees every segment falls.
  The brief asks whether the channel has enough fall for a CA to flow along or
  whether it pools: with a *minimum* slope enforced per segment, it falls by
  construction. The quantity to check is whether `S · segment_length` exceeds
  one voxel at the tuned `ρ` - if not, adjacent nodes round to the same integer
  Y and the channel is stepped-flat. That is a concrete, checkable inequality
  and it should be asserted in a test rather than discovered in play.
- **A landing.** Plunge pools at knickpoints give falling water somewhere to
  settle instead of spreading across a flat floor.

### 8.2 Precomputed networks (the brief's Direction 3)

Deriving the network offline into a sampled asset would permit genuinely global
algorithms - real flow accumulation, stream-power erosion, guaranteed drainage
to ocean.

**It should not be built.** It does not violate "generation is a pure function
of seed and coords" so much as *relocate* the seed, which the brief already
suspects, and the relocation is expensive: the asset becomes an input the save
format must version, hot-reload must invalidate, and world size must bound. The
proposal reaches ~0.8 % depression surface without any of that. Revisit only if
measurement shows the local model failing at a scale that matters.

### 8.3 The region graph (the brief's Direction 4)

The brief flags this as the option to take seriously if 1 and 2 fail, and asks
whether that leaves 0.4.0 with no rivers.

**It does not need to be taken, and 0.4.0 keeps its rivers.** The region graph's
distinguishing capability is genuine cross-border *dependency* - joining
per-chunk summaries. The proposal needs no dependency: every chunk re-derives
the same network from the same seed and the same control channel. Moving rivers
there would buy nothing the local model lacks, and would give up
order-independence-by-construction for order-independence-by-argument.

The one thing the region graph could add later is **long-range flow
accumulation** - true upstream drainage area rather than the depth-3 proxy of
§5.6 - which would improve width and river-type selection. That is a refinement
of an output, not a replacement of the model, and it can land whenever the
region graph does.

---

## 9. The brief's checklist, answered

**How does a chunk compute its portion without reading a neighbour's output?**
Every input is a pure function of world position: `world_cell_seed(seed, cx, cz)`
for key-point jitter, `noise_seed` (chunk-independent by construction) for the
control chain, and `sample_column(world_x, world_z)` for the control value.
Nothing reads a generated chunk. This is the same construction that makes
`JitteredGrid` seam-continuous and `PoissonDisk` not.

**What is the per-chunk cost, and how does it scale with river length?**
≈0.7 ms/chunk estimated (§6), against a measured 8.75 ms generate job. It does
**not** scale with river length - that is the point of a locally computable
model, and the reason the margin-band cost ceiling that excludes rivers from the
feature model does not apply. It scales with `4^n` in level count and with
`cell_size^-2`, both authored.

**Is the result identical regardless of generation order?** Yes, by
construction, subject to the margin rule of §5.5 - which is the one place it can
break and is therefore stated as a checkable inequality with a CI sweep rather
than as a constant.

**Does the bed follow the terrain, and where does that agreement come from?**
The relation is inverted: the terrain follows the bed, within `valley_width`.
The agreement comes from both being derived from one wired `control` input in
one node. There is no second field.

**Does the channel have enough vertical fall for a CA to flow along later, or
does it pool?** It falls by construction (Flint's-law minimum slope, §5.3 Step
2). The quantity that must be checked is `S · segment_length > 1 voxel` at the
tuned `ρ`; below that, adjacent nodes round to the same integer Y and the
channel is flat in practice despite falling in principle. Assert it (§8.1).

**What happens at a biome boundary?** Nothing. The network lives in the
**WorldGraph**, above zones and biomes, so a river crosses a biome border
without any discontinuity in bed, width or planform - only materials change.
This is strictly better than the design document's current placement of
`ZoneGraph.rivers`, and it is an argument for the move independent of everything
else here.

**What happens at a chunk-Y boundary?** Nothing. The network is 2D over XZ and
Y-invariant; `bed` and `water` are world-Y values, so a chunk at any Y computes
the same floor for the same column. The one Y-dependent obligation is that the
`FluidOutput` level must be authored by whichever chunk contains `water_y`,
which is a stage-9 detail, not a seam.

**Verification tooling.** Dendry's own evaluation method is directly reusable
and worth building as a diagnostic: render a large region's `elevation` to a
heightmap, run Priority-Flood (Barnes et al. 2014) to count depressions and
depression surface area, and compare against the paper's table. A regression in
drainage quality then has a number, which is the same standard `perf-baseline.md`
holds performance to - *a budget nobody can observe a violation of is not a
budget*.

---

## 10. Risks, and what would falsify this

| Risk | Signal | Response |
|---|---|---|
| Cost estimate is wrong by >2× | Generate job mean rises above ~10.5 ms | Land the edge map (§1.1) — worth 3.5× on its own; then drop to `n = 2`; then drop the 5×5 primitive blend for the single-nearest form, accepting a crease at valley medial axes |
| Valley corridor over-constrains authoring in practice | Authors report terrain "fighting" the rivers | `valley_width` is per-node; a biome that wants no valleys sets it to zero and gets today's behaviour |
| Terraced banks read badly at half-voxel granularity | Visual review | Widen the corridor and lower `tanθ`; the failure mode is a staircase on a *steep* wall, and shallow walls do not terrace |
| Depression count is worse than Dendry's 0.8 % because `control` is a noise field with many local minima | Priority-Flood diagnostic | Dendry states the cause explicitly: *"If [the control function] is convex, it will avoid depression formation."* Bias the control chain toward convexity, or accept the depressions as lakes (§5.7) |
| The margin rule is violated at some parameter combination | Seam test sweep | The sweep is the mitigation; it must run in CI, not by hand |

**What would falsify the recommendation:** a measurement showing the level-2
resolve cost dominating, or a visual review finding that the corridor blend
reads as an obvious tube of smoothed ground running through otherwise detailed
terrain. The second is the more likely of the two and the harder to fix; it is
worth prototyping Step 4 alone, against existing terrain, before building the
rest.

---

## 11. Recommended sequence

Ordered so each step is independently valuable and independently revertible.

**Prerequisites, neither of which is about rivers.**

0a. **Precompute the `(node, pin) -> edge` map** (§1.1). Worth 3.5× on the
    proposal's cost, benefits every pointwise consumer, and is a contained
    change. Do this first regardless of what else is decided.

0b. **Promote arithmetic and curve nodes into the column domain** (§1.1). The
    column domain currently supports four node kinds and no arithmetic, so a
    control elevation cannot be authored at all today. Port from the density
    domain; the ops are elementwise.

**Then, ordered so each step is independently valuable and revertible.**

1. **Move `River` to the column domain** (§1.2). Pure win: same output, 32× less
   work, and it is the precondition for everything after.
2. **Wire `control`** as a graph input, replacing the private potential field.
   This alone is the brief's Direction 1, and is worth landing to see how far it
   gets on its own.
3. **Build Step 4, the elevation reconciliation** (§5.3), behind a
   `valley_width = 0` default so it is opt-in. **Prototype and review this before
   proceeding** — it is the step the whole proposal rests on and the one most
   likely to disappoint visually. Everything before it is cheap and safe;
   everything after it is wasted if this looks wrong.
4. **Add levels 1-2 and segment bending** (§5.3 Steps 1, 3), **widening the
   margin in the same change** (§5.5) and extending the seam sweep to cover
   levels and bend amplitude. The margin is correct today and stops being correct
   here; the two must not be separated.
5. **Add the bucket index** (§6) if measurement asks for it.
6. **Add Flint's-law slope, knickpoints, plunge pools** (§5.3 Step 2, §5.4).
   Waterfalls.
7. **Add the bounded order proxy** (§5.6) and three-zone bank materials (§7).
8. **Measure**, and record against the pre-content baseline the way
   `perf-baseline.md` §0.4.0 sets up. Add the Priority-Flood drainage diagnostic
   (§9) so the *look* has a number too.

Prerequisites and steps 1-2 are small and land in 0.4.0 comfortably. Step 3 is
the decision point. Steps 4-7 are 0.4.0 if step 3 succeeds and 0.5.0 if it needs
rework.

---

## Sources

- [Why Are Rivers So Complicated? / Procedural Unbounded River Generation](https://alcatrazescapee.com/rivers/) — AlcatrazEscapee (TerraFirmaCraft). The context-free / bounded / unbounded taxonomy, and the comparative evaluation of shipped Minecraft implementations. Also at [gist](https://gist.github.com/alcatrazEscapee/8bd54572c8d3fe070d1ed92704dd6aed).
- [Dendry: A Procedural Model for Dendritic Patterns](https://www.mgaillard.fr/content/publications/pdfs/Gaillard19I3D.pdf) — Gaillard, Benes, Guérin, Galin, Rohmer, Cani. I3D 2019. The construction, the terrain-height derivation (Eq. 4-7), the neighbourhood recurrence (Eq. 3), and the drainage-quality measurements (Table 1). [DOI](https://dl.acm.org/doi/10.1145/3306131.3317020)
- [Terrain Generation Using Procedural Models Based on Hydrology](https://www.cs.purdue.edu/cgvlab/www/resources/papers/Genevaux-ACM_Trans_Graph-2013-Terrain_Generation_Using_Procedural_Models_Based_on_Hydrology.pdf) — Génevaux, Galin, Guérin, Peytavie, Benes. SIGGRAPH 2013. River-patch terrain synthesis; the model Dendry's height derivation is adapted from.
- [Procedural Riverscapes](https://perso.liris.cnrs.fr/eric.galin/Articles/2019-riverscapes.pdf) — Peytavie, Dupont, Guérin, Cortial, Benes, Gain, Galin. Pacific Graphics 2019. Compactly supported elevation modifiers; river type by slope and Strahler order.
- [Streams Reflowing](https://modrinth.com/mod/streams-reflowing) — closed source; its own documentation is the evidence for the cost of terrain-query-based river placement.
- [Streams (delvr)](https://github.com/delvr/Streams) and [delvr's worldgen notes](https://gist.github.com/delvr/bfac43cd48675ec74b905ffe640a6a11) — shadow chunk generation for height queries; partition-bounded drain-to-source.
- [TerraFirmaCraft (TFC-TNG)](https://github.com/TerraFirmaCraft/TerraFirmaCraft) — watershed partitioning and midpoint-bisection segment refinement.
- Langbein & Leopold (1966), sine-generated curve; Kinoshita extension — [overview](https://www.flow3d.com/wp-content/uploads/2014/08/Hydrodynamics-in-Kinoshita-generated-meandering-bends-Importance-for-river-planform-evolution.pdf).
- Barnes, Lehman & Mulla (2014), Priority-Flood — the depression-counting method Dendry's evaluation uses, recommended here as a diagnostic.
- Cordonnier et al. (2016), Large Scale Terrain Generation from Tectonic Uplift and Fluvial Erosion — surveyed and rejected (§3.8).

### Engine sources

- `docs/river-research-brief.md` — the brief this answers
- `docs/engine-design.md` §5 (Generation Pipeline, Cross-chunk feature generation, River networks), §7 (Water System), §12 (region graph, performance budgets)
- `docs/perf-baseline.md` §0.3.0 (job system: generate 8.5-9.0 ms), §0.4.0 (generation frontier baseline)
- `crates/nodegraph-eval/src/river.rs`, `column_eval.rs`, `world_eval.rs`, `eval.rs`, `context.rs`
- `crates/nodegraph-ir/src/node.rs` (`RiverParams`, `WorldOutputParams`, `NodeDescriptor`)
