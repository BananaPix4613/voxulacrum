# Post-Phase-1 State Audit

**Status:** Reference snapshot — Phase 2 substep 0
**Date:** 2026-06-10
**Scope:** Read-only inventory of the engine as it stands at the close of Phase 1,
taken before any Phase 2 implementation. Every later Phase 2 step (RON materials,
worker-pool eval, shim reframe, preview deletion, save-format audit) cites this
document for its starting state.

This is a *factual* audit. Where current code carries comments or markers that
conflict with settled Phase 2 decisions, the conflict is noted under
**Tension** so the relevant step can reconcile it — but nothing here proposes a
change.

---

## 1. Crate layout

The workspace (`Cargo.toml`) has **7 members**:

| Crate | Purpose | Key deps | Public surface (consumed elsewhere) |
|---|---|---|---|
| `voxel-core` | Voxel value type, material identity, shared error type. No I/O, no rendering. | none heavy (std + serde) | `Voxel`, `MaterialId`, `ShapeId`, `MaterialRegistry`, `MaterialDef`, `VoxelCoreError` |
| `nodegraph-ir` | Graph data model: nodes, edges, validation, JSON (de)serialization. | serde, glam | `Graph`, `NodeId`, `NodeKind`, `Severity`, `Graph::validate`, `Graph::from_json` |
| `nodegraph-eval` | Pure evaluator: runs a `Graph` per chunk, produces `ChunkBuffer<Voxel,32>`. | nodegraph-ir, voxel-core, glam, fastnoise | `Evaluator`, `EvalContext`, `CachedOutput`, `ChunkBuffer` |
| `nodegraph-hotreload` | Filesystem watcher that reloads a `*.graph.json` and hands back a new `Graph`. | nodegraph-ir, notify | hot-reload handle used by the engine's `graph_hot_reload_system` |
| `nodegraph-editor` | egui + egui-snarl node editor embedded in the engine window. | egui 0.32, egui-snarl 0.8, nodegraph-ir | `EditorState`, `EditorState::from_graph`, viewer/diagnostics |
| `voxulacrum-preview` | **Standalone** eframe binary: heatmap + 3D preview of a graph. Slated for deletion (Phase 2 Step 4). | eframe, egui-snarl, wgpu, all nodegraph crates, voxel-core | none — leaf binary, nothing depends on it |
| `voxulacrum-app` | The engine binary `voxulacrum`: ECS, rendering (wgpu), streaming, persistence, world generation. | bevy_ecs, wgpu, winit, rusqlite, zstd, rayon, all crates above except preview | binary entrypoint |

Dependency direction is clean and acyclic: `voxel-core` is the root; `nodegraph-*`
build on it; `voxulacrum-app` sits at the top consuming everything except
`voxulacrum-preview`. **`voxulacrum-preview` is a sink** — no crate depends on it,
which is why deleting it (Step 4) is contained to its own directory plus the
workspace member list.

---

## 2. Generation pipeline (chunk request → filled chunk)

There is **one generator type** and **three call paths** that drive it.

### The generator
`voxulacrum-app/src/world/world_generator.rs` — `WorldGenerator { graph: Arc<Graph>,
world_seed: u64, terrain_node: NodeId }`. Built once via `load_default(params)`
(the single construction site), which reads
`assets/graphs/default_biome.graph.json`. The resulting `Arc<WorldGenerator>` is
shared by `World`, the streaming workers, and background regen.

The core call is `generate_chunk_storage(position) -> ChunkStorage`:
1. `Evaluator::new(&graph, EvalContext::new(seed, position))`
2. `eval.evaluate()` — on error, logs and returns `ChunkStorage::new_air()` (infallible by contract)
3. harvest `eval.cache().get(terrain_node)` → `CachedOutput::Terrain(Arc<ChunkBuffer<Voxel,32>>)`
4. index-copy into a flat `[Voxel; CHUNK_VOLUME]` (the shim — see §3)
5. `storage::storage_from_arrays(&voxel_arr)`

### The three call paths and their threads

| Path | Where | Thread / pool | When |
|---|---|---|---|
| **Startup synchronous fill** | `World::generate` (world/mod.rs), invoked from `main.rs` ~L307–319 on cache miss | **Main thread**, nested `for` loop, blocking | First boot with no world cache |
| **Streaming** | `ChunkStreamingManager` (workers) → `streaming_tick_system` polls results | Dedicated `std::thread` workers, count `(usable_parallelism/3).max(2)`, results via `mpsc` | Continuous, as the camera moves |
| **Background regen** | `WorldManager::start_regeneration` / `generate_world_background` | `rayon::par_iter` *inside* a spawned `std::thread` named `"terrain-regen"` | Regenerate button, graph edit, param change (via `WorldRegenCoordinator::tick`) |

