# Engine Foundation Reset — Codebase Audit & Migration Plan

**Audit date:** 2026-06-03
**Reference:** `docs/engine-design-CONCURRENT.md` v1.1 ("the design doc")
**Scope:** Drift diagnosis + phased migration plan across four workstreams. No code changed in this pass.

---

## 1. Executive Summary

The workspace contains **two parallel, almost entirely disjoint worlds**. `voxulacrum-app` (binary `voxulacrum`) is the real, high-quality core engine: bevy_ecs frame schedule, streamed infinite world, SQLite+zstd persistence, mesh disk cache, a full stylized render stack (shadow / outline / palette / post / upscale / water / vegetation / cap passes), and a clean, DPI-aware egui parameter UI. The other six-ish crates (`voxel-core`, `nodegraph-ir`, `nodegraph-eval`, `nodegraph-hotreload`, `nodegraph-editor`, `voxulacrum-preview`) form the **standalone graph-editor ecosystem** — well-separated crates, but wired to their own `eframe` window, their own wgpu device, their own 3D renderer, and a single in-memory chunk at `(0,0,0)`. **The engine never evaluates a graph, and the graph never touches the engine's world.** The `voxel-core` dependency declared by the engine is dead (zero usages).

The two worlds also disagree on the *data model*. The engine's voxel is material-only (`Voxel { material: u16 }`) with no shape field — slabs are unimplemented. The editor's voxel is the **obsolete slope vocabulary**: 22 `ShapeId` variants (slopes, outer/inner corners, ceiling slopes) plus a `Rotation` field, with a whole `slope_refine.rs` pass and a `SlopeRefiner` node — all of which design doc v1.1 deleted. Neither side implements the four-layer chunk (detail/scatter/fluid/decal/overrides), the five-graph hierarchy, the walkability mask, slab smoothing, or the rich mesh vertex format.

**Rough scope: 3–5 months** of focused work to reach the design-doc target. The good news is the hard parts already exist in isolation and mostly need *relocation and wiring*, not rewriting: the engine's runtime is sound, the editor's IR/eval/snarl stack is reusable once its data model is updated, and the engine's UI quality is the standard to extend. Suggested phasing: (1) integrate the editor as an engine panel against a stubbed pipeline; (2) unify the data model on slabs + four layers; (3) make the graph drive the engine's real world; (4) build out the design-doc pipeline passes; (5) polish the panel/dock UI. Workstreams 1, 3, and 4 can largely proceed before the deep generation work in Workstream 2's tail.

---

## 2. Current State Inventory

Workspace root `Cargo.toml`: 7 members, resolver 2. Two `[[bin]]` targets: `voxulacrum` (engine) and `voxulacrum-preview` (editor). Plus dev binaries in `nodegraph-hotreload/src/bin/`.

### 2.1 `voxulacrum-app` — THE CORE ENGINE (binary `voxulacrum`)

- **Declared purpose:** the engine (no description in manifest; `[[bin]] name = "voxulacrum-app"` → `src/main.rs`).
- **Actual contents:** a monolithic but well-organized single crate (~9k LoC). Subsystems:
  - `main.rs` (677) — one `winit` `ApplicationHandler`, one wgpu device/surface, builds the bevy_ecs `World` + `Schedule`, owns the event loop. `init_ecs` wires ~30 resources.
  - `world/` — `mod.rs` (`World { chunks: HashMap<IVec3,Chunk>, generator, min/max_chunk_y }`, `WorldManager` background regen), `chunk.rs` (`Chunk` = single `Arc<ChunkStorage>` voxel layer + mesh + edit-delta list; `ChunkSnapshot` with 1-voxel neighbor border for meshing), `voxel.rs` (material-only `Voxel`, 9-entry `MATERIAL_TABLE`), `generation.rs` (`TerrainGenerator` — monolithic fastnoise-lite height/cave/material noise), `storage.rs` (sparse/uniform chunk storage), `streaming.rs` (`ChunkStreamingManager`, infinite streaming), `persistence.rs` (rusqlite + zstd dictionary, delta/full chunk saves), `regen.rs` (`WorldRegenCoordinator`).
  - `meshing/` — `mod.rs` (`MeshingPipeline`, worker threads, `MaterialConfig`), `cube_mesher.rs` (naive per-face cube mesher), `cache.rs` (mesh disk cache + world cache), `coordinator.rs`.
  - `rendering/` — 20 files: `render_context.rs`, `surface_state.rs`, `pipelines.rs` (`TerrainVertex`, `PipelineRegistry`), `render_targets.rs`, `render_graph.rs`, `uniforms.rs`, and passes: `shadow_pass`, `main_scene_pass`, `outline_pass`, `palette_pass`, `post_process`, `upscale_pass`, `water_pass`, `vegetation_pass`, `cap_pass`, `debug_lines`.
  - `ecs/` — `schedule.rs` (`FrameStage`: Input→Simulation→Meshing→UniformWrite→Render→PostFrame), `systems.rs` (667), `resources.rs`, `events.rs`.
  - `simulation/` — `manager.rs`, `time_of_day.rs`, `wind.rs` (+ `cloud_shadow.rs`).
  - `ui/` — `mod.rs` (`EguiRenderer` over egui-winit/egui-wgpu), `panels.rs` (790; the "good" UI — one resizable `SidePanel::right` of collapsing parameter sections with green/yellow/red change-cost dots).
  - `camera.rs` (`IsometricCamera`), `input.rs` (raw→mapped input, egui passthrough), `params.rs` (686; `EngineParams` = all tunables, JSON presets), `palette.rs`, `paths.rs`, `shader_reload.rs`.
