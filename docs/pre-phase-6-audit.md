# Pre-Phase-6 Audit — Consolidation (Phase 4 Deferrals + Library Activation)

State-of-the-codebase reference for **Phase 6**. Companion to `pre-phase-4-audit.md`
and `post-phase-5-audit.md`. Records exactly where the five Phase-4 deferrals,
the library scaffold, and the cleanup targets stand today, and flags the design
forks Phase 6's planning must resolve. No code changed in this pass.

Scope reminder (Phase 6, strict): typed boundary declarations; library
activation with dynamic pins; author all five standard libraries; cross-graph
dataflow; per-biome parameter sidecar; per-biome `traversal_smoothing_distance`;
density-level boundary blending; multi-distance slab smoothing; delete the
orphaned `VegetationPass`; dead-code sweep. Everything else is out of scope.

---

## 1. Library scaffold — current state

**Files:** `crates/nodegraph-ir/src/library.rs`, `LibraryRef` in `node.rs`.

- `LibraryGraphId(pub u32)` — stable id; `LibraryRefParams { library: LibraryGraphId }`.
- `LibraryGraphRegistry { graphs: HashMap<LibraryGraphId, Graph> }` with `insert/get/contains/len/is_empty` and `detect_cycle()` — an iterative 3-color DFS over `Graph::library_refs()` (the `LibraryRef` node ids each library references). Deterministic (sorted ids + sorted children). Well-tested.
- **`LibraryRef` is static no-pins.** `node.rs` descriptor: `inputs: NO_PINS, outputs: NO_PINS` with the comment *"scaffold: dynamic boundary pins deferred."*
- **The registry stores bare `Graph`s — no boundary declarations.** There is no notion of a library's named typed inputs/outputs anywhere in the IR.
- **The app never populates a registry.** `WorldEvaluator` has `with_libraries()` but `WorldGenerator::from_hierarchy` never calls it; the manifest has no library list; there is no `assets/libraries/` directory and no library loader.

**What must change for dynamic pins (Substep 1):**
- Add a boundary-declaration type (`{ name, pin_type, description }` per port) and attach it to `Graph` (or a wrapper). Libraries declare inputs **and** outputs; World/Zone/Biome declare outputs.
- `LibraryRef` must expose the referenced library's boundary as **instance-computed** pins — see the **static-descriptor blocker** in §11.

---

## 2. Hardcoded functions targeted for library conversion

The prompt lists five standard libraries. Their current reality differs per library — **only one is an actual hardcoded engine function**; the rest are either already graph nodes or don't exist yet. This materially shapes Substep 2/3.

| Library | Current form | Substep-3 meaning |
|---|---|---|
| **BiomeBorderFade** | Hardcoded fns `biome_border_fade(distance, radius)` + `blend_density(own, neighbor, weight)` in `nodegraph-eval/src/border.rs`. Both **unconsumed** today. | True port; delete the fns after the library + Substep 8 take over. Substep 2's pilot. |
| **SurfaceLayering** | Already a graph node: `NodeKind::Layer` (`LayerParams { bands, fill }`, node.rs) — depth-conditional material cake. Not "hardcoded" anywhere else. | Re-express as a library graph (or a library wrapping a canonical `Layer` config). Nothing to delete. |
| **PoissonPlacement** | Already graph nodes: `PoissonDistribution` (foliage) + `PoissonDisk` (positions). | Re-express/wrap as a library; keep the node kind calling it, or make the node a library ref. Decide in 3b. |
| **StandardCaveNoise** | Does **not** exist (no caves generated yet). | Author fresh (3D Worley + ridged), ready for future use. |
| **ExposureLayering** | Does **not** exist. | Author fresh, ready for future use. |

**Important:** `biome_border_fade` is a **linear** ramp — `(0.5 + 0.5*(distance/radius)).clamp(0.5, 1.0)` — *not* a smoothstep (the prompt says "smoothstep"; the code is linear). The byte-identical-port lock (a Substep-2 determinism requirement) must reproduce the **linear** function.

---

## 3. Current graph asset format

