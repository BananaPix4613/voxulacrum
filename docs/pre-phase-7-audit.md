# Pre-Phase-7 Audit — Fluid Simulation (state reference)

Written reference for **Phase 7 (Fluid Simulation, Standard scope)**. Companion to
`post-phase-6-audit.md`. Substep 0: no code. Records the exact current state of the
fluid scaffolding, `sea_level`, the render slot, scheduling, the node-kind patterns,
and the override/persistence paths — and flags the gaps between this prompt's stated
assumptions and what the codebase actually contains.

**Headline:** three of the prompt's "authoritative decisions" don't match the code
and need resolving before/within Substep 1 (see §11). None are blockers; all are
"the groundwork the prompt assumed exists actually has to be built."

---

## 1. `FluidLayer` scaffolding — current state

Location: `crates/voxulacrum-app/src/world/layers.rs` (NOT `voxel-core`; sidecar
layers live in the app crate per that module's charter). The whole file carries a
file-level `#![allow(dead_code)] // Phase 3 scaffolding`.

The types already match design doc §7 closely:

```rust
pub struct FluidId(pub u16);                       // layers.rs:51

pub struct FluidCell {                             // layers.rs:155
    pub fluid_id: FluidId,
    pub mass: u16,                                  // 0..=65535, 65535 = full
    pub flags: u8,
}
impl FluidCell {
    pub const FLAG_SETTLED: u8 = 1 << 0;
    pub const FLAG_FALLING: u8 = 1 << 1;
    pub const FLAG_SOURCE:  u8 = 1 << 2;
}

pub enum FluidFillMode { #[default] Empty, Submerged(FluidId) }   // layers.rs:175

pub struct FluidLayer {                            // layers.rs:185
    pub fill_mode: FluidFillMode,
    pub cells: HashMap<LocalPos, FluidCell>,
    pub active: HashSet<LocalPos>,
}
```

- **Constructed:** only via `Default` (Empty fill, empty maps). `LoadedChunk::new`
  builds `Chunk` with `fluids: FluidLayer::default()`. No generation writes it.
- **Consumed:** nowhere. No renderer, no simulation, no generator reads it.
- **`u16` mass + `Submerged` fast path are already the design.** Phase 7 populates
  and consumes these types; it does not redesign them.

> **Divergence from the prompt (§11-B).** The prompt's Substep 4 proposes adding
> `FluidCell.state: FluidCellState { Settled | Active }` and
> `FluidLayer.active_set: HashSet<LocalPos>`. The code already expresses both:
> settled-ness via `FLAG_SETTLED` on `flags`, and the active set as
> `FluidLayer.active`. Recommendation: use the **existing** shape (it is the design
> doc's shape) rather than adding a parallel `state` enum + second set. Decision to
> confirm at Substep 4.

## 2. `sea_level` — DOES NOT EXIST (highest-impact gap)

The prompt states as an authoritative decision: *"`sea_level` comes from `WorldGraph`.
Already established by Phase 4's cross-graph dataflow."* **This is not true in the
code.**

- The only occurrence of `sea_level` anywhere is a **test-fixture string** in
  `nodegraph-ir/src/boundary.rs:169` (`{"name":"sea_level","ty":"Scalar"}` — an
  unrelated JSON-parse test).
- `assets/graphs/world.graph.json` exposes exactly one output: a `GraphOutput` named
  `"climate"` (drives Zone biome selection). There is **no** `sea_level` output,
  node, manifest field, or constant.
- Nothing reads a sea level. The composite (`WorldEvaluator::evaluate_chunk`) has no
  notion of a global water plane.

**Consequence for Substep 1:** ocean fill needs a sea level to exist first. That is
net-new work, and *how* it's sourced is a design decision (see §11-A). The Phase-4/6
audits' claim that `sea_level` "exists but wasn't wired" over-stated it — it was
never created.

## 3. `BiomeGraph.fluid_provider` — named only, unimplemented

Design doc §7 lists `BiomeGraph.fluid_provider` as a generation source, and the
Phase-4 audit named it. In the code there is **no** `NodeKind` for it, no boundary
port, no evaluator arm, and no biome graph authors it. It is a design-doc label with
zero implementation — exactly the "named in Phase 4 but no producer/consumer" state
the prompt describes. Substep 3 builds it from scratch.

The model to follow is Phase 6's `DensityOutput`: a **terminal** node with typed
inputs whose result the composite harvests into a chunk layer
(`CachedOutput::BiomeLayer { density, material }`). A `FluidProvider` terminal would
harvest into `FluidLayer.cells`. See §7.

## 4. Render slot — a no-op `WaterPass` stub already exists (more built than the prompt assumes)

The prompt says "Water has never rendered" (true) and to "reactivate the retired pass
from Phase 1." In fact the water render path is **already wired end-to-end and
idling**, not deleted:

- `rendering/water_pass.rs`: `WaterPass { chunk_meshes: HashMap<IVec3, ChunkWaterMesh> }`
  — a Phase-1 no-op. `add_chunk_water` is `{}`; `chunk_meshes` stays empty.
- `PipelineRegistry` builds a **`water_pipeline`** from **`shaders/water.wgsl`**
  (both exist today) and hot-reloads it.
- `MainScenePassNode` already draws water, gated `if !self.hide_water &&
  !self.water_pass.chunk_meshes.is_empty()`, in draw order:
  **terrain → cap → detail-paint → scatter → water → debug lines.** So water is
  positioned **after** the foliage passes already.
- `ecs/systems.rs` calls `water_pass.remove_chunk_water` / `add_chunk_water` on
  unload / mesh-complete, and there's a "Hide water" debug toggle (`hide_water`).

> **Resolves a prompt "planning question" (Substep 2).** "before/after foliage?" is
> already answered: **after**. And `water.wgsl` + `water_pipeline` already exist, so
> Substep 2 is "feed real geometry into the existing pass + rewrite the shader for the
> new data model," not "add a pass from nothing." The old `ChunkWaterMesh` is a plain
> vertex/index buffer per chunk — a fine vehicle for both the ocean surface plane and
> explicit-cell surfaces.

## 5. Scheduling — a `Simulation` stage exists; a fixed tick rate does not

- `ecs/schedule.rs` defines `FrameStage { Input, Simulation, Meshing, UniformWrite,
  Render, PostFrame }`, ordered, one pass **per frame**.
- `SimulationManager` (`simulation/manager.rs`) is **camera + cloud-shadow + time-of-
  day** state, updated each frame in the Simulation stage. It is *not* a fluid or
  fixed-timestep manager — the name is about environment/camera simulation.
- There is **no fixed-timestep accumulator** anywhere. Meshing and generation
  schedule work onto `Arc<rayon::ThreadPool>` (the shared pool), driven per-frame /
  by streaming events, not by a clock.

**Consequence for Substep 7:** the 8-10 Hz fluid tick needs a new accumulator
(collect frame `dt`, step the sim a bounded number of fixed ticks). The `FluidSystem`
slots into `FrameStage::Simulation`, but the fixed-rate stepping is net-new. The
rayon pool for active-chunk work distribution already exists and is the right vehicle.

## 6. `ChunkOverrides.fluid_diffs` — persists; apply-on-load unwired

- `ChunkOverrides.fluid_diffs: HashMap<LocalPos, FluidCell>` exists (`overrides.rs:38`),
  reserved/empty in Phase 3.
- **Persistence round-trips it:** the blob format (v6) writes/reads it — `persistence.rs`
  section "6. fluid_diffs" (write ~245-256, read ~322-331), with a test inserting
  `FluidCell { fluid_id: FluidId(1), mass: 65535, flags: FLAG_SOURCE }` (~903).
- **Apply-on-load is NOT wired:** nothing reads `fluid_diffs` back onto a chunk's
  `FluidLayer` after generation. This is the "data present, apply path deferred"
  Phase-3 pattern. Substep 7 wires the apply path.

> **Clarification the prompt conflates (§11-C).** Only *overrides* (`fluid_diffs`)
> persist. The **generated** `FluidLayer` (ocean `fill_mode`, authored pond `cells`)
> is generated content and is **not serialized** — the blob is voxels + tags +
> overrides. Generated fluid **re-derives deterministically on load**. So Substep 1's
> "save/load preserves fill state" is satisfied by *deterministic regeneration*, not
> by round-tripping `fill_mode`. The Substep-1 "verify persistence with a test" should
> be read as "verify the generated fill is deterministic across regenerations,"
> matching how voxels/foliage already work.

## 7. Node-kind patterns — how a graph output reaches a chunk layer

Two established patterns in the Biome graph:

- **Density-contributing nodes** (Noise, Subtract, Layer, LibraryRef…) feed a value
  chain evaluated per column/voxel.
- **Terminal outputs** harvested into chunk data: `DensityOutput` (density + material
  inputs, no outputs) → the composite reads `CachedOutput::BiomeLayer { density,
  material }` out of the eval cache and fuses it. `GraphOutput` marks a cross-graph
  boundary value.

`FluidProvider` fits the **terminal** pattern: a node with a placement/density input
whose harvested result the composite turns into settled `FluidLayer.cells` at the
biome's Y range. `WorldEvaluator::evaluate_chunk` is the integration point (it already
composites density + material + border blend; fluids compose after voxel content is
final — ocean fill first, then biome-authored cells on top).

## 8. Generation pipeline integration point

`WorldGenerator::generate_chunk` (`world_generator.rs:147`) returns:

```rust
pub struct GeneratedChunk {                        // world_generator.rs:280
    pub storage: ChunkStorage,
    pub tags: ChunkTags,
    pub detail_layers: DetailLayers,
    pub scatter: ScatterStore,
    // NO fluids field yet
}
```

`World::generate` / streaming copy these onto `chunk.data.{tags,detail_layers,
scatter_instances}`. **Substep 1 adds `fluids: FluidLayer` to `GeneratedChunk`**,
computes it in `generate_chunk` (after `storage` is materialized — ocean fill needs
final voxel content), and copies it onto `chunk.data.fluids` in both the initial-fill
(`world/mod.rs`) and streaming (`world/streaming.rs`) apply sites.

## 9. Determinism testing patterns to reuse

- Phase 6's `analyze_biome_borders` is **chunk-position-independent** (samples world
  coords, not chunk-relative) and is tested with `border_scan_crosses_chunk_boundary_
  consistently` against a brute-force scan — the template for cross-chunk fluid tests.