- **Key dependencies:** wgpu, winit, egui (+winit/+wgpu, **not** eframe), bevy_ecs, rusqlite, zstd, lz4_flex, notify, fastnoise-lite, fastnoise2, rayon, seahash, bincode. **`voxel-core` (path dep, UNUSED).**
- **Files of note:** `world/voxel.rs:14` (material-only voxel), `meshing/cube_mesher.rs`, `rendering/pipelines.rs:11` (`TerrainVertex`), `ui/panels.rs:118` (`draw_engine_panel`).

### 2.2 `voxel-core` — editor data-model primitives

- **Declared purpose:** "Voxel and chunk primitives for the voxulacrum world-generation engine."
- **Actual contents:** `voxel.rs` (`Voxel` = shape 5b + rotation 2b + material 16b + flags 8b, 6 bytes, pack/unpack to 31 bits), `shape.rs` (`ShapeId` with **22 slope/corner/ceiling variants**, `Rotation`), `material.rs` (`MaterialId`), `buffer.rs` (`ChunkBuffer<T, N>` dense 32³), `neighbor.rs`, `palette.rs`, `error.rs`, `lib.rs`.
- **Dependencies:** glam, serde (opt), thiserror, static_assertions. Leaf crate.
- **Consumers:** the entire nodegraph stack + preview. **Not** the engine.

### 2.3 `nodegraph-ir` — typed dataflow graph IR

- **Declared purpose:** "Typed dataflow graph IR for the … node system."
- **Actual contents:** `node.rs` (839; `NodeId` slotmap key, `NodeKind` enum + per-kind param structs, `NodeDescriptor`, `NodeCategory` incl. **`Slope`**), `pin.rs` (`PinType`: Scalar, Vec3, Density, Material, Curve, BiomeId, Positions, Assignments, Terrain), `edge.rs`, `graph.rs` (`Graph` = single flat node/edge set + `validate()` + JSON), `prefab.rs`, `diagnostic.rs`, `error.rs`.
- **Dependencies:** voxel-core, serde, serde_json, slotmap, glam, thiserror.
- **Note:** **single flat graph type** — no WorldGraph/ZoneGraph/BiomeGraph/DetailGraph/LibraryGraph concept.

### 2.4 `nodegraph-eval` — graph evaluator

- **Declared purpose:** "turns a graph into scalar/density fields for a chunk."
- **Actual contents:** `eval.rs` (537; `Evaluator`, `CachedOutput`), `context.rs` (`EvalContext { world_seed, chunk }`), `cache.rs`, `field.rs` (`ScalarField`, `CHUNK_DIM`), `scatter.rs`, `scan.rs`, `place.rs`, `png.rs`, **`slope_refine.rs` (398, OBSOLETE)**, `error.rs`, `lib.rs`.
- **Dependencies:** nodegraph-ir, voxel-core, glam, fastnoise-lite, image, criterion (note: criterion is a *normal* dep here, should be dev-dep).

### 2.5 `nodegraph-hotreload` — graph file watcher

- **Declared purpose:** "File-watcher for nodegraph JSON graphs; reloads + re-evaluates on change."
- **Actual contents:** `watcher.rs` (`GraphWatcher`, notify, pull-based `poll_changes`), `prefab.rs` (`resolve_prefabs`, `load_prefab`), `example.rs` (`bootstrap_example_graph`), `bin/watch_demo.rs`, `bin/regen_example.rs`, `lib.rs`, `error.rs`. Self-described as mirroring the engine's `ShaderWatcher`.
- **Dependencies:** nodegraph-ir, nodegraph-eval, voxel-core, notify, glam, serde_json.

### 2.6 `nodegraph-editor` — egui_snarl node editor

