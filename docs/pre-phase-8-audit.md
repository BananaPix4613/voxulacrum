# Pre-Phase-8 State Audit — Consolidation (Mutation Command API + Drift Fixes)

**Date:** 2026-07-13 (post-Phase-7 tree, branch `mc-revision`)
**Purpose:** The written reference for Phase 8 planning (Substep 0). Establishes
the current state of every system Phase 8 touches: mutation call sites,
persistence marking, threading, PoissonDisk seeding, generation pipeline order,
and mode-tracking (absent). Line numbers refer to the tree as of this date.

Sources read: `engine-design.md` (working tree — §9 mode model present),
`architecture-drift-review.md`, `post-phase-7-audit.md`, and the code.

> **Doc-version note.** The working-tree `engine-design.md` header still reads
> **v1.3**, but §9 already carries the "Modes: content authoring vs. play"
> subsection and the mode-enforced-by-mutation-API language the Phase 8 prompt
> attributes to v1.4. The `git status` shows `engine-design.md` modified. Treat
> §9's mode model as authoritative for Phase 8; the header/revision-history bump
> to v1.4 is an outstanding edit, not a contradiction.

---

## 1. Current mutation call sites

Every place that mutates resident world state today. Phase 8 Substep 1 routes
each of these through the mutation command API. For each: what it does and which
invariants (persist-dirty, mesh-dirty, override bucket, invalidation) it
currently maintains **by hand**.

### 1.1 `World::apply_edit` — single voxel edit (`world/mod.rs:163–193`)
- **Dead code today** (`#[allow(dead_code)]`, "voxel-edit toolkit entry point;
  not yet reachable"). No live caller; the interaction layer only edits scatter.
- Clones the **entire** `ChunkStorage` (`(*chunk.data.voxels).clone()`,
  `mod.rs:166`), sets one voxel, re-wraps in a fresh `Arc`. **This is drift
  review 3.1** — one full palette-array clone per single voxel. Substep 6's
  batched entry point replaces this; the single-edit form must delegate to it.
- Invariants it maintains: creates/updates the override bucket
  (`get_or_insert_with(ChunkOverrides::default)`, `set_voxel`), auto-promotes to
  `None` (Full snapshot) past `DELTA_THRESHOLD` (`mod.rs:178–180`), sets
  `persist_dirty = true` (`:182`), `mark_mesh_dirty_from_edit()` on the chunk
  (`:183`) **and on border neighbors** via `border_dirty_neighbors(index)`
  (`:187–192`).
- This is the single site that already encodes the full "voxel edit" invariant
  set; it is the template for the batched `EditVoxel`/`EditVoxelBatch` handlers.

### 1.2 Streaming override-apply on load (`world/streaming.rs:258–340`)
- `spawn_generation` runs on the pool: generates base terrain, then overlays
  saved edits from the worker's read-only DB handle.
- Self-healing delta: drops `voxel_diffs` that already match base terrain
  (`:290–299`), applies survivors via `apply_overrides_to_storage`, carries
  `scatter_removed`/`scatter_added` (`:304–305`) and `fluid_diffs` (`:308`)
  forward, then `overrides = None` if empty.
- `apply_persisted_fluid` (`streaming.rs:74–87`) overlays `fluid_diffs` onto the
  generated `FluidLayer`, re-activating non-settled cells.
- `Full` branch replaces storage wholesale (`:311–314`).
- Sets `chunk.persist_dirty = false` (`:337`) — "matches what's in DB".
- Insert into world: `world.insert_chunk(gen_result.chunk)` (`:398`), then marks
  the 6 face neighbors mesh-dirty (`:404–410`).
- **Intent for the API:** `ApplyOverridesOnLoad { chunk_coord, overrides }` — an
  origin-agnostic load path (produces a resident chunk from generated + persisted
  state). Note it runs **off-thread** on the pool and delivers via channel, so
  the command boundary here is the main-thread `insert_chunk` + neighbor-dirty
  step, not the worker body.

### 1.3 Regen completion swap (`world/regen.rs:47–113`)
- `world.chunks.extend(new_chunks)` (`regen.rs:52`) — merges regenerated chunks
  (built by `generate_world_background`, `mod.rs:421–443`, with
  `overrides: None`) over the resident set. Adopts the new generator
  (`:53`). **Authoring-mode authoritative regen (design §9): overrides are
  intentionally discarded.**
- Marks every chunk `mesh_dirty` + `submit_all_dirty` (`:57–60`).
- Rebuilds detail-paint + scatter passes (`:62–65`).
- `water_pass.clear_all()` (`:69`) — stale comment "Water is a Phase 1 no-op"
  (drift-review observation 5.2; it is Phase-7 real).
- `persistence.clear_all_chunks()` (`:71`) — **wipes the entire DB** on regen.
  Per §9 mode model this is intended authoring behavior (drift review 1.1 is
  **reclassified as intended**, Phase 8 out of scope; do **not** add override
  preservation here).
- Clears the mesh cache (`:79–80`); dead commented-out world-cache block
  (`:82–90`, observation 5.2).
- Rebuilds streaming workers for the new generator (`:93–98`).
- **Intent for the API:** `InvalidateForGraphEdit { affected_tags }` +
  the completion swap. The tag classification already exists
  (`classify_edit`/`select_invalidated`, `regen.rs:190–206`).

### 1.4 Fluid disturbance + persist marking (`ecs/systems.rs:277–413`)
- `fluid_tick_system` runs the two-phase sim, then for every changed chunk sets
  `persist_dirty = true` and **gives it an empty override bucket**
  (`get_or_insert_with(ChunkOverrides::default)`, `systems.rs:361–367`) with the
  comment "so build_chunk_edits snapshots the fluid even without a voxel edit."
  **This is the bug half of drift review 1.2** — see §2 below; an empty bucket is
  still empty, so `build_chunk_edits` returns `None`.
- `pour_water_column` (`systems.rs:376–413`) — the debug **G**-key pour: inserts
  full water cells, activates them, writes each into `overrides.fluid_diffs`
  (`:407–409`), sets `persist_dirty = true` (`:410`). This path **does** save
  (non-empty bucket).
- **Intent for the API:** `EditFluidCell`/`PourFluid` (player/debug pour) and a
  distinct "fluid settled/flowed" internal mutation that marks persistence
  correctly (fixing 1.2 at the same seam).

### 1.5 Scatter place/remove (`interaction.rs:148–245`)
- `scatter_edit_system`: left-click removes, right-click places, at the picked
  anchor.
- `remove_scatter_at` (`:204–228`) — records generated ids into
  `overrides.scatter_removed`, drops matching `scatter_added`.
- `place_scatter_at` (`:231–245`) — pushes a player instance into
  `overrides.scatter_added` with a salted stable id.
- Sets `chunk.persist_dirty = true` (`interaction.rs:192`) when changed; rebuilds
  the chunk's scatter GPU buffer. **Does not** mark mesh-dirty (scatter is not in
  the voxel mesh).
