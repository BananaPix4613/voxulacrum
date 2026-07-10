# Post-Phase-7 Audit — Fluid Simulation

Close-of-phase snapshot for **Phase 7 (Fluid Simulation, Standard scope)**.
Companion to `post-phase-6-audit.md` and the planning doc `pre-phase-7-audit.md`.
Records what shipped, the fluid stack's coherence, test coverage, and the
deferred backlog.

**Status: complete.** Ocean fills at sea level, water renders transparent and
depth-shaded, a mass-conserving `u16` cellular-automata simulation flows water
when disturbed (down + horizontal, settling), flow crosses chunk boundaries
seamlessly, the sim runs at a fixed 10 Hz on the frame schedule, and disturbed
fluid persists (resuming mid-flow) across chunk unload/reload and save/close.
Branch `mc-revision`.

## What Phase 7 delivered

- **`sea_level` + ocean fill (Substep 1).** `sea_level` is a manifest constant
  (`world.manifest.json` → `WorldManifest`/`WorldGenerator`, decided over a graph
  output for a global scalar). `world/fluid_gen.rs::ocean_fill` runs after voxel
  materialization: chunks entirely at/below sea level use the O(1)
  `FluidFillMode::Submerged(WATER)` fast path; straddle chunks get explicit
  settled cells for below-sea empty voxels. Threaded through `GeneratedChunk.fluids`
  into every apply site (initial fill, regen, streaming). Generated fluid is not
  serialized - it re-derives deterministically.
- **Water rendering (Substep 2).** Fed the pre-existing (Phase-1-stubbed) `WaterPass`
  real geometry: `build_water_mesh` emits a top-face quad per water cell whose cell
  above is open air, carrying per-vertex water-column depth. The existing
  `water.wgsl` + alpha-blended pipeline (draw slot already after foliage) handle
  transparency, deep/shallow color, and depth. Deep chunks emit no surface.
- **`FluidOutput` node kind (Substep 3).** New biome terminal (`nodegraph-ir`) with
  `level` (Scalar) + `mask` (Density) inputs; the world evaluator harvests a
  per-column pond level (`composite_fluid` in `world_eval.rs`, reading the biome's
  density-eval cache), and `apply_biome_ponds` fills it. **The infra is proven and
  retained; the meadow's authored pond was removed** - see Deferred.
- **Active/settled state machine (Substep 4).** Reused the design-doc shape
  (`FluidLayer.active` + `FluidCell::FLAG_SETTLED`) rather than a parallel enum,
  adding a runtime `stable_ticks` counter + `activate`/`settle`/`note_stable`/
  `note_changed` (`SETTLE_AFTER_TICKS = 8`).
- **W-shadow flow (Substep 5).** `fluid_sim.rs`: two-phase per tick, `u16` fixed-
  point mass (`MAX_MASS = 65535`), down flow (gravity) then horizontal
  equalization, applied **live-capped in a deterministic order** (transfers capped
  by source mass + dest space → exact conservation, no overflow/clamp loss).
  Residue `≤ MIN_MASS` is dropped. Settling quiets a level pool (drops it from the
  active set). No up-flow/compression this phase.