- **Graph JSON** (`Graph`, graph.rs): `{ kind: GraphKind, nodes: SlotMap<NodeId,Node>, edges: Vec<Edge> }`. `kind` is `#[serde(default)]` → legacy graphs load as `Biome`. Nodes serialize `{ value: { kind: {type, ...params}, position }, version }` (slotmap format). Node kinds are internally tagged by `"type"`.
- **Manifest** (`world.manifest.json`, parsed by `WorldManifest` in `world_generator.rs`): `{ world, zone, biomes: [{ id, graph, detail? }] }`. Paths relative to the manifest dir.
- **Parser/loader:** `Graph::from_json` (serde_json); `load_hierarchy` / `load_world_graphs` in `world_generator.rs`. The editor also round-trips through the same JSON.
- **Detail graphs** (`biome_meadow.detail.json`) use the same `Graph` format with `kind: "Detail"`.
- **Prefabs** (`assets/prefabs/*.prefab.json`, `assets/prefabs.ron`) are separate and unaffected.

**Substep-5 attach point:** `BiomeManifestEntry` (`{ id, graph, detail? }`) is where the per-biome `params` map is added. **Substep-1/2:** a new `assets/libraries/*.library.json` file kind + a `libraries` array in the manifest.

---

## 4. Current biome graph contract (finished voxels, hard cut)

**End-to-end trace of how a biome graph produces voxels today:**

1. `WorldGenerator::generate_chunk` → `WorldEvaluator::evaluate_chunk(ctx)`.
2. `eval_columns(world)` and `eval_columns(zone)` → per-column caches (climate + zone/biome ids) via `ColumnEvaluator`.
3. `biome_id_column` = the Zone graph's `ZoneOutput` `IdColumn`.
4. **`composite_terrain`**: for each present biome, `eval_biome_terrain` runs a full `Evaluator` over the biome graph and harvests `CachedOutput::Terrain` from the `TerrainOutput` node — a **whole-chunk `ChunkBuffer<Voxel,32>` of finished voxels**. Columns are then copied by biome id (**hard cut**; single-biome chunks take a no-copy fast path).
5. Back in `generate_chunk`: `StorageBoundary::materialize` → `ChunkStorage`; then `smooth_slabs`; then tags + foliage.

**Node vocabulary for terrain:** density + material → `BuildTerrain` (`Density`+`Material` → `Terrain`) → `TerrainOutput` (consumes `Terrain`, `TERRAIN_TERMINAL_IN`). `Layer`/`ConstantMaterial`/`Queue` produce `Material`. So density and material are **already distinct fields inside the biome graph**; `BuildTerrain` fuses them.

**Substep-7 implication (bigger than "swap the terminal"):** today biomes emit finished whole-chunk terrain and composition is per-column *voxel* copy. The rework needs biomes to emit **density** (`DensityOutput` terminal, or `TerrainOutput`→`Density`), composition to select/blend **density per column**, and a **new shared post-composition material+build pass** (the `BuildTerrain`/`Layer` logic moves out of the biome graph into the engine, or runs once on the composed density with the winning biome's material provider). This is the highest-risk substep; flag its shape before transcription.

---

## 5. Cross-graph dataflow — confirmed absent

- **World** (`world.graph.json`): `SurfaceNoise(seed 10, freq 0.001)` → `WorldOutput(zone_bands: [])` ⇒ zone 0 everywhere.
- **Zone** (`zone.graph.json`): `SurfaceNoise(seed 20, freq 0.004)` → `ZoneOutput(biome_bands: [0.0])` ⇒ biome 0/1 split. **Zone runs its own noise (seed 20), independent of World's (seed 10).** Editing World's climate does **not** move biome boundaries.
- **Biome graphs** re-derive their own terrain via their own noise; they receive only `EvalContext` (seed + chunk position), never World/Zone climate.
- In `WorldEvaluator::evaluate_chunk`, `world_columns` and `zone_columns` are computed and used for tags, but **never fed into the biome `Evaluator`**. There is no node that reads another graph's output.

**Every place a graph *could* reference another but doesn't:** Zone→World climate (the headline case); Biome→World climate/sea_level; Biome→Zone biome_id (currently the biome graph doesn't know its own id — `BiomeContextMask` filters foliage by id via the DetailEvaluator, not the terrain Evaluator).