- **Intent for the API:** `PlaceScatter { coord, instance }` /
  `RemoveScatter { stable_id }`.

### 1.6 Initial-fill construction (`World::generate`, `mod.rs:43–84`)
- Builds the resident set at boot via `par_iter` over `generate_chunk`, each
  chunk `overrides: None` (no DB overlay — the post-7 "Restart initial-fill
  overrides" deferral, still deferred). Not a runtime mutation but a bulk
  construction path; note it constructs `LoadedChunk` directly and does not go
  through any single insert seam.

**Summary of hand-maintained invariants across sites**

| Site | persist_dirty | mesh_dirty (self) | mesh_dirty (neighbors) | override bucket | storage clone |
|------|:---:|:---:|:---:|:---:|:---:|
| `apply_edit` (dead) | ✓ | ✓ | ✓ (border) | ✓ + promote | **full, per voxel** |
| streaming load | set `false` | — | ✓ (6 faces, on insert) | built from DB | on override apply |
| regen swap | (DB wiped) | ✓ (all) | — | dropped (`None`) | fresh chunks |
| fluid tick | ✓ | — | — | ✓ **empty (bug)** | — |
| pour water | ✓ | — | — | ✓ `fluid_diffs` | — |
| scatter edit | ✓ | — | — | ✓ scatter fields | — |

The inconsistency this table shows (each site sprinkles a different subset of the
four invariants) is exactly what the mutation command API centralizes.

---

## 2. Current persistence marking + the drift-review 1.2 bug

`build_chunk_edits` (`world/persistence.rs:689–710`) is the save-decision seam:

```
if !chunk.persist_dirty { return None; }
let fluids = &chunk.data.fluids.cells;
match &chunk.data.overrides {
    None => Some(Full(storage.clone())),              // 695–699
    Some(ovr) if ovr.is_empty() => None,              // 700   <-- BUG
    Some(ovr) if voxel_override_count > THRESHOLD => Full(...),
    Some(ovr) => { ovr.fluid_diffs = fluids.clone(); Delta(ovr) }  // 704–707
}
```

**Confirmed against drift review 1.2:**
- A chunk water merely *flowed into* gets an **empty** override bucket from
  `fluid_tick_system` (§1.4). The `Some(ovr) if ovr.is_empty() => None` arm
  (`persistence.rs:700`) matches **before** the fluid snapshot in the final arm
  (`:704–707`), so the chunk returns `None` and **saves nothing**. Its flowed-in
  water vanishes on unload/save.
- Chunks the player *poured* into save correctly — the pour writes
  `fluid_diffs` directly (`systems.rs:407–409`), so the bucket is non-empty.
- **Second half (`None`/`Full` branch, `persistence.rs:695–699`):** a
  voxel-promoted chunk (`overrides == None`, `persist_dirty == true`) saves
  `Full(storage)` and **drops fluid entirely** — acknowledged in-source as "a
  rare edge, not carried here." Substep 2 must decide (carry fluid alongside the
  Full storage, or document the loss).

