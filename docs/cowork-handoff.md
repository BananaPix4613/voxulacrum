# Cowork Handoff — voxulacrum engine (Phase 5 + active meshing bug)

This document hands off in-progress work on the **voxulacrum** voxel engine. Read it
top-to-bottom. **Part 1 (the active bug) is the immediate priority** — do not start new
feature work until it's resolved. Parts 2–4 give the project context, conventions, and
architecture you need.

> Note on working style: the prior session ran in "plan-and-present" mode (the human
> transcribed code by hand from BEFORE/AFTER blocks). **You can edit files directly.**
> Keep the discipline that mattered, though: read the current code before changing it,
> surface real decisions before large changes, add determinism tests for new generation
> passes, name things after the general mechanism (not the specific scenario), resist
> scope creep, and confirm each fix builds + tests + runs before moving on.

---

## Part 1 — ACTIVE BUG (priority): terrain chunks render as all-cubes (slabs missing)

### Symptom
Some terrain chunks render with **no slabs** (all full cubes) even though the world uses
half-height slab smoothing. It's **reload/re-mesh specific**: on a fresh cache, the first
view of a region is correct (slabs present); after those chunks leave view and are
reloaded, *some groups* come back as cubes. Foliage (grass + scatter) on those same chunks
is correct. A full terrain regenerate (the UI button) temporarily "fixes" it because it
force-re-meshes everything.

Screenshot evidence from the human: correct chunks have full voxels **and** slabs **and**
foliage; broken chunks have full voxels only (zero slabs) **but still have foliage**.

### The single most important finding
On reload: **chunks that render correctly are cache HITs; chunks that render as cubes are
cache MISSes.** (Confirmed empirically by the human via a diagnostic log — see below.)

### What has been PROVEN and RULED OUT (don't re-derive these)
1. **Generation is deterministic.** Test `generate_chunk_is_deterministic` in
   `crates/voxulacrum-app/src/world/world_generator.rs` passes. ⚠️ Caveat: it uses chunk
   `(1,0,2)`, which per logs is a rocky single-biome chunk that may have **no slabs at
   all**, so the test may not actually exercise slab determinism. Consider adding a
   "guard against vacuous" assertion (pick a chunk known to have slabs, assert slab count
   > 0) — but generation being deterministic is not seriously in doubt.
2. **Slabs are enabled.** `traversal_smoothing_distance` default = `1` (params.rs:430).
3. **Startup and streaming share ONE `Arc<WorldGenerator>`** — `main.rs` passes
   `generator.clone()` to both `World::generate` and `ChunkStreamingManager::new`
   (~main.rs:440). So reload generation uses the same slab distance as startup. Not a
   "different generator" bug.
4. **`smooth_slabs` runs only in `generate_chunk`** (`world/world_generator.rs:180`). It is
   per-chunk, neighbor-independent, deterministic (uses `WalkabilityMask::from_storage`
   with an "outside-the-chunk is air" convention; `world/slab_smoothing.rs`).
5. **The mesher is faithful.** `cube_mesher::generate_chunk_mesh`
   (`meshing/cube_mesher.rs`) reads each voxel's `shape` and renders `SlabBottom` as
   half-height (`shape_y_interval`). It meshes **only the chunk interior (0..32)**; the
   neighbor border is used only for face culling, never re-shaped. So a cube mesh ⟺ the
   snapshot's **interior** was cube.
6. **The mesh cache key includes shape.** `compute_cache_key` (`meshing/cache.rs:146`)
   hashes `snapshot.materials` via `Voxel::pack()`, and `pack()` encodes shape in bits
   0–2 (`voxel-core/src/voxel.rs:88`). So a slab chunk and a cube chunk get different
   keys — a slab chunk cannot load a cube mesh file. The key **also includes the neighbor
   border** (the snapshot is the full 34³ = interior + 1-voxel border).
7. **The snapshot is taken from `chunk.data.voxels` at drain time**
   (`meshing/mod.rs:216,234`, `ChunkSnapshot::extract` in `world/chunk.rs:171`). `extract`
   copies full `Voxel`s (shape included).
