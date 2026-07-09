# Post-Phase-6 Audit — Consolidation

Close-of-phase snapshot for **Phase 6 (Consolidation)**. Companion to
`post-phase-5-audit.md` and the planning doc `pre-phase-6-audit.md`. Records what
shipped (the Phase-4 deferrals + the all-in library activation), the state of the
five-graph stack, test/verification coverage, and the deferred backlog.

**Status: complete.** Substeps 0–8 and 10–11 landed and were runtime-verified;
Substep 9 (multi-distance slab smoothing) was deferred by choice — see Deferred
below. Branch `mc-revision`.

## What Phase 6 delivered

The single flat Biome graph became a **five-graph hierarchy** (World → Zone →
Biome → Detail, plus Library) with typed boundaries, cross-graph dataflow,
per-biome parameters, a reworked density contract, and density-level biome
blending — while retiring the last of the legacy render path.

- **Typed boundary declarations (1):** `BoundaryPort { name, ty, description }`,
  `GraphBoundary { inputs, outputs }`, and the resolution types (`ResolvedPin`,
  `ResolvedBoundary`, `EffectivePin`). `NodeKind::effective_inputs()/effective_outputs()`
  return resolved-or-static pins. Resolution is **eager-cached** on the node params
  (`LibraryRefParams.resolved` / `GraphRefParams.resolved`, both `#[serde(skip)]`) so
  per-eval pin lookups don't re-walk the target graph.
- **Library activation with dynamic pins (2):** `LibraryRef` nodes resolve their pins
  from the referenced `LibraryGraph`'s boundary.
- **Five standard libraries (3):** native-kernel-backed — `LibraryKernel` enum
  (`BiomeBorderFade`, `StandardCaveNoise`, `SurfaceLayering`, `ExposureLayering`,
  `PoissonPlacement`) with shared kernel fns (`standard_cave_noise`, `surface_layering`,
  `exposure_layering`, `poisson_placement`, `biome_border_fade`, `blend_density`). Shipped as
  `assets/libraries/*.library.json`; loaded into `LoadedLibraries` / `LibrariesRes`.
- **Cross-graph dataflow (4):** `GraphRef` nodes with `GraphRefTarget { World, Zone,
  Biome(u16) }`, a `GraphOutput` marker node whose set derives a graph's output boundary
  (`derive_output_boundary`), cycle detection (`detect_graph_ref_cycle`), and evaluation via
  `UpstreamGraphs` + `ColumnEvaluator::with_upstream` (bulk reads the upstream cache;
  pointwise resamples). The world graph drives climate → the zone graph assigns biomes.
- **Per-biome parameter sidecar (5):** `BiomeParams { entries: HashMap<String, f32> }`, a
  `BiomeParam(name, default)` node that reads it, threaded through
  `Evaluator/WorldEvaluator::with_biome_params` and the manifest `params` field.