**Substep 2 shape options** (decide at planning): (a) compute a
`has_fluid_deviation` flag and let the `is_empty()` arm honor it; or (b) reorder
so the fluid-snapshot decision happens before the empty early-return. `is_empty()`
lives on `ChunkOverrides` (`overrides.rs:45–53`) and does **not** know about the
live fluid field, which is why the empty bucket reads as empty. A unit test
constructing a fluid-only-changed chunk and asserting round-trip is the missing
coverage.

**`persist_dirty` writers today:** `systems.rs:363`, `systems.rs:410`,
`interaction.rs:192`, `world/mod.rs:182` (dead `apply_edit`). Cleared at
`streaming.rs:337` (fresh load), `persistence.rs:816` (after batch save). The
save consumers are `save_dirty_chunks` (autosave, `persistence.rs:809–823`) and
`save_chunk_on_unload` (`:845–852`); both go through `build_chunk_record` →
`build_chunk_edits`, so fixing 1.2 in `build_chunk_edits` fixes both.

---

## 3. Current threading model

### 3.1 The shared generation pool (`gen_pool`)
- Built **once** in `main.rs:316–322`:
  `num_cpus::get().saturating_sub(2).max(2)` threads, named `chunk-gen-{i}`.
- Handed (as `Arc<rayon::ThreadPool>`) to: `World::generate` (startup fill,
  `main.rs:328`), `WorldRegenCoordinator::new` (`:433`),
  `ChunkStreamingManager::new` (`:444–450`), `FieldProbe::new` (`:453`).
- Consumers `spawn` fire-and-forget tasks or `pool.install(|| par_iter…)`.

### 3.2 Meshing workers — raw `std::thread` (drift review 2.2)
- `MeshingPipeline::new` (`meshing/mod.rs:114–178`) does **not** receive
  `gen_pool`. It spawns its own `num_workers` raw threads
  (`thread::Builder::new().name("mesh-worker-{i}")`, `meshing/mod.rs:11, 149–160`)
  running `worker_loop` (`:358–440`), pulling `MeshRequest`s off an
  `mpsc::sync_channel` guarded by `Arc<Mutex<Receiver>>`.
- **These are the only raw chunk-work threads in the app** (grep for
  `std::thread` in `voxulacrum-app/src` returns only `meshing/mod.rs:11,155`).
- Substep 5 moves this work onto `gen_pool` (pass the `Arc` into
  `MeshingPipeline::new`; replace the worker loop with `rayon::spawn` or an
  equivalent pattern). The request/result channel plumbing can stay; only the
  worker-construction changes.

### 3.3 Duplicated core-budget arithmetic
`num_cpus::get().saturating_sub(2).max(2)` (call it `usable`) appears in **three**
independent places that must agree by hand:

| File | Expression | Purpose |
|------|-----------|---------|
| `main.rs:318` | `usable` | `gen_pool` thread count |
| `world/streaming.rs:213–214` | `usable`, then `(usable/3).max(2)` | streaming `max_in_flight` |
| `meshing/mod.rs:120–122` | `usable`, then `gen=(usable/3).max(2)`, `num_workers=(usable-gen).max(2)` | mesh worker count |

