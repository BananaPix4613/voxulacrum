# Macro terrain: continental elevation, and what rivers need from it

Companion to `river-generation-research.md`. That document proposed wiring the
river network's control function to "the WorldGraph's macro elevation channel".
This one establishes that **no such channel exists, cannot currently be built,
and could not be consumed if it were** — and proposes the architecture that
fixes all three.

**The headline.** This is not a companion feature to river generation. It is the
prerequisite the river proposal assumed was already there. Rivers that run from a
highland lake to the sea require the world to *have* highlands and a sea as facts
about elevation, and today it has neither: it has three biomes that each invent
their own absolute height from private noise, selected by a single unrelated
noise field. There is nothing for a river to flow down.

**Three hard blockers, all in graph plumbing rather than in terrain design.**

| # | Blocker | Consequence |
|---|---|---|
| **B1** | The column domain implements exactly four node kinds — `SurfaceNoise`, `WorldOutput`, `ZoneOutput`, `GraphOutput` — and no arithmetic | The WorldGraph cannot compute an elevation in world-Y units. It can emit raw noise in `[-1, 1]` and nothing else. |
| **B2** | `Evaluator::fill_node` returns `UnresolvedGraphRef` for `GraphRef`, and `Evaluator::evaluate` has no skip for it | A BiomeGraph containing a `GraphRef` **fails to evaluate at all**. Biomes cannot read World or Zone channels, at all, today. |
| **B3** | Coercion is `Scalar → Density` and `Curve → Scalar` only | Even with B2 fixed, a per-column `SurfaceField` cannot enter a density chain. |

B2 is the surprising one. The design document §4 describes cross-graph dataflow
as the mechanism "that makes the hierarchy actually compose", and describes a
BiomeGraph reading "whatever inputs it declares by name from its containing
ZoneGraph and grandparent WorldGraph". The column evaluator implements this. The
density evaluator does not, and no shipped biome graph contains a `GraphRef`, so
nothing has discovered it. **The composition story is half-built and the built
half is the half biomes don't use.**

Everything else in this document is downstream of fixing those three.

---

## 1. What ships today, exactly

### 1.1 The world graph is one noise node

`assets/graphs/world.graph.json`, in full:

```
[1] SurfaceNoise { seed: 10, frequency: 0.001, octaves: 3, FBm }
[2] WorldOutput  { zone_bands: [0.1], zone_ids: [0, 1] }
[3] GraphOutput  { name: "climate" }
edges: 1 -> 2.0,  1 -> 3.0
```

One field, at frequency 0.001 (≈1,000-voxel wavelength), doing double duty as
zone selector and as the sole exported channel. It is a proto-continentalness in
scale, and it is not used for height by anything.

### 1.2 Zones select biomes from that one field

```
zone.graph.json            GraphRef(World) -> ZoneOutput{ bands [0.0, 0.25], ids [0,1,0] }
zone_highlands.graph.json  GraphRef(World) -> ZoneOutput{ bands [0.0],       ids [1,2]   }
```

Note what "highlands" means here: a zone that *contains different biomes*. It is
not higher. Nothing in the pipeline makes it higher. The name describes an
intent the data does not implement.

### 1.3 Each biome invents its own absolute height

All three follow the identical shape — `Simplex2D → Remap → Add(ridged detail) →
Subtract(WorldAxis Y)`:

| Biome | Base band (world Y) | Detail | Effective range |
|---|---|---|---|
| `meadow` | Remap → **12 … 52** | ±8 ridged, masked by a 0.0008 field | 4 … 60 |
| `rocky` | Remap → **20 … 80** | ±16 ridged | 4 … 96 |
| `ocean` | Remap → **4 … 18** | none | 4 … 18 |

`sea_level = 24`.

**Three consequences, all load-bearing.**

- **Height is discontinuous at every biome border.** Meadow can be at 12 where
  rocky is at 80, three voxels apart. Fade blending softens the transition over
  the border band; it does not make the two agree, because there is nothing for
  them to agree *about*.
- **The zone/biome selector and the height are independent.** A column's biome
  comes from `climate`; its height comes from that biome's private noise. A
  mountain biome placed at a coastline is at 80 and the ocean beside it is at 12,
  and the only thing that prevents a cliff wall is the fade band.
