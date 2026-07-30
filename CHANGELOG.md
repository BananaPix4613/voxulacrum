# Changelog

All notable changes to the voxulacrum engine.

Pre-1.0 versioning follows `roadmap.md` §7.1: a minor version is a completed arc
shipped when its exit gates pass; patches are fixes only. Saves may be wiped on
upgrade through 0.5.x.

> **Entries for 0.1.0 through 0.2.0 were reconstructed at 0.3.0** from the commit
> history and the phase audits in `docs/`, because no CHANGELOG entries or git
> tags existed for them — the gap this version exists partly to close
> (`roadmap.md` §1 rule 6). Version boundaries were placed at the natural seams
> in the history — the foundation reset, and the first engine-integrated phase.
> Retroactive tags at those commits are what make the boundaries verifiable
> rather than asserted; see `roadmap.md` §7.5.

---

## [0.3.0] — Consolidation & Observability

Phase 10. **No new player-visible features.** The theme was closing the gap
between what the documents claimed and what the code did, making the engine
explain itself, and fixing the release process.

The headline: the version was gated on diagnosing a "~28 ms at-rest frame hitch
recurring several times per second" that had been the highest-priority open
defect since 0.2.0. **It did not exist.** It was a debug build (7× slower; the
workspace had no `[profile.dev]` at all) measured with a wall-clock metric that
*inverts* under vsync — a faster engine waits longer in `get_current_texture()`.
Release holds a locked 60 fps, and at the worst supported zoom the engine uses
5.5 ms of its 16.6 ms CPU budget.

### Added

- **`FrameTimings`** — per-`FrameStage` CPU timing with the swapchain present
  block isolated, so the frame budget is stated against work rather than
  blocking. Surfaced live and logged once per second.
- **Unified job system** (pillar P14) — one scheduler owning dispatch and
  concurrency for all continuous background work. Priority is
  `(class, distance²-to-camera)`; an interactive class lets an edit-driven
  re-mesh preempt bulk streaming.
- **Six further diagnostics** completing roadmap §4.4's Category C: job system
  inspector, streaming visualizer with a fill-time metric against the §7.3
  budget, memory/residency panel with session-growth tracking, mesh cache
  hit/miss with cold-vs-stale key attribution, mutation and event log, and an
  in-place determinism checker that regenerates resident chunks and surfaces the
  first divergent voxel.
- **Headless generation CLI** — `--verify-generation [N] [--seed S]` generates
  chunks twice in parallel and requires bit-identical output, wired into CI.
- **CI** — build, test and determinism verification on every push. The repository
  had none.
- **`[profile.dev]`** optimization levels, so development builds are usable for
  observation. Performance measurement remains release-only.
- Determinism tests for a **slab-bearing** chunk (guarding against a vacuous
  test) and for scatter seed derivation, pinned from both directions.
- Perf baseline document (`docs/perf-baseline.md`) and branch/tag discipline
  (`roadmap.md` §7.5).

### Changed

- **Concurrency is one global budget** dispatched by priority, replacing a static
  per-kind split that capped generation at a quarter of the pool while the rest
  sat reserved for meshing that was usually idle. `CoreBudget` reduces to a
  single `pool_threads`.
- **`StorageBoundary` is the single eval→storage crossing** for all three
  generated layers — terrain, fluid and foliage — closing drift 2.3. It carries
  the world's sea level.
- **Mutation door** gained `FinalizeSeam`, `EvictChunk` and `CommitFluidPlans`
  (which subsumes and removes `MarkFluidDirty`). `MutationOrigin::System` is now
  a per-intent whitelist rather than a universal exemption from the mode gate.
- **Mesh cache key** excludes the snapshot's edge and corner border cells, which
  are load-order dependent and which the mesher never reads.
- **Streaming view test is altitude-aware.** Under an isometric projection an
  altitude `Δy` displaces geometry up-screen by `Δy·√2` of ground-equivalent
  distance, so the test compares a column's projected span rather than its
  footprint.
- `half_extents` derives the away-axis from `zoom / sin(pitch)` rather than
  `zoom * aspect`, agreeing with `Camera::visible_radius`.
- `max_mesh_per_frame` is wired to the meshing snapshot budget; it had been
  declared and never read.
- **Worldgen hot-reload is save-triggered.** A graph file changing on disk is now
  the *only* trigger, so an editor Save and an external text-editor change take
  the identical path. Regeneration always reloads the whole hierarchy from disk,
  which makes a mix of edited and on-disk state unrepresentable rather than
  merely avoided. Editing no longer regenerates on every keystroke — which also
  removes a pathology where dragging a slider regenerated the world continuously.
  The authoring loop becomes edit → Save → see result; P11's bar is that an
  author needn't restart the engine, hand-edit a file, or guess at a result, and
  a save keystroke is none of those.
