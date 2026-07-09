# Post-Phase-5 Audit — Foliage Content

Close-of-phase snapshot for **Phase 5 (Foliage Content)**. Companion to
`post-phase-4-audit.md`. Records what shipped, the state of the foliage stack,
test/verification coverage, and the deferred/known-limitation backlog.

**Status: complete.** Every substep landed and was runtime-verified; the
Substep-11 cleanup sweep is applied; the phase is committed as a single Phase-5
commit on branch `mc-revision`. Substep 7 (LOD billboards) was deferred by
choice — see Deferred below.

## What Phase 5 delivered

Three foliage tiers now generate from DetailGraphs and render end-to-end, plus
anchor-based player interaction and a data-driven prefab system:

- **Tiers + eval (1a–2b):** foliage `PinType`s + `NodeKind`s (Poisson, SurfaceFilter,
  BiomeContextMask, SpeciesPicker, PaintDensity, ScatterPlace); eval-domain
  `ChunkFoliage`; `DetailEvaluator`; `WorldEvaluator::evaluate_foliage` (per-biome,
  disjoint union).
- **Tier-1 paint (3, 4a):** `paint_to_detail_layers` boundary; `DetailPaintPass` +
  `shaders/detail_paint.wgsl` — GPU-generated grass blades from a per-chunk density
  storage buffer, with density-driven count, wind sway, and tint. Retired the old
  voxel-material `VegetationPass` from the render path.
- **Tier-2/3 scatter (5, 6):** `scatter_to_store` boundary; `ScatterPass` +
  `shaders/scatter.wgsl` — per-chunk instanced props; the meadow authors a
  `Poisson → SpeciesPicker → ScatterPlace` chain.
- **Prefabs (10):** data-driven `PrefabRegistry` (`prefabs.rs` + `assets/prefabs.ron`,
  RON-primary with a `load_initial()` fallback locked by a test). Three procedural,
  vertex-colored props — `grass_tuft`, `bush`, `rock` — built per `PrefabShape`;
  `ScatterPass` holds one mesh per prefab and groups each chunk's instances by prefab
  (one instanced draw per prefab), tinting from the prefab color. The meadow graph
  authors distinct grass/bush/rock chains by `type_id`/`prefab_id`.
- **Diff model + persistence (9):** `ScatterInstance.stable_id`; `ChunkOverrides.
  effective_scatter` (generated − removed + added); persistence `BLOB_VERSION` 5→6 and
  `VOXEL_FORMAT_VERSION` 2→3 (the latter one-shot-wipes stale saves + mesh cache on open).
  The streaming Delta-reload path now carries `scatter_added`/`scatter_removed` forward.
- **Interaction (8a–8c):** `PointerState` (cursor + L/R buttons); `interaction.rs`
  `picking_system` (screen→world ray via inverse `view_projection` + voxel DDA →
  `PickState.anchor`, highlighted by reusing `DebugLinePass`); `scatter_edit_system`
  (left-click removes props at the picked surface anchor → `scatter_removed`; right-click
  places one → `scatter_added`, `PLAYER_PLACED`, with a salted stable id).