- **There is no world elevation.** Not a weak one — none. The quantity the river
  proposal's `control` pin needs does not exist as a value anywhere in the
  pipeline.

### 1.4 The world is 128 voxels tall

`StreamingParams::default()` gives `min_chunk_y: 0, max_chunk_y: 4` — four
32-voxel layers, world Y `0..127`, `sea_level = 24`. The tallest thing the shipped
content can produce is rocky at 96. Roughly 30 voxels of headroom above the
highest terrain, and 24 below sea level for ocean floor.

For reference: Minecraft's world is 384 voxels tall by default (`−64 … 320`) with
sea level at 63, Tectonic's mountains "approach the build limit", and its
`Increased Height` option raises generation to y640. Tectonic's own
`deep_ocean_depth` default of `−0.45` is described in its config as roughly 58
blocks below sea level — **half the current world's total height.**

### 1.5 The node vocabulary

47 `NodeKind` variants. Relevant to terrain:

- **Noise:** `Perlin2D/3D`, `Simplex2D/3D`, `SurfaceNoise`, `DomainWarp`
- **Math:** `Add`, `Subtract`, `Multiply`, `Min`, `Max`, `Clamp`, `Lerp`, `Remap`, `Constant`
- **Curves:** `Threshold`, `CurveMapper { stops: Vec<(f32, f32)> }`
- **Density:** `Union`, `Intersect`, `DensitySubtract`, `Mix`, `Mask`, `River`
- **Position:** `WorldPos`, `WorldAxis{X|Y|Z}`, `YBand`
- **Params:** `BiomeParam { name, default }` reading a per-biome sidecar
- **Cross-graph:** `GraphRef { target }`, `GraphOutput { name }`, `LibraryRef`

`PinType` in code: `Scalar`, `Density`, `SurfaceField`, `Material`,
`FluidProvider`, `Positions`, `Assignments`, `Curve`. (The design doc §4 lists
`Vec3`, `BiomeId`, `ZoneId`, `PlacementMask`, `SpeciesWeights`, `PaintOutput`,
`ScatterOutput`, `Terrain` as well — recorded drift, not relevant here.)

**There is no `Abs` node.** Worth noting early because the standard ridge fold
needs one.

---

## 2. What Tectonic and Minecraft 1.18 actually do

The mechanism matters more than the mod. Tectonic is a datapack: it ships *data*
for a pipeline vanilla provides. Understanding that pipeline is understanding
what to build.

### 2.1 The inversion

Vanilla pre-1.18 and voxulacrum today share a shape: **biome first, height
second.** Pick a biome, ask it how tall it is. This produces exactly the seams
described in §1.3, which is why pre-1.18 Minecraft had abrupt biome cliffs.

1.18 inverted it: **height first, biome second.**

```
continentalness, erosion, weirdness, temperature, humidity   (low-frequency 2D noises)
ridges     = 1 − |3·|weirdness| − 2|                          (folded weirdness)

offset     = spline(continentalness, erosion, ridges)         → base surface height
factor     = spline(continentalness, erosion, ridges)         → vertical squash/stretch
jaggedness = spline(continentalness, erosion, ridges)         → roughness amplitude

depth          = y_gradient(y) + offset
initial_density = depth · factor      (+ jaggedness · high-freq noise)
final_density   = initial_density + 3D detail noise
```

Terrain height is a **function of the same low-frequency fields that select the
biome**. The biome does not decide the height; both are read off the same
climate vector. A mountain biome is placed where the terrain is already high,
because "high" and "mountain biome" are two readings of one erosion value. This
is what removes the seam — not blending, but a shared cause.

The wiki states the intent directly: the continents and erosion functions "can be
integrated with the Biome Source to help place biomes in a way that conforms to
the terrain (Mountain biomes on elevated terrain, Plains biomes on flatter
terrain)".

### 2.2 The spline is the whole authoring surface

`offset`, `factor` and `jaggedness` are **splines**: piecewise curves whose knot
*values may themselves be splines over another input*. A three-input shaper is a
spline over continentalness whose knots are splines over erosion whose knots are
splines over ridges.