- CHANGELOG moved to the repository root.

### Fixed

- **Startup-fill chunks silently discarded persisted player edits.** Chunks
  within the boot radius regenerated without their override overlay, and the next
  edit in one wrote a record that had forgotten every earlier edit — data loss on
  disk, not just on screen. Both loaders now share one override-application path,
  and persistence opens before world generation.
- **Chunks meshed in scattered order**, with some filling in only after
  everything else. Meshing had no priority at all: its queue was filled in
  `HashMap` order, `swap_remove` scrambled it further, and chunks failing the
  face-neighbour check accumulated while later arrivals jumped ahead.
- **Tall terrain at the screen edge never loaded**, because the view test ignored
  altitude entirely.
- **Mesh cache re-key churn** — 1,400 of 4,758 lookups on a warm second run were
  rebuilds of meshes already on disk, each also deleting and rewriting a file.
  Warm-run hit rate went from 66 % to 100 %.
- **Graph hot-reload mixed in-editor and on-disk state.** Regeneration read the
  edited graph from memory and *every other graph from disk*, so editing one
  graph while another had unsaved changes produced a world matching neither.
  Higher-tier graphs suffered worst: a Zone graph's `GraphRef(World)` resolved
  against the on-disk World, so unsaved World edits were silently dropped. A
  second path — a filesystem watcher hardcoded to biome 0 — could regenerate the
  same content twice and refresh a canvas the author wasn't looking at. This is
  the defect 0.2.0 shipped with.
- **Zone-graph edits regenerated too few chunks.** Invalidation matches against
  `ChunkTags`, which describe the *previous* generation — sound for a Biome edit
  (which changes how an assigned biome looks) but not for a Zone edit (which
  changes which biome a column *is*). Moving a biome band boundary left every
  reassigned chunk untouched, since each was still tagged with the biome it was
  leaving. Zone edits now invalidate broadly; the constraint is recorded against
  design §4's invalidation table.
- **The editor's `● modified` marker did not survive switching graphs**, so
  returning to an edited graph showed the edits with Save greyed out and no way
  to commit them. Unsaved state is now tracked per slot and marked in the
  selector.
- Redundant face-neighbour marking on every chunk insert (the handler already did
  it); a doc-comment/behaviour mismatch in the streaming margin formula.

### Verified closed, not changed

Drift findings 1.4 (pipeline order), 2.1 (PoissonDisk seeding) and 2.2 (mesh
worker threading) were assigned to this version by the roadmap but had already
been fixed in 0.2.0's Phase 8 — the roadmap had been written against the drift
review rather than the audit that superseded it. Re-derived against the tree and
recorded.

### Known limits carried forward

Frame cost scales with *visible* chunk count through draw-call recording — three
terrain passes at ~5 µs per chunk draw. Outside the supported zoom range this
exceeds budget (31.4 ms at radius 25). ~490 MB of resident GPU mesh buffers at
that radius, ≈3.4M quads, is the first measured input to decision D8 (greedy
meshing, due at 0.4.0 exit). Both are downstream of naive one-quad-per-face
meshing, so they are one decision.

---

## [0.2.0] — Node-graph worldgen, foliage, fluid, player

Phases 1–9, 2026-06-10 through 2026-07-24. Announced but never version-stamped;
the workspace remained at `0.1.1`.

> **The stated core shipped complete** — infinite streamed world, the five-graph
> hierarchy, and multi-biome terrain rendering visibly distinct biomes.
>
> What went wrong was late and narrow: an eleventh-hour attempt to let an author
> switch the biomes used for world generation **from inside the engine**, by
> editing the Zone graph, rather than editing files externally and restarting.
> It was implemented too hastily to be tested properly, and shipped with an
> undiagnosed defect in terrain hot-reload and graph saving — see 0.3.0, where it
> is fixed.

### Added

- **Node-graph world generation.** A five-graph hierarchy — World (climate, zone
  assignment) → Zone (biome assignment) → Biome (density, material) → Detail
  (foliage), plus Library — assembled from a manifest and composited per chunk.
  Typed boundary ports, cross-graph dataflow (`GraphRef`/`GraphOutput`), per-biome
  parameter sidecars, density-blended biome borders, and tag-driven invalidation
  so a graph edit regenerates only the chunks it affects. The editor became a
  first-class panel inside the engine with a graph selector.