Note `meshing` computes `usable/3` **to model gen's share** and takes the
remainder for itself, while `streaming` computes `usable/3` **for its own** cap —
two files independently reconstructing the same split. Substep 5 centralizes this
in one module both consult (a `core_budget` helper returning the gen/mesh/stream
split).

---

## 4. Current PoissonDisk seeding — and a scope nuance

### 4.1 The drift-review 2.1 target
- `poisson_disk` (`nodegraph-eval/src/scatter.rs:162–172`) seeds the Bridson
  process from `ctx.scatter_seed(p.seed)`.
- `scatter_seed` (`context.rs:49–55`) folds `world_seed`, `node_local_seed`,
  `chunk.x`, `chunk.z` — **chunk-coordinate-based, and omits `chunk.y`**
  (observation 5.3).
- Every sibling uses world-absolute derivation: `noise_seed` (`context.rs:37–45`,
  excludes chunk coords), `world_cell_seed` (`context.rs:60–66`, used by
  `jittered_grid` at `scatter.rs:76`), `stable_instance_id`
  (`detail_eval.rs:350–358`, hashes world pos).
- In-code acceptance to replace (Substep 4): `scatter.rs:6–7`
  ("`PoissonDisk` seeds per-chunk (not seam-continuous - acceptable for
  Phase 10)").

### 4.2 Consumption — where `PoissonDisk` actually flows
- `NodeKind::PoissonDisk` is evaluated by the **terrain/biome `Evaluator`**
  (`nodegraph-eval/src/eval.rs:473–474`) as a `Positions` producer.
- `poisson_placement` (the shared Bridson core, `scatter.rs:95–158`) also backs
  the **`PoissonPlacement` library kernel** (`library_kernel.rs:34,45,57`;
  `assets/libraries/poisson_placement.library.json`).

### 4.3 ⚠ Audit nuance — two different "Poisson" node kinds
There are **two** distinct scatter primitives, and the drift review's 2.1 target
is **not** the one the shipping meadow content uses:

| Node kind | Impl | Seeding | Seam-safe? | Used by live content? |
|-----------|------|---------|:---:|:---:|
| `PoissonDistribution` | `DetailEvaluator::poisson` (`detail_eval.rs:123–149`) | `world_cell_seed` (per-cell, world-absolute) | **yes** | **yes** — `biome_meadow.detail.json` |
| `PoissonDisk` | `scatter::poisson_disk` (`scatter.rs:162`) | `scatter_seed` (chunk-based) | no | no (available node; not in the meadow detail graph) |

Implication for **Substep 4 planning:** fixing `PoissonDisk`/`scatter_seed`
corrects the *primitive and its determinism contract*, but likely changes **no
currently-rendered scatter** (the meadow uses `PoissonDistribution`, already
world-absolute). The "accepted one-time stable-ID break" is therefore mostly
prospective. This does not reduce the fix's value (it unblocks any future graph
that uses `PoissonDisk`, and closes the `chunk.y` gap), but the runtime
"seams disappear" verification needs a graph that actually uses `PoissonDisk`, or
should be reframed as a determinism/`chunk.y` test.

### 4.4 ⚠ Audit gap — Bridson seam-continuity is not free
`JitteredGrid`/`PoissonDistribution` are **per-cell**: each world-absolute cell is
seeded independently, so two chunks whose margin bands overlap compute the
*identical* point for a shared cell — genuinely seam-continuous. `poisson_disk`
is a **global sequential** Bridson process over the whole margin band, seeded
once. Swapping `scatter_seed` for a world-absolute seed makes it
**deterministic per world-position**, but two neighboring chunks still run
*independent* Bridson walks over their own (offset) margin bands, so points in the
overlap will **not** match — the seam does not vanish the way it does for the
grid. **This is a design question for Substep 4, not a mechanical swap:** true
seam-continuity for blue-noise needs either (a) a world-tiled Bridson with
per-tile deterministic seeds and cross-tile conflict resolution, or (b) accepting
that `PoissonDisk` is "world-absolute deterministic but not seam-identical" and
documenting that. Surface this before writing Substep 4 code.

---

## 5. Current generation pipeline order (drift review 1.4)

