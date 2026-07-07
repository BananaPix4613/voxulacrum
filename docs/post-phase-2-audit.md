# Post-Phase-2 State Audit

**Status:** Reference snapshot — close of Phase 2
**Date:** 2026-06-15
**Scope:** Read-only inventory of the engine as it stands at the end of Phase 2,
taken after all five Phase 2 steps landed (RON materials, worker-pool eval, the
permanent eval→storage boundary, preview deletion + field-probe port, and the
save-format audit). It is the counterpart to `post-phase-1-audit.md` and is the
intended starting-state reference for whatever Phase 3 turns out to be.

This is a *factual* audit. It records what each Phase 2 step delivered and the
current shape of the affected subsystems. Items still open or deliberately
deferred are gathered under **§8 (Carried forward)** so a later phase can pick them
up — but nothing here proposes a change.

---

## 0. What Phase 2 changed (one-paragraph delta vs. Phase 1)

The workspace dropped from **7 to 6 crates** (`voxulacrum-preview` deleted). The
material table became **data-driven** (`assets/materials.ron`, loaded at startup
with `load_initial` as a logged fallback). All chunk generation — startup fill,
streaming, and background regen — now shares **one rayon pool** (`gen_pool`),
ending the previous mix of raw `std::thread` and ad-hoc rayon. The eval→storage
copy that Phase 1 labelled a "PHASE 3 SHIM" was reframed as a **permanent named
seam** (`StorageBoundary::materialize`). The preview crate's genuinely-unique
visualization (colormap + field probe) was **ported into the engine** as a
selection-driven debug panel before the crate was removed. The save format was
audited end-to-end, two pieces of dead write-only scaffolding were removed, a
roundtrip test was added, and that test **uncovered and fixed a latent
populated-chunk load bug**.

---

## 1. Crate layout

The workspace (`Cargo.toml`) now has **6 members** (was 7):

| Crate | Purpose | Public surface (consumed elsewhere) |
|---|---|---|
| `voxel-core` | Voxel value type, material identity, RON-backed registry, shared error type. No I/O beyond RON load, no rendering. | `Voxel`, `MaterialId`, `ShapeId`, `MaterialRegistry`, `MaterialDef`, `VoxelCoreError` |
| `nodegraph-ir` | Graph data model: nodes, edges, validation, JSON (de)serialization. | `Graph`, `NodeId`, `NodeKind`, `Severity`, `Graph::validate`, `Graph::from_json` |
| `nodegraph-eval` | Pure evaluator: runs a `Graph` per chunk, produces `ChunkBuffer<Voxel,32>`. | `Evaluator`, `EvalContext`, `CachedOutput`, `ChunkBuffer`, `ScalarField` |
| `nodegraph-hotreload` | Filesystem watcher that reloads a `*.graph.json` and hands back a new `Graph`. | hot-reload handle used by the engine's hot-reload system |
| `nodegraph-editor` | egui + egui-snarl node editor embedded in the engine window; now also owns canvas **selection** + a `revision` counter for derived views. | `EditorState`, `EditorState::from_graph`, `build_graph_with_selection`, `revision` |
| `voxulacrum-app` | The engine binary `voxulacrum`: ECS, rendering (wgpu), streaming, persistence, world generation, integrated editor + field-probe debug panel. | binary entrypoint |

Dependency direction remains clean and acyclic: `voxel-core` is the root;
`nodegraph-*` build on it; `voxulacrum-app` sits at the top consuming everything.
**The previous `voxulacrum-preview` sink is gone** — its only unique value was
ported into `voxulacrum-app` (§7) before deletion. `Cargo.lock` regenerated on the
next build with the preview entry removed.

---

## 2. Generation pipeline — now one shared worker pool

There is still **one generator type** and **three call paths**, but in Phase 2 the
three paths were unified onto a **single rayon pool**.

### The generator
`world/world_generator.rs` — `WorldGenerator { graph: Arc<Graph>, world_seed: u64,
terrain_node: NodeId, storage_boundary: StorageBoundary }`. Built once via
`load_default(params)`, which reads `assets/graphs/default_biome.graph.json`. The
resulting `Arc<WorldGenerator>` is shared by `World`, the streaming workers, and
background regen.

