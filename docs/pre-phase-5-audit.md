# Pre-Phase-5 State Audit

**Status:** Substep 0 deliverable. Reference-only — no code changes.
**Date:** 2026-06-22.
**Supersedes as starting reference:** `docs/post-phase-4-audit.md` (close-of-Phase-4 snapshot).
**Authoritative targets:** `docs/engine-design.md` v1.2 — §6 (Foliage System, 3 tiers), §8 (Anchor Model), §9 (Authored/Generated Diff Model + StableInstanceId), §10 (Mesh / `FaceVertex`), §11 (Rendering, LOD billboards, sway).

This audit grounds Phase 5's substeps in the *actual* state of the code as read
on 2026-06-22, and surfaces the gaps and **reconciliation points** where the
current implementation diverges from (or already overlaps) the Phase 5 target. It
is a factual snapshot plus an explicit set of decisions to settle before code
planning begins.

> **Headline finding:** the engine **already renders grass** via a working
> `VegetationPass` that CPU-instances grass blades — but driven by *voxel
> material* (`grass_soil`), **not** by `DetailLayers`/`DetailGraph`. Phase 5's
> design-mandated Tier 1 (GPU-generated blades from density texels) overlaps and
> must reconcile with this existing system. See §4 and §9.

---

## §0. Scope of Phase 5 (recap, for grounding only)

Phase 5 makes foliage real across all three tiers in one phase: Tier 1 density
paint (GPU-generated blades from `DetailLayers` texels), Tier 2/3 discrete scatter
(`ScatterStore` instances, hardware-instanced; trees get LOD billboards), the
anchor model for player interaction, and the generated/authored diff model with
stable instance IDs. 11 substeps (0 audit + 1–11). Determinism mandatory: same
seed + DetailGraph + chunk coord → same texels, same instance positions, same
stable IDs. The verification sweep requires runtime smoke testing.

Out of scope (carried Phase 4 deferrals + new): density-level boundary blending,
cross-graph named boundary pins, per-biome `traversal_smoothing_distance`,
multi-distance slab smoothing, library activation; fluid sim, decals, audio
occlusion, AI/movement, full edit toolkit, animated foliage beyond shader sway.

---

## §1. Foliage data model — homes exist, two shape gaps