**Substep-4 plumbing need:** the biome/zone `Evaluator` must receive the upstream `ColumnCache`(s) plus a `GraphRef`-style node that reads a **named** channel from them. `ColumnCache` is keyed by `NodeId → ColumnOutput{Surface|Id}`; named channels require mapping a boundary name → the producing node's output (Substep-1 boundary declarations provide the name).

---

## 6. Current slab smoothing

**File:** `crates/voxulacrum-app/src/world/slab_smoothing.rs`. `smooth_slabs(&mut ChunkStorage, distance: u32)`.

- `distance == 0` → early return (kill switch).
- `distance >= 1` → single-step only: builds a `WalkabilityMask` from the storage, demotes each walkable full `Cube` that has a walkable neighbor exactly one cell lower into a `SlabBottom`. Order-independent (decide-from-original, then apply). Authored slabs (`SlabBottom`/`SlabTop`) are left untouched.
- **Values > 1 behave as 1** (documented). **No cross-chunk access** — `has_lower_walkable_neighbor` treats out-of-bounds neighbors as non-walkable, so steps that straddle a chunk boundary are left sharp.
- Called once per chunk from `generate_chunk` with the generator's global `traversal_smoothing_distance`.

**Substep-9 blockers (both real):** (a) multi-distance staircasing of taller drops needs to *insert* geometry across N cells, not just demote one cube; (b) correct boundary smoothing needs neighbor-chunk voxel reads. Note the mesher already reads a 34³ neighbor border (`ChunkSnapshot`) for AO — Substep 9 can mirror that read-only accessor, but generation-time neighbor access is a new pattern for the generator (the generator currently produces chunks independently/in parallel via `par_iter`; introducing neighbor dependence has ordering/determinism implications — flag in §11).

---

## 7. `TerrainGenParams` and `traversal_smoothing_distance`

- Defined in `crates/voxulacrum-app/src/params.rs` (part of `TerrainGenParams`); `default` currently `1`.
- Threaded as a `u32` field on `WorldGenerator` (constructed in `load_default`, `from_manifest*`, `new`, `from_path`). Also referenced in `world/regen.rs` (regeneration reuses the param).
- Consumed only by `smooth_slabs`.

**Substep-6 move:** remove from `TerrainGenParams`; source per-biome from the Substep-5 sidecar. Because smoothing runs on a *composited* chunk (multiple biomes possible per chunk), "which biome's distance" is per-column — the pass must read the biome-id column (available from the Zone output) and look up each column's biome params. This couples Substep 6 to Substep 5's sidecar being threaded to the smoothing site (currently `smooth_slabs` gets only `&mut ChunkStorage`).

---

## 8. Orphaned `VegetationPass` — deletion surface

**Already not compiled:** `rendering/mod.rs:11` comments out `pub mod vegetation_pass;` ("retired in Phase 5 4a-2i… deletion is a deferred cleanup"). So `vegetation_pass.rs` (struct `VegetationPass`, `ChunkVegetation`, `create_grass_blade_mesh`, etc.) is dead-on-disk and **produces no warnings** because it isn't in the module tree.

**Still live-but-purposeless (must be explicitly deleted in Substep 10):**
- `crates/voxulacrum-app/src/rendering/vegetation_pass.rs` — delete the file.
- `crates/voxulacrum-app/src/rendering/pipelines.rs`: `create_vegetation_pipeline` (≈271), `PipelineId::Vegetation` (≈734), `PipelineRegistry.vegetation_pipeline` field (≈764), the `vegetation.wgsl` load (≈786), the registered `PipelineEntry` (≈829-831), and the rebuild arms (≈906-953). This pipeline is built and hot-reloaded but **never used to draw**.
- `shaders/vegetation.wgsl` — delete.
- `crates/voxulacrum-app/src/params.rs`: `VegetationParams` (≈180) + `EngineParams.vegetation` field (≈572, 595) — blade geometry that only fed the dead pipeline.
- `crates/voxulacrum-app/src/ui/panels.rs`: `draw_vegetation` (≈514) + its call site (≈157) — the "Vegetation" blade-params UI section.
- `rendering/mod.rs:11-12` — remove the retired-mod comment.

