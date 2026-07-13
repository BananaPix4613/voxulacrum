# Architectural Drift Review — voxulacrum

**Date:** 2026-07-10 (post-Phase-7 tree, branch `mc-revision`)
**Scope:** Drift diagnosis against `engine-design.md` v1.2 and the post-phase audits 1–7. Deferral lists in each post-phase audit were read first; items on them are excluded as findings. This review does not propose alternative architectures.

Line numbers refer to the tree as of this date.

---

## 1. Documented-vs-implemented drift

### 1.1 Graph-edit regeneration destroys player work — §9's core guarantee is inverted

**Priority: highest (silent, total loss of user data).**

**Evidence:** Design doc §9: "Worldgen edits don't destroy player work: regenerating a chunk subtracts removed-overrides from new-generated, adds added-overrides" and "The graph remains the source of truth for the default world, **even after extensive player modification**." §4's invalidation table exists to make graph iteration cheap on a live (played) world.

In code, every regeneration completion — including the editor's auto-regen on any graph edit (`world/regen.rs:137–160`, `pending_graph` → `start_regeneration` → the same `poll_regeneration` handler) — does the following:

- `world/regen.rs:52` — `world.chunks.extend(new_chunks)`: regenerated chunks replace loaded chunks wholesale. `generate_world_background` (`world/mod.rs:415–436`) builds fresh `LoadedChunk`s with `overrides: None`, so in-memory player edits on invalidated chunks are discarded.
- `world/regen.rs:71` — `persistence.clear_all_chunks()`: **deletes every saved chunk record in the database**, not just invalidated ones, on every regen completion.

No deferral covers this. Post-7's "Restart initial-fill overrides" deferral covers the startup fill only and explicitly says "only the streaming load path does" apply overrides — regen is a third path with the same gap, plus the active DB wipe.