8. **Foliage is not involved.** All three chunk-assembly sites set `detail_layers` +
   `scatter_instances`; the foliage GPU buffers (`DetailPaintPass`, `ScatterPass`) build
   correctly. The `mesh@` diagnostic confirms foliage data/buffers are correct on the
   cube chunks.
9. **`upload_mesh_result`** (`world/mod.rs:100`) always sets `chunk.mesh = Some(...)`;
   `mesh_seq` only gates whether `mesh_dirty` is cleared (stale-result handling).
10. **`save_cached_mesh`** calls `remove_stale_for_chunk` (`meshing/cache.rs:219`), which
    deletes *other-key* cache files for that chunk position when a new one is saved.

### The contradiction (this is where the prior session got stuck)
Every path analyzed says a MISS rebuild should produce a **slab** mesh (snapshot interior
= chunk data = slab, mesher faithful). Yet **MISS → cube**. So on the MISS path, the
**snapshot's interior must be cube** even though `chunk.data.voxels` reportedly has slabs.
That means one of:
- **(A)** `ChunkSnapshot::extract` is producing a cube interior from slab storage — i.e.
  the chunk object being snapshotted is not the one the `mesh@` diagnostic reads (stale /
  placeholder / wrong-Arc). → fix `extract` / the chunk lookup / the submit ordering.
- **(B)** The reloaded chunk genuinely has cube voxels despite `generate_chunk` running
  `smooth_slabs` — something on the reload path (`spawn_generation` override/self-healing,
  the `ChunkEdits::Full` branch, or `apply_overrides_to_storage`) overwrites the smoothed
  storage with a pre-slab one. → fix the reload generation/override path.
- **(C)** The mesh is a stale on-disk cache file after all (contradicts the key analysis,
  but the HIT/MISS data is empirical). → fix the cache key / invalidation.

### The decisive next measurement (proposed, not yet applied)
Add this in `crates/voxulacrum-app/src/meshing/mod.rs`, in `drain_pending_submissions`,
immediately after `let snapshot = ChunkSnapshot::extract(...)` (~line 236). It compares
the chunk's stored slab count to the snapshot's **interior** slab count at snapshot time:

```rust
let chunk_slabs = (0..crate::world::chunk::CHUNK_VOLUME)
    .filter(|&i| matches!(
        chunk.data.voxels.voxel(i).shape,
        voxel_core::ShapeId::SlabBottom | voxel_core::ShapeId::SlabTop
    ))
    .count();
let snap_interior_slabs = {
    let mut c = 0usize;
    for z in 0..CHUNK_SIZE as i32 {
        for y in 0..CHUNK_SIZE as i32 {
            for x in 0..CHUNK_SIZE as i32 {
                if matches!(
                    snapshot.get_voxel(x, y, z).shape,
                    voxel_core::ShapeId::SlabBottom | voxel_core::ShapeId::SlabTop
                ) { c += 1; }
            }
        }
    }
    c
};
log::info!("submit@{:?} chunk_slabs={} snap_interior_slabs={}", pos, chunk_slabs, snap_interior_slabs);
```

Run with `$env:RUST_LOG="voxulacrum_app=info"`, reload a group that goes cube, and read one
`submit@` line for a cube chunk:
- `chunk_slabs>0` but `snap_interior_slabs=0` → case **(A)** (extract/lookup).
- `chunk_slabs=0` and `snap_interior_slabs=0` → case **(B)** (reload generation).
- both `>0` → case **(C)** (cache), because the built mesh should then be slab.

### Strong leading hypothesis to investigate first
The mesh cache key **includes the neighbor border**, so the *same* chunk is re-keyed every
time a neighbor changes load state, and `remove_stale_for_chunk` deletes the previously
good file. On reload the border differs → MISS → rebuild. Investigate whether the cube is
tied to that: e.g. does the border ever cause `extract` to snapshot before the chunk's
slab storage is in place, or is there a placeholder-chunk / mesh-before-slab race on the
reload path (`streaming.rs::spawn_generation` → `insert_chunk` → neighbor-dirty at
`streaming.rs:377-383`)? A plausible clean fix (pending the `submit@` result) is to key the
mesh cache on the chunk's **own interior voxels only** (border affects only boundary face
culling), which makes a chunk always HIT its correct cached mesh — but confirm the root
cause first so you don't just mask it.