### Fixed in-flight this phase
- **Mesh-cache lossy vertex quantization** (root cause of the "reloaded chunks lose
  slabs" saga): the cache's compact vertex format quantized positions and dropped the
  half-voxel offset that defines a slab top, so any mesh round-tripped through the cache
  lost its slabs. Fixed by Cowork/Fable 5 in `meshing/cache.rs`.
- **World cache removed:** it stored voxels only (no foliage) and went stale as the
  pipeline grew, so cached chunks loaded without grass. The world always regenerates at
  startup now. `clear_world_cache` is retained (used by regen + the format-version wipe).

## Foliage stack coherence
- Generation: `WorldGenerator::generate_chunk` → eval → materialize → `smooth_slabs` →
  tags → `paint_to_detail_layers` + `scatter_to_store`. Same generator `Arc` everywhere
  (startup / streaming / regen).
- Storage: `Chunk.detail_layers` (Tier-1), `Chunk.scatter_instances` (generated Tier-2/3),
  `Chunk.overrides.scatter_added/removed` (player diff). `effective_scatter` unifies them.
- Render: `MainScenePassNode` draws terrain → cap → detail paint → scatter → water →
  debug lines → pick highlight. `PipelineRegistry` hot-reloads shaders by filename.

## Test / verification coverage
- Determinism/mapping tests: `scatter_is_deterministic_and_nonempty` (2a),
  `scatter_translates_to_store_carrying_stable_id` (5/9), `paint_translates_to_detail_layers`
  (3), `effective_scatter_filters_removed_and_appends_added` (9), `smooth_slabs`
  determinism (Phase 3), `generate_chunk_is_deterministic` (added during the slab hunt —
  ⚠️ see backlog: it may test a slab-free chunk, so add a non-vacuous slab-bearing case).
- Prefab registry: `parses_ron_and_orders_by_id`, `non_contiguous_ids_rejected`,
  `fallback_matches_shipped_ron` (locks `prefabs.ron` to `load_initial()` via `include_str!`).
- Persistence roundtrip (`sample_overrides`) exercises `stable_id`.
- Runtime smoke is the real validator for all rendering/interaction.

## Deferred (intentional)
- **Substep 7 — Tier-3 LOD billboards:** deferred. With the orthographic iso camera and
  ~16 px/voxel there's no near/far distance gradient to exploit and props are already
  pixel-sized, so billboards give little visual gain; per-prefab instanced draws are
  already cheap, so perf gain is marginal. Revisit if scatter density/draw counts become a
  bottleneck (then it's a perf feature, not a visual one).
- **Phase-4 deferrals still open:** density-level boundary blending, cross-graph dataflow
  with named boundary pins, per-biome `traversal_smoothing_distance`, multi-distance slab
  smoothing, library activation.
- **Retired `VegetationPass`:** `rendering/vegetation_pass.rs` is orphaned (not in the mod
  tree); `create_vegetation_pipeline` + `vegetation.wgsl` + `VegetationParams` + the
  registry's `vegetation_pipeline` are still built but undrawn. Safe to delete wholesale in
  a future tidy.
- **Prefab size at pixel density:** props were tuned up (`grass_tuft`/`bush`/`rock` scale
  0.65/0.85/0.75) for visibility, but at ~16 px/voxel `grass_tuft` still reads as a speck.
  Further sizing is a one-line tune in `prefabs.ron` (mirror in `load_initial()` to keep the
  lock test green — the sync is enforced, so a drift here fails `fallback_matches_shipped_ron`).
- Only the meadow biome (0) has a DetailGraph; the rocky biome (1) has no foliage by design.

## Known tech-debt backlog (pre-existing, out of Phase-5 scope)
`cargo check -p voxulacrum` reports ~30 dead-code warnings that predate Phase 5 —
intentional Phase-3 stubs (`Chunk.fluids/decals/lighting`, `overrides::set_voxel`) and
accumulated unused items (`CacheStats`, `disk_usage`, `PostProcess` variant,
`render_targets` fields, `ChunkSnapshot::get_voxel`, `border_dirty_neighbors`,
`world_generator` `new/from_path/graph`, etc.). Not addressed here; worth a dedicated
dead-code pass. Phase 5 introduced zero net-new warnings after the sweep in Substep 11.

## Verification results
- `cargo check` / `cargo build` clean — warnings down to the pre-existing backlog above;
  Phase 5 added zero net-new after the Substep-11 sweep.
- `cargo test` green, including `fallback_matches_shipped_ron` after syncing `load_initial()`
  to the tuned `prefabs.ron` scales.
- Runtime: three tiers render on the meadow; place/remove interaction works and persists
  across unload→reload and regen; slabs persist on reload (the cache bug is fixed).
- Committed as a single Phase-5 commit on branch `mc-revision`.
