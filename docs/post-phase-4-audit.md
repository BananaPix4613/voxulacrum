# Post-Phase-4 State Audit

**Status:** Close-of-Phase-4 snapshot. Reference-only — no code changes.
**Date:** 2026-06-22.
**Supersedes as starting reference:** `docs/pre-phase-4-audit.md` (open-of-Phase-4 snapshot).
**Authoritative targets:** `docs/engine-design.md` v1.2 (§4 five-graph hierarchy, §5 generation pipeline + `ColumnCache` + fade blending + traversal smoothing distance).

This audit records the *actual* state of the code at the close of Phase 4, after
all 14 substeps landed (each built clean and passed tests; the engine runs and
renders two visibly distinct biomes). It documents what was built, the concrete
code surface, the decisions taken along the way, and the items deliberately
deferred — the starting reference for Phase 5.

---

## §0. Phase 4 outcome (recap)

Phase 4 delivered the **five-graph hierarchy** and **multi-biome terrain**. The
world now generates from a manifest-assembled hierarchy — a singleton World
graph (climate + zone assignment), a Zone graph (biome assignment), and one
BiomeGraph per biome — composited per chunk. The embedded editor edits any graph
in the hierarchy via a selector, and edits invalidate only the chunks they
affect. All 14 substeps are complete:

1. `GraphKind` model in the IR — **done**
2. `PinType` expansion (`SurfaceField`, `FluidProvider`, `ZoneId`) — **done**
3. `LibraryRef` node + `LibraryGraphRegistry` + cycle detection — **done (scaffold)**
4. Multi-graph evaluator + `ColumnCache` — **done**
5. Functional WorldGraph (`SurfaceNoise` → `WorldOutput`) — **done**
6. Functional ZoneGraph (`ZoneOutput`) — **done**
7. Functional BiomeGraph density (`YBand`) — **done**
8. Computational cross-chunk neighbor access (`sample_column`) — **done**
9. Per-column fade blending (`BiomeBorderFade` kernel + border analysis) — **done (mechanism)**
10. Meaningful `ChunkTags` — **done**
11. Tag-driven targeted invalidation — **done**
12. `traversal_smoothing_distance` parameter + distance gate — **done (kill switch)**
13. Hierarchical two-biome starter world (manifest + composite) — **done**
14. Editor multi-graph selector — **done**

---

## §1. nodegraph-ir — graph kinds, pin types, library scaffold

### 1.1 `Graph` + `GraphKind` (`graph.rs`)

`Graph` now carries a `kind: GraphKind` field (`#[serde(default)]` → legacy
kind-less JSON loads as `Biome`). `GraphKind = { World, Zone, Biome (default),
Detail, Library }`. `Graph::of_kind(kind)` constructs an empty graph of a kind.
`validate()` dispatches through `validate_kind_rules` — a scaffold that currently
enforces nothing per-kind (every kind validates as a generic dataflow graph);
the dispatch exists so per-kind root-output rules can be added without
re-threading the call site. `Graph::library_refs()` iterates the `LibraryGraphId`s
referenced by `LibraryRef` nodes.

### 1.2 `PinType` (`pin.rs`)

Twelve variants: `Scalar, Density, SurfaceField, Material, FluidProvider,
Positions, Assignments, Curve, Vec3, BiomeId, ZoneId, Terrain`. Coercion is
unchanged and strict: only `Scalar→Density` and `Curve→Scalar`. The new types
(`SurfaceField`, `FluidProvider`, `ZoneId`) are self-compatible only.

> **Intentional divergence (still owed reconciliation):** the code keeps
> `Positions` where the design doc says `ScatterPoints`. Documented in the pin
> doc-comment; the design doc's `PlacementMask`/`SpeciesWeights`/`PaintOutput`/
> `ScatterOutput` remain unintroduced (Phase 5 foliage territory).

### 1.3 Node kinds (`node.rs`)

`NodeKind` gained five variants this phase:
- `SurfaceNoise(NoiseParams)` — Source, `SurfaceField` output (per-column climate).
- `WorldOutput(WorldOutputParams { zone_bands })` — Output terminal, `SurfaceField` in → per-column zone id.
- `ZoneOutput(ZoneOutputParams { biome_bands })` — Output terminal, `SurfaceField` in → per-column biome id.
- `YBand(YBandParams { min, max })` — Source, `Density` out: `1.0` where `min ≤ worldY < max`, else `0.0` (vertical layering gate).
- `LibraryRef(LibraryRefParams { library: LibraryGraphId })` — `NodeCategory::Library`, **static `NO_PINS` placeholder** (dynamic boundary pins deferred).