### Diagnostics currently in the tree (temporary — remove once fixed)
- `crates/voxulacrum-app/src/ecs/systems.rs`, in `meshing_tick_system`: a `log::info!`
  starting `mesh@{pos} slabs=... biomes=... data[detail_layers=.. scatter=..] gpu[paint=.. scatter=..]`.
  `slabs` = interior slab count of `chunk.data.voxels` at upload time.
- `crates/voxulacrum-app/src/meshing/mod.rs`, in the worker: a `log::info!` starting
  `snapshot@{pos} snap_slabs=.. HIT|MISS key=..`. **Note:** `snap_slabs` there is the
  **whole 34³ snapshot including border**, so a cube-interior chunk can still show
  `snap_slabs>0` from slab neighbors — that is why "snap_slabs is never 0" was misleading.
  The proposed `submit@` log above measures interior-only and is the one that matters.

---

## Part 2 — Project context

**voxulacrum**: a Rust voxel-generation engine driven by a node-graph. Cargo workspace,
6 crates:
- `voxel-core` — core types: `Voxel` (packed u32: shape 3 bits, material 16, flags 8),
  `ShapeId` (Empty/Cube/SlabBottom/SlabTop), `MaterialId`, `LocalPos`, `ChunkCoord`,
  `ChunkBuffer<T,N>`.
- `nodegraph-ir` — the graph IR: one flat `Graph { kind: GraphKind, nodes: SlotMap, edges }`.
  `GraphKind = World|Zone|Biome|Detail|Library`. Closed `PinType` / `NodeKind` enums.
- `nodegraph-eval` — evaluators: `Evaluator` (3D), `ColumnEvaluator` (2D),
  `WorldEvaluator` (multi-graph harness), `DetailEvaluator` (foliage). Eval-domain foliage
  types in `foliage.rs` (`ChunkFoliage`, `PaintLayer`, `ScatterBucket`, `FoliageInstance`).
- `nodegraph-hotreload`, `nodegraph-editor` — editor + hot-reload (egui/snarl).
- `voxulacrum-app` — the binary. `bevy_ecs` + `winit` + `wgpu`. Everything in Part 1 and
  Part 4 lives here.

**Git**: branch `mc-revision` (main branch is `master`). Windows / PowerShell primary
shell; a Bash tool (Git Bash) is also available. Run the engine with
`$env:RUST_LOG="voxulacrum_app=info"; cargo run -p voxulacrum-app` (debug is default; no
world/mesh cache to delete for the world, but `cache/meshes/` holds mesh cache files).

### Phase 5 (Foliage Content) — status
Design refs: `docs/engine-design.md` (v1.2, §6 Foliage, §8 Anchor, §9 Diff/StableInstanceId,
§10 Mesh, §11 Rendering), `docs/post-phase-4-audit.md`, `docs/pre-phase-5-audit.md`.

**Done:**
- 1a/1b — foliage `PinType`s + `NodeKind`s (PoissonDistribution, SurfaceFilter,
  BiomeContextMask, SpeciesPicker, PaintDensity, ScatterPlace) + editor colors + Detail
  graph validation.
- 2a/2b — eval-domain `ChunkFoliage` + `DetailEvaluator` + `WorldEvaluator::evaluate_foliage`.
- 3 — `paint_to_detail_layers` boundary + `GeneratedChunk.detail_layers` + assembly wiring.
- 4a-1 — meadow grass DetailGraph (`assets/graphs/biome_meadow.detail.json`) + manifest
  `detail` field.
- 4a-2i / 4a-2ii — **Tier-1 GPU grass paint**: `DetailPaintPass`
  (`rendering/detail_paint_pass.rs`) + `shaders/detail_paint.wgsl` (per-chunk density
  storage buffer, GPU-generated blades, wind, tint). Retired the old `VegetationPass` from
  the render path (its module is orphaned; UI panel + pipeline still compile but unused —
  a deferred cleanup).
- 5 — Tier-2/3 scatter data plumbing: `scatter_to_store` + `GeneratedChunk.scatter` +
  assembly wiring.
