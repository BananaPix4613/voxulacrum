# Pre-Phase-4 State Audit

**Status:** Substep 0 deliverable. Reference-only — no code changes.
**Date:** 2026-06-17.
**Supersedes as starting reference:** `docs/post-phase-3-audit.md` (close-of-Phase-3 snapshot).
**Authoritative targets:** `docs/engine-design.md` v1.2 (§4 five-graph hierarchy + `PinType` enum + LibraryGraph mechanics + standard libraries, §5 generation pipeline + `ColumnCache` + fade blending + traversal smoothing distance), the Phase 4 system prompt.

This audit grounds Substeps 1–14 in the *actual* state of the code as read on
2026-06-17, and surfaces the gaps where the current implementation diverges from
the Phase 4 target. It is a factual snapshot plus an explicit set of decisions
the user must make before code planning begins.

---

## §0. Scope of Phase 4 (recap, for grounding only)

Phase 4 builds the **five-graph hierarchy** (`WorldGraph` singleton, `ZoneGraph`,
`BiomeGraph`, `DetailGraph` type-only scaffold, `LibraryGraph` instanced/shared)
and **multi-biome terrain** with computational cross-chunk boundary blending. The
14 ordered substeps: (1) graph-type model in the IR, (2) `PinType` expansion +
coercions, (3) `LibraryRef` node + `LibraryGraphRegistry` + cycle detection, (4)
multi-graph evaluator with `ColumnCache`, (5) functional WorldGraph, (6)
functional ZoneGraph, (7) functional BiomeGraph density, (8) computational
cross-chunk neighbor access, (9) per-column fade blending via `BiomeBorderFade`
standard library, (10) meaningful `ChunkTags`, (11) tag-driven targeted
invalidation, (12) `traversal_smoothing_distance` as a per-biome parameter
driving distance-N slab smoothing, (13) migrate `default_biome.graph.json` into a
hierarchical two-biome starter world, (14) editor multi-graph selector. Determinism
is mandatory: every new generation pass / evaluator path needs a determinism test.
`voxel_format_version` bumps to wipe old saves. Out of scope: foliage placement
(Phase 5), fluid sim, decals, cross-chunk for walkability/slab, major editor UI,
AI/movement, networking/audio/new materials.

---

## §1. Current nodegraph-ir surface — ONE flat graph, no type concept

### 1.1 `Graph` (`nodegraph-ir/src/graph.rs`)

There is exactly **one** graph type and it has **no notion of a graph kind**:

```
pub struct Graph {
    nodes: SlotMap<NodeId, Node>,
    edges: Vec<Edge>,
}
```

Serde + `Default`. Public API: `add_node`, `add_node_at`, `remove_node`,
`connect` (validates pin types/range/single-input), `disconnect`,
`validate() -> Vec<Diagnostic>`, `has_errors`, `topological_order() -> Result<…>`,
`to_json`/`from_json`. `validate()` checks edge integrity, ≤1 edge per input,
required-inputs-connected, and acyclicity (`detect_cycle`). **This is the entry
point Substep 1/3 must extend** for per-graph-type root-output rules and library
cycle detection.

### 1.2 `PinType` (`nodegraph-ir/src/pin.rs`) — missing 4 design-doc variants

Current enum (full): `Scalar, Density, Material, Positions, Assignments, Curve,
Vec3, BiomeId, Terrain`.

Design-doc §4 enum additionally requires: **`SurfaceField`, `FluidProvider`,
`ZoneId`** (and the design-doc placement/output family `ScatterPoints`,
`PlacementMask`, `SpeciesWeights`, `PaintOutput`, `ScatterOutput`). Note the
current code uses **`Positions`** where the design doc says **`ScatterPoints`** —
a naming reconciliation is owed.

Coercion today: `can_coerce(from, to) = matches!((from, to), (Scalar, Density) | (Curve, Scalar))`.
`is_compatible(output, input)` = exact match OR coercible. Substep 2 expands both
the variant set and the coercion table (design doc allows only `Scalar→Density`
and `Curve→Scalar`).

### 1.3 `NodeKind` / descriptors (`nodegraph-ir/src/node.rs`, 854 lines)