**⚠ Must NOT be deleted (reused by Phase-5 foliage):** `DebugParams.hide_vegetation` (params.rs ≈451, 467) is a **separate** field that `main_scene_pass.rs:116,137` uses to gate the **detail-paint and scatter** passes, plumbed via `ecs/systems.rs:632`. Deleting `VegetationParams` must leave `hide_vegetation` intact (consider renaming it `hide_foliage` for honesty, but that's a wider rename — decide in Substep 10). Comments referencing "vegetation" in `ecs/systems.rs` (≈325-351) and `streaming.rs:319` are stale narration, harmless, tidy opportunistically.

---

## 9. Dead-code warnings inventory (`cargo check -p voxulacrum` → 32 warnings)

Categorized for Substep 10. (a) = intentional scaffold, keep with narrow `#[allow(dead_code)]` + comment; (b) = actual dead code, delete; (c) = needs judgment during the sweep.

**(a) Intentional scaffolds — keep:**
- `fields fluids, decals, and lighting are never read` — `Chunk` layers awaiting fluid/decal/lighting systems (design §3). Keep.
- `method set_voxel is never used` — `overrides::set_voxel`, awaiting the voxel edit toolkit. Keep.
- `associated items new_air … / get_voxel` on chunk/storage types that are part of the deliberate storage API — keep if used by tests/future; verify.

**(b) Likely delete:**
- `unused import: storage::ChunkStorage`; `unused variable: sin_theta`; `variable does not need to be mutable`; `hiding a lifetime that's elided elsewhere` (clippy) — trivial fixes.
- `function border_dirty_neighbors is never used`; `function disk_usage is never used`; `struct CacheStats is never constructed`; `associated constant DEPTH_FORMAT is never used` — obsoleted helpers.
- `variant PostProcess is never constructed`; assorted never-read fields (`total_vertices`, `texture`, `sky_color`, `post_process_bind_group_layout`, `name`, `generation`, `camera_world_pos`, `border_min`, `scene/processed/depth/normal`, `clip_dirs/clip_pos`, `regen_min_y/regen_max_y`) — verify each is genuinely unreachable, then delete or wire.

**(c) Judgment:**
- `associated items new, from_path, and graph are never used` — `WorldGenerator::new/from_path/graph`: these are public API exercised by unit tests but not the app; **Substep 6/7 will touch these constructors** (smoothing-distance signature change), so re-evaluate then rather than pre-emptively deleting.
- `methods len, palette_len, sun_intensity, sharpness_changed, selected, priority_score, just_released, apply_edit are never used` — small accessors; keep any that are natural API, delete the rest.

Net expectation after Substep 10: warnings down to a small justified set; every remaining `#[allow(dead_code)]` carries an in-source reason. (The VegetationPass items above are *not* in this warning list — they're live-but-unused, hence explicit deletion.)

---

## 10. Save-format impact (precise)

- **Chunk blob:** `BLOB_VERSION = 6` (persistence.rs:47) serializes voxels + tags + overrides. Phase 6 changes **none** of these on-disk shapes (voxels stay `Voxel`; tags stay zone/biome/library-refs; overrides unchanged). **No `BLOB_VERSION` bump.**
- **Mesh cache:** `compute_cache_key(snapshot, colors)` (cache.rs:156) is **content-keyed** on the actual chunk voxels (34³ snapshot) + material colors — *not* a graph hash. So any graph-contract change that alters generated voxels **auto-invalidates** the mesh cache by changing the snapshot. The regen path also clears caches explicitly.
- **`VOXEL_FORMAT_VERSION = 3`** (persistence.rs:52) is a meta key; bumping it wipes stale saves + mesh cache on open. Phase 6 does **not require** a bump: old saves store only overrides (still valid), and generated content regenerates under the new graphs. A bump is **optional** — a convenience "start clean" for developers if the biome-density rework makes old generated terrain confusingly different. Recommendation: **do not bump** unless a concrete incompatibility surfaces; rely on content-keyed invalidation.

---

## 11. Surfaced audit gaps / open design questions (resolve during planning)

These are constraints the Phase 6 prompt did not fully anticipate. Per the "surface audit gaps immediately" directive, flag each as a decision **before** transcription of the relevant substep.

1. **Static pin descriptors block dynamic pins (Substep 1, foundational).** `NodeDescriptor.inputs/outputs` are `&'static [PinSpec]` and `NodeKind::descriptor()` returns them by value from `match` arms. `LibraryRef`/`GraphRef` need pins computed from the *referenced* graph's boundary — impossible with `&'static`. Substep 1 must change the descriptor mechanism (e.g. `Cow<'static, [PinSpec]>` or owned `Vec<PinSpec>` in `NodeDescriptor`, plus a resolution step that fills a node's pins from a registry at load/edit time). This ripples to `Graph::connect`/`validate` (which read `descriptor().outputs/inputs`) and to the editor's snarl viewer. **Decide the descriptor representation first — it's load-bearing for the whole phase.**

2. **The node vocabulary cannot currently express `BiomeBorderFade` as a graph (Substep 2).** The fade is `0.5 + 0.5*(distance/radius)` clamped — it needs (a) **library input binding** (how do the boundary inputs `distance`, `radius` feed nodes inside the library graph? no mechanism exists — all sources read world-pos/noise, not external inputs), and (b) a **division** operation (there is no `Divide` node; arithmetic nodes are `Add/Multiply/Subtract/Min/Max/Clamp/Lerp/Remap` over `Density`/`SurfaceField` fields). Options: extend the vocabulary + add input-binding so the library is a genuine node graph; or represent a library as a parameterized native kernel keyed by boundary (contradicts "library graph" framing). **This is the crux of "library activation" — resolve at Substep 1/2. The other four libraries inherit whatever mechanism this establishes.**

3. **Generation-time neighbor access changes the generator's independence model (Substep 9).** Chunks currently generate independently (startup `par_iter`, streaming workers, regen) with no cross-chunk reads. Multi-distance smoothing with correct boundaries needs neighbor voxels at generation time. Introducing that coupling has determinism/ordering implications (a chunk's slabs would depend on neighbors' generated-but-unsmoothed voxels). Mirror the mesher's read-only border snapshot, and define the smoothing input as **neighbors' pre-smoothing voxels** (a pure function of seed+graph+position, so still deterministic and order-independent). **Confirm the neighbor-state definition before implementing.**

4. **Density composition is per-column, but the evaluator produces whole-chunk fields (Substep 7).** `Evaluator` yields whole-chunk `Density`/`Terrain`. Per-column density blending means evaluating each biome's *whole-chunk* density and selecting/blending per column — acceptable, but for a fade band it evaluates *both* neighbor biomes' full density for boundary chunks (a cost the current single-biome fast path avoids). Confirm this is acceptable, and where the material+build pass lives (engine-side shared pass vs. a per-biome material provider invoked post-composition).

5. **`generate_chunk_is_deterministic` non-vacuity.** The test (world_generator.rs) does a full-voxel compare at seed 7 / chunk (1,0,2) but doesn't assert slabs are present, so it could pass on a slab-free chunk. `slab_smoothing.rs` tests do guard non-vacuously. Strengthen the generate-chunk test to a slab-bearing case as new smoothing/density mechanisms land (carried from the Phase 5 audit's ⚠).

