# Post-Phase-8 Audit — Consolidation (Mutation Command API + Drift Fixes)

Close-of-phase snapshot for **Phase 8 (Consolidation, Standard scope)**.
Companion to `post-phase-7-audit.md`, the drift diagnosis
`architecture-drift-review.md`, and the planning reference
`pre-phase-8-audit.md`. Records what shipped, the mutation seam's coherence,
test coverage, the accepted stable-ID break, and the deferred backlog.

**Status: complete.** Every world-state mutation now flows through a single
mode-aware command API; the two silent fluid/foliage data-loss bugs are fixed;
the generation pipeline order matches design §5; PoissonDisk seeds world-absolute;
meshing runs on the shared `gen_pool`; and batched voxel edits clone chunk storage
once. Branch `mc-revision`.

## What Phase 8 delivered

- **Mutation command API (Substep 1, three parts).** `World::execute(MutationCommand)`
  is the single entry point for every world mutation (design §9). A command
  declares its `MutationOrigin` (`Authoring` / `PlayTime`) and its intent
  (`WorldMutation`); `execute` validates origin against the runtime `EngineMode`
  on `World` (`Authoring` only this phase; `Play` in Phase 9) and returns
  `MutationError::WrongMode` on mismatch, else dispatches to the intent's handler.
  Handlers own their own persistence marking, mesh-dirty flagging, override-bucket
  creation, and invalidation, and return a `MutationOutcome` naming the GPU-side
  rebuilds (scatter/water) the caller must service. Intents shipped: `EditVoxel`,
  `EditVoxelBatch`, `RemoveScatter`, `PlaceScatter`, `PourFluidColumn`,
  `MarkFluidDirty`, `InsertLoadedChunk`, `SwapRegeneratedChunks`. Every prior
  mutation call site migrated: `scatter_edit_system`, `fluid_tick_system` +
  `pour_water_column` (deleted, relocated as handlers), streaming's chunk insert,
  and the regen swap. `World::apply_edit` is now a thin wrapper over `EditVoxel`.
  Mode enforcement is real and unit-tested against synthesized `PlayTime` commands
  (both rejection directions).
- **Fluid persistence empty-bucket fix (Substep 2, drift 1.2).** `build_chunk_edits`
  decouples "has fluid to snapshot" (`!fluids.cells.is_empty()`) from "override
  bucket empty": the only `None` case is now *empty bucket AND no fluid*, so a
  chunk water merely *flowed into* (empty bucket, real cells) saves its fluid via
  the delta arm. The `Full`-branch fluid drop (voxel-promoted chunks) is documented
  as an accepted constraint — unreachable by any live Phase-8 path (needs
  >`DELTA_THRESHOLD` voxel edits; any real fluid change creates a bucket) and its
  proper fix rides with the deferred 1.3 diff-with-tombstones migration.
- **Pipeline order restoration (Substep 3, drift 1.4).** `generate_chunk` now
  initializes fluids (stage 9) before translating foliage (stage 10) and drops any
  paint/scatter whose surface the finished fluid field submerges, via a shared
  `fluid_gen::foliage_submerged` predicate (covers ocean fast-path, explicit
  cells, and ponds). A dry fast-path (`Empty` fill + no cells) skips the surface
  scan, so above-sea biomes generate byte-identically. `WorldEvaluator::evaluate_chunk`
  reorders `composite_fluid` before `evaluate_foliage` to match §5's stage sequence.
- **PoissonDisk world-absolute seeding (Substep 4, drift 2.1).** `scatter_seed`
  (chunk-coordinate-based) became `chunk_world_seed` (world-absolute, seeded from
  `chunk * CHUNK_DIM`, folding `chunk.y` — fixes observation 5.3). The stale
  "acceptable for Phase 10" acceptance comment is gone. **See "Accepted stable-ID
  break" below.**
- **Meshing worker pool unification (Substep 5, drift 2.2).** Mesh work is now
  fire-and-forget `spawn`ed onto the shared `gen_pool`; the dedicated `std::thread`
  mesh workers and bounded request channel are gone (`grep std::thread` finds no
  chunk work in the app). Core-budget arithmetic centralizes in
  `core_budget::CoreBudget::detect()`, consulted by pool sizing (`main`),
  streaming (`gen_in_flight`), and meshing (`mesh_in_flight`); the two in-flight
  caps sum to `pool_threads`, eliminating the prior oversubscription (the old code
  ran a full `gen_pool` *plus* ~2/3·usable separate mesh threads). `num_cpus::get()`
  now appears only in `core_budget.rs`.
- **Batched voxel edit entry point (Substep 6, drift 3.1).** `EditVoxelBatch`
  clones a chunk's storage exactly once per batch, applies all edits, does one
  delta→full promotion check, and marks persist/mesh dirty once (plus the
  deduplicated border-neighbor union). `EditVoxel` delegates as a batch of one, so
  the single- and batched-edit forms share one implementation. A 100-voxel brush
  stroke now costs one clone, not 100 — the foundation Phase 9's brush tools build
  on. No live caller yet (`#[allow(dead_code)]` on the variant).

## Mutation seam coherence
- **Every** resident-set mutation flows through `World::execute`. Raw
  `chunks.insert` / `chunks.extend` / `persist_dirty = true` / `set_voxel`-on-storage
  live only in `world/mod.rs` handlers (and the `insert_chunk` primitive the insert
  handler delegates to). ECS systems, streaming, and regen construct commands and
  service the returned `MutationOutcome`.
- **Origin assignment is provisional.** Every live site declares
  `MutationOrigin::Authoring` this phase (the engine is authoring-only; scatter/
  fluid/debug edits are dev actions). When Play mode lands in Phase 9, the
  player-edit systems that replace these sites will declare `PlayTime`, and the
  mode gate — already exercised by tests — becomes load-bearing at runtime.