`NodeId` is a slotmap key. `NodeCategory` enum: Source/Math/Curves/Domain/Density/
Material/Positions/Scanners/Props/Biome/Output. `NodeKind` (serde `tag="type"`) is
a flat list of ~35 variants: Perlin2D/3D, Simplex2D/3D, Constant, WorldPos,
WorldAxis, Add, Multiply, Subtract, Min, Max, Clamp, Lerp, Remap, Threshold,
CurveMapper, DomainWarp, Union, Intersect, DensitySubtract, Mix, Mask,
ConstantMaterial, Layer, Queue, TerrainOutput, BuildTerrain, JitteredGrid,
PoissonDisk, FindFlat, PlaceTree, PlacePrefab, Output. Each carries a static
`descriptor()` (display name, category, color, input/output `PinSpec`s).

**There is no `LibraryRef` node and no graph-type-scoped root-output node.** The
single root today is `TerrainOutput`. Phase 4 adds a `LibraryRef` node (Substep 3)
and per-graph-type root outputs (Substep 1/5/6/7).

### 1.4 `lib.rs`

`#![warn(missing_docs)]`, re-exports. No `GraphKind`, no `LibraryGraph`, no
registry type. All net-new in Phase 4.

---

## §2. Current nodegraph-eval surface — single-graph, NodeId-keyed CSE cache

### 2.1 `Evaluator` (`nodegraph-eval/src/eval.rs`, 558 lines)

```
pub struct Evaluator<'g> {
    graph: &'g Graph,
    ctx: EvalContext,
    cache: EvalCache,
}
```

`new(graph, ctx)`, `evaluate()` (validates, then fills every node in topological
order), `fill_node(id)` (the big per-`NodeKind` match producing `CachedOutput`),
typed input resolvers (`input_scalar/vec3/material/terrain/positions`), and
`sample_density` (a pull-based cross-validation path). **Strictly single-graph:
the evaluator references one `&Graph` and cannot resolve a `LibraryRef` or descend
into a child graph.** Substep 4 makes this multi-graph.

### 2.2 `EvalContext` (`nodegraph-eval/src/context.rs`)

```
pub struct EvalContext { world_seed: u64, chunk: IVec3 }
```