All foliage container types live in `voxulacrum-app/src/world/layers.rs` (per
voxel-core's charter, sidecar layers live in the app crate). They are **empty
scaffolding** (`#![allow(dead_code)]`, default-constructed, no producers).

### 1.1 Tier 1 — `DetailLayers` (`layers.rs`)

```
DetailTexel  { species: u8, density: u8, tint: u8, flags: u8 }   // 4 bytes
DetailLayer  { layer_id: DetailLayerId, map: Box<[DetailTexel; 32*32]> }
DetailLayers { layers: SmallVec<[DetailLayer; 4]> }   // Default = empty
DetailLayerId(pub u16)
```

Matches design doc §6 exactly (one texel per chunk column). `DetailLayer::new(id)`
builds an all-empty map. **Ready as-is** for Phase 5 to populate.

### 1.2 Tier 2/3 — `ScatterStore` (`layers.rs`) — **shape gap**

```
ScatterInstance { anchor: LocalPos, sub_offset: [i8;3], rotation_y: u8,
                  scale_variant: u8, prefab_id: PrefabId, flags: ScatterFlags }
ScatterStore    { by_type: HashMap<ScatterTypeId, Vec<ScatterInstance>> }   // Default = empty
ScatterTypeId(pub u16)   PrefabId(pub u32)   ScatterFlags(pub u8)
ScatterFlags consts: PLAYER_PLACED, HARVESTABLE, PERSISTENT
```

**Two gaps vs Phase 5 requirements:**
1. **`ScatterStore` is `{ by_type }`, not `{ generated, overrides }`** (design doc
   §9). Substep 6/9 splits it: `generated: HashMap<ScatterTypeId, Vec<…>>` +
   `overrides: ScatterOverrides { removed: HashSet<StableInstanceId>, added: Vec<…> }`.
2. **`ScatterInstance` has no `stable_id` field.** Substep 5 must add one (and a
   `generated` flag distinguishing generated from player-placed). This ripples to
   persistence (§2) and the `Default`/`Pod`-free derive set.

> **Note:** `ScatterInstance` derives `Copy` (no heap fields). Adding a
> `StableInstanceId(u64)` keeps it `Copy`. Anchor is already `LocalPos` per design
> doc §8 — the field exists but is never populated meaningfully today.

### 1.3 Identity types (`layers.rs`) — ready

`StableInstanceId(pub u64)` exists with the right derives (`Copy/Eq/Hash`). The
doc's scheme `hash(world_seed, world_pos, prefab_id, sequence_in_anchor)` is
**not yet implemented** — Substep 5 implements it. `FluidId`, `DecalEntry` exist
(out of scope this phase).

---

## §2. `ChunkOverrides` + persistence — round-trips, not applied

### 2.1 `ChunkOverrides` (`world/overrides.rs`)

Matches design doc §9 shape exactly:

```
ChunkOverrides {
    voxel_diffs: HashMap<LocalPos, Voxel>,            // applied today (Phase 3)
    voxel_removed: HashSet<LocalPos>,                 // reserved, empty
    scatter_removed: HashSet<StableInstanceId>,       // reserved, empty — Phase 5 makes live
    scatter_added: Vec<ScatterInstance>,              // reserved, empty — Phase 5 makes live
    detail_diffs: HashMap<(DetailLayerId, LocalPos), DetailTexel>,  // reserved
    fluid_diffs, decal_diffs                          // out of scope
}
```

Only `voxel_diffs` is populated/applied today. `set_voxel`, `is_empty`,
`voxel_override_count` are the API. `Chunk.overrides: Option<ChunkOverrides>`.

### 2.2 Persistence (`world/persistence.rs`) — v5, round-trips scatter

The v5 `BLOB_VERSION = 5` format **already serializes `scatter_removed` and
`scatter_added`** (persistence.rs imports `ScatterFlags, ScatterInstance,
StableInstanceId`; `read_tags`/override read paths handle them; tests at
persistence.rs ~860–890 round-trip `StableInstanceId` + a `PLAYER_PLACED`
instance). **But the load path does not *apply* them** — overrides are
deserialized into `ChunkOverrides` and only `voxel_diffs` reaches the chunk.

> **Adding `stable_id` to `ScatterInstance` (§1.2) changes the on-disk encoding**
> of `scatter_added`. Substep 5/9 must bump the scatter section version (or
> handle absence) so old saves load forward-compatibly. **Surface during 5/9.**

### 2.3 Application site

Generation flows: `WorldGenerator::generate_chunk(pos) -> GeneratedChunk {
storage, tags }` → `streaming.rs`/`world/mod.rs` assemble `LoadedChunk::new(pos,
storage)` and set `chunk.data.tags`. For loaded chunks, `streaming.rs` reads the
DB `ChunkRecord { edits, tags }` and applies `voxel_diffs` to storage. **Foliage
override application** (subtract `scatter_removed`, union `scatter_added`) belongs
here — after `ScatterStore.generated` populates, before the chunk enters the live
set. `GeneratedChunk` must grow to carry foliage (`{ storage, tags, detail,
scatter }`).

---

## §3. DetailGraph — tag-only, no evaluator dispatch

- **IR:** `GraphKind::Detail` exists (Phase 4) as a variant only;
  `validate_kind_rules` is a no-op for it. No detail-specific nodes or pin types.
  `PinType` has 12 variants (no `PaintOutput`/`ScatterOutput`/`PlacementMask`/
  `SpeciesWeights`). **Substep 1 adds them.**
- **Evaluator:** `WorldEvaluator` (`nodegraph-eval/src/world_eval.rs`) holds
  `world`, `zone`, `biomes: Vec<BiomeGraph>`, `libraries`. **No detail slot, no
  detail dispatch.** `evaluate_chunk` produces `ChunkEvaluation { terrain,
  world_columns, zone_columns }` — no foliage. **Substep 2 adds a per-biome
  optional DetailGraph + a `ChunkFoliage` output.**
- **Manifest:** `world.manifest.json` → `WorldManifest { world: String, zone:
  String, biomes: Vec<BiomeManifestEntry { id: u16, graph: String }> }`.
  **Substep 10 extends `BiomeManifestEntry` with an optional `detail: Option<String>`.**
- **Existing voxel-stamping nodes (distinct from foliage):** `NodeKind::PlaceTree`,
  `PlacePrefab`, `JitteredGrid`, `PoissonDisk`, `FindFlat` exist (Phase 3-era) and
  stamp **voxels** into terrain via `PrefabTemplate` (`nodegraph-ir/src/prefab.rs`).
  **These are not foliage instances** — a `PlaceTree` builds a voxel trunk+canopy
  baked into the terrain mesh. The starter biome graphs (meadow/rocky) **do not
  use them.** Phase 5 Tier 3 trees are *instanced foliage*, a separate path. Avoid
  conflating `PrefabTemplate` (voxel stamp) with the Tier 2/3 `PrefabId` (mesh ref).

---

## §4. Renderer — the big reconciliation surface

### 4.1 Draw flow

There is no modular per-pass render graph in practice for the scene; `render_graph.rs`
defines a `RenderPassNode` trait + ordered list, but the scene is drawn by **one
combined `MainScenePassNode`** (`rendering/main_scene_pass.rs`) that, in order,
into `SCENE`/`NORMAL`/`DEPTH`:
1. **Terrain** — per visible `LoadedChunk`, `chunk.mesh` (indexed draw).
2. **Cross-section cap** mesh.
3. **Vegetation** — `vegetation_pipeline`, per-chunk instanced grass (frustum-culled).
4. **Water** (Phase-1 no-op meshes).
5. **Debug lines.**

Other passes: `shadow_pass`, `outline_pass`, `palette_pass`, `post_process`,
`upscale_pass`. Pipelines + WGSL hot-reload via `pipelines.rs` (a `PipelineId`
registry; `vegetation.wgsl` is hot-reloadable).

**Phase 5 render passes slot in** as new draws after terrain/cap and around
vegetation: a Tier-1 paint pass (or a rework of the vegetation draw) and a Tier-2/3
instance pass, then a billboard pass. The directive "two new siblings, terrain
unchanged" maps cleanly onto adding draws in/after `MainScenePassNode`.

### 4.2 Actual mesh vertex format — **not** the doc's `FaceVertex`

The real terrain vertex (`pipelines.rs`):

```
TerrainVertex { position:[f32;3], normal:[f32;3], color:[f32;3], ao:f32,
                material_id:u32, cell_flags:u32, _pad:[u32;2] }  // 64 bytes
```

This is **simpler** than design doc §10's aspirational 32-byte `FaceVertex`
(no `sway_weight`, `biome_tint_index`, `occlusion_class`, etc.). **Implication:**
Phase 5's "mesh format gains `sway_weight`" applies to the **foliage** vertex
formats (grass blade + prefab meshes), **not** terrain. Terrain stays as-is.

### 4.3 Existing `VegetationPass` — overlaps Tier 1, wrong data source

`rendering/vegetation_pass.rs` (a `bevy_ecs::Resource`) already:
- Holds a shared 3-vertex `GrassVertex { position, uv, _pad }` blade mesh +
  per-chunk `GrassInstance { position, scale, rotation, blade_phase,
  terrain_color, _pad }` buffers (`chunk_vegetation: HashMap<IVec3, …>`).
- **Generates instances by scanning chunk voxels for `grass_soil` material**,
  checking air-above + slope (`collect_chunk_grass_instances`), with per-blade
  world-position hashing (`hash_position_seed`). `blades_per_voxel`,
  `blade_height`, `blade_half_width` come from `VegetationParams`.
- Rebuilds fully on terrain regen (`VegetationPass::new`); `add_chunk_vegetation`/
  `remove_chunk_vegetation`/`clear_all` for streaming; rebuilt in `regen.rs` after
  regeneration.
- Wind sway already exists in `vegetation.wgsl` via `blade_phase`.

**This is a proto-Tier-1 that predates the foliage data model.** It is CPU-side
per-blade instancing (Phase 5's Tier 1 wants GPU-generated-from-density-texels),
and it sources from voxel material instead of `DetailLayers`. The note at
`vegetation_pass.rs:226` even flags "flora_id dropped from storage (Phase 3);
reconstruct eligibility from material type." **This is the central Phase 5 render
decision (§9).**

### 4.4 No instancing infra for prefab meshes / billboards

There is **no scatter-instance pass, no prefab-mesh system, no billboard pass, no
2D texture-array upload for detail maps**. Tier 2/3 (Substep 6/7) is net-new GPU
work. The vegetation per-chunk instance-buffer pattern (and frustum-culled
instanced draw) is a reusable template.

---

## §5. Input / interaction — keyboard + scroll only; no targeting

`input.rs` is a clean action-mapping system, but its surface is **narrow**:
- `RawInputEvent` = `KeyPressed | KeyReleased | Scroll`. **No mouse buttons, no
  cursor position, no mouse-move events.**
- `GameAction` = camera pan/rotate/zoom + `ToggleUI/GraphEditor/FieldProbe`. **No
  interaction/edit actions.**
- **No raycast utility, no voxel-targeting code, no picking** anywhere in the app
  (confirmed by search; the Phase 4 audit's "no manual edit controls" holds).
- Camera is isometric/orbit (pan + Q/E rotate + scroll zoom).

**Substep 8 builds from scratch:** mouse-button + cursor-position raw events, new
`GameAction`s (remove/place foliage), a camera/cursor → world raycast (DDA voxel
march), the anchor-target query (`ScatterInstance`s + `DetailTexel` at a cell),
and anchor-highlight visual feedback (the `debug_lines` pass is a candidate for
the wireframe overlay).

---

## §6. `StableInstanceId` production readiness

The type is ready (§1.3). Production requires (Substep 2/5): a stable hash over
`(world_seed, world_pos, prefab_id, sequence_in_anchor)` using the established
non-order-dependent RNG discipline (same family as `EvalContext::world_cell_seed`
/ `scatter_seed` in `nodegraph-eval/context.rs`, which already hash
`(world_seed, node_seed, cell coords)` seam-safely). `sequence_in_anchor`
disambiguates multiple instances sharing an anchor cell. IDs are **not persisted**
(rederivable); only diffs (`scatter_removed` ids + `scatter_added` instances) are.

---

## §7. Deferred items inherited from Phase 4 (still deferred)

Confirmed from the Phase 4 audit §5, all still deferred and out of Phase 5 scope:
density-level boundary blending; single→multi neighbor borders; `LibraryGraph`
activation + dynamic boundary pins + standard libraries; `DetailGraph` was on this
list — **Phase 5 activates it**; multi-distance slab smoothing; multi-zone-per-chunk
tags; per-biome `traversal_smoothing_distance`; cross-graph boundary inputs;
fluids/sea-level/rivers. The Phase 5 audit will re-confirm the survivors.

---

## §8. Decisions to settle before / during code planning

1. **VegetationPass reconciliation (§4.3) — the load-bearing one.** Options:
   (a) **Replace** the voxel-material grass with a new Tier-1 paint pass driven by
   `DetailLayers` density texels (GPU-generated per the doc); retire
   `collect_chunk_grass_instances`. (b) **Rework in place**: keep the
   `VegetationPass` machinery but feed it from `DetailLayers` instead of voxel
   material, and move blade generation GPU-side. (c) Build new alongside, retire
   old at Substep 4. The doc mandates GPU-generated-from-texels; (a)/(b) both honor
   it. Decide at Substep 4.
2. **Tier 1 GPU approach:** vertex-pulling from a per-chunk detail texture array
   vs. instancing. The doc says "millions of blades → GPU-side." The existing
   instanced path may not scale to the density target; surface a performance call
   at Substep 4 before transcription.
3. **`ScatterInstance` shape:** where `stable_id` + a `generated` flag live, and
   the resulting scatter-section persistence version bump (§2.2). Decide at
   Substep 5.
4. **`ScatterStore` split** `{ generated, overrides }` and effective-content
   computation (lazy vs cached) — Substep 6/9.
5. **Manifest extension** for per-biome `detail` reference (`BiomeManifestEntry`)
   — Substep 2/10.
6. **Prefab mesh asset format:** `assets/prefabs/*.mesh.json` vs runtime-procedural
   starter meshes. The doc allows runtime-procedural to avoid asset-pipeline
   ballooning. Decide at Substep 6.
7. **Foliage RNG seed derivation** convention `(world_seed, node_id, world_pos,
   purpose)` — reuse the `context.rs` mix64 family. Substep 2.
8. **`GeneratedChunk` growth** to carry `{ storage, tags, detail, scatter }` and
   the streaming/regen assembly updates — Substep 3/5.

Several substeps are split candidates per the prompt: 1 (pins / nodes), 4 (pass /
shader), 5 (poisson+filter / species+assembly / store+persistence), 6 (instance
pipeline / prefab system). The split call is made during each substep's planning
based on transcription length.

---

## §9. Build-vs-exists quick table

| Capability | State |
|---|---|
| `DetailLayers` / `DetailTexel` types | exists (empty, ready) |
| `ScatterStore` `{ by_type }` | exists — **needs `{ generated, overrides }` split** |
| `ScatterInstance` | exists — **needs `stable_id` + `generated` flag** |
| `StableInstanceId` type | exists; **hashing scheme not implemented** |
| `ChunkOverrides` scatter/detail fields | exist; **round-trip but not applied** |
| v5 persistence of scatter overrides | exists (encoding changes when `stable_id` lands) |
| Foliage pin types / nodes (`PaintOutput`, …) | **not built** |
| `DetailGraph` evaluator dispatch | **not built** (tag-only) |
| `ChunkFoliage` output / per-biome detail slot | **not built** |
| Tier 1 grass rendering | **exists, but voxel-material-driven** (reconcile) |
| Tier 1 from `DetailLayers` texels (GPU-gen) | **not built** |
| Tier 2/3 instance pass / prefab meshes | **not built** |
| Tier 3 LOD billboards | **not built** |
| Wind sway (grass) | exists (`blade_phase` in `vegetation.wgsl`) |
| Raycast / voxel targeting / mouse input | **not built** (keyboard+scroll only) |
| Anchor-target query / player tools | **not built** |
| Manifest per-biome detail reference | **not built** |
| Starter-world DetailGraphs / prefab assets | **not built** |

---

*End of pre-Phase-5 audit. This is the reference for Phase 5 substep planning.*