- 6a — scatter chain (Poisson→SpeciesPicker→ScatterPlace) added to the meadow detail graph.
- 6b — **Tier-2/3 instance rendering**: `ScatterPass` (`rendering/scatter_pass.rs`) +
  `shaders/scatter.wgsl` — placeholder **lit cubes** per instance (real prefabs are
  Substep 10).
- 9 — scatter diff model: `ScatterInstance.stable_id` field; `ChunkOverrides.effective_scatter`;
  persistence bump `BLOB_VERSION` 5→6 and `VOXEL_FORMAT_VERSION` 2→3 (the latter forces a
  one-shot wipe of stale saves + mesh cache on open).
- 8a/8b/8c — **anchor-based player interaction** (`interaction.rs`): `PointerState`
  (cursor + L/R buttons), `picking_system` (screen→world ray via inverse `view_projection`
  + voxel DDA → `PickState.anchor` + yellow highlight box reusing `DebugLinePass`),
  `scatter_edit_system` (left-click removes scatter props at the picked surface anchor →
  `scatter_removed`; right-click places one → `scatter_added`, `PLAYER_PLACED`).

**Also done this session (outside the numbered substeps):**
- Fixed a real persistence bug: `spawn_generation`'s `ChunkEdits::Delta` reload path was
  dropping `scatter_added`/`scatter_removed` (only carried `voxel_diffs`). Now carried.
- **Removed the world cache** (the on-disk snapshot of generated chunks): it stored voxels
  only (no foliage) and went stale as the pipeline grew, so cached chunks loaded without
  grass. Deleted `save_world_cache`/`load_world_cache`/`compute_world_cache_key`/
  `world_cache_path`/`WORLD_CACHE_FORMAT` from `meshing/cache.rs`, `World::from_cached_chunks`
  from `world/mod.rs`, and the load/save sites in `main.rs`. `clear_world_cache` is kept
  (used by the regen path + the persistence format wipe). The world now always regenerates
  at startup. **The mesh cache (`cache/meshes/`) is separate and still active** — it is the
  focus of the Part 1 bug.

**Remaining Phase 5 substeps (do the Part 1 bug FIRST):**
- 7 — Tier-3 LOD billboards (deferred; weakly motivated until real prefab textures exist).
- 10 — starter-world DetailGraphs + prefab meshes + billboard textures (real content /
  art assets; replaces the placeholder scatter cubes).
- 11 — verification sweep (with mandatory runtime smoke test).

**Out of scope for Phase 5:** Phase-4 deferrals (density-boundary blending, cross-graph
named boundary pins, per-biome traversal_smoothing_distance, multi-distance slab
smoothing, library activation), fluid sim, decals, audio, AI/movement, full edit toolkit,
animated foliage beyond wind sway, the retired `voxulacrum-preview`.

---

## Part 3 — Conventions the human expects
- **Determinism tests are mandatory for every new generation/evaluator pass**; pure
  refactors get equivalence tests. Guard against *vacuous* determinism (assert the pass
  actually did something).
- **Name code after the general mechanism, not the specific scenario** (saved memory:
  `feedback_naming_conventions`). E.g. "traversal smoothing", not "stairs for the plains".
- Read current code before proposing changes; surface genuine decisions before big/
  irreversible changes; resist scope creep and note deferred items explicitly.
- Match surrounding code style (comment density, naming, idiom).
- After a change: `cargo build` clean, `cargo test`, and — for anything visual — a runtime
  smoke test is the real validator (the Part 1 bug is invisible to unit tests).

---

## Part 4 — Architecture facts relevant to the bug (voxulacrum-app)

**Chunk data model** (`world/chunk.rs`): `LoadedChunk { data: Chunk, mesh_dirty, mesh_seq,
mesh, ... }`. `Chunk { coord, voxels: Arc<ChunkStorage>, detail_layers, scatter_instances,
overrides: Option<ChunkOverrides>, fluids, decals, tags, ... }`. `ChunkStorage` = storage
domain (Uniform | Populated). `ChunkSnapshot` (34³ = 32³ interior + 1-voxel border) is the
meshing input, built by `ChunkSnapshot::extract(chunk, neighbors, ...)`.