**Impact:** Editing any graph in the editor (the engine's headline hot-reload workflow, §1 "Editing the active graph edits the live world") silently erases all player edits — voxels, scatter, poured fluid — both in memory and on disk, world-wide. Every phase that adds player-authored content raises the stakes.

**Suggested resolution shape:** Regen completion needs an override re-overlay mirroring the streaming path (DB read + `apply_overrides_to_storage` + scatter/fluid overlay) for each regenerated chunk, and `clear_all_chunks()` must be removed from the graph-edit path (defensible only for explicit seed/terrain-param changes, and even then arguably behind a confirmation). A dirty-chunk save should precede the swap so unsaved edits aren't lost either.

### 1.2 Fluid persistence never saves chunks that water flowed into

**Priority: highest (silent data loss).**

**Evidence:** `ecs/systems.rs:359–368` gives every fluid-changed chunk an **empty** overrides bucket, with the comment "so build_chunk_edits snapshots the fluid even without a voxel edit." But `build_chunk_edits` (`world/persistence.rs:689–710`) matches `Some(ovr) if ovr.is_empty() => None` **before** the Delta arm that performs the fluid snapshot (`persistence.rs:704–707`). An empty bucket is still empty, so these chunks return `None` and save nothing. Chunks where the player poured do save (the pour writes `fluid_diffs` directly, `ecs/systems.rs:404–410`); chunks that water merely flowed into do not. The voxel-promoted `Full` branch also drops fluid, acknowledged in-source as a "rare edge" (`persistence.rs:696–698`).

Post-7 states "disturbed fluid persists (resuming mid-flow) across chunk unload/reload and save/close" as shipped behavior; its deferral list does not cover this.

**Impact:** Water that crosses a chunk boundary into an unedited chunk vanishes on that chunk's unload or on save/quit — the chunk reloads with only generated fluid. Mass silently appears and disappears across sessions; the two-phase cross-chunk conservation work (§7, Phase 7 Substep 6) is undone by persistence.

**Suggested resolution shape:** Decouple "has fluid deviation" from override emptiness — e.g. the fluid snapshot decision happens before the `is_empty` arm, or `is_empty` treats a fluid-changed flag as non-empty. The `Full` branch needs a decision too (carry fluid alongside, or document the loss).

### 1.3 Fluid persistence stores generated content and cannot represent removal

**Priority: high (save-model correctness).**

**Evidence:** Design doc §3: on evict "only its `ChunkOverrides` plus any tag changes are written back — **generated content is reproducible and not persisted**"; §9 defines every layer's split into generated (not serialized) and overrides (serialized). `build_chunk_edits` (`persistence.rs:691–706`) snapshots the **entire current fluid field** — including untouched generated ocean cells on sea-level-straddling chunks — into `fluid_diffs`. On load, `apply_persisted_fluid` (`world/streaming.rs:74–87`) only inserts; there is no tombstone concept, so generated cells the player drained or displaced re-derive and reappear.

Post-7 records the full-field snapshot as a shipped decision in its body, but it isn't on the deferral list, and the design doc's diff model was not revised.

**Impact:** (a) Player-drained generated water resurrects on reload — visible correctness bug once player-water interaction lands (deferred, post-7). (b) Save bloat: hundreds of generated cells serialized per edited straddle chunk. (c) If sea level or a fluid-authoring graph changes, stale snapshot cells overlay the new generated field on load — currently masked only because finding 1.1 wipes the DB first; fix 1.1 and this surfaces.

**Suggested resolution shape:** Store fluid as a true diff against re-derived generated fluid — changed/added cells plus removed-cell tombstones — mirroring the `voxel_diffs` self-healing pattern already in `spawn_generation` (`streaming.rs:283–295`).

### 1.4 Pipeline order: foliage is generated before fluids and is blind to them

**Priority: medium (latent visible generation bug).**

**Evidence:** Design doc §5 stage order: fluid initialization is stage 9, DetailGraph paint + scatter is stage 10 — detail placement runs after (and can respect) fluid. In code the order is inverted and disconnected: `WorldEvaluator::evaluate_chunk` (`nodegraph-eval/src/world_eval.rs:270–275`) runs `evaluate_foliage` (line 272) before `composite_fluid` (line 273), and neither foliage evaluation nor `SurfaceFilter` receives fluid data; in the app, `ocean_fill`/`apply_biome_ponds` run after `paint_to_detail_layers`/`scatter_to_store` (`world/world_generator.rs:196–207`). Not on any deferral list.

**Impact:** Grass paints and props scatter onto columns that fluid initialization then submerges. Invisible today only because the meadow's terrain happens to sit above `sea_level: 24` (`assets/graphs/world.manifest.json`); the first biome whose surface dips below sea level, and the deferred ponds/rivers when they land, will generate grass and bushes underwater.

**Suggested resolution shape:** Compute per-column fluid levels before foliage evaluation and expose submersion to the foliage pass (a `SurfaceFilter` condition or a post-filter on paint/scatter), restoring the §5 stage-9-before-stage-10 dependency.

### 1.5 Mesh vertex format diverges structurally from §10 `FaceVertex`

**Priority: medium (constrains all §11 rendering work; data shape declared settled).**

**Evidence:** §10 specifies `FaceVertex` (~32 bytes) with `face_axis`, `occlusion_class`, `biome_tint_index`, `variant_index`, `light_level_index`, `enclosure_factor`, `edge_flag`, `sway_weight`, `ao_factor`, `material_id: u16`, and **no color** — §11 has the shader compose the palette index from those attributes. Implemented `TerrainVertex` (`rendering/pipelines.rs:10–18`, 64 bytes) is `position/normal/color:[f32;3]/ao/material_id:u32/cell_flags`: registry color is baked at mesh time, and the mesh cache key therefore includes material colors (`meshing/cache.rs:146`). §1 says "data model decisions are settled now… data shapes do not [grow incrementally]," and no audit defers the vertex format.

**Impact:** Every §11 feature — tri-tonal axis ramps, palette/time-of-day shifts, biome tint, quantized light levels, edge-flag outlines, sway, and §12's attribute-swap debug views and audio occlusion (which "reuses `enclosure_factor` baked into mesh vertices") — requires a vertex-format break that invalidates the disk mesh cache and touches every terrain-consuming pass at once. Baking color also means any palette change forces full re-mesh of the world rather than a shader-side lookup.

**Suggested resolution shape:** A deliberate decision point: either record the current format as the accepted interim shape (audit/design-doc revision, with the convergence phase named), or schedule the §10 format migration as its own substep so later rendering phases don't each pay a format break. The `CACHE_VERSION` machinery already handles cache invalidation.

### 1.6 Greedy meshing policy unimplemented and unrecorded

**Priority: low.**

**Evidence:** §10: "Top faces: NOT greedy-merged… Side faces: greedy-merged. Bottom faces: greedy-merged." `meshing/cube_mesher.rs:1–5`: "Naive cube mesher: one quad per exposed voxel face… No greedy merging, no AO, no edge dedup." Not on any deferral list.

**Impact:** Constant-factor triangle/vertex overhead at streaming radii; primarily a doc-vs-code truth gap — a reader of §10 believes a policy exists that doesn't.

**Suggested resolution shape:** Record it as deferred in the next audit (with the trigger condition — e.g. draw-time budget), or schedule it. Nothing structural blocks it.

### 1.7 Per-layer save versioning replaced by single-version wipe migrations

**Priority: low now; release-gating later.**

**Evidence:** §3 chunk file layout: "Each layer has its own version field. Layers default to empty when absent so older saves load forward-compatibly"; §12: "Migration code path required for breaking version changes." Implemented: one `BLOB_VERSION = 6` for the whole blob plus `VOXEL_FORMAT_VERSION = 3` whose bump wipes all saves and the mesh cache (`world/persistence.rs:47–52, 758–768`). The audits consistently describe wipes as "pre-release; saves are not migrated" (post-3 §7, post-5) — a knowing practice, but the design doc was never revised to bless it or bound it.

**Impact:** None today. At the first release where saves must survive engine upgrades, the wipe policy and the doc's forward-compat model collide; retrofitting per-layer versions onto a monolithic blob is a format redesign better done deliberately than discovered.

**Suggested resolution shape:** Revise §3/§12 to state the interim wipe policy and the release boundary at which per-layer versioning becomes mandatory — or pull the per-layer version fields into the format at the next planned `BLOB_VERSION` bump while wipes are still cheap.

---

## 2. Cross-system consistency

### 2.1 PoissonDisk seeds RNG from chunk coordinates; every sibling uses world-absolute derivation

**Priority: determinism-pattern divergence (highest in this section).**

**Evidence:** §12 determinism rules: "All RNG seeded from explicit context: `hash(world_seed, layer, node_id, world_pos, purpose)`. No use of chunk coordinates alone for RNG (breaks at fade boundaries)." The rest of the stack complies: `noise_seed` deliberately excludes chunk coords (`nodegraph-eval/src/context.rs:37–45`), `JitteredGrid` uses `world_cell_seed` (world-absolute; `context.rs:60+`, `scatter.rs:58–85`), `stable_instance_id` hashes world position (`detail_eval.rs:251, 350`). `PoissonDisk` alone uses `scatter_seed` (`context.rs:49–56`), which folds `chunk.x`/`chunk.z` — documented in `scatter.rs:6–7` as "per-chunk (not seam-continuous — acceptable for Phase 10)." That is an in-code acceptance; it appears on no audit deferral list.

**Impact:** Poisson patterns are chunk-framed: no continuity across chunk borders (pattern seams where scatter density is high), and any future change to chunk framing silently relocates all Poisson scatter — moving the stable ids derived from those positions, which invalidates players' `scatter_removed` tombstones. Also `scatter_seed` omits `chunk.y`, so vertically stacked chunks share XZ layouts (see observation 5.3).

**Suggested resolution shape:** Derive Poisson from world-absolute cells like `JitteredGrid` — the margin-band ownership model (`PROP_MARGIN`, `scatter.rs:2–16`) already anticipates cross-border points, so the placement side needn't change.

### 2.2 Meshing workers bypass the unified rayon-pool model

**Evidence:** §12: "The generation pipeline (§5) and the mesher **both** run on `rayon`-backed worker pools." Phase 2 unified all generation onto the shared `gen_pool` (post-2 §2). Meshing still spawns raw `std::thread` workers (`meshing/mod.rs:149–160`, `mesh-worker-{i}`), and the core-budget split between the two pools is duplicated arithmetic that must agree by hand: `meshing/mod.rs:120–122` computes `usable/3` for gen to size its own pool as the remainder, while `streaming.rs:277–279` computes `usable/3` independently for its in-flight cap.

**Impact:** Two threading models to reason about (the exact situation Phase 2 was run to eliminate, post-1 §2 "Tension"), and a silent-desync hazard: retuning one file's arithmetic oversubscribes or starves cores without any compile-time link to the other.

**Suggested resolution shape:** Either move mesh workers onto the shared pool, or centralize the core-budget split in one place both consumers read. No behavior change implied otherwise.

### 2.3 Eval→storage boundary crossings follow three conventions

**Evidence:** Terrain crosses through the named permanent seam `StorageBoundary::materialize` (`world/storage_boundary.rs`; pattern established post-2 §3). Foliage crosses through free functions `paint_to_detail_layers` / `scatter_to_store` (`world/world_generator.rs:228–267`). Fluid has no eval→storage translation point at all: `ocean_fill`/`apply_biome_ponds` (`world/fluid_gen.rs`) build storage-domain `FluidLayer` directly from eval outputs inside `generate_chunk`.

**Impact:** Low today. But "the boundary is a deliberate, named seam" is one of the engine's few stated structural principles, and it has eroded once per phase; the next layers to generate (decals, lighting) have three inconsistent precedents to copy from.

**Suggested resolution shape:** None urgent. When any of these is next touched, fold the translation into the boundary module so the seam stays one thing.

### 2.4 Pin vocabulary reconciliation still owed (`Positions` vs `ScatterPoints`)

**Evidence:** §4 names the pin `ScatterPoints`; code keeps `Positions` with an in-source note (`nodegraph-ir/src/pin.rs:21–24`); post-4 §1.2 flags it "intentional divergence (**still owed reconciliation**)". Two phases later it remains unreconciled. (The other §4/§6 foliage pins — `PaintOutput`, `ScatterOutput`, `PlacementMask`, `SpeciesWeights` — do exist under their doc names, the latter two as reserved vocabulary; `pin.rs:38–49`.)

**Impact:** Minor, but the debt is explicitly on the books and every doc-guided reader of graph tooling hits it.

**Suggested resolution shape:** Close the loop either way — rename the pin or revise §4 — in whichever phase next touches `nodegraph-ir`.

Error-handling and scheduling conventions were checked and are consistent: generation is infallible-with-logged-air-chunk everywhere (`world_generator.rs:152–168`), both data registries are RON-primary with locked fallback (materials, prefabs), persistence is `Result`-based throughout, and fluid simulation runs main-thread in `FrameStage::Simulation` per §12 (`ecs/schedule.rs:49–51`). No findings there.

---

## 3. Future-phase constraints

### 3.1 Terrain modification: per-voxel edits clone the whole chunk

**Evidence:** `World::apply_edit` (`world/mod.rs:163–168`) clones the full `ChunkStorage`, sets one voxel, and re-wraps in a new `Arc` — per single voxel. §8 specifies "Brush/radius operations: bulk versions of single-anchor ops" as an interaction primitive.

**Impact:** A 100-voxel brush stroke is 100 full palette-array clones and repacks in one frame, per affected chunk. The current shape is correct but cannot host the documented brush tools; this is the first wall the terrain-modification phase hits.

**Suggested resolution shape:** A batched edit entry point — one storage clone (or `Arc::make_mut`) per chunk per edit burst, with override recording and dirty-marking amortized across the batch. The existing single-edit API can delegate to it.

### 3.2 Player systems: the walkability mask can't serve its documented runtime consumers

**Evidence:** §5 stage 6: the mask is "computed once and reused by three consumers" — slab smoothing, AI pathfinding, player movement — and "computing it once, in worldgen, before meshing, is what makes pathfinding and movement cheap at runtime." Today it is transient: computed inside `smooth_slabs` and dropped (`world/slab_smoothing.rs:37`; transience noted in post-3 §9's deferral list). Two structural gaps go beyond "not stored yet": it is computed **pre-override**, so player-placed/removed voxels are invisible to any mask a future system would consult; and nothing in `apply_edit` invalidates or recomputes it.

**Impact:** Player movement and pathfinding per §5 can't be built as a small extension — they need mask storage on the chunk, recompute-on-edit invalidation, and a post-override compute point. Building movement against ad-hoc storage queries instead would fork the single-source-of-truth the doc mandates.

**Suggested resolution shape:** When the player phase is planned, budget a substep for mask residency: store it on `LoadedChunk`, recompute on `apply_edit` (and regen/stream insert), and move (or re-run) the computation after override application.

### 3.3 Rivers and structures need a cross-chunk generation stage that doesn't exist

**Evidence:** §5 stages 8–9 specify structures ("deferred placement; may straddle chunks; cross-chunk template stamping with priority resolution") and rivers ("settled fluid cells along channels" — inherently multi-chunk). The generation contract is strictly single-chunk: `spawn_generation` (`world/streaming.rs:258`) evaluates one chunk in isolation and returns one `LoadedChunk`; `generate_chunk` has no neighbor context. Post-6's deferral already names the cost for the smoothing case ("on-demand neighbor generation (~5× eval cost) or a cached/two-phase generation pass"), and post-7 ties basin ponds to "the same terrain-modification work as the deferred rivers" — so the deferrals are acknowledged; the finding is what they jointly imply.

**Impact:** Three deferred features (rivers, structures, cross-chunk slab smoothing) all block on the same missing capability: a two-phase or staged generation pipeline where a cross-chunk pass runs between per-chunk evaluation and finalization. Building any one of them ad hoc will hard-code a shape the other two must then live with.

**Suggested resolution shape:** When the first of these features is scheduled, design the staged-generation contract once (per-chunk eval → cross-chunk pass(es) over a staging set → finalize/insert), sized for all three consumers, rather than per-feature.

### 3.4 Additional fluid types: the simulation assumes water

**Evidence:** §7 defines `FluidId` and names lava and "future fluids" as the layer's purpose. The simulation hardcodes water at both cell-creation sites: transfers into empty cells `or_insert(FluidCell { fluid_id: FluidId::WATER, … })` (`world/fluid_sim.rs:315`) and new cells at `:361`; there is no per-fluid parameterization (flow rate, settle threshold) and no cross-fluid transfer rule. "Non-water fluids" is listed out-of-scope in post-7's deferral list, so the absence is not drift — but the shape constraint is real.

**Impact:** Adding lava is not a data change (`FluidId` exists) but a simulation rework: source `fluid_id` must thread through plan/commit, and adjacent different-fluid cells need defined semantics, or lava flowing beside water silently converts to water via the `or_insert`.

**Suggested resolution shape:** Contained rework within `fluid_sim.rs` when the second fluid lands: carry the source cell's `fluid_id` through transfers and add an interaction rule table. Worth noting in the phase plan so it's budgeted as sim work, not content work.

### 3.5 Audio occlusion, fog, and debug attribute views block on the vertex format

**Evidence:** §12 audio occlusion "reuses `enclosure_factor` baked into mesh vertices — no additional data required"; §11 cave fog is "driven by `enclosure_factor` baked per vertex"; §12 debug visualization swaps which mesh attribute drives color. None of these attributes exist on `TerrainVertex` (finding 1.5).

**Impact:** These features' doc-stated "no additional data required" premise is false until 1.5 is resolved; they should not be scheduled before the vertex-format decision.

**Suggested resolution shape:** Covered by 1.5 — sequence the format decision ahead of any phase containing these.

Decal content was checked and is **not** constrained: `DecalLayer`, `decal_diffs`, and the `FaceAxis` key exist and round-trip (post-3), and a decal render path is purely additive. Additional biomes are likewise unconstrained — the manifest + per-biome params + tag invalidation already scale by data.

---

## 4. Genuine architectural mistakes

No findings beyond what is already classified above. The two candidates that approached this bar — the regen DB wipe (1.1) and the fluid-save empty-bucket ordering (1.2) — are implementation divergences from a correct documented architecture, not wrong architecture, so they are filed as drift. The remaining candidate (mesh-cache key composition) is a defensible tradeoff with costs, filed as observation 5.1.

---

## 5. Observations for consideration

**5.1 Mesh cache is content-addressed, not input-addressed as §10 specifies — stronger for correctness, weaker for hit rate.** §10 derives the key from graph hash + mesher version + registry hash + seed; the implementation hashes the actual 34³ snapshot plus material colors (`meshing/cache.rs:146–160`). This automatically captures everything §10 worries about (its stated failure mode can't occur). But the key includes the 1-voxel neighbor border, so a chunk is re-keyed every time a neighbor's edge changes during streaming, and `remove_stale_for_chunk` (`cache.rs:219`) deletes sibling-key files on every save — §10 says stale entries become "unreachable (not deleted)". Net effect: systematic miss/rebuild/delete churn during load-order-dependent streaming (the same churn that surfaced the Phase-5 slab-cache bug). If cache hit rate ever matters, keying on interior content only (border affects only boundary face culling) would make a chunk always hit its own mesh.

**5.2 The regen completion path carries stale scaffolding.** `world/regen.rs:68` still says "Water is a Phase 1 no-op" (it is Phase-7 real; `clear_all()` there is now load-bearing for correctness), and `regen.rs:82–90` is a commented-out world-cache block from the removed world cache. This is the same function housing finding 1.1 — it has drifted furthest from the systems around it and would benefit from a pass when 1.1 is fixed.

**5.3 `scatter_seed` omits `chunk.y`** (`context.rs:52–55`): vertically stacked chunks share Poisson XZ layouts. Invisible while foliage is surface-only; becomes visible the first time a DetailGraph places on cave floors or overhangs (identical prop columns aligned through the world). Folding `chunk.y` in — or resolving 2.1, which subsumes this — changes generated scatter placement, so it also moves stable ids; cheaper to do before players accumulate scatter tombstones.

**5.4 §11's LOD strategy presumes a distance gradient the camera doesn't produce.** The orthographic isometric camera at ~16 px/voxel has no near/far field — the reasoning post-5 used to defer Tier-3 billboards ("no distance gradient to exploit") applies equally to §11's "distant background chunks render as simplified meshes." The doc section may describe a camera model the engine no longer has; a settled-decision revision (or a note on what "distant" means for an iso camera — e.g. zoom levels) would prevent a future phase from building to a moot spec.

**5.5 `PrefabDef` carries none of §6's interaction metadata.** `prefabs.rs:31–42` is `{ id, id_name, shape, color, scale }`; §6's `PrefabMeta` specifies `anchor_offset`, `footprint`, `on_anchor_destroyed`, `sway_weights`. Expected while props are procedural placeholders (post-5), but two documented behaviors depend on the missing fields: placement validation ("footprint… for placement validation") and §8's "Modify terrain under foliage triggers `on_anchor_destroyed`." The terrain-modification phase will need at least the destruction policy, so the prefab schema grows again then — worth co-scheduling with real prefab meshes to avoid two `prefabs.ron` format changes.

**5.6 Streaming eviction is immediate-outside-margin rather than §12's LRU-with-hysteresis.** `streaming.rs:464–471` unloads everything outside the unload margin each tick; the load/unload margin difference supplies hysteresis, but there is no LRU band of retained out-of-radius chunks. Equivalent behavior at current radii; the difference only matters if memory pressure and view radius ever decouple (many observers, large radii), at which point re-generation cost replaces what LRU retention would have absorbed.

---

*End of review. Findings 1.1 and 1.2 are the two I'd verify and address first; both are cheap to confirm at runtime (edit a graph after placing blocks; pour water across a chunk seam, save, reload) and both silently destroy player state.*