- **Chunk data model.** `ChunkCoord`/`LocalPos`/`FaceAxis` primitives; the runtime
  chunk split into a pure data `Chunk` and a `LoadedChunk` wrapper; typed sidecar
  layers (detail, scatter, fluid, decal, lighting); `ChunkTags`; the canonical
  `ChunkOverrides` diff set replacing the former edit list; a multi-layer save
  blob.
- **Slab smoothing** with a cross-chunk seam finalization pass, persisted so
  reloads neither re-derive nor revert it.
- **Foliage**, three tiers: GPU-generated grass blades from a per-chunk density
  buffer with wind sway and tint; instanced prefab scatter grouped per prefab; a
  data-driven prefab registry; anchor-based placement and removal.
- **Fluid** — ocean fill from a global sea level, cellular-automata flow with
  two-phase plan/commit, cross-chunk mass conservation, and water rendering with
  reflection, refraction and depth colouring.
- **Mutation command API** — a single door for every world write, declaring
  origin (authoring / play) and validated against the engine mode, with handlers
  owning persistence marking, mesh invalidation and override bookkeeping.
- **Player character** — semantic action layer with rebindable input, pure
  fixed-timestep simulation, slab-aware collision with auto-step, interpolated
  camera follow, capsule avatar with drop shadow and a two-pass stencil
  silhouette, reach-limited break/place, and a material HUD.
- **Underground visibility** — an air-side face rule with a cost-priority march
  and clarity radius, replacing an earlier clip-box cutaway that leaked the
  surface world under an angled camera.
- Tri-tonal axis lighting; the 32-byte packed `FaceVertex` format; a mesh disk
  cache; RON-driven material and prefab registries; SQLite + zstd persistence.
- Simulated climate driving cloud shadows.

### Removed

- The `voxulacrum-preview` standalone crate, its unique visualization (colormap
  and field probe) ported into the engine first.
- The resident walkability mask, after it shipped and produced a chunk-Y seam
  defect, a streaming regression and a poor fit for continuous collision.
  Movement, collision and room detection query geometry directly.
- The param-based `TerrainGenerator`, once the meadow graph reproduced its output.

### Fixed

- Water crossing a chunk boundary into an unedited chunk vanished on unload.
- Foliage generated before fluid and underwater.
- `PoissonDisk` seeded from chunk coordinates rather than world-absolute cells.
- Mesh workers ran on raw `std::thread` outside the shared pool, with
  hand-duplicated core-budget arithmetic.
- Per-voxel edits cloned an entire chunk's storage.

---

## [0.1.1] — Foundation reset

2026-05-26 through 2026-06-02. The prototype was peeled back to a cube-only voxel
world and rebuilt on a crate boundary.

### Added

- `voxel-core` — voxel value type, material identity, shared errors. No I/O, no
  rendering.
- `nodegraph-ir`, `nodegraph-eval`, `nodegraph-hotreload` — graph data model,
  per-chunk evaluator, filesystem watcher.
- A node vocabulary and a visual graph editor with 2D heatmap and 3D preview.
- Scatter point, scanner and prop nodes; a shape-aware mesher.

### Changed

- The voxel model unified on `voxel_core::Voxel` — a small shape vocabulary with
  no rotation field, packed into a `u32`. Chunk storage holds packed voxels
  behind a palette rather than bare material ids.
- Save blob format `BLOB_VERSION` 3 → 4; world disk-cache format 4 → 5.

### Migration

- **One-shot save wipe, intentional and pre-release.** Saved chunks under
  `saves/<world>/` and the mesh cache under `cache/meshes/` are cleared when the
  stored `voxel_format_version` is older than the current. Terrain regenerates
  from seed; no forward migration.

---

## [0.1.0] — Prototype

2026-02-09 through 2026-03-27. The original single-crate engine, superseded by
the 0.1.1 reset.

Isometric camera and window; voxel chunks and terrain generation; a dual
contouring mesher later replaced by marching cubes; lighting, shadows and a
day-night cycle; wind-driven instanced vegetation; water and lakes with
transparency; post-processing, a render-target upscale pass keyed to world pixel
density, Oklab colour quantization, pixel-perfect edge highlighting and camera
snapping; shader hot-reloading; asynchronous multithreaded meshing; frustum
culling; mesh and world caching; camera rotation and cross-section controls;
`bevy_ecs` and a render graph; an input abstraction layer; world streaming with a
priority queue; sparse storage and persistence.