So at steady state generation is already off the main thread (streaming workers +
the terrain-regen thread). The **lone main-thread synchronous evaluation is the
startup fill** in `World::generate`.

**Tension (Step 2 / worker-pool):** Step 2 wants graph eval moved onto a rayon
worker-pool model driven from `FrameStage::Meshing`, with a debug assertion in the
evaluator entry that panics if invoked on the main thread. The startup
`World::generate` path would trip that assertion as written. Step 2 must reconcile
the initial fill (route it through the same worker path, or exempt/replace it)
before the assertion can be added safely. The three paths also use **two different
threading primitives** (raw `std::thread` for streaming, `std::thread`+`rayon` for
regen); §12 of the design doc mandates rayon-backed worker pools for both
generation and meshing.

---

## 3. The eval-to-engine boundary (the "shim")

**Location:** `world_generator.rs::generate_chunk_storage`, lines ~94–104.

**What it does:** the evaluator produces `ChunkBuffer<Voxel,32>` (the *evaluation*
container). The engine stores `ChunkStorage` built on `PalettedBitArray` (the
*storage* container). The boundary index-copies between them:

```
for i in 0..CHUNK_VOLUME {
    x = i % 32; y = (i / 32) % 32; z = i / (32*32);
    voxel_arr[i] = terrain.get(x, y, z);
}
storage_from_arrays(&voxel_arr)
```

The index decode matches the engine's `voxel_index(x,y,z) = x + y*32 + z*32²`. The
Phase-0 parity test `engine_and_eval_containers_are_semantically_equivalent` in
`world/storage.rs` proves the two orderings agree.

**Current framing (comments):** the header comment calls this a "PHASE 3 SHIM" and
states "When the engine adopts `ChunkBuffer` directly (Phase 3), this copy is
deleted." The module doc-comment repeats "PHASE 3 SHIM."

**Tension (Step 2 — reframe):** Phase 2 has settled that this boundary **stays
permanently** — `ChunkBuffer` is the evaluation domain, `PalettedBitArray` is the
storage domain, and they are deliberately *not* unified. The "Phase 3 deletes this"
comments directly contradict that decision. Step 2 renames/relocates this into a
named boundary (e.g. `storage_boundary.rs`, a type like `EvalToStorage` /
`StorageBoundary` / `ChunkConverter`), strips the scaffolding/Phase-3 language, and
documents it as intentional. No behavior change — the index copy and parity test
stay.

---

## 4. Scheduling — where eval lives in the frame

`voxulacrum-app/src/ecs/schedule.rs` defines `FrameStage` (bevy_ecs `SystemSet`):

```
Input → Simulation → Meshing → UniformWrite → Render → PostFrame
```

System placement relevant to generation:

- **Input:** `graph_hot_reload_system` (picks up edited `*.graph.json`)
- **Simulation:** `param_change_detection_system`, `palette_load_system`
- **Meshing:** `streaming_tick_system` (before) → `meshing_tick_system` — these
  *poll* worker results and upload meshes; they do not themselves evaluate graphs
- **PostFrame:** `world_regen_system` (regen completion polling), `persistence_autosave_system`