`NodeCategory` gained `Library`. `WorldOutput`/`ZoneOutput` band-quantization is
named for the general mechanism, not a scenario.

### 1.4 Library scaffold (`library.rs`)

`LibraryGraphId(pub u32)` (the single unified id; `voxulacrum-app/world/tags.rs`
re-exports it). `LibraryGraphRegistry { graphs: HashMap<LibraryGraphId, Graph> }`
with `insert/get/contains/len/is_empty` and `detect_cycle() -> Option<Vec<LibraryGraphId>>`
(iterative 3-color DFS over **sorted** ids → deterministic cycle reporting).
**No library is registered or evaluated anywhere yet** — the registry's first
(currently trivial) consumer is `WorldEvaluator::new`, which logs a detected
cycle. `LibraryRef` evaluation is a hard error in both evaluators.

---

## §2. nodegraph-eval — multi-graph evaluator + column domain

### 2.1 Column domain (`column.rs`, `column_eval.rs`)

- `ColumnField` — dense `CHUNK_DIM²` plane of `f32` (a `SurfaceField` value).
- `IdColumn` — dense `CHUNK_DIM²` plane of `u16` (a zone or biome id field).
- `ColumnOutput = { Surface(Arc<ColumnField>), Id(Arc<IdColumn>) }` — the per-node 2D output (the deliberately *general* id variant, not separate Zone/Biome).
- `ColumnCache` — `NodeId → ColumnOutput`, the 2D analog of `EvalCache`.
- `ColumnEvaluator` — topological per-column fill of `SurfaceNoise`/`WorldOutput`/`ZoneOutput`; voxel-domain nodes raise `EvalError::WrongGraphDomain`. `quantize_bands`/`quantize_one` shared by the World (zone) and Zone (biome) terminals.
- `ColumnSample = { Surface(f32), Id(u16) }` + `sample_column(node, world_x, world_z)` — **pointwise, chunk-independent** column evaluation (the cross-chunk neighbor primitive). Seam-safe by construction (world coords + chunk-independent `noise_seed`); tested to agree with the bulk fill and across chunk framings.

### 2.2 Border / fade (`border.rs`)