Design §5 stage order: **stage 9 fluid → stage 10 foliage**. Two places invert
it, and neither threads fluid into foliage.

### 5.1 Evaluator (`nodegraph-eval/src/world_eval.rs:270–275`)
```
let biome_col = self.biome_id_column(&zone_columns);          // 270
let terrain   = self.composite_terrain(...)?;                 // 271
let foliage   = self.evaluate_foliage(ctx, &terrain, ...)?;   // 272  <-- foliage first
let fluid_levels = self.composite_fluid(ctx, ...)?;           // 273  <-- fluid after
```
- `evaluate_foliage` (`:280–294`) runs each biome's `DetailEvaluator` against
  `terrain` only. `SurfaceFilter` (`detail_eval.rs:151–173`) filters by height /
  material / slope — it has **no submersion predicate and no fluid input**.
- `composite_fluid` (`:311–352`) produces only per-column pond **levels**
  (`Option<Arc<ColumnField>>`), not a full fluid field. Ocean fill from
  `sea_level` happens later, app-side (§5.2). So even reordering the evaluator,
  the foliage pass needs a *submersion signal* derived from `sea_level` +
  pond levels, since the evaluator never sees the ocean.

### 5.2 App generate_chunk (`world/world_generator.rs:195–208`)
```
smooth_slabs(...)                            // 195
let detail_layers = paint_to_detail_layers(&eval.foliage.paint);  // 198 (translate)
let scatter       = scatter_to_store(&eval.foliage.scatter);      // 199 (translate)
let mut fluids = ocean_fill(&storage, position.y, self.sea_level); // 202  <-- fluid after
if let Some(levels) = &eval.fluid_levels { apply_biome_ponds(...) } // 203–207
```
- Note lines 198–199 only *translate* already-computed eval output into storage
  form; the actual foliage decision was made in the evaluator (§5.1). So the real
  fix is in the evaluator (make foliage fluid-aware); the app-side reorder is
  cosmetic unless the submersion mask is computed app-side.

### 5.3 Fix-shape considerations for Substep 3
- `sea_level` lives on the generator (`world_generator.rs:45–47,137`) /
  manifest (`sea_level: 24`), **not** in the evaluator (`EvalContext` has only
  `world_seed` + `chunk`). Foliage submersion therefore needs `sea_level` (and
  optionally pond levels) plumbed into the foliage pass, or the submersion filter
  applied app-side against the finished `fluids` layer.
- Options (drift review names three): (a) `SurfaceFilter` gains an is-submerged
  predicate; (b) post-filter `paint`/`scatter` against a submersion mask;
  (c) both. Option (b) fits cleanest with the current split (evaluator has no
  `sea_level`; the app has the finished `fluids` field to test against) but note
  scatter/paint were already *translated* by then — the mask would filter the
  translated outputs, or the translation functions take a mask argument.
- **Current invisibility** is real: meadow surface sits above `sea_level: 24`
  (both biomes; the rocky biome uses `traversal_smoothing_distance: 0.0`). The
  first below-sea biome or any pond generates submerged grass/bushes. Substep 3
  needs a temporary/gated below-sea test biome to exercise the fix (meadow and
  rocky must stay visually unchanged).

---

## 6. Current mode-tracking — none exists

- **No mode concept anywhere.** No `Mode`/`Authoring`/`Play` enum, no
  "editor active" flag that maps to a mode. `VoxelWorld(pub World)`
  (`ecs/resources.rs:132–142`) wraps the world with no mode field; `World`
  itself (`world/mod.rs:29–34`) is `{ chunks, generator, min_chunk_y,
  max_chunk_y }`.
- The closest existing thing is the editor's `graph_editor.is_modified()` /
  `consume_dirty()` gating (`systems.rs:199`, `:872`) and `UiState.pending_graph`
  — but that is "the editor has unsaved edits", not a runtime mode. It is **not**
  a candidate for absorption into mode; it is an editor-UI concern.
- Phase 8 Substep 1/2 **introduces** the mode state fresh. Since only
  `Authoring` exists (Play lands in Phase 9), the mode-rejection contract is
  tested against synthesized `PlayTime`-origin commands (per the prompt's
  Authoritative Decisions). Natural home: a field on `World` (so every
  `execute` call can check it) or a sibling ECS resource; decide at Substep 1
  planning. A field on `World` keeps the mutation API self-contained (the API
  is `World::execute`), which the prompt's "singleton, enum-dispatched" framing
  favors.