- **Per-biome `traversal_smoothing_distance` (6):** sourced from the sidecar per column
  (the smoothing pass reads the biome-id column and looks up each column's distance);
  `smooth_slabs(&mut ChunkStorage, distances: &[u32])`.
- **Biome density contract rework (7):** biome graphs now terminate in a `DensityOutput`
  (density + material inputs, no outputs) producing `CachedOutput::BiomeLayer { density,
  material }`; the world composite fuses `density > 0` into cubes, selecting the winning
  biome per column.
- **Density blending (8):** `analyze_biome_borders` (a chunk-independent scan that
  precomputes the radius-extended biome-id grid once, then reads it — the perf-critical
  path) yields per-column distance + nearest differing neighbor. The composite blends
  density toward that neighbor by `biome_border_fade(d, r) = (0.5 + 0.5·d/r).clamp(0.5, 1.0)`
  via `blend_density(own, nb, w) = own·w + nb·(1−w)` (`FADE_RADIUS = 10`). Material stays the
  **winning (assigned) biome's**, reapplied at depth below the *blended* surface through a
  `BiomeMaterialRule` (a `Layer`'s bands via `surface_layering`, a `ConstantMaterial`'s
  uniform value, or a fall-back to the precomputed field) — so it's consistent with the
  biome the column belongs to, never the fade neighbor's.
- **VegetationPass deletion + dead-code sweep (10):** deleted the orphaned
  `rendering/vegetation_pass.rs`, `create_vegetation_pipeline`, `PipelineId::Vegetation`, the
  registry's `vegetation_pipeline`, `GrassInstance`, `VegetationParams`, and
  `shaders/vegetation.wgsl`. The surviving debug toggle was renamed `hide_vegetation →
  hide_foliage` (it gates the detail-paint + scatter passes). Swept the dead-code warnings
  from **32 → 0**: real deletions (`CacheStats`, `disk_usage`, `DEPTH_FORMAT`,
  `LoadedChunk.generation`), `cargo fix` trivia, and `#[allow(dead_code)]` + in-source reason
  on every intentional scaffold (Chunk fluid/decal/lighting layers, the voxel-edit toolkit,
  RAII texture holders, the reserved post-process pipeline slot, natural accessors).

## Five-graph stack coherence
- IR: `Graph { kind: GraphKind, boundary: GraphBoundary, nodes, edges }`; `GraphKind`
  distinguishes World/Zone/Biome/Detail/Library. Library and graph refs cache their
  resolved boundary on the node.
- Eval: `WorldEvaluator` holds the world/zone graphs + per-biome `BiomeGraph { id, graph,
  density_node, material_rule, detail, params }`. `evaluate_chunk` builds the Zone evaluator
  with upstream world graph, computes border analysis, evaluates each biome's `BiomeLayer`,
  and composites (blended density + winning-biome material). `evaluate_foliage` runs each
  biome's DetailGraph on the composited terrain.
- App: `generate_chunk` → eval → materialize → `smooth_slabs(distances)` → tags →
  `paint_to_detail_layers` + `scatter_to_store`. Libraries load once into `LibrariesRes`.

## Test / verification coverage
- Cross-graph / composite (determinism + mapping): `world_climate_drives_zone_biome_assignment`,
  `composite_selects_winning_biome_material`, `biome_param_threads_into_biome_terrain`,
  `biome_param_reads_the_right_biome`, plus the single-biome composite/air baselines.
- Border analysis: `single_biome_has_no_borders`, `border_analysis_is_deterministic_and_bounded`,
  `border_scan_crosses_chunk_boundary_consistently` (validates the precomputed grid against a
  brute-force scan — the perf rewrite is byte-identical).
- Library kernels: per-kernel determinism/shape tests behind `LibraryKernel`.
- Runtime smoke is the real validator for rendering; the density-blend material saga
  (black voxels → shifted field → winning-biome rule) was resolved against live output.

## Deferred (intentional)
- **Substep 9 — multi-distance slab staircasing:** deferred. A correct implementation is
  genuine terrain morphology with three unresolved tensions — a min-cone erosion carves
  cliffs into slopes (contradicting "cliff faces stay sharp"), bounded radius produces
  mid-slope artifacts on drops taller than the distance, and the erosion lowers surfaces so
  scatter props (placed pre-smoothing) would float. It's a secondary feature; the per-biome
  distance **infrastructure** is live and already gives a visible meadow-vs-rocky difference.
  Net effect: `traversal_smoothing_distance` currently behaves as **on/off** (0 = terraced,
  ≥1 = the single half-step); values beyond 0/1 are reserved-but-inert until this lands.
- **Cross-chunk seam smoothing:** deferred with the above. `smooth_slabs` runs intra-chunk
  (window clamped at chunk edges), so staircase transitions that straddle a chunk boundary
  can seam. A correct version needs neighbors' pre-smoothing voxels — either on-demand
  neighbor generation (~5× eval cost) or a cached/two-phase generation pass.
- **Foliage float on eroded surfaces:** the re-anchor pass that would keep scatter props on a
  smoothing-lowered surface is unbuilt (only relevant once multi-distance lands).

## Verification results
- `cargo check --workspace` and `cargo check --workspace --tests`: **0 warnings** (down from
  32 at phase start), 0 errors.
- `cargo test --workspace`: green, 0 failures.
- Runtime (verified per substep): the five-graph world generates; changing the world graph's
  climate noise rearranges biomes; biome borders blend with the correct winning-biome
  materials and no black/mismatched voxels; the border-grid precompute restored generation
  speed; foliage renders and the renamed **"Hide foliage"** toggle hides the detail-paint +
  scatter passes.
- No on-disk format change this phase (`BLOB_VERSION` unchanged); dropped/renamed
  `EngineParams` fields are absorbed by `#[serde(default)]` + unknown-field tolerance.