`BorderAnalysis { distance: ColumnField, neighbor: IdColumn }` +
`analyze_biome_borders(eval, biome_node, chunk, radius)` — scans each column's
radius-neighborhood via `sample_column` for the nearest differing biome.
`biome_border_fade(distance, radius)` (own-biome weight: `0.5` at a border →
`1.0` at radius; `0` radius disables) and `blend_density(own, neighbor, weight)`.
**Standalone mechanism — not wired into generation.** Single-neighbor only
(doc's up-to-4 multi-junction deferred); voxel-level compositing means the fade
is currently unused (see §2.3).

### 2.3 `WorldEvaluator` + composite (`world_eval.rs`)

`WorldEvaluator` owns `world`, `zone`, `biomes: Vec<BiomeGraph>`, and a
`LibraryGraphRegistry`. Builders: `with_world`, `with_zone`, `with_biomes`,
`with_libraries`. `evaluate_chunk` → `ChunkEvaluation { terrain:
Arc<ChunkBuffer<Voxel,32>>, world_columns, zone_columns }`:

1. World graph → per-column climate + zone ids.
2. Zone graph → per-column climate + biome ids.
3. **Composite:** evaluate each biome graph present in the chunk, copy each
   column's voxels from its assigned biome's terrain. **Fast path:** a chunk that
   is a single biome returns that biome's terrain `Arc` directly (no copy) — the
   common case, byte-identical to the pre-Phase-4 single-biome path.

Biome borders are **hard cuts** at the voxel level. `EvalCache`/`Evaluator` per
biome are internal to the composite; the per-graph CSE cache is no longer exposed
on `ChunkEvaluation`.

---

## §3. voxulacrum-app — generation, tags, invalidation, assets, editor

### 3.1 `WorldGenerator` (`world/world_generator.rs`)

No longer holds a single `terrain_node`; it owns a `WorldEvaluator` and harvests
`eval.terrain` (composited). Construction:
- `from_manifest(path, seed, dist)` — load the hierarchy and assemble.
- `from_manifest_with_override(path, seed, dist, slot, graph)` — reassemble with one graph (`GraphSlot`) substituted; rest reloaded from disk (the editor-edit path).
- `from_path`/`new` retained for single-biome test loads.
- `load_world_graphs(path)` — returns `Vec<(GraphSlot, String, Graph)>` (slot + label) for the editor selector.
- `generate_chunk(pos) -> GeneratedChunk { storage, tags }` — composite terrain → `StorageBoundary::materialize` → slab smoothing (distance-parameterized) → `derive_tags`.

`GraphSlot = { World, Zone, Biome(u16) }`. `WorldManifest`/`BiomeManifestEntry`
parse `world.manifest.json`.

### 3.2 `ChunkTags` (`world/tags.rs`) — now meaningful

`derive_tags` populates `biomes` as the **distinct biome-id set** present in a
chunk (from the Zone graph's assignment) and `zone` as a single representative
(smallest distinct zone id). Empty World/Zone graphs fall back to the legacy
`Zone(0)` + `[Biome(0)]`. The struct shape is unchanged → **no persistence-format
change** (`zone` stays singular; multi-zone-per-chunk invalidation deferred).
`single_biome` retained for the `Chunk` default + the persistence round-trip test.

### 3.3 Tag-driven invalidation (`world/regen.rs`)

`InvalidationTarget = { AllChunks, Zone(ZoneId), Biome(BiomeId) }` +
`select_invalidated(world, target)` (tag-set lookup). `classify_edit(slot:
GraphSlot)`: World → all chunks, Zone → `Zone(0)`, Biome(id) → `Biome(id)`. The
regen completion **merges** (`world.chunks.extend`) rather than replaces, so a
subset regen retains untouched chunks. Editing the Rocky biome now regenerates
**only rocky-tagged chunks**.

### 3.4 Slab smoothing (`world/slab_smoothing.rs`)

`smooth_slabs(storage, distance: u32)`: `0` disables (the doc's flat-plains
case), `≥1` applies the existing single-step smoothing. **Values >1 currently
behave as 1** — true multi-distance staircasing needs generated geometry +
cross-chunk neighbor *voxels* (the per-chunk hook still lacks them) and remains
future work. Sourced from `TerrainGenParams.traversal_smoothing_distance` (global
default 1) → `WorldGenerator`. Per-biome variation awaits biome-param infra.

### 3.5 Starter world assets (`assets/graphs/`)

`world.manifest.json` → `world.graph.json` (`SurfaceNoise → WorldOutput[]`, single
zone), `zone.graph.json` (`SurfaceNoise → ZoneOutput[0.0]`, two biomes),
`biome_meadow.graph.json` (biome 0, the migrated former `default_biome.graph.json`
— grass), `biome_rocky.graph.json` (biome 1 — taller, jaggier, bare stone).
`default_graph_path()` now points at `biome_meadow.graph.json`.

### 3.6 Editor multi-graph selector (`ui/hierarchy_editor.rs`)

`HierarchyEditor` wraps the generic `EditorState` + an in-memory graph set keyed
by `GraphSlot` + a ComboBox selector. Switching saves the current canvas back to
the set (edits survive switches); the edit→regen bridge emits `(selected_slot,
graph)`. Loaded from the manifest at startup; opens on the primary biome.
Hot-reload tracks `biome_meadow.graph.json` (= `Biome(0)`) and refreshes that
slot. `EguiRenderer.editor` → `EguiRenderer.graph_editor`.

---

## §4. Determinism posture

Every new generation/evaluation pass added this phase carries a determinism or
seam test: `SurfaceNoise`/column eval (repeat-run equality), `sample_column`
(chunk-independence + bulk-fill parity), `YBand` (gate + determinism), border
analysis (determinism + cross-boundary consistency), and the composite
(per-column terrain matches each biome's own terrain). Pure refactors (e.g.
`ChunkEvaluation.cache → terrain`) carry equivalence tests instead.

`voxel_format_version` / save-format note: **no on-disk format bump was needed** —
`ChunkTags` kept its shape, so `BLOB_VERSION` (5) is unchanged and old saves still
load. (The pre-phase plan anticipated a wipe; it proved unnecessary given the
singular-`zone` decision.)

---

## §5. Deferred / known limitations (carried into Phase 5+)

1. **Density-level fade is unwired.** Compositing is voxel-level (hard biome
   cuts). `BiomeBorderFade` + `blend_density` exist and are tested but unused;
   smooth borders need biome graphs to expose density (not finished voxels) and a
   shared material/build pass — a biome-graph-contract rework.
2. **Single-neighbor borders.** `BorderAnalysis` tracks one nearest differing
   biome; the doc's up-to-4 `neighbor_biomes` multi-junction blend is deferred.
3. **`LibraryGraph` is type/scaffold only.** `LibraryRef` has static no-pins;
   dynamic boundary pins, library expansion/evaluation, and the standard
   libraries (`StandardCaveNoise`, `SurfaceLayering`, `ExposureLayering`,
   `BiomeBorderFade` as a library, `PoissonPlacement`) are unbuilt. `BiomeBorderFade`
   shipped as an engine function, not an authored library.
4. **`DetailGraph` is unbuilt** beyond the `GraphKind::Detail` tag (Phase 5 foliage).
5. **Multi-distance slab smoothing** (values >1) needs generated geometry +
   cross-chunk neighbor voxels; only the 0/≥1 kill switch is live.
6. **Multi-zone-per-chunk** is not distinguished — `ChunkTags.zone` is singular;
   zone invalidation is approximate for boundary chunks (no current world
   exercises it).
7. **`traversal_smoothing_distance` is global**, not per-biome — needs a
   biome-param registry (biomes are graphs with no scalar-metadata sidecar).
8. **Hot-reload covers only the primary biome file.** Editing World/Zone/other
   biomes is in-editor only; disk hot-reload for those files is future polish.
9. **No UI** for `traversal_smoothing_distance` (lives in `TerrainGenParams`,
   default 1; changeable only in code/config).
10. **Cross-graph boundary inputs are absent.** The Zone graph re-derives its own
    climate via `SurfaceNoise` rather than reading the World graph's channels;
    named root-output/input pins between graphs are unbuilt.
11. **Sea level / fluids** (`WorldGraph.sea_level`, `BiomeGraph.fluid_provider`,
    `ZoneGraph.rivers`) — out of Phase 4 scope, unbuilt.

---

## §6. Key decisions taken during Phase 4 (for traceability)

- **`GraphKind` as an enum field** (serde-default `Biome`), permissive
  scaffold validation (enforce later).
- **Kept `Positions`** (not renamed to `ScatterPoints`); documented divergence.
- **`LibraryRef` ships with static no-pins**; dynamic pins deferred. `LibraryGraphId`
  unified in `nodegraph-ir`.
- **`ColumnCache` as a general per-node memo** (not the doc's fixed
  climate/zone/biome struct); **`ColumnOutput` with one general `Id` variant**
  (zone vs biome carried by the producing node).
- **General SurfaceField climate channels** (author N `SurfaceNoise` nodes), not
  a fixed 5-channel `ClimateVector`.
- **One general `YBand{min,max}`** (not three `YBetween`/`YBelow`/`YAbove`); hard
  0/1 gate.
- **Pointwise `sample_column` only** for cross-chunk access (scan deferred to its
  consumer); **unified `ColumnSample`**.
- **Per-column fade mechanism only** (not wired); **nearest-single-neighbor**.
- **`ChunkTags.zone` stays singular** (no persistence bump); biomes a real set.
- **Invalidation: selector + classification + merge** (partial regen
  infrastructure).
- **`traversal_smoothing_distance`: parameter + kill switch**; richer distance-N
  deferred; sourced from `TerrainGenParams`.
- **Two-biome terrain via voxel composite (hard cuts)**; **separate asset files +
  JSON manifest**; multi-biome eval split into 13a (composite infra) / 13b-i
  (loader) / 13b-ii (assets + wiring).
- **Editor: in-memory graph set + save-on-switch, ComboBox**; split into 14a
  (slot-aware plumbing) / 14b (selector UI).

---

## §7. Build-vs-exists quick table

| Capability | State |
|---|---|
| Five `GraphKind`s | exists (World/Zone/Biome functional; Detail/Library tag-only) |
| `PinType` expansion | exists |
| `LibraryRef` + registry + cycle detection | exists (scaffold; no live library) |
| Multi-graph evaluator + `ColumnCache` | exists |
| Functional World/Zone/Biome graphs | exists |
| `YBand` vertical layering | exists |
| Cross-chunk `sample_column` | exists |
| `BiomeBorderFade` kernel + border analysis | exists (unwired) |
| Density-level fade blending | **not built** |
| Meaningful `ChunkTags` | exists (zone singular) |
| Tag-driven targeted invalidation + merge regen | exists |
| `traversal_smoothing_distance` | exists (0/≥1 only) |
| Multi-distance slab staircasing | **not built** |
| Two-biome composite terrain | exists (hard cuts) |
| Manifest-driven hierarchy load | exists |
| Editor multi-graph selector | exists |
| `DetailGraph` / foliage | **not built** (Phase 5) |
| Library expansion / standard libraries | **not built** |
| Fluids / sea level / rivers | **not built** |

---

*End of post-Phase-4 audit. Phase 5 (foliage/detail) starts from this snapshot.*