---

## 12. Deferred items inherited from Phase 5 (confirmed accurate)

Per `post-phase-5-audit.md`, still open and consistent with the code:
- **Substep 7 LOD billboards** — deferred (iso camera; out of Phase 6 scope).
- **Phase-4 deferrals** — these ARE Phase 6's body: density-blend, cross-graph named pins, per-biome smoothing distance, multi-distance smoothing, library activation. All confirmed still open by this audit.
- **Retired `VegetationPass`** — confirmed orphaned (§8); Substep 10 deletes.
- **Pre-existing dead-code warnings** — confirmed 32 (§9); Substep 10 sweeps.
- **Prefab sizing** — content concern, explicitly out of Phase 6 scope.

---

## Substep sequencing note

The dependency spine: **Substep 1 (boundary declarations + descriptor rework)** unblocks both **Substep 2/3 (libraries)** and **Substep 4 (cross-graph)**. **Substep 5 (sidecar)** unblocks **Substep 6 (per-biome distance)**. **Substep 2 (BiomeBorderFade)** + **Substep 7 (density contract)** together unblock **Substep 8 (blending)**. **Substep 9 (multi-distance)** depends on **Substep 6**. **Substep 10 (cleanup)** is independent and could land any time but is sequenced last. This matches the prompt's ordering; the only cross-coupling to watch is Substep 5's sidecar needing to reach the Substep-6 smoothing site and the Substep-1 descriptor decision gating everything.