`generate_chunk_storage(position) -> ChunkStorage`:
1. `Evaluator::new(&graph, EvalContext::new(seed, position))`
2. `eval.evaluate()` — on error logs and returns `ChunkStorage::new_air()` (infallible by contract)
3. harvest `eval.cache().get(terrain_node)` → `CachedOutput::Terrain(Arc<ChunkBuffer<Voxel,32>>)`
4. `self.storage_boundary.materialize(terrain)` → `ChunkStorage` (the permanent seam, §3)

### The shared pool
`main.rs` builds **one** `Arc<rayon::ThreadPool>` named `chunk-gen-{i}` with
`num_cpus::get().saturating_sub(2).max(2)` threads, described in-source as "the
single worker-pool model for all chunk generation (engine-design.md §12)." That
one `gen_pool` is handed to:

| Consumer | Use |
|---|---|
| `World::generate(generator, &gen_pool, …)` | **Startup fill** — runs on the pool, no longer a blocking main-thread nested loop |
| `ChunkStreamingManager` | Continuous camera-driven streaming |
| `WorldManager` (regen) | `start_regeneration` does **one** `pool.spawn` fan-out task whose internal `par_iter` runs on the same pool; result delivered over an `mpsc` channel, polled on the main thread |
| `WorldRegenCoordinator` | Regen orchestration |
| `FieldProbe` (§7) | Dispatches the selected-node evaluation onto the same pool |

`mpsc::Receiver` is `Send` but not `Sync`, so where a receiver lives inside a
bevy_ecs `Resource` (`WorldManager.regen_rx`) it is wrapped in a `Mutex` to supply
the `Sync` bound. This is the same pattern used by `FieldProbe`.

**Net effect:** the Phase-1 "lone main-thread synchronous startup fill" and the
"two different threading primitives" tensions are both resolved — every generation
path now flows through `gen_pool`.

---

## 3. The eval→storage boundary — now a permanent named seam

**Location:** `world/storage_boundary.rs` — `StorageBoundary` (a unit struct) with
`materialize(&self, chunk: &ChunkBuffer<Voxel,32>) -> ChunkStorage`.

The Phase-1 "PHASE 3 SHIM" language is **gone**. The module documents the boundary
as deliberate and permanent:

- **Evaluation domain:** `ChunkBuffer<Voxel, 32>` — dense, fixed-size, tuned for the
  evaluator's write-once / read-by-coordinate access.
- **Storage domain:** `ChunkStorage` (`PalettedBitArray` underneath) — compact,
  palette-compressed, tuned for footprint and runtime access.

`materialize` index-copies using the engine decode
`voxel_index(x,y,z) = x + y*32 + z*32²`; the two orderings are still proven equal by
`engine_and_eval_containers_are_semantically_equivalent` in `storage.rs`. The type
is stateless today but modeled as a value so future boundary configuration
(alternate packings, validation hooks) has a home without disturbing call sites.
`WorldGenerator` holds one `StorageBoundary` and calls it as the last step of
`generate_chunk_storage`.

---

## 4. Scheduling — where eval lives in the frame

`ecs/schedule.rs` still defines `FrameStage`:

```
Input → Simulation → Meshing → UniformWrite → Render → PostFrame
```

Generation-relevant placement is unchanged in shape, with the field probe added:

- **Input:** graph hot-reload (picks up edited `*.graph.json`)
- **Simulation:** param-change detection, palette load
- **Meshing:** `streaming_tick_system` → `meshing_tick_system` (poll worker results,
  upload meshes); **`field_probe_system`** (dispatch/poll the selected-node eval)
- **Render:** `render_present_system` draws the egui editor **and** the field-probe
  window
- **PostFrame:** regen completion polling, persistence autosave

No system runs graph evaluation *on the schedule thread* — eval is dispatched to
`gen_pool` and polled. The field probe follows the same dispatch-then-poll shape as
streaming/regen.

---

## 5. MaterialRegistry — now data-driven (RON)

**Definition:** `voxel-core/src/material_registry.rs`. Unchanged shape:
`MaterialRegistry { entries: Vec<MaterialDef>, by_id_name: HashMap<String, MaterialId> }`,
index-into-`entries` = numeric `MaterialId` (identity is positional).