- **The API absorbs the Substep-2 fix cleanly.** The fluid persistence bug lived at
  one seam (`MarkFluidDirty` handler → `build_chunk_edits`), so the fix touched one
  place rather than chasing call sites — the payoff the centralization was designed
  to deliver.

## Accepted stable-ID break (PoissonDisk)
Per the phase decision, `PoissonDisk`'s seed derivation changed, so **every
`StableInstanceId` derived from a `PoissonDisk` scatter position relocates** — a
one-time break, accepted because no pre-release player content exists to migrate;
post-fix, IDs are stable across future runs. In practice **no shipped scatter
moves**: the live meadow detail graph uses `PoissonDistribution` (a world-cell
jittered grid, already world-absolute and seam-continuous), not `PoissonDisk`.
`PoissonDisk` (and the unwired `PoissonPlacement` kernel) are the only consumers of
the changed seed, so the break is prospective — it affects any future graph that
uses `PoissonDisk`.

**Seam honesty:** the fix makes the *seed* world-absolute; it does **not** make the
Poisson point set seam-continuous. Bridson is a single global sequential walk
seeded once per chunk, so margin-band points still differ across a chunk border.
True seam continuity would need a world-tiled Bridson (out of scope); the
margin-band ownership model already handles props whose footprint straddles a
border. This is documented in `context.rs::chunk_world_seed` and `scatter.rs`.

## Test / verification coverage
- **Mutation API** (`world/mod.rs::mutation_tests`): mode rejection both directions
  (`PlayTime` in `Authoring`, `Authoring` in `Play`), voxel apply, scatter place
  (persist + rebuild), pour fluid (overrides + activation + determinism), mark
  fluid dirty (persist + bucket), insert-loaded-chunk (face-neighbor mark + seq
  bump), swap-regenerated-chunks (merge + adopt generator + all-dirty),
  batch≡sequential-singles (byte-identical storage + overrides), empty-batch no-op.
- **Fluid persistence** (`persistence.rs`): `fluid_only_chunk_saves_and_reloads`
  (the 1.2 regression), `empty_bucket_without_fluid_saves_nothing`.
- **Foliage submersion** (`fluid_gen.rs`, `world_generator.rs`):
  `foliage_submerged_detects_water_above_surface`, `submerged_scatter_is_dropped`,
  `submerged_fill_mode_drops_all_foliage`; existing translator + determinism tests
  updated to the new signatures and still green.
- **PoissonDisk** (`scatter.rs`): `poisson_disk_is_deterministic` (order-independent),
  `poisson_disk_varies_with_chunk_y` (5.3), existing min-distance test unchanged.
- **Core budget** (`core_budget.rs`): floors-at-two / reserves-cores.
- **Runtime smoke (operator-confirmed):** cross-seam water flow persists through
  unload/reload (2); foliage does not generate underwater when `sea_level` is
  raised to submerge the meadow, and the meadow is visually unchanged at
  `sea_level: 24` (3); scatter place/remove and G-key pour behave as before through
  the API (1); meshing runs on `gen_pool`, marginally faster (5).

## Deferred (intentional, unchanged from Phase 7 unless noted)
- **Fluid diff-with-tombstones** (drift 1.3) — interim full-snapshot retained; the
  `Full`-branch fluid carry rides with this migration (Substep 2 documented the
  constraint). Lands with player-water interaction or ocean-save-bloat pressure.
- **Regen override preservation** (drift 1.1) — reclassified as intended §9
  authoring behavior; regen wipes overrides + DB by design. No fix; do not add
  preservation to the regen path.
- **Vertex format `TerrainVertex` → `FaceVertex`** (drift 1.5) — Phase 9's first
  substep, before any §11 rendering feature.
- **Per-layer save versioning** (drift 1.7) — before first release.
- **Greedy meshing** (drift 1.6) — deferred; trigger condition is draw-time budget.
- **Eval→storage boundary consistency** (drift 2.3), **pin vocabulary
  `Positions`/`ScatterPoints`** (drift 2.4) — opportunistic when next touching
  those paths.
- **Restart initial-fill overrides, upward fluid flow / compression, per-tick
  deferral budget, cross-chunk corner-cell leak, rivers, player-water interaction,
  non-water fluids, walkability mask residency, terrain modification** — all
  Phase 9+ per the Phase-7 deferral list; confirmed still deferred.

## Verification results
- `cargo build --workspace`: **0 warnings** (Phase-7 bar restored after fixing the
  two mutation-API warnings — an unused `MutationOrigin` import and the Phase-9
  `EngineMode::Play` variant, now `#[allow(dead_code)]`).
- `cargo test --workspace`: **green, 0 failures** across all crates.
- `cargo clippy --workspace`: pre-existing pedantic lints only (map_or, div_ceil,
  too-many-arguments, Range::contains families); Phase 8 added no new lint family —
  new `map_or(false, …)` uses match the established house style.
- On-disk format unchanged (`BLOB_VERSION`/`VOXEL_FORMAT_VERSION` untouched; the 1.2
  fix changes *which* chunks populate `fluid_diffs`, not the encoding).

## State for Phase 9
The workspace is in the state Phase 9 (the player phase) begins from: a coherent
mutation seam that player-edit, networking, and modding systems consume; a real
(if single-valued) mode model whose enforcement Phase 9's `Play` mode will
exercise; batched voxel edits ready for brush tools; and the pipeline/persistence/
threading drift closed. Phase 9's first substep is the `FaceVertex` migration
(drift 1.5), then walkability mask residency, player movement/collision, camera
occlusion + cutaway (§11), and a controller-ready input layer.