- `generate_chunk_is_deterministic` (`world_generator.rs:473`) re-generates a chunk
  and asserts equality — the template for "ocean fill / pond is deterministic."
- Simulation determinism (Substep 5/6) needs explicit tests: same initial state +
  same disturbance + same tick count ⇒ identical final `cells`, independent of thread
  ordering (the two-phase read/apply split is what makes this hold; test it).

## 10. Adjacent facts worth not tripping over

- `Voxel::FLAG_WATER_LOGGED` (`voxel-core/src/voxel.rs:61`) is a **voxel-level** wetness
  bit on solid voxels — a *different* concept from the fluid layer. Don't conflate a
  water-logged solid voxel with a `FluidCell`.
- No `WATER` `FluidId` constant exists yet; Substep 1 defines one (likely
  `FluidId(0)`), extensible for later fluids.
- `LocalPos` is the per-cell key used by `cells`/`active`/`fluid_diffs`; it already
  has `from_index`/`to_index` helpers used by persistence.

## 11. Audit gaps → decisions to surface (before writing the relevant substep)

- **A. How is `sea_level` sourced? (Substep 1, blocking that substep).** It doesn't
  exist. Options: (i) a `WorldGraph` `GraphOutput`-style scalar the composite reads via
  the Phase-6 cross-graph path (most "designed," most work — sea level becomes a
  graph-authored value); (ii) a `world.manifest.json` constant read at generator
  construction (simplest, still data-driven, no per-column eval); (iii) a
  `WorldParam`-style sidecar. Recommendation leans (ii) for Phase 7 (a global Y is a
  scalar, not a per-column field) with a note that promoting it to a graph output later
  is cheap. **Will ask at Substep 1.**