`world_pos(x,y,z)`, `noise_seed(node_local_seed)` (deliberately **excludes** chunk
coords for seam continuity), `scatter_seed` (folds chunk coords), and
`world_cell_seed(seed, cell_x, cell_z)` (world-absolute, seamless). Phase 4 must
extend this to carry the **graph registry** (so library refs resolve) and the
per-column inputs feeding `ColumnCache`. The seam-safe seeding discipline is
exactly the rule design-doc §5 fade blending restates ("RNG seeded from
`(world_seed, biome_id, x, z, purpose)`, never chunk coords alone").

### 2.3 `EvalCache` / `CachedOutput` (`nodegraph-eval/src/cache.rs`)

`CachedOutput` enum: `Scalar/Vec3/Material/Terrain/Positions`, each `Arc`-wrapped.
`EvalCache { outputs: HashMap<NodeId, CachedOutput> }` — the **chunk-scoped CSE
cache**, keyed on `NodeId`. There is **no per-column cache**. Substep 4 adds the
design-doc `ColumnCache` `{ climate, zone_id, zone_border_distance, biome_id,
biome_border_distance, neighbor_biomes: SmallVec<[(BiomeId,f32);4]> }` **alongside**
this CSE cache (they serve different scopes: CSE is per-node-per-chunk, ColumnCache
is per-(x,z)-column).

---

## §3. Graph file format & the default graph

- `assets/graphs/default_biome.graph.json` — a **flat** terrain graph, 11 nodes
  (index 0 is the null slotmap slot): Simplex2D→Remap (base height 12–52),
  Simplex2D(Ridged)→Remap (detail ±8), Add, WorldAxis(Y), Subtract (height − Y =
  density), Layer (bands `[[6,1],[3,3],[1,8]]` fill 2), BuildTerrain, TerrainOutput.
- Sibling `assets/graphs/example.graph.json` also exists.
- Serialization is whatever `Graph::to_json`/`from_json` (serde) emits — a single
  `{nodes, edges}` object with **no graph-kind tag, no hierarchy, no library
  references**.

**Phase 4 impact:** Substep 13 migrates this single file into a hierarchical
**two-biome starter world** (a WorldGraph + ZoneGraph + ≥2 BiomeGraph instances,
possibly LibraryGraph instances). The on-disk schema must gain a graph-kind
discriminator and a way to express the registry of instanced graphs. This is the
format-level reason `voxel_format_version` and any graph-asset versioning need a
coordinated bump.

---

## §4. WorldGenerator flow — the single shared generation hook

`WorldGenerator { graph: Arc<Graph>, world_seed: u64, terrain_node: NodeId, storage_boundary: StorageBoundary }`
(`world/world_generator.rs`).

- `new(graph, world_seed)` locates the **single** `TerrainOutput` node (errors if
  absent), logs validation diagnostics.
- `generate_chunk_storage(position) -> ChunkStorage` is the **single shared path
  for all generation** (startup fill, streaming, regen):
  `Evaluator::new(&graph, EvalContext::new(seed, position))` → `evaluate()` (air on
  error) → harvest `terrain_node` → `storage_boundary.materialize(terrain)` →
  `slab_smoothing::smooth_slabs(&mut storage)` → return.
- `default_graph_path()`, `load_default(params)` (re-reads the file each call so
  regen picks up disk edits).

**Phase 4 impact:** the multi-graph evaluator (Substep 4) and functional
WorldGraph/ZoneGraph/BiomeGraph (Substeps 5–7) all land **behind this single hook**,
exactly as `materialize` and `smooth_slabs` already do. `WorldGenerator` will hold
the graph **registry** rather than a single `Arc<Graph>` + single `terrain_node`.
The per-biome `traversal_smoothing_distance` (Substep 12) flows from the resolved
biome into the `smooth_slabs` call here.

---

## §5. ChunkTags producers & consumers

`world/tags.rs`: `ZoneId(u16)`, `BiomeId(u16)`, `LibraryGraphId(u32)`;
`ChunkTags { zone: ZoneId, biomes: SmallVec<[BiomeId;4]>, library_refs: SmallVec<[LibraryGraphId;8]> }`
(`PartialEq`/`Default`); `single_biome(zone, biome)` builds the one-zone-one-biome
shape.

- **Producer (today):** `Chunk::new` calls `ChunkTags::single_biome(ZoneId(0), BiomeId(0))` —
  *every* chunk is trivially tagged Zone0/Biome0, `library_refs` always empty.
- **Consumer (today):** **none.** A workspace grep shows nothing reads `ChunkTags`
  to make a decision; it is inert scaffolding.

**Phase 4 impact:** Substep 10 makes tags *meaningful* — the evaluator records the
actual zone, the set of biomes blended into the chunk, and the library graphs it
referenced. Substep 11 adds the **first consumer**: tag-driven targeted
invalidation (§6).

---

## §6. The invalidation path — currently invalidate-ALL

The live regen flow (`world/regen.rs`, `WorldRegenCoordinator { manager: WorldManager }`):

1. `tick(...)` polls `poll_regeneration` (swaps `world.chunks`, adopts
   `world.generator`, marks **all** chunks dirty, rebuilds vegetation/streaming,
   clears caches/saved chunks).
2. Handles `ui_state.regenerate_requested`.
3. Drains `ui_state.pending_graph` (live editor edits / hot-reload) **only while
   idle** (latest-wins): builds `WorldGenerator::new(graph, seed)` and calls
   `start_regeneration` with **ALL loaded chunk positions**.

`WorldManager::start_regeneration(params, generator, positions)` fans out one
`pool.spawn` over the supplied positions; `poll_regeneration()` collects the result
map.

**This is the invalidate-ALL behavior Substep 11 replaces** with tag-driven
targeting per design-doc §4: WorldGraph edit → all chunks; ZoneGraph[Z] → zone Z +
its fade buffer; BiomeGraph[B] → biome B + fade buffer; LibraryGraph[L] → only
chunks whose `library_refs` contain L. The targeting consumes the meaningful
`ChunkTags` from Substep 10 and narrows the `positions` set handed to
`start_regeneration`.

---

## §7. Editor state & hot-reload

### 7.1 `EditorState` (`nodegraph-editor/src/state.rs`) — single flat graph

Source of truth is `snarl: Snarl<NodeKind>`. Tracks `dirty`, `modified`, `undo`/
`redo` stacks (`UNDO_DEPTH = 100`), a `toast`, a monotonic `revision` (bumped on
any edit **or** selection change), and `selected_node: Option<SnarlNodeId>`.
Key API: `from_graph`, `build_graph()`, `build_graph_with_selection() -> (Graph, Option<NodeId>)`,
`revision()`, `consume_dirty()`, `is_modified()`/`mark_saved()`, `undo()`/`redo()`,
`show(ui)`. **There is exactly one active graph — no multi-graph selection.**
Substep 14 adds a multi-graph selector (single panel, no `egui_dock`).

### 7.2 Editor → regen bridge (`ecs/systems.rs`)

- **Embedded editor:** after drawing (`systems.rs:679`), if
  `egui.editor.consume_dirty()` then `ui_state.pending_graph = Some(egui.editor.build_graph())`.
- **Hot-reload** (`graph_hot_reload_system`, `systems.rs:179`): polls
  `GraphWatcherRes`; for the active `default_biome.graph.json` only, **skips while
  `egui.editor.is_modified()`** (don't clobber unsaved edits), else re-reads the
  file, rebuilds `egui.editor = EditorState::from_graph(&graph)`, and routes via the
  same `ui_state.pending_graph = Some(graph)` transport.
- **Field probe** (`ui/field_probe.rs`): reads `egui.editor.revision()` and
  `build_graph_with_selection()` to evaluate/display the author-selected node.

### 7.3 Watcher (`nodegraph-hotreload/src/watcher.rs`)

`GraphWatcher` wraps a `notify` recommended watcher over a directory
(non-recursive); `poll_changes() -> Vec<PathBuf>` drains events and returns
deduplicated created/modified `*.json` paths. `GraphWatcherRes` is the bevy_ecs
Resource wrapper (panics on watcher-create failure).

**Phase 4 impact:** the single-`default_biome.graph.json` assumption (one active
graph, one watched file) breaks once the world is a registry of graph instances.
Substep 13/14 must decide how the editor selects among graphs and how hot-reload
maps a changed file back to a graph instance + its targeted invalidation set. The
`pending_graph: Option<Graph>` transport (`ui/panels.rs`) is currently a single
graph and will need to carry *which* graph changed.

---

## §8. Slab smoothing — the distance-1 hardcoding location

`world/slab_smoothing.rs`: `smooth_slabs(storage: &mut ChunkStorage)` computes
`WalkabilityMask::from_storage`, then demotes each walkable surface `Cube` that has
a **lower walkable horizontal neighbor** to `SlabBottom`. Intra-chunk only (OOB
neighbors count as non-walkable). Has 5 tests incl. `smoothing_is_deterministic`.

**Distance-1 is hardcoded in `has_lower_walkable_neighbor`** (the `let ny = y - 1;`
single-cell-below check, ~lines 62–78). **This is the exact site Substep 12
generalizes** to distance-N, driven by a per-biome `traversal_smoothing_distance`
parameter (design-doc §5 table: 0 = none, 1 = single-voxel, 2–4 short landings,
5+ long landings). The parameter must flow from the resolved biome (Substep 7/10)
through `generate_chunk_storage` (§4) into this call. Per the Phase 4 prompt,
distance-N slab smoothing remains **intra-chunk** (cross-chunk is explicitly out of
scope here).

---

## §9. Decisions required before Substep 1 code planning

**(a) Graph-type representation: `GraphKind` enum vs. `GraphType` trait (drives
Substep 1 — CENTRAL).** The Substep 1 prompt calls out this choice directly. The
five graph types differ only in (i) which root-output node(s) they require and (ii)
which `PinType`s their roots emit. Options:
  - **`GraphKind` enum field on `Graph`** (`Graph { kind: GraphKind, nodes, edges }`):
    one concrete type, a `match` in `validate()` for per-kind root rules. Minimal
    plumbing, serde-trivial, but root-output rules live in conditionals.
  - **`GraphType` trait** (typed wrappers around a shared `Graph` body): compile-time
    distinction, but more boilerplate and a harder serde story (tagged
    deserialization into a trait object / enum-of-wrappers anyway).
  The enum is the lighter fit given serde + a single `validate()` extension point,
  but this is the user's call and frames every subsequent substep.

**(b) `Positions` → `ScatterPoints` rename (drives Substep 2).** The IR uses
`PinType::Positions` and `NodeCategory::Positions`; the design doc uses
`ScatterPoints`. Reconcile now (rename to the design-doc term) or defer and keep the
divergence documented? Naming-after-the-general-mechanism favors adopting the
design-doc vocabulary, but it touches node descriptors and the JSON assets.

**(c) DetailGraph depth (drives Substep 1).** The prompt scopes DetailGraph as a
**type-only scaffold, empty**. Confirm it ships as a registered `GraphKind` variant
with root-output *validation rules* but **no functional nodes/evaluation** this
phase — i.e. an author can create one but it produces nothing.

**(d) LibraryGraph identity & sharing model (drives Substep 3).** `LibraryGraphId`
is a `u32` today and tags carry `library_refs`. Confirm the registry shape: a
`LibraryGraphRegistry` mapping `LibraryGraphId → Graph`, instanced/shared by
reference (multiple parents cite the same id), with cycle detection at
connect/validate time. The exact storage (where the registry lives — on
`WorldGenerator`, in `EvalContext`, or both) is a Substep 3/4 concern but the
ownership question should be settled.

**(e) Graph-asset on-disk schema + version bump coordination (drives Substep 13,
touches Substep 1).** Moving from one flat `default_biome.graph.json` to a
registry-of-typed-graphs world changes the asset schema. Confirm whether Phase 4
introduces a single "world bundle" file vs. multiple per-graph files keyed by id,
and that `voxel_format_version` bumps to wipe saves at the same time the graph
schema changes (so a loaded save never references a graph layout that no longer
exists).

---

## §10. Inherited Phase 3 deferred / carried-forward items (still open)

From `docs/post-phase-3-audit.md` §9 carried-forward table (recorded so no Phase 4
substep mistakes the absence for a regression):

- **Typed sidecar layers (lighting / simulation / flora / fluid) remain empty
  scaffolding** — no bake, no sim, no fluid fill landed. Phase 4 does not add them
  (foliage = Phase 5, fluid sim out of scope). `FluidProvider` enters `PinType`
  (Substep 2) as a *type-system* addition only; no fluid evaluation is implied.
- **Walkability mask + slab smoothing are intra-chunk.** Phase 4 keeps them
  intra-chunk (cross-chunk neighbor access in Substep 8 is **for biome boundary
  blending only**, not for walkability/slab).
- **`ChunkOverrides` coexists with the live edit path** as the richer multi-layer
  diff structure; Phase 4 does not change the override mechanism.
- **Persistence is v5 sectioned multi-layer** (40-byte empty blob, key-sorted
  determinism, tags section). The Phase 4 `voxel_format_version` bump is a clean
  wipe — the existing gate already hard-rejects mismatched versions, so no in-place
  migration code is needed. The **tags section** must grow to encode the now-meaningful
  multi-biome `ChunkTags` (Substep 10) rather than the trivial Zone0/Biome0 shape.

---

## §11. Summary of what Phase 4 must build vs. what already exists

| Concern | Exists today | Phase 4 work |
|---------|-------------|--------------|
| Graph-type concept (`GraphKind`/types) | ❌ (one flat `Graph`) | Define (Substep 1, **decision §9a**) |
| `PinType` SurfaceField/FluidProvider/ZoneId | ❌ | Add + coercions (Substep 2) |
| `Positions` vs `ScatterPoints` naming | ⚠️ divergent | Reconcile (**decision §9b**) |
| `LibraryRef` node + `LibraryGraphRegistry` + cycle detect | ❌ | Define (Substep 3) |
| Multi-graph evaluator + `ColumnCache` | ❌ (single-graph, NodeId CSE only) | Build (Substep 4) |
| Functional WorldGraph / ZoneGraph / BiomeGraph | ❌ (single TerrainOutput) | Implement + determinism tests (Substeps 5–7) |
| Cross-chunk neighbor access (computational) | ❌ | Implement (Substep 8) |
| Per-column fade blending (`BiomeBorderFade`) | ❌ | Implement + determinism test (Substep 9) |
| Meaningful `ChunkTags` | ❌ (trivial Zone0/Biome0, no consumer) | Populate from eval (Substep 10) |
| Tag-driven targeted invalidation | ❌ (invalidate-ALL in regen.rs) | Implement (Substep 11) |
| `traversal_smoothing_distance` / distance-N slabs | ❌ (distance-1 hardcoded §8) | Parameterize (Substep 12) |
| Hierarchical two-biome starter world asset | ❌ (one flat JSON) | Migrate (Substep 13, **decision §9e**) |
| Editor multi-graph selector | ❌ (single graph) | Add panel selector (Substep 14) |
| Shared gen call path | ✅ `generate_chunk_storage` | Multi-graph lands behind it (§4) |
| Determinism discipline | ✅ seam-safe seeding in `EvalContext` | Extend to every new pass |

---

*End of pre-Phase-4 state audit. Awaiting signoff before Substep 1 code planning.
No code has been written.*