---

## 7. Deferred items inherited from Phase 7 — still deferred

Confirmed against the post-7 audit's Deferred list; none are resolved:

- **Basin-contained biome ponds** — meadow pond content removed; `FluidOutput`
  node + evaluator + `apply_biome_ponds` retained. Still deferred (needs
  terrain-aware basin fill). ✓ still deferred.
- **Restart initial-fill overrides** — `World::generate` (§1.6) does not apply DB
  overrides; only streaming does. ✓ still deferred (Phase 8 does not fix this;
  but the mutation API's `ApplyOverridesOnLoad` intent is the seam a future fix
  would reuse).
- **Upward flow / compression** — sim has no up-flow. ✓ still deferred.
- **Per-tick deferral budget** — only a coarse tick-count throttle
  (`systems.rs:305–307`). ✓ still deferred.
- **Cross-chunk corner-cell leak** — bounded, accepted. ✓ still deferred.
- **Rivers, player-water interaction, waterfall carving, non-water fluids,
  waves/reflections** — out of scope. ✓ still deferred.

Additional drift-review items **explicitly out of Phase 8 scope** (record, don't
fix): regen override preservation (1.1, reclassified intended), fluid full-
snapshot→diff-with-tombstones (1.3), `FaceVertex`/`TerrainVertex` migration (1.5,
Phase 9 first substep), per-layer save versioning (1.7), greedy meshing (1.6),
eval→storage boundary consistency (2.3), pin vocabulary `Positions`/`ScatterPoints`
(2.4).

---

## 8. Phase 8 substep readiness map

| Substep | Primary files | Confirmed state | Open question flagged |
|---------|---------------|-----------------|----------------------|
| 1 — mutation API | `world/mod.rs`, `streaming.rs`, `regen.rs`, `ecs/systems.rs`, `interaction.rs` | 6 mutation sites (§1); no mode state (§6) | Mode home: `World` field vs ECS resource |
| 2 — fluid empty-bucket | `world/persistence.rs:689–710`, `ecs/systems.rs:361–367` | Bug confirmed both halves (§2) | `Full`-branch fluid: carry vs document |
| 3 — pipeline order | `world_eval.rs:270–275`, `world_generator.rs:195–208`, `detail_eval.rs` | Order inverted; foliage has no fluid input; `sea_level` not in evaluator (§5) | Submersion signal: evaluator vs app-side mask |
| 4 — Poisson world-absolute | `scatter.rs:162`, `context.rs:49–55` | `scatter_seed` chunk-based, omits `chunk.y` (§4) | Bridson seam-continuity ≠ per-cell; live content uses `PoissonDistribution` not `PoissonDisk` |
| 5 — mesh pool | `meshing/mod.rs:11,120–160`, `main.rs:318`, `streaming.rs:213` | Raw `std::thread`; budget math in 3 places (§3) | — |
| 6 — batched edits | `world/mod.rs:163–193`, `persistence.rs:679–686` | `apply_edit` clones full storage per voxel; dead code (§1.1) | — |

---

## 9. Two questions to resolve before coding

Both surfaced during this audit and change substep scope:

1. **Substep 4 (Poisson):** Do we target *world-absolute deterministic* (a
   mechanical `scatter_seed`→world-absolute + `chunk.y` swap, accepting that
   Bridson seams don't fully vanish), or *true seam-continuity* (a world-tiled
   redesign)? The drift review implies the former ("derive from world-absolute
   cells like JitteredGrid"), but Bridson is not per-cell, so "seams disappear"
   in the done-signal is not automatic. Recommend: world-absolute + `chunk.y`,
   reframe the done-signal to determinism/`chunk.y`, and document the residual
   seam — but confirm.

2. **Substep 3 (pipeline):** Where does the submersion signal live? The
   evaluator has no `sea_level`; the app has the finished `fluids` layer. Recommend
   an app-side post-filter mask (option b) since it keeps `sea_level` out of the
   evaluator's context, but this means `paint_to_detail_layers`/`scatter_to_store`
   (or the eval-domain foliage) get a submersion mask argument — confirm the seam.

Everything else in the ordered plan matches the code as the drift review
described it. **Substep 0 complete; awaiting sign-off to plan Substep 1.**