- **B. Reuse `flags`/`active` or add `state`/`active_set`? (Substep 4).** Recommend
  reusing the existing design-doc shape (`FLAG_SETTLED` + `FluidLayer.active`). Avoids
  a parallel representation. **Will confirm at Substep 4.**
- **C. "Persistence of fill state" = deterministic regeneration, not serialization
  (Substep 1).** Framed above; the Substep-1 test verifies regeneration determinism,
  not a `fill_mode` round-trip. No decision needed, just the corrected framing.
- **D. `FluidProvider` output shape (Substep 3).** Terminal harvested into
  `FluidLayer.cells` (recommended, mirrors `DensityOutput`) vs a per-voxel value in the
  density chain. **Will decide at Substep 3.**
- **E. `#![allow(dead_code)]` on `layers.rs`** should be narrowed as Phase 7 consumes
  the fluid types (so newly-dead decal/lighting bits stay covered but consumed fluid
  types drop out of the blanket). Tidy opportunistically.

## 12. Deferred items inherited from Phase 6 (stay deferred)

- Multi-distance slab staircasing; cross-chunk slab-smoothing seams. Unchanged by
  Phase 7. `traversal_smoothing_distance` remains on/off.
- Phase 7's own out-of-scope set (rivers, player-water interaction, waterfall carving,
  non-water fluids, waves/reflections) per the prompt.