**Observation:** no system *runs* graph evaluation on the schedule thread today —
eval happens on worker/regen threads, polled from `Meshing`/`PostFrame`. The
exception remains the off-schedule startup `World::generate` (§2). Step 2's
worker-pool model fits naturally into `FrameStage::Meshing` (dispatch + poll),
matching design doc §12 ("FrameStage::Meshing — feed dirty chunks to mesher worker
pool") and the existing streaming-tick placement.

---

## 5. MaterialRegistry usage

**Definition:** `voxel-core/src/material_registry.rs`.
`MaterialRegistry { entries: Vec<MaterialDef>, by_id_name: HashMap<String, MaterialId> }`.
The **index into `entries` is the numeric `MaterialId`** — identity is positional.

`MaterialDef { id_name: String, display_name: String, color: [f32;3], sharpness: f32,
hardness: f32, permeable: bool, supports_flora: bool }`.

**Construction:** `load_initial()` hand-authors **9 materials** in fixed order,
giving the stable IDs Phase 2 must preserve:

| ID | id_name | display_name |
|---|---|---|
| 0 | air | Air |
| 1 | limestone | Limestone |
| 2 | granite | Granite |
| 3 | soil | Soil |
| 4 | clay | Clay |
| 5 | sand | Sand |
| 6 | grass_soil | Grass Soil |
| 7 | water | Water |
| 8 | gravel | Gravel |

(Exact colors/sharpness/hardness/permeable/supports_flora values are recorded in
the source; Step 1's `materials.ron` must reproduce them byte-for-byte to keep the
RON==load_initial lock test passing.)

`from_entries` builds the `by_id_name` map and **panics on duplicate `id_name`**.

**Consumption (the id-vs-name boundary):**
- **`resolve(id_name) -> Option<MaterialId>`** — graphs and authoring reference
  materials by snake_case *name*; this is the only name→id crossing.
- **`get(id) -> Option<&MaterialDef>`** — rendering/meshing reads color etc. by
  *numeric id*.
- `len` / `is_empty` for bounds.

So **names are the authoring/graph-facing key; numeric IDs are the storage/runtime
key**, and `resolve`/`get` are the two crossing points. Saves persist numeric IDs
(see §6), which is why ID stability across the RON transition is mandatory.

**`load_from_ron(_path)`** currently returns
`Err(VoxelCoreError::RonLoadingDeferredToFuturePhase)`. The module header and the
method doc-comment both attribute RON loading to "Phase 5 modding."

**Tension (Step 1):** Phase 2 Step 1 implements `load_from_ron` *now*, authors
`assets/materials.ron` (the 9 materials, stable IDs), switches startup to RON
primary with `load_initial` as a logged fallback (engine must never fail to start
over materials), adds load validation (no dup `id_name`, no dup numeric IDs,
contiguous range from 0), adds `ron = "0.8"` to voxel-core, and adds a unit test
locking RON == `load_initial`. The "deferred to Phase 5" comments contradict this
and must be removed. No `ron` dependency exists in the workspace yet, and no
`assets/materials.ron` file exists.

---

## 6. Save / load paths

**Backend:** `voxulacrum-app/src/world/persistence.rs` — SQLite (WAL) + optional
zstd (optionally with a trained dictionary).

**Per-chunk blob format:**
```
[BLOB_VERSION: u8 = 4][tag: u8][payload…]
```
Tags:
- `TAG_DELTA = 0` — payload is a sequence of edits: `index: u16` + `packed voxel: u32` (6 bytes each)
- `TAG_FULL_UNIFORM = 1` — single uniform voxel
- `TAG_FULL_POPULATED = 2` — full populated chunk

**Version constants:**
- `BLOB_VERSION: u8 = 4` (per-chunk payload format; "was 3 — chunk payloads now store packed u32 Voxels")
- `VOXEL_FORMAT_VERSION: u64 = 1` (semantic voxel layout)

**DB meta keys:**
- `seed`
- `voxel_format_version` — **read** on open (L501–510): if `stored < VOXEL_FORMAT_VERSION`, performs a one-shot **wipe migration** and rewrites the key.
- `format_version` — **written** as `BLOB_VERSION` (L513) but **never read anywhere**. Dead/asymmetric.
- `zstd_dictionary`

**Write==read symmetry:** the blob (de)serializers mirror each other for the three
tags. `BLOB_VERSION` is written (L69) and checked on read (L101 — rejects mismatch).
That part is consistent.

**Inconsistencies found (Step 5 targets):**
1. **`format_version` meta is write-only** — set on open (L513) but no reader. Either it should gate something or be removed. It duplicates `BLOB_VERSION` semantics already enforced at the blob level.
2. **`EDIT_FLAG_MOISTURE` / `EDIT_FLAG_FLORA_ID` / `EDIT_FLAG_FLORA_GROWTH` constants are defined but unused** — the `TAG_DELTA` path writes only `index:u16 + packed voxel:u32`. Flora/moisture state is **not serialized**, and is set to `None` on read. The flag scaffolding implies an intended-but-unimplemented richer delta format.

**Tension / gap (Step 5):** Step 5 must verify write==read across all paths,
confirm versioning/material-ID stability, fix these inconsistencies, and add a
save/load **roundtrip integration test** under `voxulacrum-app/tests/`. If the
format version bumps as part of the fix, it must ship with a wipe migration (the
mechanism already exists via `voxel_format_version`).

---

## 7. Preview crate state (`voxulacrum-preview`)

**Binary:** standalone `eframe` window — graph heatmap + 3D voxel preview. Nothing
depends on it (§1).

**Modules:**
- `main.rs` — eframe entrypoint
- `app.rs` — the egui application (panels, graph editor embed, controls)
- `runtime.rs` — drives the evaluator to produce preview chunks
- `colormap.rs` — heatmap color ramp
- `material_colors.rs` — material → color table for the preview
- `render_3d/{mod,camera,renderer,mesh}.rs` — its **own** wgpu camera + renderer + mesher

**Deps:** `eframe`, `egui-snarl`, `wgpu`, plus all nodegraph crates and `voxel-core`.

**Duplicate vs unique (Step 4 porting review feeds off this):**
- **Duplicate of engine functionality:** `render_3d/*` (the engine already has wgpu
  rendering + meshing), `material_colors.rs` (the engine has `MaterialRegistry`
  colors), and the egui-snarl editor embed (the engine now hosts the integrated
  editor — the settled "only editor").
- **Potentially unique / port candidates:** `colormap.rs` (heatmap ramp — no engine
  equivalent for a 2D field heatmap visualization) and any `runtime.rs` field-probe
  logic that visualizes intermediate graph outputs rather than final terrain. These
  are the items Step 4's porting list must evaluate for genuine usefulness *and*
  uniqueness before deletion.

**Step 4 contract reminder:** the porting list is its own deliverable requiring
user signoff before any port code is written; deletion of the crate + its workspace
member entry happens after that.

---

## 8. Inherited deferred items (TODO / FIXME / phase markers)

Markers found that intersect Phase 2 scope:

| Marker | Location | Phase 2 disposition |
|---|---|---|
| "PHASE 3 SHIM … this copy is deleted" | `world_generator.rs` (header + doc-comment) | **Step 2** removes; boundary is permanent, reframed/renamed |
| `load_from_ron` "deferred to a later phase (Phase 5 modding)" | `material_registry.rs` (header + method doc) | **Step 1** implements now; remove the deferral comments |
| `RonLoadingDeferredToFuturePhase` error variant | `voxel-core/src/error.rs` | **Step 1** — variant becomes unused once `load_from_ron` works; remove or repurpose for real RON parse errors |
| `format_version` meta write-only | `persistence.rs` L513 | **Step 5** — remove or wire a reader |
| `EDIT_FLAG_*` constants unused; flora/moisture not serialized | `persistence.rs` L46–48 + `TAG_DELTA` path | **Step 5** — resolve (implement or remove); decide whether richer delta is in scope (likely note as deferred — flora/moisture model is a later-phase concern) |

**Explicitly out of Phase 2 scope (do not touch):** four-layer chunk model,
walkability mask, slab smoothing, ChunkOverrides formal type, ChunkTags/tag
invalidation, five-graph hierarchy, new pin types, full egui_dock, fluid sim,
foliage, audio occlusion, networking/determinism beyond existing. The
flora/moisture *serialization* gap (item above) touches the edge of the deferred
flora model — Step 5 should fix only the format inconsistency, not build the flora
system.

---

## 9. Step-readiness summary

| Step | Starting state (from this audit) | Primary file(s) |
|---|---|---|
| 1 — RON materials | 9 materials hand-authored in `load_initial`; `load_from_ron` stubbed to error; no `ron` dep; no `materials.ron` | `material_registry.rs`, new `assets/materials.ron`, workspace + voxel-core `Cargo.toml`, `main.rs` startup |
| 2 — reframe shim | Index-copy in `generate_chunk_storage` labelled "PHASE 3 SHIM / deleted in Phase 3" | `world_generator.rs` → new boundary module |
| 3 — worker-pool eval | Steady-state eval already off main thread (streaming + regen); startup `World::generate` is the lone main-thread fill; two threading primitives; no main-thread assertion | `world/mod.rs`, streaming, regen, `nodegraph-eval` entry, `schedule.rs` |
| 4 — delete preview | Leaf crate; duplicate render_3d/material_colors/editor embed; possibly-unique colormap/runtime probes | `voxulacrum-preview/*`, workspace `Cargo.toml` |
| 5 — save format | write==read holds for blob tags; `format_version` meta dead; `EDIT_FLAG_*` unused / flora-moisture unserialized; wipe-migration mechanism exists | `persistence.rs`, new `voxulacrum-app/tests/` |

> Note: the audit lists Steps 2–4 by *concern*; the agreed **execution order** is
> Step 1 (RON) first, then the remaining concerns in the Phase 2 prompt's order.

---

*End of audit. No code has been changed. Awaiting signoff before Phase 2 Step 1
planning.*