- **Cross-chunk flow (Substep 6).** `plan_chunk`/`commit_chunk` split + a
  `FluidSample` trait (`SelfSample`, `NeighborSample`). Boundary transfers use
  **symmetric snapshot recomputation** (both chunks compute the identical transfer
  from the shared pre-tick snapshot; gated on both edge cells being active) with a
  bounded corner-cell leak accepted per the decision. The scheduler runs a global
  two-phase (plan all → commit all → wake changed edges' mirrors).
- **Scheduler (Substep 7).** `FluidClock` (fixed 10 Hz accumulator, capped
  catch-up) drives `fluid_tick_system` in `FrameStage::Simulation`: global two-
  phase over active chunks, water-mesh rebuild for changed chunks (coalesced once
  per frame), cross-chunk edge waking, a runaway metric, and a large-flow throttle.
  A debug **pour-water disturbance** (press **G** over terrain) drops a settled
  column at the picked cell.
- **Persistence.** `fluid_diffs` round-trips (Phase-3 blob); the loader now carries
  it forward (was dropped), `build_chunk_edits` **snapshots the full current fluid
  field** at save, and the loader **resumes** it (re-activating cells that were
  mid-flow). Chunks whose fluid changes are marked `persist_dirty`. Net: a mid-fall
  stream resumes on reload rather than rewinding to its source.

## Fluid stack coherence
- Generation: `generate_chunk` → materialize storage → `ocean_fill` → (biome
  `apply_biome_ponds` if any biome authors fluid) → `GeneratedChunk.fluids`.
- Runtime: `FluidClock` → `fluid_tick_system` → per-tick `plan_chunk`
  (read-only, via `NeighborSample`) then `commit_chunk` (settling) then edge
  waking → `WaterPass.add_chunk_water` for changed chunks.
- Persistence: fluid change → `persist_dirty` → `build_chunk_edits` snapshots
  `fluids.cells` into `fluid_diffs` → blob; load → `apply_persisted_fluid`
  overlays + resumes.

## Test / verification coverage
- `nodegraph-eval` (77 tests): `fluid_output_produces_pond_levels` + the Phase-6
  suite.
- `voxulacrum-app` fluid tests: `ocean_fill` (submerged/straddle/dry/solid/
  determinism), `apply_biome_ponds` (above-sea + open-ocean guard), `FluidLayer`
  state machine (activate/settle/note_stable), `fluid_sim` (falling-shaft
  conservation, horizontal leveling, settling, determinism, **two-chunk boundary**
  symmetry + conservation), `FluidClock` accumulator.
- Determinism is asserted on every flow path (fixed snapshot + deterministic apply
  order); the two-chunk test proves cross-boundary flow conserves and levels.
- Runtime smoke (per substep): ocean renders at sea level; poured water flows,
  spreads, pools, settles; flow crosses chunk seams with matching levels; large
  flows throttle instead of stalling; disturbed water persists mid-flow across
  unload/reload and save/close.

## Deferred (intentional)
- **Basin-contained biome ponds.** The authored meadow pond was a raised puddle
  with open edges - physically metastable, so the (correct) sim drained it to sea
  level when disturbed, cascading into a large flow. The `fluid_provider` node +
  evaluator + generator path are proven and retained; a stable pond needs terrain-
  aware basin fill (flood-fill / carved basin), which is coupled to the same
  terrain-modification work as the deferred rivers. **Removed the meadow's pond
  content; kept the infra.**
- **Restart initial-fill overrides.** Startup `World::generate` regenerates spawn
  chunks from seed but does not apply persisted DB overrides - only the streaming
  load path does. So spawn-area edits (voxel, scatter, **and** fluid) come back
  fresh on the first frame after a full restart and only restore once re-streamed.
  A general gap (not fluid-specific); fixing it means an at-startup override-apply
  pass mirroring the streaming overlay.
- **Upward flow / compression.** Substep 5 skipped up-flow (design's "simple case
  first"). Without it, water can't pressurize against a ceiling; the live-capped
  scheme still conserves.
- **Per-tick deferral budget.** Only a coarse tick-count throttle exists; true
  active-cell deferral is deferred (it would break the global two-phase's snapshot
  consistency). The active set is bounded by disturbances; the runaway metric
  guards it.
- **Cross-chunk corner-cell leak.** Symmetric-recompute (Option A) is exact for
  face-adjacent flow; a cell fed by 2-3 chunks at once during active flow can clamp
  a bounded amount. Negligible in practice (settled ocean doesn't tick).
- **Rivers, player-water interaction, waterfall carving, non-water fluids, waves/
  reflections** - out of Phase-7 scope per the prompt.

## Verification results
- `cargo check --workspace`: **0 warnings** (Phase-6 bar held, after the one `mut`
  fix); `cargo test --workspace`: green, 0 failures.
- `cargo clippy --workspace`: pre-existing pedantic lints only (the tree has never
  been clippy-clean); Phase 7 added none of a new family - the fluid-file lints
  match existing `world_eval` patterns.
- No on-disk format change (`fluid_diffs` already round-tripped since Phase 3; the
  save now populates it from the live field).