This is the entire terrain authoring surface of modern Minecraft. Tectonic's
"massive mountain ranges", "deeper oceans", "plateaus", "canyons" and "dunes" are
not new algorithms — they are different knot values in these three splines, plus
biome-level surface treatment. That is the single most encouraging fact in this
document: **the mechanism is small, and it is data.**

Voxulacrum has `CurveMapper` with `(x, y)` stops — a 1D spline, piecewise linear.
It lacks nesting. §4 addresses this.

### 2.3 `depth × factor`, and why it beats a height comparison

Today's biomes compute `height − Y`: a signed distance to a surface, which is a
perfectly good density but has one shape and no control over it. The
`depth × factor` form separates two questions:

- `depth` — how far below the nominal surface am I? A pure function of Y and the
  offset spline.
- `factor` — how *fast* does density change with depth here?

Low factor stretches terrain vertically (towering, overhang-prone mountains);
high factor compresses it (flat plains, sharp plateau edges). It is the mechanism
behind "the same noise reads as plains here and cliffs there", and it costs one
multiply.

Both operands are already expressible in the density domain (`WorldAxis(Y)`,
`Subtract`, `Multiply`, `Remap`, `Clamp`). **The missing half is the world-level
`offset` and `factor` values, not the density arithmetic** — which is B1 again.

### 2.4 Tectonic's parameter vocabulary

Its config is a well-tested list of the dials this kind of terrain needs, and is
worth stealing wholesale as a naming and scoping guide:

| Group | Parameters |
|---|---|
| Global | `vertical_scale` (stretches terrain above sea level), `increased_height`, `ultrasmooth` |
| Continents | `ocean_offset` (skews toward/away from ocean), `continents_scale`, `erosion_scale`, `ridge_scale` |
| Oceans | `ocean_depth`, `deep_ocean_depth` — expressed in *normalized* units: "0 is at sea level, −0.5 is 64 blocks below" |
| Biomes | `temperature_{multiplier,scale,offset}`, `vegetation_{multiplier,scale,offset}` |
| Caves | `depth_cutoff_start` / `depth_cutoff_size` — caves gated by the **depth** channel, not by absolute Y |

Two of these are directly instructive:

- **Ocean depth in normalized units.** Depth is expressed as a fraction of world
  height, not an absolute Y. That makes the whole terrain rescalable by changing
  one number — which is exactly what `vertical_scale` and `increased_height` do
  without re-authoring anything.
- **Caves gated by depth, not Y.** Voxulacrum's cave subtraction (stage 4) and
  its `YBand` node are absolute-Y. With a depth channel, "caves stop 12 blocks
  below the surface" becomes expressible, and it follows the terrain up a
  mountain instead of slicing through it at a fixed altitude.

### 2.5 Underground rivers — Tectonic's answer to a problem this engine will hit

> "In mountainous terrain that is too towering to allow regular river generation,
> terrain gets carved for rivers to continue underground. These link right up to
> the regular rivers, so there's no interruption in exploring along rivers."