**Construction is now RON-first:**
- `load_from_ron(path)` is **implemented** (was a stub returning a "deferred to
  Phase 5" error). It parses `assets/materials.ron` via `from_ron_str`, which:
  sorts entries by their explicit declared `id`, validates the ids form a
  **contiguous `0..len` range** (rejecting duplicates and gaps), validates
  **unique `id_name`s**, and orders the table so positional `MaterialId` == declared
  `id`.
- `load_initial()` (the 9 hand-authored materials, Air=0 … Gravel=8) is retained as a
  **logged fallback** so the engine never fails to start over materials.
- A lock test `ron_matches_load_initial` asserts `materials.ron` stays
  byte-equivalent to `load_initial()`; negative tests cover non-contiguous ids,
  duplicate ids, and duplicate id_names.

**Error type:** the Phase-1 `RonLoadingDeferredToFuturePhase` variant is **removed**.
`voxel-core/src/error.rs` now carries real RON variants: `MaterialRonRead`
(with path + io source), `MaterialRonParse(String)`, `MaterialRonValidation(String)`.

**id-vs-name boundary (unchanged and still load-bearing):** `resolve(id_name)` is the
only name→id crossing (graphs/authoring use snake_case names); `get(id)` reads
`MaterialDef` by numeric id (rendering/meshing). Saves persist numeric ids (§6), so
the RON loader's contiguity/uniqueness validation is what guarantees id stability.

---

## 6. Save / load paths — audited, fixed, and tested

**Backend:** `world/persistence.rs` — SQLite (WAL) + zstd (optional trained
dictionary). Per-chunk blob format unchanged:

```
[BLOB_VERSION: u8 = 4][tag: u8][payload…]
```
- `TAG_DELTA = 0` — `index:u16 + packed voxel:u32` per edit
- `TAG_FULL_UNIFORM = 1` — single packed voxel
- `TAG_FULL_POPULATED = 2` — full populated chunk (`PalettedBitArray`)

**Phase 2 Step 5 changes:**
1. **`format_version` meta key removed.** It was written on open but never read; the
   per-chunk `BLOB_VERSION` (written at L69, rejected on mismatch at read) and the
   `voxel_format_version` wipe gate already cover versioning. Stale `format_version`
   rows in pre-release DBs are simply ignored.
2. **Unused `EDIT_FLAG_MOISTURE/FLORA_ID/FLORA_GROWTH` constants removed.** The delta
   path only ever serialized `index + packed voxel`; the flags implied a richer
   format that wasn't written. A source comment now records that per-voxel
   moisture/flora persistence is **intentionally deferred** to the later flora model
   (when it lands, the delta gains a per-edit flag byte). The `VoxelEdit` Option
   fields and the runtime `apply_edits_to_storage` logic are untouched — only the
   dead serialization scaffolding was removed.
3. **Roundtrip tests added** (in-module `#[cfg(test)]`, since `voxulacrum` is a
   binary-only crate with no lib target a `tests/` integration test could link).
   They cover all three tags plus a bad-`BLOB_VERSION` rejection.
4. **Latent bug found and fixed by the new test.** `PalettedBitArray::serialize_to_bytes`
   writes `palette_len` as a `u16`, but `deserialize_from_bytes` read it back as a
   `u32` from a 2-byte slice — `try_into::<[u8;4]>()` always failed, so **every
   populated-chunk load returned `Corrupt("material palette failed")`**. Latent
   because populated full-saves (heavily-modified chunks) are rare versus
   delta/uniform. Fixed to read a `u16`. The roundtrip test now guards it.

**Versioning:** no format-version bump was needed (the changes only removed
write-side dead code and fixed a reader). The `voxel_format_version` wipe-migration
mechanism remains available for future bumps.

---

## 7. Field-probe debug infrastructure (replaces the preview crate)

Before deleting `voxulacrum-preview`, its two genuinely-unique pieces were ported
into the engine as a **selection-driven debug panel** — the graph itself is the
selector (Houdini/Blender model), not a dropdown.

- `ui/colormap.rs` — heatmap color ramp (`Colormap`, e.g. Viridis) ported from the
  preview's `colormap.rs`. No prior engine equivalent for 2D field visualization.
- `ui/field_probe.rs` — `FieldProbe` (`#[derive(Resource)]`, holds the shared
  `gen_pool`) plus `field_probe_system` and `draw_field_probe_window`. It visualizes
  the cached output of **whichever graph node is currently selected on the editor
  canvas**, evaluated on the shared pool and polled. Scalar outputs render through
  the colormap (with auto/manual range); terrain outputs render material colors via
  `MaterialRegistry`. Controls: chunk coords, Y-slice, colormap, range, pan/zoom,
  and a hover read-out. Toggled with **F3**.

**Editor plumbing that makes this work** (in `nodegraph-editor`): plain left-click
on a node header now drives selection ourselves (the whole colored header band is
clickable except the collapse arrow), because egui-snarl 0.8.0 only selects on
Shift/Ctrl-click. `EditorState` exposes a monotonic `revision` (bumped on any graph
edit *or* selection change) and `build_graph_with_selection()`, which the probe
polls to know when to recompute.

The duplicate parts of the preview (`render_3d/*`, `material_colors.rs`, the
egui-snarl editor embed) were **not** ported — the engine already owns wgpu
rendering/meshing, `MaterialRegistry` colors, and the integrated editor.

---

## 8. Carried forward (open / deferred items)

Neutral list of items that were explicitly out of Phase 2 scope or deferred by a
Phase 2 step. None block anything; they are recorded so a later phase can pick them
up deliberately.

| Item | Where | Note |
|---|---|---|
| Per-voxel **moisture/flora serialization** | `persistence.rs` `TAG_DELTA` path | Deferred with the flora/moisture simulation model. Format extension point documented in-source (per-edit flag byte). |
| `VoxelEdit` optional fields applied but not persisted | `chunk.rs` + `apply_edits_to_storage` | Runtime apply logic exists; serialization deferred (above). |
| **1-frame lag** on field-probe control changes | `ui/field_probe.rs` / texture refresh | Texture is uploaded before the render pass; a control change takes effect next frame. Cosmetic. |
| Stale `.idea/voxulacrum.iml` `sourceFolder` to the deleted preview | `.idea/voxulacrum.iml` | Cosmetic IDE config; no build impact. |
| Historical docs still describe the preview / pre-Phase-2 state | `docs/engine-foundation-reset-audit.md`, `docs/post-phase-1-audit.md` | Intentionally historical; not edited. |

**Explicitly still out of scope (unchanged from Phase 1):** four-layer chunk model,
walkability mask, slab smoothing, formal `ChunkOverrides` type, ChunkTags / tag
invalidation, five-graph hierarchy, new pin types, full egui_dock, fluid sim,
foliage, audio occlusion, networking/determinism beyond existing.

---

## 9. Phase 2 outcomes summary

| Step | Delivered | Primary file(s) |
|---|---|---|
| 1 — RON materials | `load_from_ron` implemented + validated (contiguous ids, unique names); `materials.ron` authored; RON-primary with `load_initial` fallback; lock + negative tests; `RonLoadingDeferredToFuturePhase` removed | `voxel-core/material_registry.rs`, `error.rs`, `assets/materials.ron` |
| 2 — worker-pool eval | Single `gen_pool` (rayon) drives startup fill, streaming, regen, and the probe; `mpsc` receivers `Mutex`-wrapped for `Sync` | `main.rs`, `world/mod.rs`, streaming/regen |
| 3 — reframe shim | `StorageBoundary::materialize` — permanent named seam; Phase-3 language removed; parity test retained | `world/storage_boundary.rs`, `world_generator.rs` |
| 4 — delete preview + port | `voxulacrum-preview` removed; colormap + selection-driven field probe ported into the engine (F3); editor selection plumbing added | `ui/field_probe.rs`, `ui/colormap.rs`, `nodegraph-editor/{viewer,state}.rs`, workspace `Cargo.toml` |
| 5 — save format | `format_version` + `EDIT_FLAG_*` dead code removed; roundtrip tests added; **populated-chunk `palette_len` read-width bug fixed** | `world/persistence.rs`, `world/storage.rs` |

---

*End of audit. This is a factual snapshot at the close of Phase 2; no code was
changed in producing it. It supersedes `post-phase-1-audit.md` as the current
starting-state reference for Phase 3.*