- **Declared purpose:** "Visual node-graph editor for nodegraph-ir, built on egui-snarl."
- **Actual contents:** `viewer.rs` (`GraphViewer: SnarlViewer`, node `catalog()` incl. **Slope Refiner**), `state.rs` (`EditorState`, undo/redo, toasts), `bridge.rs` (`graph_to_snarl`/`snarl_to_graph`), `params.rs` (per-node param widgets), `colors.rs` (pin/category colors), `lib.rs`.
- **Dependencies:** nodegraph-ir, voxel-core, egui, egui-snarl, glam. (No window/wgpu — it's a pure UI library, which is good.)

### 2.7 `voxulacrum-preview` — THE STANDALONE EDITOR (binary)

- **Declared purpose:** "Standalone 2D preview window for nodegraph density fields."
- **Actual contents:** `main.rs` (`eframe::run_native`, own viewport/window), `app.rs` (403; `PreviewApp: eframe::App` — top toolbar, right control panel, bottom 2D/3D tabbed panel, central snarl canvas), `runtime.rs` (`Runtime` — owns watcher, evaluates graph for `PREVIEW_CHUNK = (0,0,0)`, produces `ScalarField` + `ChunkBuffer<Voxel,32>`), `render_3d/` (`renderer.rs` 466, `camera.rs` `OrbitCamera`, `mesh.rs`, `shape_table.rs` 410 — **slope geometry tables**, `mod.rs`), `colormap.rs`, `material_colors.rs`.
- **Dependencies:** all 4 nodegraph crates + voxel-core + **eframe, egui-wgpu, wgpu** (its own GPU stack).

### 2.8 Dependency shape

```
voxel-core ─┬─ nodegraph-ir ─┬─ nodegraph-eval ─┬─ nodegraph-hotreload ─┐
            │                └─ nodegraph-editor ┘                       │
            └────────────────────────────────────────── voxulacrum-preview (BIN)

voxulacrum-app (BIN) ── voxel-core   [DEAD EDGE: 0 usages]
```
The two binaries share *no runtime code* — only the unused `voxel-core` edge nominally connects them.

---

## 3. Drift Findings

> Format: `Finding N: title. Location. Description. Reference.`

**Finding 1: The editor is a separate executable, not an engine panel.**
Location: `crates/voxulacrum-preview/src/main.rs:18-44` (`eframe::run_native`), `Cargo.toml:6-11` (two `[[bin]]`).
Description: A second binary owns its own `eframe` window, viewport, and event loop. The design doc mandates one `main`, one renderer, one event loop, editor as an in-engine panel.
Reference: §1 ("The engine is the host"), §12 ("Editor as engine panel").

**Finding 2: The graph never drives the engine's world.**
Location: engine generation `crates/voxulacrum-app/src/world/generation.rs` (fastnoise, param-driven) vs editor `crates/voxulacrum-preview/src/runtime.rs:174-217` (graph→`ScalarField`/`ChunkBuffer`). No code path connects them.
Description: "The graph is the world generator" is unrealized. Editing the graph repaints the preview's own chunk; the engine regenerates from `TerrainGenParams`, not a graph.
Reference: §1 ("The graph is the world generator"), §4, §12 (hot reload).

**Finding 3: Engine declares an unused `voxel-core` dependency.**
Location: `crates/voxulacrum-app/Cargo.toml:11`; `grep voxel_core crates/voxulacrum-app` → 0 matches.
Description: Dead dependency edge; the engine has its own parallel `world/voxel.rs` model. Relocation/cleanup item, but also the *symptom* of the two-worlds split.
Reference: §2, §3 (single shared data model intended).

**Finding 4: Obsolete slope shape vocabulary in `voxel-core`.**
Location: `crates/voxel-core/src/shape.rs:21-130` (`ShapeId` 22 variants: `SlopeN/E/S/W`, `Outer/InnerCorner*`, `CeilingSlope*`, `CeilingOuterCorner*`; `Rotation`). `voxel.rs:29-38,42-57` packs shape 5b + rotation 2b.
Description: Design doc deleted slopes. Target shape set is exactly `Empty/Cube/SlabBottom/SlabTop` (3 bits, no rotation field). The current model has neither slab variant and carries a rotation field the doc explicitly forbids.
Reference: §2 ("Four shapes total… No rotation field").

**Finding 5: `slope_refine.rs` is a whole obsolete pass.**
Location: `crates/nodegraph-eval/src/slope_refine.rs:1-398` — reclassifies cubes into slopes/corners; cites superseded `docs/voxel_shape_atlas_spec.md`.
Description: The slope-refinement marching/cascade logic is dead under the slab model. Slabs are produced by the engine slab-smoothing pass keyed on the walkability mask, not by per-voxel slope reclassification in eval.
Reference: §2, §5 stage 7 (slab smoothing).

**Finding 6: `SlopeRefiner` node + `Slope` category.**
Location: `crates/nodegraph-ir/src/node.rs:37-38` (`NodeCategory::Slope`), `crates/nodegraph-editor/src/viewer.rs:45` (catalog `Slope Refiner`), `colors.rs:31` (`NodeCategory::Slope` fill).
Description: Node vocabulary still exposes slope refinement as an authoring node.
Reference: §2, §4 (node vocabularies per graph type).

**Finding 7: Preview 3D renderer carries slope geometry tables.**
Location: `crates/voxulacrum-preview/src/render_3d/shape_table.rs` (410), `render_3d/mesh.rs`.
Description: Per-shape vertex tables for the 22-variant slope set. Obsolete; also part of a duplicate renderer (Finding 9).
Reference: §2, §10.

**Finding 8: Engine voxel has no shape field — slabs unimplemented.**
Location: `crates/voxulacrum-app/src/world/voxel.rs:14-17` (`Voxel { material: u16 }`); `chunk.rs`/`storage.rs` store materials only.
Description: The engine cannot represent `SlabTop`/`SlabBottom`. The design doc's foundational geometry (half-height slabs) does not exist on the engine side.
Reference: §2.

**Finding 9: Duplicate rendering + meshing between engine and preview.**
Location: engine `rendering/` + `meshing/cube_mesher.rs` vs preview `render_3d/renderer.rs:1-466` (independent wgpu pipeline, `OrbitCamera`, `RenderCallback`, mesh builder).
Description: Two renderers, two mesh builders, two cameras. The preview's exists only because it is a separate app. Under integration it is fully redundant with the engine stack.
Reference: §1, §10, §11.

**Finding 10: Single-layer chunk — no detail/scatter/fluid/decal/overrides layers.**
Location: `crates/voxulacrum-app/src/world/chunk.rs:37-52` (`Chunk` = one voxel storage + mesh + edit list).
Description: The design doc's four parallel layers + reserved decals + overrides split (generated vs authored) is not modeled. The engine has an ad-hoc `edit_list`/`VoxelEdit` delta for persistence, not the structured `ChunkOverrides`.
Reference: §3, §9.

**Finding 11: Flat graph — no five-graph hierarchy, no `ChunkTags`.**
Location: `crates/nodegraph-ir/src/graph.rs` (single `Graph`); no World/Zone/Biome/Detail/Library types anywhere.
Description: No graph-type system, no per-graph node subsets, no `LibraryRef`, no zone/biome registration, no `ChunkTags`-driven invalidation.
Reference: §4 (entire section), §5 (staged pipeline).

**Finding 12: Pin vocabulary is a partial subset.**
Location: `crates/nodegraph-ir/src/pin.rs` / `nodegraph-editor/src/colors.rs:5-17` (9 pin types).
Description: Missing design-doc pins: `SurfaceField`, `FluidProvider`, `ZoneId`, `ScatterPoints`, `PlacementMask`, `SpeciesWeights`, `PaintOutput`, `ScatterOutput`. Coercion rules (`Scalar→Density`, `Curve→Scalar`) not formalized as the doc specifies.
Reference: §4 (Pin types, Coercion rules).

**Finding 13: No walkability mask, no slab-smoothing pass.**
Location: pipeline absent in both `world/generation.rs` and `nodegraph-eval/src/eval.rs`.
Description: The two engine passes the doc makes foundational (Stage 6 walkability, Stage 7 slab smoothing) don't exist. Pathfinding/movement reuse of the mask is therefore impossible.
Reference: §5 stages 6–7, "Walkability mask", "Traversal smoothing distance".

**Finding 14: Mesh vertex format lacks the stylized attributes.**
Location: `crates/voxulacrum-app/src/rendering/pipelines.rs:11-19` (`TerrainVertex` = position, normal, color, ao, material_id, cell_flags, pad).
Description: Missing `face_axis`, `occlusion_class`, `biome_tint_index`, `variant_index`, `light_level_index`, `enclosure_factor`, `edge_flag`, `sway_weight`, packed `uv`. The mesher (`cube_mesher.rs`) is naive (no greedy policy, AO hardcoded to 1.0, no neighbor-sampled AO/edge flags).
Reference: §10 (vertex format, mesher requirements, greedy policy).

**Finding 15: UI has no panel/dock architecture.**
Location: `crates/voxulacrum-app/src/ui/panels.rs:118` (`draw_engine_panel` = one hardcoded `SidePanel::right`); `ui/mod.rs:80` (single `visible` bool toggles all UI).
Description: Layout is hardcoded; there is no panel abstraction, no docking, no game-mode/dev-mode distinction beyond a global visibility flag. Quality is high but not extensible to many tools.
Reference: §12 (multiple dev tools as panels), Workstream 3 goal.

**Finding 16: Node editor uses default snarl styling.**
Location: `crates/voxulacrum-preview/src/app.rs:255-257` (`SnarlStyle::default()`), `nodegraph-editor/src/viewer.rs:118-131` (bare `ui.label` pins).
Description: The "ugly" node UI is unstyled defaults on top of a capable framework (egui_snarl), not a framework limitation — re-skinnable.
Reference: Workstream 4.

**Finding 17 (divergence/ambiguity): Two unrelated water models.**
Location: engine `rendering/water_pass.rs` (visual water surface from sea level) vs design doc §7 `FluidLayer` (mass-conserving CA, `FluidFillMode`, `FluidCell`).
Description: The engine renders water but has no fluid *simulation layer*. The doc specifies a full CA fluid system. Flag for sequencing (likely a late phase), not a v1.1 contradiction.
Reference: §7.

---

## 4. Workstream 1 Plan — Editor-Engine Integration

**Goal:** one binary, one event loop, one renderer, one chunk store; the node editor is an engine panel; graph edits regenerate the engine's real chunks.

Ordered steps (each with a done-signal):

1. **Make the engine depend on the nodegraph stack.** Add `nodegraph-ir`, `nodegraph-eval`, `nodegraph-hotreload`, `nodegraph-editor` to `voxulacrum-app/Cargo.toml`; remove the dead `voxel-core` edge once the data model is unified (Workstream 2). *Done-signal:* engine builds with the IR/eval crates linked.
2. **Host the snarl editor inside `EguiRenderer`.** Add an `EditorState` (from `nodegraph-editor`) to `UiState`; render it in a new egui window/panel inside `panels::draw_engine_panel`'s context (Workstream 3 generalizes this to a dock). Reuse `snarl_to_graph`/`graph_to_snarl`. *Done-signal:* the node canvas appears in the running engine; nodes can be added/connected; no second window.
3. **Introduce a `GraphWorldGenerator` seam.** Define a trait the world uses to fill a chunk (`fn generate_chunk(coord) -> ChunkData`), with two impls: the existing `TerrainGenerator` (param-based) and a new graph-backed one wrapping `nodegraph_eval::Evaluator`. Route `World::generate` / streaming workers / `WorldRegenCoordinator` through the trait. *Done-signal:* a feature flag or UI toggle selects graph-backed generation and the engine renders a graph-generated world (initially via a translation shim from `ChunkBuffer<Voxel>` to engine `ChunkStorage`).
4. **Wire graph edits → engine regeneration.** On `EditorState::consume_dirty()`, rebuild the `Graph`, diff against the running graph, and enqueue affected-chunk regeneration through the existing `WorldRegenCoordinator`/streaming path (`world/regen.rs:107`). Initially invalidate-all; refine to `ChunkTags` later (Workstream 2). *Done-signal:* editing a node in-engine visibly regenerates terrain in place.
5. **Fold disk hot-reload into the same mutation path.** Add `nodegraph_hotreload::GraphWatcher` as an ECS resource ticked in `FrameStage::Input` (mirroring `ShaderWatcher`); on-disk graph edits and in-engine edits converge on step 4's regeneration entry point. *Done-signal:* editing the `.graph.json` on disk and editing in-engine produce identical regen.
6. **Delete `voxulacrum-preview`.** Once steps 2–5 land, the standalone binary, its `render_3d/`, `OrbitCamera`, `colormap`, and `eframe` dep are redundant. *Done-signal:* `voxulacrum-preview` removed from the workspace; engine retains all capability (2D heatmap/3D preview become debug panels if still wanted, built on the engine renderer).

**Effort: ~3–4 weeks** (the seam in step 3 and the data-model shim are the bulk; cheap because the engine's regen/streaming plumbing already exists).

---

## 5. Workstream 2 Plan — Crate Restructuring

**Principle:** keep the editor stack's clean separation and extend that discipline to the engine, which is currently one large crate. Split by *data ownership and update cadence* (the design doc's own justification for four layers), so the build stays parallel and subsystems stay swappable.

### Target layout

| Crate | Purpose | Notes / key deps |
|---|---|---|
| `voxel-core` | Voxel/shape/material/chunk-buffer primitives. **Rewrite `ShapeId` to `Empty/Cube/SlabBottom/SlabTop`, drop `Rotation`.** Single shared model for engine + eval. | glam, serde, bytemuck, static_assertions |
| `voxel-chunk` | The four-layer `Chunk` (voxels, detail, scatter, fluids, decals), `ChunkTags`, `ChunkOverrides`, serialization/versioning. | voxel-core, serde, bincode, smallvec, slotmap |
| `nodegraph-ir` | IR extended to **five graph types** + `LibraryRef`, full pin vocabulary, coercion rules. | voxel-core |
| `nodegraph-eval` | Evaluator + per-column caches. **Remove `slope_refine.rs`.** Produces layer data, not slopes. | nodegraph-ir, voxel-chunk, fastnoise, rayon |
| `worldgen-pipeline` | The 13-stage pipeline orchestrator: walkability mask, **slab smoothing**, fade blending, structure stamping, fluid init, lighting bake, override application. | nodegraph-eval, voxel-chunk, rayon |
| `voxel-mesher` | Greedy-by-policy mesher, rich `FaceVertex`, neighbor-aware AO/edge/tint, slab face handling. | voxel-chunk |
| `fluid-sim` | `FluidLayer` mass-conserving CA (design §7). Separable; lands late. | voxel-chunk |
| `foliage` | Detail layers + scatter store + prefab metadata + anchor model + instanced rendering data. | voxel-chunk, voxel-core |
| `nodegraph-hotreload` | Keep ~as-is; engine-agnostic watcher. | nodegraph-ir, nodegraph-eval |
| `nodegraph-editor` | Keep as a pure egui_snarl UI library; **re-skin** (Workstream 4); update catalog per graph type, drop slope nodes. | egui, egui-snarl, nodegraph-ir |
| `ui-shell` | Shared panel/dock framework, theme, font stack, change-cost-dot widgets (extracted from `ui/panels.rs`). Consumed by engine UI *and* the editor panel. | egui, egui_dock |
| `voxulacrum-app` | The thin host: `main`, wgpu/winit, bevy_ecs schedule, render passes, streaming, persistence, camera, input. Depends on all of the above. | wgpu, winit, egui, bevy_ecs, rusqlite, zstd, notify |

### Migration steps

1. **Unify the voxel model first** (blocks almost everything): rewrite `voxel-core::ShapeId` to the slab set, delete `Rotation`, repack `Voxel` to shape 3b + material 16b + flags 8b. Update the engine to *use* `voxel-core` (kills Finding 3 and Finding 8 together) — the engine's `world/voxel.rs` `MATERIAL_TABLE` becomes data feeding `voxel-core::MaterialId`. *Done:* one voxel type across the workspace.
2. **Extract `voxel-chunk`** from the engine's `world/{chunk,storage}.rs` + design-doc layer structs; move the engine onto it incrementally (voxels layer first, others stubbed empty). *Done:* engine chunk = four-layer type with only the voxel layer populated.
3. **Extract `voxel-mesher`** from `meshing/cube_mesher.rs`, growing the vertex format toward §10. *Done:* engine meshes through the new crate; vertex format extended.
4. **Extract `ui-shell`** (pairs with Workstream 3). *Done:* `panels.rs` widgets live in `ui-shell`; engine consumes them.
5. **Stand up `worldgen-pipeline`** as the home for the staged generator; the param-based `TerrainGenerator` moves in as the bootstrap impl, then graph-eval becomes the primary path. *Done:* generation is staged and graph-driven.
6. **Split off `foliage` / `fluid-sim`** as those systems are built out (late).

**Effort: the splits themselves are ~2–3 weeks of mechanical work**, but they interleave with the feature build-out; treat as ongoing rather than a single phase. Step 1 is the critical, do-it-first item (~3–4 days).

---

## 6. Workstream 3 Plan — Panel-Based UI Architecture

**Current:** high quality, zero extensibility — one `SidePanel::right`, one `visible` bool (Finding 15).

**Target architecture:**

- Adopt **`egui_dock`** for docking/undocking/rearranging. It is the mature, idiomatic choice for egui tab/dock trees and integrates with the engine's existing egui-winit/egui-wgpu setup (no eframe needed). Justification over alternatives: `egui_tiles` is the main competitor and is also viable, but `egui_dock`'s tab semantics match a tools-IDE layout most directly; either avoids a custom dock implementation.
- Define a `Panel` trait in `ui-shell`: `fn title(&self) -> &str; fn ui(&mut self, ui: &mut egui::Ui, cx: &mut EngineUiContext);`. Each tool (graph editor, engine parameters, asset browser, prefab editor, palette editor, biome inspector, profiler, debug-viz) is one `Panel`.
- A `PanelRegistry` + `DockState` owned as an ECS resource; the existing `EguiRenderer::draw` renders the dock tree instead of calling `draw_engine_panel` directly. The current `draw_engine_panel` body becomes the `EngineParametersPanel`.
- **Shared theme/font stack:** a single `apply_theme(&egui::Context)` in `ui-shell` (visuals, spacing, font sizes, DPI via `pixels_per_point` already wired in `ui/mod.rs:25,75`). Both the engine panels and the snarl editor consume it — the snarl editor additionally gets a custom `SnarlStyle` (Workstream 4).
- **Game mode vs dev mode:** replace the single `visible` bool with a `UiMode { Game, Dev }`. In `Game`, the dock is hidden and the runtime view is full-screen; in `Dev`, panels coexist over/around the viewport. The 3D world always renders to the framebuffer underneath; panels are egui on top (as today).

**Migration steps:**
1. Extract widgets/theme into `ui-shell`; wrap the existing engine panel as `EngineParametersPanel` (no behavior change). *Done:* engine looks identical, now panel-shaped.
2. Add `egui_dock` + `PanelRegistry`; render the params panel through the dock. *Done:* params panel is a dockable tab.
3. Add the graph editor as a second `Panel` (consumes Workstream 1 step 2). *Done:* editor and params dock side by side.
4. Add `UiMode` toggle. *Done:* a hotkey switches game/dev; game mode is clean full-screen.

**Effort: ~2 weeks.** Low risk — additive over a working UI.

---

## 7. Workstream 4 Plan — UI Quality Reconciliation

**What the engine UI does well (keep, extract to `ui-shell`):** DPI-correct `pixels_per_point` from `window.scale_factor()`; consistent `collapsing` section hierarchy; the green/yellow/red **change-cost dot** pattern (`ui/panels.rs:99-116`) that tells the user an edit's cost (instant / remesh / regen) — genuinely good UX worth generalizing; disciplined `add_enabled_ui` gating during long ops; combo/slider/dragvalue consistency.

**What the standalone editor UI does badly, and why:** `SnarlStyle::default()` (`app.rs:256`) with bare `ui.label` pins and no theme application (`viewer.rs:118-131`). Root cause = **(a) bad authoring on a reasonable framework**, not (b) misuse or (c) a framework limit. egui_snarl supports custom `SnarlStyle`, pin shapes/colors (already partly used via `pin_color`), header frames (already used via `category_fill`), and body widgets. The ugliness is missing styling + no shared theme + default fonts, all fixable in place.

**Remediation (re-skin within egui_snarl; no library change needed):**
1. Apply the shared `ui-shell` theme to the editor panel's context so fonts/spacing/colors match the engine.
2. Author a project `SnarlStyle`: node corner radius, header height, wire thickness/curvature, background grid, selection highlight — matched to the engine palette.
3. Upgrade pin rendering: typed pin shapes + the existing `pin_color`, aligned labels, hover tooltips with pin type/coercion info; keep the diagnostic severity badge but restyle it to match the change-dot language.
4. Restyle the add-node menu (`show_graph_menu`) and param widgets (`params.rs`) to the engine's slider/dragvalue conventions.
5. Re-skin happens against the panel from Workstream 3 so it inherits docking/theme for free.

**Effort: ~1 week.** Pure styling; gated behind Workstream 1 (editor in-engine) and Workstream 3 (theme exists).

---

## 8. Cross-Workstream Dependencies

- **W2 step 1 (unify voxel model on slabs) is the global prerequisite.** It must precede the graph→engine generation wiring (W1 step 3/4) and any slab/mesher/pipeline work. Do it first.
- **W1 step 2 (editor in-engine) requires** W2 step 4 / W3 step 1 (a place to host the panel + shared theme). Practically: extract `ui-shell` (W3.1) before or alongside hosting the editor.
- **W3 (panels) gates W4 (re-skin):** the editor must live in an engine panel with a shared theme before re-skinning is meaningful.
- **W1 step 6 (delete preview) requires** W1 steps 2–5 complete.
- **W2 tail (`worldgen-pipeline`, `fluid-sim`, `foliage`) depends on** the four-layer `voxel-chunk` (W2 step 2) and is otherwise independent of the UI workstreams — it can proceed in parallel once the data model lands.
- **Obsolete slope removal** (Findings 4–7) is unblocked immediately and should ride along with W2 step 1.

---

## 9. Phased Migration Roadmap

Each phase ends building and demonstrable; none breaks the build.

**Phase 0 — Cleanup & model unification (≈1 week).**
Remove obsolete slope code (`slope_refine.rs`, `SlopeRefiner` node, `NodeCategory::Slope`, preview slope tables). Rewrite `voxel-core::ShapeId` → `Empty/Cube/SlabBottom/SlabTop`, drop `Rotation`, repack `Voxel`. Make the engine *use* `voxel-core` (delete its private `Voxel`, keep `MATERIAL_TABLE` as material data). *Artifact:* engine runs on the unified, slab-capable voxel type; slope code gone; one voxel model in the tree.

**Phase 1 — UI shell + editor in-engine (≈2–3 weeks).**
Extract `ui-shell` (theme, change-dots, panel trait) and wrap the current params UI unchanged. Add `egui_dock`. Host the snarl editor as a dockable panel (no second window). *Artifact:* the running engine shows dockable Parameters + Graph Editor panels; `voxulacrum-preview` still exists but is now redundant for editing.

**Phase 2 — Graph drives the world (≈3–4 weeks).**
Add the `GraphWorldGenerator` seam; route streaming/regen through it; wire in-engine + disk graph edits to chunk regeneration via `WorldRegenCoordinator`. Delete `voxulacrum-preview`. *Artifact:* editing the graph in-engine regenerates the real streamed world in place; one binary.

**Phase 3 — Four-layer chunk + slab pipeline (≈4–6 weeks).**
Extract `voxel-chunk` (four layers + overrides + tags). Stand up `worldgen-pipeline` with the staged order; implement the walkability mask and slab smoothing; extend `voxel-mesher` for slab faces + richer `FaceVertex`; greedy-by-policy + neighbor AO/edge/tint. *Artifact:* terrain shows emergent slab "slopes"; mesher produces the stylized vertex attributes; pathfinding-ready walkability mask exists.

**Phase 4 — Five-graph hierarchy + invalidation (≈4–6 weeks).**
Extend the IR to World/Zone/Biome/Detail/Library graphs with per-type node subsets, `LibraryRef`, full pin vocabulary + coercion, and `ChunkTags`-driven targeted invalidation. Build the matching multi-graph editor UX (graph-type picker). *Artifact:* multi-biome worlds authored across the graph hierarchy; targeted regen on edits.

**Phase 5 — Foliage, fluids, polish (≈4+ weeks, parallelizable).**
`foliage` crate (three tiers + anchors + prefabs), `fluid-sim` CA layer, lighting bake, atmospheric/outline polish toward §11. Re-skin pass (W4). *Artifact:* the design doc's full system surface.

---

## 10. Open Questions

1. **Engine `TerrainGenParams` vs graph as source of truth.** The engine's noise generator + preset system is mature. Is it (a) replaced by graphs, (b) kept as a fast bootstrap path behind the `GraphWorldGenerator` trait, or (c) re-expressed as a default `BiomeGraph`? The doc says "the graph is the world generator," implying (a)/(c), but (b) de-risks Phase 2. Need your call.
2. **`CHUNK_DIM` / chunk size parity.** Engine `CHUNK_SIZE = 32`; eval `field.rs CHUNK_DIM`/`ChunkBuffer<_,32>` also 32 — confirm they're meant to be the same constant and unify it (likely in `voxel-core`).
3. **Material identity unification.** Engine has a 9-entry `MATERIAL_TABLE` (u16 ids); IR/eval use `voxel_core::MaterialId` + material nodes. What is the authoritative material registry, and is it static or graph/data-driven?
4. **Persistence model vs `ChunkOverrides`.** The engine's `VoxelEdit`/`edit_list` delta persistence (rusqlite+zstd) predates the doc's `ChunkOverrides` (§9). Migrate the on-disk format to the override model, or keep the engine's delta format as the serialization of overrides? Affects save-file compatibility.
5. **Water: two systems.** Confirm the engine's visual `water_pass` is subsumed by the §7 `FluidLayer` CA (with `water_pass` becoming its renderer), and when (Phase 5 assumed).
6. **`egui_dock` vs `egui_tiles`.** I recommend `egui_dock`; confirm before Phase 1, since panels are built against it.
7. **Doc silence: the engine's mesh/world *disk cache* and *streaming*** (`meshing/cache.rs`, `world/streaming.rs`) are real, load-bearing systems the design doc doesn't mention. They should be added to the doc as foundational, or explicitly scoped as implementation detail. Flagging per "surface ambiguity."

---

## 11. Risks

- **Determinism regressions.** The doc mandates bit-identical output and RNG seeded from `(world_seed, …, world_pos, purpose)`, never chunk coords alone (§5, §12). The engine's noise gen and the eval path must be reconciled to one deterministic scheme; mesh/world caches key on params (`compute_world_cache_key`) and will silently serve stale data if the keying doesn't capture the graph. *Test:* same seed+graph → identical chunk hashes across runs and chunk-load orders; invalidate caches on graph hash.
- **The voxel-model rewrite (Phase 0) is wide-blast-radius.** Repacking `Voxel` touches serialization, GPU upload, and every consumer. *Mitigation:* land it behind the existing `voxel-core` tests (`roundtrip.rs`) extended for slabs; do it before any feature build-out.
- **Save-file breakage.** Changing the voxel format and moving to `ChunkOverrides` can invalidate existing `saves/` and mesh caches. *Mitigation:* version bump + the doc's "missing layers default to empty" forward-compat rule; provide a one-shot cache wipe (the regen path already clears caches, `world/regen.rs:74-95`).
- **Two renderers during transition.** Until `voxulacrum-preview` is deleted (Phase 2), the slope-table 3D renderer and the engine renderer coexist; don't invest in the preview renderer — it's slated for deletion.
- **egui input routing leakage.** Input already splits cleanly (`main.rs:636-645` sets `egui_wants_keyboard/pointer`; `input.rs` respects it). Adding docked panels increases egui's pointer-capture surface; *test* that game camera/edit input still works when panels are open vs. closed, and in game mode.
- **Scope creep in the graph hierarchy (Phase 4).** The five-graph system is the largest single piece. *Mitigation:* keep Phase 2's flat-graph generation working throughout; introduce graph types additively (start with World+Biome, add Zone/Detail/Library).
- **Performance of in-place regen on graph edits.** Invalidate-all (Phase 2) is fine for the initial bounded world but won't scale to streaming; ensure the `ChunkTags` targeted-invalidation (Phase 4) lands before large worlds are the norm.

---

*End of audit. No source files were modified. The report file itself is the only artifact created.*