This is worth recording now because the river proposal will hit exactly this
case. A network routed on macro elevation will occasionally want to cross a ridge
that the detail layer made taller than the bed. The options are: reroute
(unbounded), dam the river (ugly), cut a canyon through the ridge (the current
model's failure mode, and the reason it "looks wrong"), or **tunnel**.

Tunnelling is the cheapest and it is already expressible: the river's carve at
stages 3-4 removes density above `bed` within the width profile. If the carve is
clamped to a *maximum height above the bed* rather than "everything above the
floor", a channel passing under a ridge automatically becomes a tunnel and
re-emerges on the far side. One clamp. It is the single highest
quality-per-line item in either document, and it should be recorded as a
follow-up to the river work regardless of what happens here.

---

## 3. Why the current shape cannot produce a source-to-sea river

Restating the user's diagnosis precisely, because the precision determines the
fix.

The river model's flow topology is steepest descent on a control field, and its
termination condition is "reach a node below sea level, where stage 9's ocean
fill takes over". For that to describe a river running from a highland lake to
the sea, three things must be true:

1. **The control field must be in world-Y units**, so that "below sea level" is a
   comparison and not a coincidence. Today the only exported channel is raw noise
   in `[-1, 1]` and `sea_level` is 24. *(Blocked by B1.)*
2. **The terrain must actually be low where the control field is low**, or the
   river's mouth is at Y = 8 in a place the biome decided is a 60-voxel hill.
   Today height and the exported channel are unrelated. *(Blocked by B2 + B3.)*
3. **There must be enough vertical range for a source to be meaningfully above
   the sea.** With `sea_level = 24` and terrain topping out near 96, the largest
   possible fall is ~72 voxels — and the meadow biome, which covers most of the
   world, tops out at 60. A river with 36 voxels of total fall spread over
   thousands of voxels of run has a gradient of roughly 1 %, which after
   rounding to integer Y is a flat channel with occasional steps.

Point 3 is the one that makes the vertical-range question *not* cosmetic. **The
current world is too short for a river to fall.** That is an arithmetic fact
about 24 and 96, not an aesthetic preference.

---

## 4. The gaps: what the nodes allow, and what must be added

Direct answer to "do the nodes we currently have allow this kind of generation".

**No — but the shortfall is smaller than it looks, and most of it is porting
rather than designing.**

### 4.1 Blockers (nothing works without these)

**B1 — column-domain arithmetic.** Port `Add`, `Subtract`, `Multiply`, `Min`,
`Max`, `Clamp`, `Lerp`, `Remap`, `Constant`, `CurveMapper`, `Threshold` into
`ColumnEvaluator::fill_node` and `sample_column`. The density-domain
implementations are elementwise and transfer directly; `ColumnField` is the same
shape as `ScalarField` minus a dimension. This is the same prerequisite
`river-generation-research.md` §1.1 identifies, and it is the single item both
documents block on.

**B2 — `GraphRef` in the density evaluator.** Two changes, mirroring what
`ColumnEvaluator` already does:
- `Evaluator::evaluate` must skip `GraphRef` nodes in the topological fill, as
  the column evaluator does ("GraphRef reads upstream graphs on demand at input
  resolution; it has no single cached output").
- `Evaluator::input_scalar` and `sample_input` must resolve a `GraphRef` source
  against `UpstreamGraphs`, as `ColumnEvaluator::input_surface` does.

Until this lands, no biome graph may contain a `GraphRef` node at all — it does
not degrade, it errors the whole chunk.

**B3 — `SurfaceField → Density`.** A per-column value entering a 3D chain is a
broadcast along Y. Two options:

- *Implicit coercion.* Add `SurfaceField → Density` to the coercion rules.
  Cheapest, but the design document says "No other implicit coercions.
  Strictness is a feature", and a silent Y-broadcast is exactly the kind of thing
  that rule exists to prevent.
- *An explicit node.* `SurfaceToDensity` / `Broadcast` — one input `SurfaceField`,
  one output `Density`, semantics stated on the node. **Recommended.** It keeps
  the strictness rule intact, it is self-documenting in the editor, and it gives
  a place to hang the eventual variants (broadcast, or broadcast-with-gradient).

### 4.2 New nodes

| Node | Domain | Why | Composable today? |
|---|---|---|---|
| `Abs` | both | Ridge fold `1 − \|3\|w\| − 2\|`; also the standard billow/ridged transform | Only as `Max(x, Multiply(x, −1))` — three nodes and a constant for one operation used twice per shaper |
| `Spline` | column | Nested multi-input shaper (§2.2). Knots whose values are themselves splines | **Yes, in principle**: a 2-input spline is `Mix(CurveMapper_a(u), CurveMapper_b(u), v)`. A 3-input spline with 5 knots per level is ~30 `CurveMapper` nodes and ~25 `Mix` nodes. Technically expressible, practically unauthorable. **Build the node.** |
| `SurfaceToDensity` | density | B3 | No |
| `WorldParam` | any | World-level named scalars from a manifest sidecar, mirroring `BiomeParam`. Tectonic's whole config is this. | No — `BiomeParam` reads the *biome* sidecar only |

Everything else Tectonic-shaped is already expressible:

- **`depth × factor`** — `WorldAxis(Y)`, `Subtract`, `Multiply`. ✔
- **Jaggedness (amplitude-masked detail)** — already done in
  `biome_meadow.graph.json`, nodes 10-12: a low-frequency field remapped to
  `[0, 1]` and multiplied into the ridged detail. The pattern is in the shipped
  content; it just needs to be driven by a world channel instead of a private
  noise. ✔
- **Ridge fold** — `Abs`, `Multiply`, `Subtract`, `Constant`. ✔ once `Abs` exists.
- **Depth-gated caves** — `Remap` + `Clamp` on a depth channel, feeding the
  existing stage-4 subtraction. ✔ once the channel exists.

### 4.3 New parameters

**`WorldParams` sidecar on the manifest**, mirroring the per-biome `params` map
that already ships:

```json
{
  "sea_level": 24,
  "min_y": 0,
  "max_y": 384,
  "params": {
    "continents_scale": 1.0,
    "erosion_scale": 1.0,
    "ridge_scale": 1.0,
    "ocean_offset": -0.5,
    "vertical_scale": 1.0,
    "ocean_depth": -0.2,
    "deep_ocean_depth": -0.45
  }
}
```

Read by a `WorldParam` node exactly as `BiomeParam` reads its sidecar. This is
what makes the terrain tunable without editing graph topology, and it is the
thing that lets a "Tectonic preset" and a "gentle preset" be two manifests over
one graph.

**World height moves to the manifest — and this is a determinism defect, not a
convenience.** `min_chunk_y` / `max_chunk_y` currently live in
`StreamingParams`, alongside `load_margin` and `max_mesh_per_frame`. Those are
view properties. World height is not: §12's determinism rule is *same seed + same
graphs + same region parameters + same coordinates → bit-identical output*, and
a world generated with `max_chunk_y = 4` and reopened with `max_chunk_y = 12` is
a different world at every column where the extra range changes what a
normalized-depth terrain function returns. `sea_level` is already on the manifest
for exactly this reason (§7, "a single scalar constant across the world"). World
height is the same kind of constant and belongs beside it.

This is cheap now and data-corrupting later, which is the same shape as the
registry-identity defect §12 already records.

---

## 5. Proposed WorldGraph architecture

### 5.1 Channels

The WorldGraph grows from one exported channel to a small, named set:

| Channel | Units | Consumers |
|---|---|---|
| `continentalness` | `[-1, 1]` | zone selection, offset/factor splines, ocean determination |
| `erosion` | `[-1, 1]` | offset/factor splines, biome selection (mountains vs plains) |
| `weirdness` → `ridges` | `[-1, 1]` | splines, river corridor placement |
| `temperature`, `humidity` | `[-1, 1]` | biome selection, foliage, Streams-Reflowing-style lake dryness |
| **`elevation`** | **world Y** | **the river node's `control`**; biome density base; the chunk-skip bound (§6) |
| `factor` | `[0, ∞)` | biome density vertical scaling |
| `depth_scale` | normalized | depth-gated caves |

`elevation` is the one that matters. It is `spline(continentalness, erosion,
ridges) · vertical_scale + sea_level`, in world Y, exported as a `GraphOutput`.

### 5.2 What biomes become

A biome stops answering "how tall am I" and starts answering "what does my
surface look like, and what is it made of". Concretely, every biome graph
becomes:

```
GraphRef(World).elevation ──> SurfaceToDensity ──┐
                                                  ├─> Subtract(WorldAxis Y) ─> ...
detail_noise · GraphRef(World).jaggedness ───────┘
```

instead of `Simplex2D → Remap(12, 52)`. The biome contributes **detail
amplitude, surface layering, material, and foliage**. It no longer owns absolute
height.

**The seam at biome borders disappears by construction**, not by blending: two
adjacent biomes read the same `elevation` and differ only in the detail they add
on top. This is the same argument the river document makes about bed/terrain
agreement, and for the same reason — one source of truth, evaluated twice.

Fade blending stays; it now blends *detail and material*, which is what it is
good at, rather than being asked to reconcile a 68-voxel height difference.

### 5.3 What zones become

Zones keep their job — partitioning the world into biome sets — but now select on
a *meaningful* vector. "Highlands" becomes a zone selected where `erosion` is low
and `continentalness` is high, and its biomes are high **because the world is
high there**, not because their `Remap` says so. The zone graphs need almost no
change; the `WorldOutput` bands just read a better channel.

---

## 6. Vertical range: the real cost, and what pays for it

### 6.1 The naive scaling

Extrapolating from the `perf-baseline.md` r=15 row (1,220 resident chunks at 4
Y-layers) and the r=25 memory row (29.9 MB CPU / 490.5 MB GPU at 4,388 chunks):

| Y-layers | World height | Resident @ r=15 | CPU | GPU (naive) | Fill |
|---|---|---|---|---|---|
| **4** (current) | 128 | 1,220 | 8.3 MB | 136 MB | 177 ms |
| 6 | 192 | 1,830 | 12.5 MB | 205 MB | 266 ms |
| **8** | **256** | **2,440** | **16.6 MB** | **273 MB** | **354 ms** |
| **12** | **384** | **3,660** | **24.9 MB** | **409 MB** | **531 ms** |
| 16 | 512 | 4,880 | 33.3 MB | 546 MB | 708 ms |

**The measured ceiling is 4,388 resident chunks** — the r=25 diagnostic row,
which recorded 31.4 ms CPU worst against a 16.6 ms budget and is *declared
unsupported by measurement*. Sixteen Y-layers at r=15 puts 4,880 chunks resident,
past a configuration already measured as over budget. Twelve layers (3,660) sits
under it with margin.

**So the supported vertical range is 8-12 chunk layers — 256 to 384 voxels — and
384 is Tectonic's default world height.** That is a comfortable coincidence, and
it means the target is reachable without a streaming redesign.

### 6.2 The GPU column is pessimistic, and the CPU column is too

Both scale sublinearly in reality, for a reason the codebase already implements.

`ChunkBuffer` has a `Uniform` storage kind — O(1) for a chunk of a single value.
The baseline's memory line reads `29.9 MB CPU + 490.5 MB GPU (2,396 of 4,388
chunks uniform — 55 %)`, which on the most likely reading is
`StorageKind::Uniform` and not the per-chunk render uniform. *Confirm which
before relying on it* — but the structural argument does not depend on the
figure: every Y-layer added above the terrain is all-air and every layer added
below is all-stone, and both collapse to `Uniform` by construction.

GPU cost scales with *exposed surface area*, which does not change when empty
layers are added above it — an all-air chunk emits zero quads. So the CPU and GPU
columns above should grow by a fraction of what the table shows, and the growth
should be in bookkeeping — `HashMap` entries, streaming sort, job submissions —
rather than in data. That prediction is checkable at step 9 and is the thing to
check first.

### 6.3 The cost that *is* real, and the finding that matters

**`evaluate_chunk` has no vertical early-out.** It unconditionally runs
`resolve_columns` (World + Zone evaluation over 1,024 columns) and then
`composite_terrain` (a full 32³ density evaluation per present biome), regardless
of where the chunk sits relative to the terrain.

An all-air chunk 300 voxels above the surface costs the same ~8.75 ms as the
chunk containing the surface. **Tripling the vertical range triples generation
throughput cost, and roughly all of the added work produces `Uniform(air)`.** The
fill column in §6.1 is the honest one; the memory columns are not.

**The fix is the elevation channel, which is why this section belongs in this
document.** Once the WorldGraph exports `elevation` in world Y:

```
bound = [ min(elevation over chunk footprint) − max_carve_depth,
          max(elevation over chunk footprint) + max_detail_amplitude ]

if chunk.y_span entirely above bound.max  ->  Uniform(air),   graph never evaluated
if chunk.y_span entirely below bound.min  ->  Uniform(stone), graph never evaluated
```

The footprint bound costs a handful of `sample_column` probes on a coarse grid
over the chunk's XZ — the same probes the river network already pays for, and
shareable with them. It is a **bounded** method in the taxonomy of the river
document: pure function of world position, no neighbour reads, order-independent.

**This is the reciprocity that makes the whole proposal affordable.** The
Tectonic-style architecture is not merely compatible with a taller world; it is
what pays for one. Without a world-level elevation there is no cheap way to know
a chunk is empty, and a 384-voxel world costs 3× to generate. With it, the added
layers cost close to nothing, because the layers that were added are precisely
the ones the bound rejects.

**The safety condition, stated so it is not discovered.** The bound must be
*conservative* — too tight and terrain is clipped flat at a chunk boundary, which
is the worst class of bug this engine can ship (silent, position-dependent, and
invisible until someone flies up there). `max_detail_amplitude` is currently
implicit in each biome's `Remap` ranges and nowhere declared. It must become
**declared per-biome metadata**, validated against the graph rather than inferred
from it, and the CI generation-verification sweep must include a chunk whose
terrain sits within one voxel of a chunk-Y boundary.

### 6.4 What a taller world buys the rivers

With `min_y = 0`, `max_y = 384`, `sea_level` moved to ~96:

- Mountain sources at 300+ against a sea at 96 gives **~200 voxels of fall**,
  against ~36 today for the biome that covers most of the world.
- At the river proposal's Flint's-law minimum slope, that is enough fall for the
  `S · segment_length > 1 voxel` test in §8.1 of that document to pass by a wide
  margin — which is the difference between a river that flows and one that pools.
- Waterfalls become possible as *landforms* rather than as one-voxel steps.
  Tectonic's mountains approaching the build limit is the same design decision.
- Deep oceans below sea level give the network somewhere to terminate that is not
  a coastline, and give `ocean_depth` / `deep_ocean_depth` something to mean.

---

## 7. How this changes the river proposal

`river-generation-research.md` stands, with three amendments.

**Amendment 1 — the `control` pin now has a source.** §5.2 of that document
specifies a `HydroNetwork` node whose `control` input is "macro elevation, world
units, the single source of truth". That is `GraphRef(World).elevation`. The
river node lives in the WorldGraph, downstream of the elevation spline and
upstream of everything else, and its `elevation` output *replaces* the raw spline
output for all downstream consumers. The chain is:

```
climate noises -> splines -> raw_elevation -> HydroNetwork -> elevation (exported)
                                                           -> channel, bed, water, order
```

**Amendment 2 — the prerequisite list is shared.** That document's prerequisites
0a (edge map) and 0b (column-domain arithmetic) are the same two items as this
document's B1 and the performance fix. Neither project pays for them twice. B2
(`GraphRef` in the density evaluator) and B3 (`SurfaceToDensity`) are new here
and are needed by the river carve as well, since `RiverCarve` is a density-domain
node consuming per-column `channel` and `bed`. **The river proposal as written
cannot be implemented today for exactly the same reason this one cannot.**

**Amendment 3 — tunnels replace canyons.** §2.5 above. Clamping the carve to a
maximum height above the bed turns the "river cuts a canyon through a hill" failure
mode into "river passes under the hill". Add it to that document's step 6.

---

## 8. Sequencing

Merged with the river document's sequence, since the prerequisites are shared.

**Phase A — plumbing (blocks everything, contains no terrain design).**

1. Precompute the `(node, pin) → edge` map. *(river 0a; 3.5× on pointwise cost)*
2. Port arithmetic + `CurveMapper` into the column domain. *(B1 / river 0b)*
3. Resolve `GraphRef` in the density evaluator. *(B2)*
4. Add `SurfaceToDensity` and `Abs`. *(B3)*
5. Move `min_y` / `max_y` to the manifest; add the `WorldParams` sidecar and the
   `WorldParam` node.

Phase A ships no visible change. It is the whole reason both projects are stuck,
and it is mostly porting.

**Phase B — macro terrain.**

6. Build the `Spline` node.
7. Rewrite `world.graph.json`: five climate noises, ridge fold,
   offset/factor/jaggedness splines, `elevation` exported.
8. Rewrite the three biome graphs to consume `elevation` and contribute detail
   only. **This is the visible moment** — biome-border height seams should
   disappear here, and that is the checkable success criterion.
9. Raise the world to 8 Y-layers, `sea_level` to ~64. Measure.
10. Add the chunk-skip bound (§6.3) with declared per-biome detail amplitudes and
    a chunk-Y-boundary CI case. Measure again — this is what should pay for
    step 9.
11. Raise to 12 layers, `sea_level` ~96, if step 10 holds.

**Phase C — rivers**, per `river-generation-research.md` steps 1-8, now with a
real control channel, real fall, and real oceans to drain into.

Phases A and B are worth doing on their own merits: the height seam at biome
borders is a live visual defect, and cross-graph composition being half-built is
a live architectural one. Rivers are the forcing function, not the only payoff.

---

## 9. Risks

| Risk | Signal | Response |
|---|---|---|
| **Chunk-skip bound clips terrain** | A flat ceiling or floor at a chunk-Y multiple, in one region only | Declared per-biome amplitude, validated against the graph; CI case with terrain within one voxel of a chunk-Y boundary. This is the highest-severity item here — the failure is silent |
| Generation cost scales with vertical range before the skip lands | Generate job mean flat, but 3× as many jobs | Sequence step 10 before step 11; do not raise to 12 layers until the skip is measured |
| Splines over-constrain biome authoring | Authors want a biome that is tall regardless of `elevation` | Biomes may still add unbounded detail; a floating-island biome ignores `elevation` and works as today. The constraint is a default, not a wall |
| `Spline` node authoring is unusable in the editor | Nobody edits the terrain after it ships | Nested splines need a dedicated editor widget, not a generic param list. Budget for it, or accept 2-input splines composed from `CurveMapper` + `Mix` initially |
| Rewriting the three biome graphs breaks existing saves | Any world generated before Phase B | Terrain change is a full regeneration by definition. Take it while there are three biomes and no players, not later |
| The `Uniform` fast path does not hold for the added layers | CPU memory scales linearly with Y-layers rather than sublinearly | Verify the assumption by measuring at 8 layers before committing to 12 — the 55 % uniform figure is at 4 layers, and its composition, not just its value, is what matters |

**What would falsify the proposal:** if measurement at 8 Y-layers shows resident
chunk bookkeeping (not data) dominating — streaming sort, `HashMap` pressure, job
submission — then the vertical range is bounded by the scheduler rather than by
generation, and the chunk-skip bound will not rescue it. That would push toward
vertical streaming culling (only load Y-layers near the terrain surface), which
is a larger change and belongs to the region graph era.

---

## Sources

- [Tectonic](https://modrinth.com/datapack/tectonic) — the datapack; feature list, continent and mountain scale, underground rivers. MIT, [source](https://github.com/Apollounknowndev/tectonic).
- [Tectonic config reference](https://github.com/Apollounknowndev/tectonic/wiki/Config) — the parameter vocabulary quoted in §2.4, including normalized ocean depths and depth-gated caves.
- [Noise router — Minecraft Wiki](https://minecraft.wiki/w/Noise_router) — `continents` / `erosion` / `depth` / `ridges` channel definitions, `final_density`, and the statement that these channels are integrated with the biome source so biomes conform to terrain.
- [Noise settings — Minecraft Wiki](https://minecraft.wiki/w/Noise_settings) — the offset / factor / jaggedness spline pipeline.
- [World generation — Minecraft Wiki](https://minecraft.wiki/w/World_generation) — "three 2D noise maps mapped using splines to calculate the height offset and a vertical stretch factor".
- `docs/river-generation-research.md` — the companion proposal this one unblocks.

### Engine sources

- `assets/graphs/` — `world.graph.json`, `zone.graph.json`, `zone_highlands.graph.json`, `biome_{meadow,rocky,ocean}.graph.json`, `world.manifest.json`
- `crates/nodegraph-eval/src/column_eval.rs` — `fill_node` and `sample_column`, four supported kinds (B1)
- `crates/nodegraph-eval/src/eval.rs` — `evaluate`, `fill_node`, `input_scalar`; `GraphRef → UnresolvedGraphRef` (B2)
- `crates/nodegraph-eval/src/world_eval.rs` — `evaluate_chunk`, no vertical early-out (§6.3)
- `crates/nodegraph-ir/src/pin.rs`, `node.rs` — `PinType`, the 47 `NodeKind` variants, `BiomeParamParams`, `CurveMapperParams`
- `crates/voxel-core/src/buffer.rs` — `Inner::Uniform` fast path (§6.2)
- `crates/voxulacrum-app/src/params.rs` — `StreamingParams::{min,max}_chunk_y`, default `0..4` (§4.3)
- `docs/engine-design.md` §4 (pin types, coercion, cross-graph dataflow), §5 (stage order), §7 (`sea_level` on the manifest), §12 (determinism)
- `docs/perf-baseline.md` — resident counts, memory split, uniform-chunk fraction, the r=25 unsupported row