**Generation** (`world/world_generator.rs`): `WorldGenerator::generate_chunk(pos)` →
`GeneratedChunk { storage, tags, detail_layers, scatter }`. Steps: `WorldEvaluator`
(world→zone→biome graphs, per-column biome composite in
`nodegraph-eval/src/world_eval.rs`) → `StorageBoundary::materialize` → **`smooth_slabs`** →
`derive_tags` → `paint_to_detail_layers` → `scatter_to_store`. Same generator `Arc` is used
by `World::generate` (startup, blocking par_iter), streaming, and regen.

**Meshing** (`meshing/mod.rs`, `meshing/coordinator.rs`, `meshing/cube_mesher.rs`):
async worker pool. `MeshingCoordinator::tick` → `drain_pending_submissions` (build
snapshots, gated by `world.has_face_neighbors`, capped/frame) → workers (check on-disk
cache by key; on miss run `generate_chunk_mesh` + save) → `poll` → `world.upload_mesh_result`.
`mesh_seq` (`mark_mesh_dirty` increments it) discards stale mesh results for `mesh_dirty`
clearing. Newly-meshed positions are returned to `meshing_tick_system`, which then calls
`DetailPaintPass::add_chunk` + `ScatterPass::add_chunk` (foliage GPU buffers).

**Mesh cache** (`meshing/cache.rs`): on-disk in `cache/meshes/`, file
`x_y_z_{key:016x}.bin`. Key = seahash of the whole snapshot (`Voxel::pack()`, shape
included) + material colors. `save_cached_mesh` → `remove_stale_for_chunk` deletes
other-key files for the chunk. `CACHE_VERSION` gates format compatibility.

**Streaming** (`world/streaming.rs`): `ChunkStreamingManager` holds its own
`Arc<WorldGenerator>` (same one). `spawn_generation` runs `generate_chunk` in a worker,
applies saved overrides (`ChunkEdits::Delta` self-heals voxel_diffs and carries scatter
diffs; `ChunkEdits::Full` replaces storage), assembles a `LoadedChunk`, sends it over a
channel. `tick` inserts received chunks (rate-capped), pushes to `result.inserted`, and
**marks the 6 face neighbors `mark_mesh_dirty`** so their borders re-cull (lines ~377-383)
— this is the neighbor-dirty re-mesh churn relevant to the bug.

**Rendering** (`rendering/`): `MainScenePassNode` draws terrain → cap → **detail paint** →
**scatter** → water → debug lines → **pick highlight**, into SCENE/NORMAL/DEPTH.
`PipelineRegistry` holds named pipelines with hot-reload keyed by shader filename
(terrain/shadow/vegetation/detail_paint/scatter/water). Camera is orthographic isometric
(`camera.rs`; `view_projection()` = proj*view). Window size from
`SurfaceState.surface_config`.

**Persistence** (`world/persistence.rs`): `WorldPersistence` over a `.vxdb`. `ChunkOverrides`
(voxel_diffs, voxel_removed, scatter_removed, scatter_added, detail_diffs, fluid_diffs,
decal_diffs). `BLOB_VERSION=6`, `VOXEL_FORMAT_VERSION=3` (bumping the latter one-shot-wipes
saved chunks + the mesh cache on open — useful if you change generation/mesh encoding).

**ECS schedule** (`ecs/schedule.rs`): stages Input → Simulation → Meshing → UniformWrite →
Render → PostFrame. `picking_system` + `scatter_edit_system` run in Simulation after
`simulation_tick_system`. `streaming_tick_system` runs before `meshing_tick_system` in
Meshing. `world_regen_system` in PostFrame.

---

### Suggested first moves for Cowork
1. Apply the `submit@` diagnostic (Part 1), reproduce, and read one cube chunk's line.
2. Follow the decision tree to case A / B / C and fix the root cause.
3. Rebuild, `cargo test`, and **runtime-verify**: load a large area, pan away and back,
   confirm reloaded chunks keep their slabs (this is the only real test for this bug).
4. Remove the temporary `mesh@` / `snapshot@` / `submit@` diagnostics.
5. Resume Phase 5 at Substep 10 (content/prefabs) or 7 (LOD), per the human's direction.
