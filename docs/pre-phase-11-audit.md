# Pre-Phase-11 Audit — Worldgen Completeness & Authoring (0.4.0)

Planning reference for **Phase 11**, which serves version **0.4.0**.
Companions: `post-phase-10-audit.md` (prior state), `roadmap.md` §11 (the scope
contract), `engine-design.md` v1.9, `perf-baseline.md` (the 0.3.0 figures),
`architecture-drift-review.md`. Branch `v0.4.0`, based on `cf055cf`.

**Method note.** Every claim below is derived from the tree as it stands, not
from prior documents. Where a document and the code disagree, §10 records it.
Line references are current as of `cf055cf`.

---

## 0. Headline findings — read these before the ordering

### 0.1 "3+ biomes across 2 zones" is engine work, not content

The roadmap files this under *content proof*. It is not. A second zone cannot
be expressed by the current data model:

| Site | Current shape |
|---|---|
| `WorldManifest` (`world_generator.rs:396`) | `zone: String` — one path, not a list |
| `GraphSlot` (`world_generator.rs:384`) | `World \| Zone \| Biome(u16)` — `Zone` is a unit variant |
| `GraphRefTarget` (`nodegraph-ir`) | `World \| Zone \| Biome(_)` — `Zone` is a unit variant |
| `WorldEvaluator` (`world_eval.rs:75`) | `zone: Graph` — a single field |
| `world.graph.json` | `WorldOutput { zone_bands: [0.0], zone_ids: [] }` — empty |
| `ChunkTags.zone` | Populated, but from a `WorldOutput` that assigns nothing |

Adding a second zone touches the manifest schema, the slot enum, the cross-graph
target enum, the evaluator's graph set, the hierarchy editor's selector, and the
invalidation table. It is the single largest hidden item in the version, and it
is a prerequisite for one exit gate and a precondition for another (the
zone-edit invalidation path, §0.4).

### 0.2 The "zero hand-edited JSON" gate is unmeetable today, for a small reason

`nodegraph-editor`'s canvas catalog (`viewer.rs:28`) has 44 entries against 50
`NodeKind` variants. The six absent kinds are:

`DensityOutput`, `FluidOutput`, `GraphRef`, `GraphOutput`, `LibraryRef`,
`BiomeParam`.

Those are precisely the nodes a biome graph terminates in, a graph exposes
outputs through, a graph imports through, and a library is referenced by. A
biome graph authored entirely from the canvas cannot be terminated; a zone graph
cannot read the world graph.

The good news is that this is narrower than it looks: `params.rs:127–176`
already renders parameter UIs for `LibraryRef`, `GraphRef`, `GraphOutput` and
`BiomeParam`, and `DensityOutput`/`FluidOutput` have no parameters. **The gap is
insertion, not editing.**

### 0.3 The three biomes are two biomes, and neither is what the gate asks for

Shipped content, read from `assets/graphs/`:

| Biome | Assigned? | Detail graph | Density chain | Material |
|---|---|---|---|---|
| 0 `meadow` | yes | yes | Simplex FBm + masked Ridged − Y | Layer cake grass/soil/stone, fill=2 |
| 1 `rocky` | yes | **no** | Simplex FBm + Ridged − Y | Layer `[(stone,1)]`, fill=stone |
| 2 `test` | **no** | **no** | Simplex FBm − Y | Layer grass/soil, fill=stone |

`zone.graph.json` assigns `biome_ids: [0, 1, 0]` across bands `[0.0, 0.25]`.
Biome 2 is registered in the manifest and never selected.

Against the gate's clauses: no biome sits below sea level (`sea_level = 24`;
meadow remaps to `[12, 52]`, rocky to `[20, 80]`), **no biome is cave-bearing**
(no `LibraryRef` appears in any shipped graph), and **no biome authors
`FluidOutput`** — so `composite_fluid` (`world_eval.rs:336`) returns `None` on
every chunk and the entire biome-pond path is unexercised by shipped content.

Likewise unexercised: `PlacePrefab` / `PlaceTree` appear in no shipped biome
graph, so `assets/prefabs/rock.prefab.json` and
`nodegraph_hotreload::resolve_prefabs` are live code with no live consumer.

None of the five libraries under `assets/libraries/` is referenced by any graph.

### 0.4 Zones make the recorded invalidation bound live for the first time

`regen.rs:177–186` carries `InvalidationTarget::Zone(ZoneId)` marked
`#[allow(dead_code)]`, with a comment stating it becomes reachable "once multiple
Zone graphs exist and a zone assignment is stable across a Zone-graph edit."
`classify_edit` currently routes `GraphSlot::Zone → AllChunks`.

Phase 10 recorded this as a bound on the invalidation table
(`engine-design.md` §4). Phase 11 is the version that reaches it. The correct
mapping is not obvious and must be decided deliberately, because there are two
different edits wearing one name:

- Editing **ZoneGraph[Z]'s body** (biome bands inside Z) changes which biome
  each column in Z is assigned. Chunks tagged `zone == Z` are still in Z, so
  `Zone(Z)` matching is sound *for this edit*.
- Editing the **WorldGraph's zone selector** changes which zone a column is in.
  Tags describe the assignment being replaced. That is already `AllChunks`.
- Editing the **manifest's zone list** is structural → `AllChunks`.

So the row becomes correct at exactly the granularity the design doc states —
but only if the slot enum distinguishes "a zone's body changed" from "zone
assignment changed", which today it cannot, because there is one zone.

### 0.5 A `LibraryRef` in a density chain reintroduces chunk-Y grass banding

Confirmed by reading, not inferred. `world_eval.rs:453–476` compensates for the
chunk-Y material seam by pointwise-sampling density *above* the chunk window and
pushing the material cake's banding depth down by the continuation:

```rust
let Some(od) = own_layer.sample_density(x, ay, z) else { break };
```

`LayerEval::sample_density` (`world_eval.rs:593`) delegates to
`Evaluator::sample_density`, which returns
`Err(EvalError::UnresolvedLibraryRef)` for a `LibraryRef` node
(`eval.rs:599–601`). The `else { break }` then leaves `above = 0`, i.e. the
pre-continuation behaviour: banding measured from the window top, which is the
grass-strata artifact.

**The two halves of the `LibraryRef` work are therefore one substep, not two.**
Closing the fill path (`eval.rs:502`) without closing the pointwise path
(`eval.rs:599`) ships a visible regression the moment the cave biome lands.

Note also that `Evaluator` has no access to a kernel map at all: it is
constructed as `Evaluator::new(&bg.graph, ctx).with_biome_params(...)`
(`world_eval.rs:517`), and the `LibraryGraphId → LibraryKernel` map lives on the
**app** crate's `LoadedLibraries` (`libraries.rs:41`), which the evaluator never
sees. `WorldEvaluator` holds only a `LibraryGraphRegistry` (boundaries, for pin
resolution). Closing the gap requires threading kernels into `nodegraph-eval`.

### 0.6 There are two "prefab" concepts, and the blueprint format must pick one

| Type | Crate | Shape | Consumers |
|---|---|---|---|
| `PrefabDef` | `voxulacrum-app/prefabs.rs:31` | `{id, id_name, shape: Rock\|Bush\|GrassTuft, color, scale}` | scatter render pass; `ScatterInstance.prefab_id` |
| `PrefabTemplate` | `nodegraph-ir/prefab.rs:21` | `{anchor: [i32;3], voxels: Vec<{at, voxel}>}` | `PlacePrefab` node, via `resolve_prefabs` at the hot-reload boundary |

The roadmap's §6.6 target ("voxel template, anchor offset, footprint,
destruction policy, sway weights, variant sets, placement rules, protected
volume") is a superset of `PrefabTemplate` and orthogonal to `PrefabDef`.
Design §6's `PrefabMeta` exists **only in the document** — no Rust type carries
`anchor_offset`, `footprint`, `on_anchor_destroyed`, or `sway_weights`
(drift-review 5.5, still open).

If the blueprint format is designed without deciding which of these it
generalizes, 0.4.0 ships a *third* prefab concept. Decision Q4 in §14.

### 0.7 Job dependencies: the second edge is real, but it is not in this half

`jobs.rs:10–16` records the omission and names the trigger: "the staged
cross-chunk generation pass that structures and rivers require." That pass has
no other consumer. Building the dependency machinery before structures and
rivers exist would repeat exactly the mistake Phase 10 declined to make. This is
the strongest single argument for where the phase split falls (§11).

### 0.8 `--verify-generation` cannot express generation-order independence

`verify.rs:107–116` maps over positions in parallel and, *per position*,
generates the same chunk twice and compares. That proves per-chunk purity. It
does not and cannot prove that generating a region in a different **order**
yields the same result, because each position is independent and order never
varies.

Cross-chunk placement needs a different assertion shape: generate the same
region under at least two different orders (and ideally with a different subset
resident) and compare bit-for-bit. The roadmap's instruction to extend
`--verify-generation` rather than add a parallel harness still holds — but it is
an extension in kind, not in count.

---

## 1. Tree state and baseline

### 1.1 Versions and build

| | |
|---|---|
| Workspace version | `0.3.0` (`Cargo.toml`) |
| Branch | `v0.4.0`, based on `cf055cf` |
| Tags | `v0.1.0`, `v0.1.1`, `v0.2.0`, `v0.3.0` |
| Working tree | clean |
| `cargo test --workspace` | **281 passed, 0 failed** (exit 0) |
| Crates | 6: `voxel-core`, `nodegraph-ir`, `nodegraph-eval`, `nodegraph-hotreload`, `nodegraph-editor`, `voxulacrum` |
| Source | 143 `.rs` files, ~24.5 kLOC across the src trees inspected |

`voxulacrum` has **no `lib` target** — `[[bin]] voxulacrum-app` only. This
constrains where a validation CLI can live (§7.1, Q3).

CI (`.github/workflows/ci.yml`) runs `cargo build --workspace`,
`cargo test --workspace`, and `--verify-generation 64` on Windows on every push.
A release workflow builds Windows and Linux on `v*` tags.

### 1.2 Shipped content inventory

```
assets/graphs/     world.manifest.json  world.graph.json  zone.graph.json
                   biome_meadow.graph.json (+ .detail.json)
                   biome_rocky.graph.json  biome_test.graph.json
                   example.graph.json          <- bootstrap demo, not world content
assets/libraries/  5 x *.library.json          <- declared, none referenced
assets/prefabs/    rock.prefab.json            <- loadable, no live consumer
assets/            materials.ron  prefabs.ron  input.ron
```

**No graph asset carries a version field.** `biome_meadow.graph.json` — the
primary biome — carries no `kind` field either, so it deserializes to the
default. Graph asset versioning (a 0.4.0 deliverable) starts from zero.

The manifest has no `libraries` key; libraries load by directory scan
(`libraries.rs:75`), contradicting design §4's "the manifest is the resolution
root." It also has no `seed` (seed comes from `TerrainGenParams`) and no
`version`.

---

## 2. Workstream A — cross-chunk generation infrastructure

### 2.1 The generation contract today is strictly single-chunk

`WorldGenerator::generate_chunk(position)` (`world_generator.rs:159`) evaluates
one chunk from `(seed, coords)` and returns a `GeneratedChunk`. It has no
neighbour context, no staging set, and no second phase.
`ChunkStreamingManager::submit_generation` (`streaming.rs:330`) submits one job
per chunk, each fully independent.

Design §5 stage 8 ("ZoneGraph structures — deferred placement; may straddle
chunks; cross-chunk template stamping with priority resolution") has no
implementation and no call site.

### 2.2 What already exists that a cross-chunk stage should generalize from

Three mechanisms, and they are genuinely different in kind:

**(a) The margin-band ownership model** (`scatter.rs:16–18`). Scatter covers
`[-M, N+M)²` with `PROP_MARGIN = 12.0`, so a prop whose footprint crosses a
border is emitted (and clipped) by the owning chunk. This is the model the
roadmap says to generalize. It works because placement is derived from
world-absolute cell seeds (`EvalContext::world_cell_seed`, `context.rs:73`) and
because the owner is decided by the point's own position, never by which chunk
ran first.

Its limit is that the margin is a **constant**, sized for props. A structure
larger than 12 voxels in XZ, or one that extends vertically across chunk-Y,
needs the margin to be a property of the feature rather than of the module.

**(b) The seam finalization pass** (`world/seam.rs`). Residency-gated,
idempotent, order-independent by construction, applied through the door via
`FinalizeSeam`, recorded in overrides plus a persisted `seam_finalized` flag.
This is a genuinely good precedent for *post-insert* cross-chunk work, and the
module doc argues its order-independence explicitly.

Its limit is that it runs on **resident** chunks after insertion. A structure
must be present in the chunk before it meshes, and must be identical whether or
not its neighbours were ever resident.

**(c) The biome-border scan** (`border.rs::analyze_biome_borders`, called at
`world_eval.rs:284` with `FADE_RADIUS = 10`). This already reads *outside* the
chunk during column evaluation, to find the nearest differing biome. It is the
closest thing in the tree to a cross-chunk read inside generation, and it is
pure: it re-derives neighboring columns rather than reading neighbour chunks.

**The design question A must answer** is which of these three the structure/river
stage is. My reading: (c) is the model — *re-derive, never read a neighbour's
output* — with (a)'s ownership rule deciding who stamps and (b)'s idempotence
argument as the correctness proof. Reading a neighbour's generated output would
make generation order-dependent, which is the property P1 forbids and which the
gate explicitly tests.

### 2.3 Job dependencies

`jobs.rs` is 362 lines: `JobKind {Generate, Mesh, ChunkIo}`,
`Priority {class, distance}` with a reversed `BinaryHeap`, one global
`max_running` from `CoreBudget`, per-kind submission limits as backpressure, and
per-kind atomics for introspection. There is no dependency edge, no
`JobId`, and no completion notification other than each consumer's own channel.

`ChunkIo` has zero submitters (confirmed: `JobKind::ChunkIo` appears only in the
enum, `ALL`, `index`, and `name`).

Adding dependencies means adding job identity (`PendingJob` already carries
`key: IVec3` and `seq: u64`, both currently `#[allow(dead_code)]` or private),
plus a ready-predicate at dispatch. The heap-based dispatch loop in `pump`
(`jobs.rs:257`) would need to skip-and-defer rather than break, or maintain a
blocked set.

---

## 3. Workstream B — event bus

### 3.1 There is no bus

`ecs/events.rs` is 20 lines: four `bevy_ecs` `Event` structs
(`RegenerateWorld`, `RemeshAll`, `ClearMeshCache`, `LoadPaletteRequest`), all
UI-request events, none emitted by the mutation door or by generation.

### 3.2 The two subscribers the gate needs both exist as holes

- **The mutation log's "emitted events" column.** `MutationLog`
  (`mutation.rs:323`) records `counts[intent][origin]`, two rejection counters,
  and a 12-entry ring of non-`System` records carrying
  `mesh_invalidated / scatter_rebuild / water_rebuild`. Post-phase-10 §1A
  records that the specified events column has no referent. This is subscriber
  one, and it is genuinely independent of subscriber two.
- **Generation / invalidation announcing itself.** `regen.rs::tick` currently
  *polls*: it reads `ui_state.pending_graph_reload`, `ui_state.regenerating`,
  `ui_state.regenerate_requested`. The streaming manager reports through
  `StreamingStats`, read each frame. Both are "systems observe each other's
  resources directly", which §6.7 names as the state being replaced.

`MutationOutcome` (`mutation.rs:109`) is already the shape an emission would
carry — it is what each handler returns to describe its side effects. That is a
convenient seam: the door already computes, per command, exactly what an event
would announce.

`FrameStage` ordering (`ecs/schedule.rs`, six stages per design §12) is the
ordering guarantee the bus must state its contract against.

---

## 4. Workstream C — content systems

### 4.1 Blueprint / prefab format

See §0.6 for the two-concepts problem. What exists:

- `PrefabTemplate { anchor: [i32;3], voxels: Vec<PrefabVoxel> }` — voxel data
  with an anchor. **No footprint, no destruction policy, no sway, no variants,
  no placement rules, no protected volume, no version field.**
- `PrefabDef { id, id_name, shape, color, scale }` — procedural silhouette,
  RON-primary with a `load_initial()` fallback locked by a test
  (`prefabs.rs:176`). Three entries.
- `ScatterInstance` (`layers.rs:128`) carries `anchor`, `sub_offset[3]`,
  `rotation_y`, `scale_variant`, `prefab_id`, `flags`, `stable_id` — which is
  design §6's target shape exactly. The storage side is done; the *definition*
  side is not.
- `AnchorDestructionPolicy` — **does not exist in code**, document only.

There is no in-world capture path and no stamping intent on the mutation door.

Format design must carry a version field from the first byte and must not
foreclose **D1** (octant occupancy mask, 0.6.0) or **D5** (string-keyed registry
identity, 0.6.0). Concretely: blueprint voxels must not be serialized as a bare
`ShapeId` discriminant without a version, and material references inside a
blueprint should be **names**, not numeric `MaterialId`s — otherwise every
blueprint is a second instance of the D5 defect. (Note `rock.prefab.json`
currently stores `"material": 2` — a bare numeric id, and also a stale
`"rotation": "None"` field the voxel format no longer has.)

### 4.2 Structures

Nothing. No `NodeKind` for a structure pool, no ZoneGraph structure output, no
priority resolution, no stage-8 call site. Blocked on A and on 4.1.

### 4.3 Rivers

Nothing. `ZoneGraph.rivers` is document-only (design §7 "Generation sources"),
marked there as future work coupled to terrain modification and cross-chunk
infrastructure.

The four multi-distance-smoothing tensions (design §5) are the trap to scope
clear of. Rivers carve beds, i.e. they *lower surfaces*, which is tension 1
("correct multi-distance smoothing is terrain morphology, not step insertion")
and tension 3 ("scatter placement runs before smoothing… props would float").
A river that carves during the density stage — before stage 7 smoothing and
before stage 10 scatter — avoids both, because everything downstream sees the
carved surface as the surface. A river that carves *after* voxelization walks
straight into them. This is a scoping decision worth taking explicitly.

Fluid initialization already has the hook: `composite_fluid` produces a
per-column `ColumnField` of water-surface levels consumed by
`StorageBoundary::materialize_fluid → fluid_gen::apply_biome_ponds`. A river's
channel could produce the same per-column level field, which is a much smaller
change than it first appears — *if* the carving happens upstream in density.

### 4.4 Library graph bodies and the `LibraryRef` density gap

See §0.5. Additional detail:

- `LibraryKernel` (5 variants) is native code in `nodegraph-eval`; the authored
  asset supplies identity and boundary only. `standard_cave_noise` and
  `exposure_layering` both carry doc comments saying "nothing consumes it yet;
  it ships ready for use."
- Design §4 says the file format "extends to hold a graph body instead of a
  kernel reference" as "a compatible addition." `LibraryAsset`
  (`libraries.rs:25`) has a required `kernel: String`. Making it optional and
  adding an optional body is the compatible shape; making `kernel` optional
  without a version field is where a migration problem starts.
- `ChunkTags.library_refs` is **never populated** — `derive_tags`
  (`world_generator.rs:259`) sets `library_refs: SmallVec::new()`
  unconditionally. So design §4's `LibraryGraph[L]` invalidation row cannot fire
  even once libraries are referenced. That is a third piece of the same substep.

### 4.5 Foliage tiers 2/3

Tier 1 (paint) and Tier 2 (discrete scatter) ship and work: `PoissonDistribution
→ SpeciesPicker → ScatterPlace` × 3, in `biome_meadow.detail.json`, rendering
`grass_tuft`, `bush`, `rock`. Submersion filtering is applied at the storage
boundary (`paint_to_detail_layers`, `scatter_to_store`).

Tier 3 (hero foliage, trees as blueprints, sway, LOD billboards) does not exist
as such — `PlaceTree` is a procedural trunk+canopy stamp into *voxels*, not a
scatter instance, and appears in no shipped graph.

`AnchorDestructionPolicy` enforcement on terrain edits: no code. The door's
`EditVoxel`/`EditVoxelBatch` handlers do not consult scatter anchors.

---

## 5. Workstream D — authoring tools: what the graph editor actually has

Read against roadmap §4.2's row, which lists the whole set as T0:

| Capability | State | Evidence |
|---|---|---|
| **Undo / redo** | **ships**, with depth 100 and param-drag coalescing into one entry per interaction | `state.rs:15–230` |
| Multi-select | absent — `selected_node: Option<SnarlNodeId>`, singular | `state.rs:38` |
| Cross-graph copy/paste | absent | — |
| Node search palette | absent; insertion is nested category submenus | `viewer.rs:293–317` |
| Comments / groups | absent | — |
| Collapse-to-library | absent | — |
| Inline node docs | absent; `descriptor()` supplies `display_name` + `category` only | `viewer.rs:147` |
| Diagnostic badges on nodes | **ships** — severity dot from `Graph::validate()` each frame | `viewer.rs:271–280` |
| Type-checked connections | **ships** — incompatible drags are refused, single-input rule enforced | `viewer.rs:337–360` |
| Per-node param UIs | **ships** for 46 of 50 kinds (4 have no params) | `params.rs:42–186` |
| **Insertable kinds** | **44 of 50** — see §0.2 | `viewer.rs:28` |

**Hierarchy / manifest editor** (`ui/hierarchy_editor.rs`, 350 lines) is at T1
and the roadmap is right about that. It has: a slot combo with a dirty marker,
save-active-to-file, create-biome (writes the graph file *and* the manifest
entry), delete-biome (id 0 protected). It does **not** have:

- a slot for **DetailGraph** — `GraphSlot` has no `Detail` variant, so the
  meadow's detail graph is not editable in-engine at all;
- a slot for **LibraryGraph**;
- multiple **Zone** slots (§0.1);
- editing `sea_level`, seed, per-biome `params`, or the `detail` path;
- any way to attach a detail graph to a biome (`add_biome_entry`
  hardcodes `detail: None`, `world_generator.rs:597`).

That last group is the T1→T2 work, and the DetailGraph slot is load-bearing for
the content gate: three biomes with foliage cannot be authored in-engine
without it.

**Hot reload** obeys the settled rule. `regen.rs:142–166` reloads the whole
hierarchy from the manifest on a disk change; `HierarchyEditor` has no `refresh`
and no `consume_dirty`. Any new authoring tool must preserve this: **the
watcher must never push disk state onto a canvas.**

---

## 6. Workstream E — preview tools

**Field probe** (`ui/field_probe.rs`, 434 lines) ships and is good: it evaluates
the live canvas graph on the shared pool, captures the *selected node's* cached
output (`Scalar` or `Terrain`), and renders one Y slice as a colormapped heatmap
with pan/zoom, auto-range, and a texture cache keyed on
`(data_version, y_slice, colormap, range)`. The 0.4.0 extensions asked for —
volumetric view, cross-section scrubbing, A/B against the previous value — build
on this rather than replacing it. A/B needs one addition: the probe keeps only
the newest capture (`data: Option<ProbeData>`), so retaining a previous capture
is a small structural change.

**Column inspector**: absent. The data it must show mostly exists and is
reachable — `ColumnCache` / `IdColumn` (`column.rs`), `BorderAnalysis`
(`border.rs`), `WorldEvaluator::biome_column` (a cheap terrain-free
World+Zone-only evaluation, already used by the climate sim), the per-biome
`LayerEval` density/material, `fluid_levels`, `ChunkFoliage`. What is missing is
an assembly point: `evaluate_chunk` returns a `ChunkEvaluation` that discards the
per-stage intermediates the inspector wants to show *per stage*.

**Biome/zone map preview**: absent, and `biome_column` is exactly the primitive
it needs — a per-column biome id for an arbitrary chunk coordinate without
generating terrain, foliage or fluid. This is the cheapest high-value preview in
the version.

**Live parameter scrubbing**: partially present in effect — param drags mark the
editor dirty each frame, and the field probe re-evaluates on `revision` change.
What is absent is scrubbing that shows the *world* updating, since world regen is
save-triggered by rule.

**Isolated preview scene**, **regeneration diff view**: absent.

---

## 7. Workstream F — validation and standards

### 7.1 What validation exists

`Graph::validate()` (`graph.rs:215`) already checks: edge integrity (nodes and
pins exist), pin type compatibility, at-most-one-edge-per-input, required inputs
connected, acyclicity, and per-`GraphKind` terminal presence (e.g. a DetailGraph
must have `PaintDensity` or `ScatterPlace`). `LibraryGraphRegistry::detect_cycle`
covers library reference cycles. `WorldEvaluator::log_validation`
(`world_eval.rs:552`) runs all of it at generator construction and logs by
severity; the editor surfaces the same findings as per-node badges.

So a validation **CLI** is mostly a packaging and coverage problem, not a
greenfield analysis problem. What is *not* covered today:

- manifest resolution (missing files, duplicate biome ids, ids referenced by
  `ZoneOutput.biome_ids` with no manifest entry — the live `biome_test` case is
  the inverse: an entry no band selects);
- library boundary mismatch at a `LibraryRef` (design §4 says this "surfaces at
  the referring node"; no code does it);
- orphan `GraphOutput`s / unreferenced graphs;
- cross-graph `GraphRef` target resolution against the manifest.

**Placement problem.** `voxulacrum` has no `lib` target, and the manifest types
(`WorldManifest`, `load_hierarchy`, `GraphSlot`) live in the app crate. So a CLI
either rides the main binary as a flag (precedent: `--verify-generation`,
`main.rs:850–856`, already in CI) or the manifest types move down into a shared
crate. Decision Q3.

### 7.2 Asset lint, console, standards

All absent. No registry-collision check, no dangling-reference check, no naming
convention, no in-engine console.

Registries today: materials (RON, `MaterialRegistry`), prefabs (RON,
contiguous-id validated, fallback locked by test), libraries (dir scan,
contiguous-id validated, fallback locked by test). Three registries, three
hand-written loaders, no shared trait — §6.8's "uniform registry trait" is a
0.4.0 item and there are exactly three instances to generalize from, which is
the right number.

Asset standards: nothing written. Graph asset versioning: no version field on
any graph, library, prefab, or manifest file.

---

## 8. Performance — what the new content will move

From `perf-baseline.md` (0.3.0, r=15 supported max):

| | 0.3.0 |
|---|---|
| Frame CPU | 5.5 ms / 16.6 budget |
| `generate` mean | 8.5–9.0 ms, flat across radii |
| `mesh` mean | 3.5–4.9 ms |
| Fill | 177 ms / 111 chunks |
| GPU mesh buffers | 490 MB at r=25 (diagnostic only) |
| Frontier worst case | **never measured** |

Three things in this phase will move `generate`:

1. **A cave library in a density chain** adds a Worley + ridged-fractal 3D
   evaluation per voxel, which is the most expensive node class in the
   vocabulary and the first 3D noise in shipped content (both current biomes are
   2D-heightfield-minus-Y).
2. **The chunk-Y continuation loop** (`world_eval.rs:460`) pointwise-samples
   density above the window for up to `Σ band thickness` cells per column, on
   every column whose surface is at `y == 31`. With a library in the chain each
   of those samples becomes a kernel call. Meadow's cake is 1+3+8 = 12 bands
   deep.
3. **A third and fourth assigned biome** means `composite_terrain` evaluates
   more `LayerEval`s per chunk — it already evaluates one full biome layer per
   *present or fade-neighboring* biome (`world_eval.rs:392–406`).

The frontier measurement is the one to take **early**, not at the close: it is a
carried-forward gate, and taking it before content lands gives a comparison
point for taking it again after.

---

## 9. Drift findings — status against the current tree

| # | Finding | Status |
|---|---|---|
| 1.5 | Mesh vertex format diverges from §10 `FaceVertex` | open; gates D2 at 0.5.0 |
| 1.6 | Greedy meshing unimplemented | open; **D8 due at this version's exit** |
| 1.7 | Per-layer save versioning | open; 0.6.0 |
| 2.4 | `Positions` vs `ScatterPoints` pin naming | open, still unreconciled; the convention in force is "name the mechanism", so the **doc** should change, not the code |
| 3.1 | Per-voxel edits clone the whole chunk | partly closed — `EditVoxelBatch` exists (`mutation.rs:143`) but is `#[allow(dead_code)]` with no live caller |
| 3.3 | Rivers/structures need a cross-chunk stage | **this version's workstream A** |
| 3.4 | Fluid simulation hardcodes water | open; relevant only if rivers introduce a second fluid (they should not) |
| 5.3 | `scatter_seed` omits `chunk.y` | **closed** — `chunk_world_seed` folds the Y base (`context.rs:60`), with a test |
| 5.4 | §11 LOD text presumes a distance gradient | open; revise alongside D8 |
| 5.5 | `PrefabDef` carries no §6 interaction metadata | **this version's blueprint work** |
| 5.6 | Eviction is margin-based, not LRU-with-hysteresis | open, benign |

---

## 10. Where the roadmap and the code disagree

Per the standing instruction. Each needs a correction or an explicit acceptance.

| # | Roadmap says | Reality | Recommendation |
|---|---|---|---|
| R1 | §4.2: graph-editor **undo/redo** is T0 | Ships, with coalescing (`state.rs`) | Mark the undo/redo clause done; keep the rest of the row |
| R2 | §11 content proof: "3+ biomes across **2 zones**" reads as content | Multi-zone is a schema + evaluator change (§0.1) | Add multi-zone support to §11's *deliverables* explicitly, not only to the gate |
| R3 | §11: "three exist as of 0.3.0" | Two are assigned; the third is registered and unreachable | State it as two-plus-one-unreachable so the gate is not read as nearly-met |
| R4 | §4.5 / §11: graph validation CLI | No `lib` target; manifest types are app-side | Record the placement decision (Q3) rather than discovering it at implementation |
| R5 | §11 exit gate: "Event bus… **two independent subscribers**" | Both candidates exist; the second (generation announcing itself) replaces polling in `regen.rs`, which is a behaviour change | Name the two subscribers in the gate so "independent" is checkable |
| R6 | Design §4: manifest names `libraries` | Libraries load by directory scan | Either add `libraries` to the manifest (P2: the manifest is the resolution root) or amend §4. I recommend the former, since `ChunkTags.library_refs` needs stable ids anyway |
| R7 | Design §4: `ZoneGraph[Z]` invalidates "chunks tagged Zone Z or within Z's fade range" | Unreachable; and once reachable, only sound for a zone *body* edit (§0.4) | Split the row by edit kind when zones land |
| R8 | §10 gate 3 is written as five clauses, three shipped | Post-phase-10 §2 asks for the amendment | Carry to the close (already on the phase's housekeeping list) |
| R9 | §4.2 lists "Foliage/detail authoring" at T1 | DetailGraphs have **no editor slot at all** — they are T0 | Correct the row; it changes what T2 costs |

None of these change the theme. All change what "done" means for a specific
gate, which §1 rule 2 requires be recorded rather than quietly dropped.

---

## 11. Is 0.4.0 one phase or two? — decided: one

**Decided 2026-07-31: one phase.** The audit recommended two; the decision was
to combine them. What survives is the *ordering* argument in §11.2, not the
split: each piece of infrastructure still sits immediately before its consumer,
so cross-chunk generation and the blueprint format land next to structures and
rivers rather than at the front. §12 is the merged ordering.

The sizing in §11.1 stands as a recorded risk rather than a recommendation —
see R-9.

### 11.1 The size argument

Sizing the seven workstreams against the tree, at the granularity Phase 10 used
(one revertible transcription package per substep):

| Workstream | Est. substeps |
|---|---|
| A — cross-chunk + job dependencies | 4–5 |
| B — event bus | 2 |
| C — content systems | 11–13 |
| D — authoring tools | 10–12 |
| E — preview tools | 8–9 |
| F — validation & standards | 5–6 |
| G — multi-zone, proof, measurement, close | 9–11 |
| **Total** | **~50–58** |

Phase 10 was 19 substeps and was described as "more than prior phases." This is
roughly three times that, serially transcribed, with a content-proof gate that
depends on nearly all of it. One phase would mean a single revert boundary
stretching across the whole version and an audit written months after its
earliest substeps.

### 11.2 Why not the infrastructure/tools ÷ content/proof seam

The brief names that seam, and it is close to right, but two of this phase's own
rules cut against it in its pure form:

- **"Do not build editors for content nobody has tried to make."** A pure split
  puts every preview and authoring tool in Phase 11 and every content system in
  Phase 12, which is precisely building editors ahead of the content.
- **"Deliberate omissions get a named trigger."** Phase 10 declined to build job
  dependencies because there was one implicit edge and no second to generalize
  from, and named cross-chunk generation as the trigger. Cross-chunk generation's
  only consumers are structures and rivers. Putting the infrastructure in
  Phase 11 and its consumers in Phase 12 recreates, one level up, exactly the
  mistake Phase 10 declined to make — and the same argument applies to the
  blueprint format, whose consumers are structures and tier-3 foliage.

### 11.3 The two halves, kept as an ordering

The split is not taken, but the grouping below is how §12 is arranged:
**each piece of infrastructure sits immediately before its consumer**, and each
authoring tool ships before the content that exercises it.

**Half one — "the authoring loop closes."** Everything required for every
content type that *exists today* to be authored, previewed and validated
in-engine, plus the infrastructure whose consumers are in this half.

- Multi-zone generalization (manifest schema + version field, `GraphSlot`,
  `GraphRefTarget`, `WorldEvaluator`, zone-aware invalidation)
- Editor node catalog completion + hierarchy slots for Detail, Library, Zone(id)
- Hierarchy/manifest editor to T2 (sea level, seed, per-biome params, detail
  attachment, zone management)
- Graph editor ergonomics: multi-select, copy/paste, search palette, comments and
  groups, inline docs, collapse-to-library
- Library graph bodies **and** the `LibraryRef` density gap, closed together
  (§0.5), plus `ChunkTags.library_refs` population
- Event bus + its two subscribers
- Column inspector; biome/zone map preview
- Graph validation CLI + asset lint, green in CI
- Asset standards and graph asset versioning written into the design doc
- Frontier worst-case measurement (taken early, per §8)

*Acceptance:* a 3-biome / 2-zone world — including one below-sea-level biome and
one cave-bearing biome via `StandardCaveNoise` in a density chain — authored
start to finish in-engine, hot-editable, deterministic, seam-free, with zero
hand-edited JSON. That is five of the ten exit gates, closed with evidence.

**Half two — "the new content systems and the proof."**

- Cross-chunk generation infrastructure + declared job dependencies
- Blueprint format, in-world capture, mutation-door stamping, blueprint and
  structure authoring
- Structures (stage 8 becomes real); rivers
- Foliage tiers 2/3 matured; `AnchorDestructionPolicy` enforcement
- Field-probe volumetric/cross-section + A/B; live scrubbing; isolated preview
  scene; regeneration diff view
- Graph diff & change review; in-engine console
- Reference-world completion: structures and a river crossing chunk and biome
  boundaries
- D8 with measurements on the new world; re-measure fill and `generate mean`
- Close: post-phase audit, CHANGELOG, version bump, design-doc revision,
  roadmap §10 gate-3 amendment and §4 maturity table, merge and tag

Each phase's infrastructure has a consumer inside it. Each authoring tool ships
with content that exercises it. Neither half is a pure refactor with nothing to
show.

**The cost of one phase**, accepted knowingly: a single revert boundary spanning
~31 substeps, and a close-of-version audit written long after its earliest
substeps. The mitigation is per-substep discipline rather than a phase boundary
— every substep stays independently revertible, and each substep's outcome is
recorded in this document as it lands rather than reconstructed at the close.
Recorded as R-9.

---

## 12. Substep ordering — merged, one phase

Rationale first, since the ordering is load-bearing in five places.

**S1 precedes everything** because every later acceptance criterion is "author X
in-engine", and today six node kinds cannot be placed on a canvas.
**S2 is taken before any generation cost changes**, so the carried-forward
frontier gate has a before-and-after rather than a single reading.
**Multi-zone (S3–S4) precedes the preview tools**, because the biome/zone map is
the thing that makes a two-zone world authorable and building it against a
single-zone model means building it twice.
**S6 is indivisible** for the reason in §0.5 — the fill path and the pointwise
path are one defect.
**Cross-chunk (S13–S14) and the blueprint format (S15–S17) sit immediately
before their consumers**, per §11.2: structures and rivers are what make a
staged pass and a declared job dependency real, and building either earlier is
designing against an imagined consumer.

The content push is deliberately **two waves**. Wave 1 (S11) proves the
authoring tools on the content types that already exist; wave 2 (S28) proves the
new content systems. Splitting it is what stops "authored in-engine" from being
verified once, late, under deadline.

| # | Substep | Depends on | Why here |
|---|---|---|---|
| **0** | This audit | — | Done |
| **0b** | Decisions recorded; roadmap R2/R3/R9 | 0 | Done |
| **1** | **Editor catalog completion** — the six missing node kinds insertable | 0b | Smallest change, largest gate effect (§0.2) |
| **2** | **Frontier worst-case measurement** on current content | 0b | Baseline before this phase moves `generate` (§8). Release build only |
| **3** | **Manifest schema v1** — `version`, `seed`, `zones` list, `libraries` list; migrate shipped assets | 1 | The schema everything keys on. Version field from the first byte |
| **4** | **Multi-zone through the evaluator** — `GraphSlot::Zone(id)`, `GraphRefTarget::Zone(id)`, `WorldEvaluator.zones`, `WorldOutput` assignment, zone-aware `classify_edit` | 3 | Largest structural change; isolated so a revert is clean. Takes the §0.4 decision |
| **5** | **Hierarchy editor to T2** — Detail and Library slots, zone management, sea level / seed / params / detail attachment | 4 | Detail slots are load-bearing for foliage authoring |
| **6** | **Library bodies + `LibraryRef` in density chains + `library_refs` tagging** | 4 | Indivisible (§0.5). Ships a chunk-Y-seam test on a library-bearing chain |
| **7** | **Event bus + two subscribers** — mutation-log events column; generation/invalidation emission replacing `regen.rs` polling | 4 | Independent of 5/6; a clean revert boundary between two large substeps |
| **8** | **Column inspector** | 4, 6 | Needs per-stage intermediates; wants the library in the chain inspectable |
| **9** | **Biome/zone map preview** | 4 | Built on `biome_column`; the tool that makes S11 tractable |
| **10** | **Graph validation CLI (`--validate-graphs`) + asset lint + CI** | 3, 4, 6 | Covers the failure modes 3/4/6 introduce, before content depends on them |
| **11** | **Content wave 1** — 3+ biomes across 2 zones, incl. below-sea-level and cave-bearing, with detail graphs; authored through S1/S5/S8/S9 | 5, 6, 8, 9, 10 | Acceptance test for every tool above. Anything that must be hand-edited is a missing tool and returns to this list |
| **12** | **Determinism coverage for wave 1**; re-measure `generate mean` and fill | 11 | Guards the content push and prices it |
| **13** | **Cross-chunk generation infrastructure** — design-doc extension, then the staging contract | 12 | After content exists to stamp into; before its consumers |
| **14** | **Declared job dependencies**, with the staged pass as consumer | 13 | The second edge is now real (§0.7) |
| **15** | **Blueprint format** — types, serde, registry, version field, name-keyed materials | 12 | Subsumes `PrefabTemplate` (Q4). Must not foreclose D1/D5 |
| **16** | **In-world capture + mutation-door stamping intent** | 15 | Capture and stamp are the format's two ends |
| **17** | **Blueprint authoring UI** | 16 | Tool before the content that uses it |
| **18** | **Structures** — ZoneGraph structure pools, priority resolution, stage 8 | 14, 17 | The first consumer of both 13/14 and 15/16 |
| **19** | **Generation-order-independence harness** — `--verify-generation` extension + structure tests | 18 | §0.8: the current flag cannot express this |
| **20** | **Rivers** — channel derivation, bed carved in the density stage, fluid initialization | 19 | Carve upstream of stage 7 and stage 10 to stay clear of the four smoothing tensions (§4.3) |
| **21** | **Foliage tiers 2/3 matured** — trees as hero blueprints, sway, `AnchorDestructionPolicy` enforcement | 17 | Second consumer of the blueprint format |
| **22** | **Field probe extensions** — volumetric, cross-section, A/B against previous | 8 | Builds on the shipped probe; A/B needs a retained capture |
| **23** | **Live parameter scrubbing + isolated preview scene** | 22 | — |
| **24** | **Regeneration diff view** | 7 | Reads the invalidation events the bus now emits |
| **25** | **Graph editor ergonomics** — multi-select, copy/paste, search palette, comments and groups, inline docs, collapse-to-library | 1 | Deliberately late: nothing gates on it, and R-7 applies to cross-graph paste |
| **26** | **Graph diff & change review** | 25 | — |
| **27** | **In-engine console** | 7 | Commands ride the bus rather than reaching into resources |
| **28** | **Content wave 2** — structures and a river crossing chunk and biome boundaries, authored in-engine | 18, 20, 21, 17 | Closes the reference-world gate |
| **29** | **Asset standards + graph asset versioning** into `engine-design.md` | 3, 15, 28 | Written after the formats have been used, not before |
| **30** | **Measurements** — D8 (triangle / vertex / draw-call counts on the new world), frontier re-measure vs S2, fill and `generate mean` | 28 | D8 is decided here, not implemented (roadmap §12) |
| **31** | **Close** — `post-phase-11-audit.md`, CHANGELOG, version bump, design-doc revision, roadmap §10 gate-3 and §4 maturity table, remaining §10 amendments, merge and tag | all | §7.4, §7.5 |

**If the phase must be trimmed**, the deferrable substeps are **25, 26, 27, 23,
24** — ergonomics and polish, none of which gates an exit criterion. The ones I
would fight to keep are **S1** (without it the version's spine clause is false
regardless of what else ships), **S10** (the content waves need a green bar to
author against), and **S19** (a cross-chunk gate with no test that can fail is
not a gate).

---

## 13. Risks

**R-1 — Multi-zone touches everything (high).** S3/S4 change a serialized
schema, two public enums, the evaluator's core structure, the editor's selector,
and the invalidation table, and they rewrite every shipped asset. Mitigated by
splitting schema (S3) from evaluator (S4), by the version field making the
migration explicit, and by taking it before any tool is built against the old
shape. This is the substep where a rollback note matters most.

**R-2 — The chunk-Y seam regression (high, specific).** §0.5. If S6 lands
half-done, the cave biome ships with grass strata at every chunk-Y seam and the
cause will look like a material bug, not a sampler gap. Mitigation: S6 ships a
test that puts a `LibraryRef` in a density chain and asserts the continuation
loop actually advances.

**R-3 — Zone invalidation is the Phase-10 defect species, one case over
(medium-high).** The recorded bound says tag matching is backward-looking.
Zones make it live. A rule that is correct for "the zone's body changed" is
wrong for "zone assignment changed", and both arrive at `classify_edit` as
`GraphSlot::Zone(_)` unless the slot enum distinguishes them. Decide the
distinction in S4, not later.

**R-4 — Generation cost moves and nobody notices (medium).** §8. Three
independent multipliers land in this phase. Mitigation: S2 takes the frontier
reading first, S14 retakes it; `generate mean` is recorded at both ends.

**R-5 — "Authored in-engine" is self-reported (medium).** The gate is a process
claim, and nothing in the build enforces it. Mitigation: during S12, every asset
file is produced by a tool action and the git diff for S12 should contain no
hand-typed JSON hunks. If a file must be hand-edited, that is the finding, and
the tool comes first.

**R-6 — Format decisions taken casually become 0.6.0's migration (medium).**
Blueprint (Phase 12), manifest (S3), and library body (S6) formats all outlive
this version. Version field from the first byte; material references by name,
not numeric id (D5); no assumption that the shape vocabulary is four (D1).

**R-7 — Editor ergonomics can quietly reintroduce the hot-reload defect (low,
severe if hit).** Copy/paste across graphs and collapse-to-library both want to
move graph content between slots. The settled rule is that the watcher never
pushes disk state onto a canvas. Cross-graph paste must go canvas→canvas, and
collapse-to-library must write a file and let the normal save-triggered path
reload — never write a file *and* mutate the canvas from the reload.

**R-8 — `--verify-generation` gives false confidence for cross-chunk work
(medium, deferred to Phase 12).** §0.8. Recorded here so Phase 12 budgets the
harness extension rather than assuming the flag already covers it.

---

## 14. Exit-gate status at phase start

| Gate | Status | Note |
|---|---|---|
| Reference world: 3+ biomes, 2 zones, structures, river | ✗ | 2 assigned biomes, 1 zone (not expressible), no structures, no river |
| Every asset authored in-engine; zero hand-edited JSON | ✗ | Unmeetable today: 6 node kinds not insertable; no Detail or Library editor slot |
| Cross-chunk placement passes order-independence tests | ✗ | No cross-chunk placement; harness cannot express the test (§0.8) |
| Blueprint format shipped; structure captured in-world places correctly | ✗ | Two prefab concepts, neither is the format |
| Event bus with two independent subscribers | ✗ | No bus; both subscribers exist as holes |
| Column inspector and biome map preview | ✗ | Neither exists; `biome_column` is the primitive for the second |
| Graph validation CLI green in CI; asset lint clean | ✗ | `Graph::validate()` covers ~half the checks; no CLI, no lint, placement undecided |
| Asset standards written into the design doc | ✗ | Nothing written; no asset carries a version field |
| D8 recorded with measurements on the new world | ✗ | 0.3.0 figures exist (490 MB / 3.4 M quads at r=25) but on the single meadow |
| Generation-frontier worst case vs 33 ms | ✗ | Never measured; carried from 0.3.0 |
| §7.4 checklist | ✗ | Version at 0.3.0 |

---

## 15. Decisions — asked, and answered 2026-07-31

Recorded with their answers. Each question's reasoning is kept because it is
the justification for the shape that was chosen, not just for the choice.

| # | Answer |
|---|---|
| **Q1** | **One phase.** §11, §12 |
| **Q2** | **Migrate.** `zone` → `zones`; `seed` moves onto the manifest and leaves `TerrainGenParams`, so "same seed + graphs + coords" is a property of one file. S3 |
| **Q3** | **(a)** — `--validate-graphs` on the main binary. (b), moving `WorldManifest` into a shared crate, is recorded with the 0.6.0 server-core extraction as its named trigger |
| **Q4** | **Confirmed.** The blueprint subsumes `PrefabTemplate`; `PrefabDef`'s procedural silhouettes become a blueprint-declared render representation or are retired. No third concept |
| **Q5** | **Confirmed**, including converting `regen.rs` from polling `ui_state` to subscribing |
| **Q6** | **R2, R3, R9 now** (they change what this phase's scope is); the remaining six at the close |

**Q1 — One phase or two.** §11 recommends two, both serving 0.4.0, with the seam
at "the authoring loop closes" / "the new content systems and the proof" rather
than at "infrastructure and tools / content." The load-bearing consequence is
that cross-chunk infrastructure and the blueprint format move to the second half,
next to their consumers.

**Q2 — Manifest schema shape.** I propose:

```
{ "version": 1, "seed": <u64?>, "sea_level": 24,
  "world": "...", "zones": [{id, graph}], "biomes": [...],
  "libraries": [{id, path}] }
```

with `zone` (singular) accepted on read and migrated on next write, and the
shipped assets rewritten in S3. Alternative: keep `zone` alongside `zones` and
never migrate. I recommend migrating — saves are wipeable through 0.5.x
(§7.2), so the cheap window is now, and a schema that carries both forever is
the drift P7 exists to prevent. Also: does `seed` move onto the manifest? It is
currently in `TerrainGenParams`, which makes "same seed + graphs + coords" a
property of two files rather than one.

**Q3 — Where the graph validation CLI lives.** (a) A `--validate-graphs` flag on
`voxulacrum-app`, matching `--verify-generation`, zero code movement, but CI
pays a full wgpu-linking build. (b) Move `WorldManifest` + `load_hierarchy` into
a shared crate and add a small binary there, faster in CI and a step toward the
0.6.0 server-core split. I lean (a) for this version and would record (b) with
the server-core extraction as its named trigger — but (b) is defensible now
precisely because the extraction is coming.

**Q4 — `PrefabDef` vs `PrefabTemplate` vs blueprint.** Not needed before S1, but
needed before Phase 12 designs the format, and worth deciding while it is cheap.
My reading: the blueprint subsumes `PrefabTemplate` (voxel data + anchor) and
adds the §6.6 metadata; `PrefabDef`'s procedural silhouettes become a *render*
representation a blueprint may declare, or are retired when trees become real
blueprints. What must not happen is a third concept.

**Q5 — The event bus's two subscribers.** I propose (1) the mutation log's
events column and (2) generation/invalidation emission, with `regen.rs`
converted from polling `ui_state` to subscribing. The second is a behaviour
change, not just an addition — confirm that is wanted, or name a different
second subscriber.

**Q6 — Roadmap amendments.** §10 lists nine. Do you want them as a transcription
package now, or folded into the close alongside the §10 gate-3 amendment already
on the list? I recommend **R2, R3 and R9 now** (they change what this phase's
scope *is*) and the rest at the close.
---

## 16. Substep outcomes

Recorded as each substep lands, per R-9's mitigation: with one phase there is no
mid-version checkpoint, so the record is kept here rather than reconstructed at
the close.

| # | Substep | Outcome |
|---|---|---|
| **0** | Audit | Landed 2026-07-31 |
| **0b** | Decisions + roadmap R2/R3/R9 | Landed. Q1–Q6 answered; §15 |
| **1** | Editor catalog completion | **Landed.** All 50 `NodeKind`s insertable; `NodeCategory::ALL` replaces the stale hand-written menu list. Also closed a defect found while reading: `EditorState` discarded `Graph::kind` and `Graph::boundary` on every canvas round trip, so **every save relabelled the graph `Biome`** — `world.graph.json` and `zone.graph.json` had shipped mislabelled and were repaired. Identity is now restamped through one `canvas_graph()` path used by save, validation and undo alike |
| **2** | Frontier worst-case measurement | **Landed, and it found a budget breach.** Figures in `perf-baseline.md` §0.4.0. Three of four motions sit under half the 33 ms budget; panning at maximum supported zoom peaks at **59.1 ms with 17 of 701 frames over**. `FrameStage::Meshing` owns the worst frame in all four readings (42.4 of 59.1 ms in the breaching one). Attribution below stage granularity is **S2b**, added to the plan |

### Additions to the plan since §12 was written

| # | Substep | Why it was added |
|---|---|---|
| **2b** | **Attribute the mesh stage** — sub-span timing for `streaming_tick_system`, `seam_smoothing_system`, `job_pump_system` and `meshing_tick_system`, captured on the frontier's worst frame | S2 located a 1.8× budget breach to a stage but not to an owner. Roadmap §7.3 requires no recurring hitch above 4 ms *without an attributed cause*, and a stage is a location, not a cause. Doing it now rather than at the close matters because this version adds biomes and zones, and the breaching frames were observed only in a biome-blend-heavy region — so the condition that produces the breach is one S11 and S28 will multiply. The instrumentation is reused at S12 and S30 |

### Standing candidates for the mesh-stage breach

Named so they are closed by measurement rather than by argument, in the manner of
`pre-phase-10-audit.md` §4.8–4.11. **None is a finding yet.**

- **C1 — snapshot extraction and GPU upload** (`meshing_tick_system`).
  `max_mesh_per_frame` defaults to 32 and its own doc comment calls it a
  main-thread budget: each submission copies a 34³ voxel snapshot. Blend-heavy
  terrain is more geometrically varied, hence more exposed faces per chunk,
  hence larger buffers to upload.
- **C2 — chunk insertion** (`streaming_tick_system`): draining the result
  channel and issuing one `InsertLoadedChunk` per chunk, each marking six
  neighbours mesh-dirty, plus per-chunk detail/scatter/water buffer rebuilds.
- **C3 — seam finalization** (`seam_smoothing_system`): scans the resident set
  every frame looking for up to 16 unfinalized chunks with all six face
  neighbours present, doing up to six hash lookups per chunk examined, then
  finalizes up to 16 per frame. The scan is unbounded in the resident set — the
  same species as the `sim`-stage scan recorded in `post-phase-10-audit.md` §4 —
  though it exits early while unfinalized chunks are plentiful, which is
  precisely the frontier case.
- **C4 — the job pump** (`job_pump_system`): expected negligible, included so
  the four sub-spans sum to the stage and the split can be shown to be complete.

### Substep 2 attribution — outcome and stopping rule

| # | Substep | Outcome |
|---|---|---|
| **2b** | Split `FrameStage::Meshing` into its four systems | **Landed.** Split is complete (four spans summed to the stage). Closed **C3 (seam, 0.1 ms)** and **C4 (pump, 1.4 ms)** as causes. Named `stream` at 42.8 of 44.3 ms |
| **2c** | Split `streaming_tick_system` into drain / submit / scan / unload | **Landed, and it disconfirmed 2b's reading.** The same motion produced `stream 0.2 / upload 18.3` and no budget breach at all. The two runs' worst frames are different events |
| **2d** | Per-span max and mean across all frontier frames | Proposed. See below |

**What went wrong with 2b/2c, recorded because it is the same species this
project keeps finding.** The frontier instrument snapshots the split of the
single frame that set the maximum. That answers "how bad does it get" and was
built for that. It was then used to answer "what causes it", which needs a
distribution: `stream` and `upload` are independently bursty, so which one the
peak frame happens to be dominated by varies run to run. **A max-of-one-frame
capture is a sample of size one for an attribution question**, and two
contradictory readings is what that looks like. The instrument was not wrong; the
question asked of it was.

Secondary confounder: the motion is not reproducible. The breach was observed
only in a blend-heavy region and whether a pan reaches one is incidental. Not
worth fixing with a scripted camera path at this stage; worth knowing when
reading any single run.

**Stopping rule for this investigation.** 2d is the last attribution substep. If
per-span distributions across a long run do not name a single owner, the breach
is recorded as a known scaling limit carrying both candidates, deferred to the
post-content re-measure at S30 — where the condition that produces it will be
several times more common and the signal correspondingly stronger — and the
phase moves to S3. Three substeps is already more than a tracked-not-gating
budget (§7.3 gates from 0.5.0) justifies spending mid-phase.

### Candidate status

- ~~**C3 — seam finalization**~~ — **closed**, 0.1–0.5 ms in every reading.
- ~~**C4 — job pump**~~ — **closed**, ≤1.4 ms.
- **C1 — snapshot extraction and GPU upload** (`meshing_tick_system`) — **open**,
  peaked at 18.3 ms in run 2c.
- **C2 — chunk insertion and eviction** (`streaming_tick_system`) — **open**,
  peaked at 42.8 ms in run 2b. Within it, the drain/submit/scan/unload split has
  been instrumented but never yet observed on a frame where `stream` was the
  peak, so the sub-phase is unknown.

**A hypothesis stated and not supported.** Before 2c I predicted `unload`, on the
chain: `FinalizeSeam` writes demotions into the override bucket → nearly every
chunk becomes `persist_dirty` → every eviction is a main-thread zstd + SQLite
write → blend-heavy terrain has more demotions and larger buckets. Run 2c
measured `unload 0.0` — but on a frame where `stream` was 0.2 overall, so it
tested nothing. The hypothesis is neither supported nor refuted and is carried
into 2d. If it is confirmed, the trigger recorded at `jobs.rs:10–15` for moving
chunk I/O off the main thread ("any measurement above ~1 ms") has fired two
versions early, which is a scope decision rather than an implementation one.

### Substep 2d — attributed, investigation closed

| # | Substep | Outcome |
|---|---|---|
| **2d** | Per-span max and mean across all frontier frames | **Landed, and it resolved the attribution.** 2,919 frontier frames. Figures in `perf-baseline.md` §0.4.0 |

**Result: two phenomena, both named.**

- **`upload` — the sustained cost.** Mean 4.29 ms, the largest of any span and
  ~31 % of frontier CPU mean. Snapshot extraction (`max_mesh_per_frame = 32`,
  each a 34³ copy) plus GPU buffer upload. **C1 confirmed as the steady-state
  cost.**
- **`unload` — the spike.** Mean 0.50 ms, peak **103.38 ms**, accounting for
  essentially all of the 118.2 ms worst frame. Main-thread `save_chunk_on_unload`
  (zstd + SQLite) during eviction. **C2 confirmed as the spike mechanism**, and
  the hypothesis stated before 2c — that `FinalizeSeam` marks nearly every chunk
  `persist_dirty`, so every eviction is a write — is supported.

**C1 and C2 both closed as candidates; the remaining named subspans are closed as
causes** (`drain` 0.01/4.78, `submit` 0.05/8.27, `scan` 0.05/2.66, `seam`
0.36/12.40, `pump` 0.50/14.80 — none capable of the observed frame).

**Residual uncertainty, stated rather than resolved.** Within `unload`, the
per-chunk save is not isolated from the eviction itself (hash removal, GPU mesh
buffer release). The save is strongly favoured — it is the only per-chunk real
work and is documented main-thread — but a fix aimed at chunk I/O is correct only
if the save is the cost. Any option that acts on this should confirm it as a side
effect rather than assume it.

**A recorded trigger has fired.** `jobs.rs:10–15` defers moving chunk I/O off the
main thread to 0.6.0 with the trigger *"or any measurement showing writes above
~1 ms."* The measurement is 103 ms. Acting on it at 0.4.0 is a scope decision.

**Method note worth keeping.** The investigation took four substeps because the
first instrument answered a different question than the one asked of it. The
sequence that worked was: condition the sample on the deficit (2) → split the
stage (2b) → split the dominant system (2c) → **replace one-frame capture with
per-span distributions (2d)**. Only the last step could distinguish "spikes
hardest" from "holds the most time", and those turned out to be different spans.

### Substep 2e — mitigated, and the Substep 2 series closed

| # | Substep | Outcome |
|---|---|---|
| **2e** | Bound the unload phase by time (`unload_budget_ms`, default 4.0) | **Landed.** `unload` peak 103.38 → 7.09 ms, `stream` peak 104.08 → 9.16, worst frame 118.2 → 62.6 ms, breaching frames 1.3 % → 0.5 %. Residency trade benign: resident holds one band above wanted and does not grow; memory flat against session peak |

**Series closed.** Five substeps: measured (2), stage split (2b), system split
(2c), per-span distributions (2d), mitigation (2e). That is more than a
tracked-not-gating budget justifies spending mid-phase, and it stops here.

**Why it stops rather than continuing.** The residual 62.6 ms worst frame is a
*different* frame from the one 2e fixed: `mesh 33.2 + render 26.1`, where
`render` had been 4.6 ms, and `upload`'s peak rose rather than fell. Both spans
are quad-volume and buffer-churn costs — snapshot extraction plus GPU buffer
creation on one side, per-chunk draw-call recording across three terrain passes
on the other. `perf-baseline.md` already records those as one root cause and
assigns them to **D8**, which this version decides at its exit. Chasing them with
another mitigation would be implementing D8's answer before taking D8's decision,
which roadmap §12 explicitly reserves for 0.5.0.

**What this buys D8.** Its evidence was previously 490 MB of mesh buffers and
24.25 ms of `render` at r=25 — *outside* the supported range, and therefore easy
to discount. It now also has an inside-the-range figure: at r≈14, 0.5 % of
frontier frames breach the 33 ms budget, in exactly the two spans greedy meshing
reduces. Carried to S30.

### Exit-gate movement

- **"Generation-frontier worst case measured against the 33 ms budget"** —
  measured, attributed, and partially mitigated. The budget is **breached at
  0.5 % of frontier frames at maximum supported zoom**, with the residual
  attributed to D8's root cause rather than left unexplained. §7.3's "no
  recurring hitch above 4 ms without an attributed cause" is satisfied on the
  attribution clause; the breach itself is recorded, not closed.

### Plan revision after Substep 2

**The `seed` move relocates from S3 to S5.** Q2 settled that `seed` belongs on
the manifest rather than on `TerrainGenParams`. Tracing its five read sites
(`main.rs:422` keying the save DB, `field_probe.rs:197`, `regen.rs:144`,
`world_generator.rs:579`, and the params panel at `panels.rs:649`) shows the move
is independent of the zones work — but it removes the only in-engine way to
change the seed, since the params panel is where that lives today. Doing it at S3
would leave the engine unable to change its own world seed until S5 adds manifest
editing. Doing it *at* S5, alongside the hierarchy editor's manifest fields,
means the capability is never absent. Pairs the tool with the content, per §12's
own rule.

**The `libraries` manifest list relocates from S3 to S6.** It has no consumer
until `ChunkTags.library_refs` is populated and `LibraryRef` resolves in a
density chain, both of which are S6. Adding it at S3 would be a schema field
nothing reads.

S3 is therefore: **manifest schema v1 — `version` field and `zones` as a list,
with migration.**

### Substeps 3, 4a — landed

| # | Substep | Outcome |
|---|---|---|
| **3** | Manifest schema v1 | **Landed.** `version` field, `zones` as a list, `WorldManifest::read` as the single parse-and-migrate path replacing five inline parse sites. Pre-v1 files migrate on read and upgrade on next write, both verified by hand. Generation hash unchanged |
| **4a** | Zone identity through the hierarchy | **Landed.** `GraphSlot::Zone(u16)`, one editable slot per zone, `slot_for_graph_file` resolves to the right zone, and `classify_edit` now returns the narrow `InvalidationTarget::Zone` that Phase 10 deferred — the condition its comment named ("zone assignment is stable across a Zone-graph edit") is met, because only World and manifest edits move zone assignment. Generation hash unchanged |

**A second defect of the recorded species, found in 4a by reading.**
`ChunkTags.zone` was a single `ZoneId` taken from `zone_ids.first()`. A chunk
straddling a zone border touches two zones and recorded one, so an edit to the
other would have skipped it — silently, because the chunk still matched a tag it
did carry. Now `zones: SmallVec<[ZoneId; 2]>`, which also satisfies design §4's
"or within Z's fade range" clause structurally rather than by a radius. Cost:
`BLOB_VERSION` 7→8, `VOXEL_FORMAT_VERSION` 3→4, one save wipe (§7.2 permits this
through 0.5.x), and two bytes per chunk blob before compression.

**A third, found by the manual check rather than by reading.**
`load_world_graphs` resolved cross-graph pins with a hardcoded `out[1]`, correct
only while the vec was exactly `[World, Zone, biomes…]`. With two zones, zone 1
sat at index 2 and went unresolved — and an unresolved `GraphRef` presents no
pins, while `snarl_to_graph` silently skips wires whose pins are out of range. So
opening the second zone and saving it would have **deleted** its
`GraphRef → ZoneOutput` edge. Fixed by resolving every entry after World, plus a
load-time warning that fires for any future call site that forgets a resolution
pass. Same category as the `Graph::kind` loss closed in Substep 1: the editor
discarding what it could not represent.

Worth noting for the method: this one was not visible by reading the diff. It
surfaced because the substep's manual check included "add a second zone entry and
open it" — an exercise of the interim shape rather than of the change.

### Deferred, added this substep

| Item | Status | Trigger |
|---|---|---|
| `generate_chunk`'s air-chunk fallback tags with `ChunkTags::default()` — empty zones *and* empty biomes — so a chunk produced by a failed graph eval is matched by neither `Zone(_)` nor `Biome(_)` invalidation, only `AllChunks`. Fixing the graph therefore does not regenerate the chunks it broke | Open, pre-existing for biomes, now symmetric for zones | Whenever graph-eval failure becomes a condition worth recovering from in place rather than by a full regen |
| `GraphRefTarget::Zone` remains a unit variant | Interim | The first BiomeGraph that reads an output from its containing zone. With several zones the target is ambiguous, and resolving it silently to one would be the defect this phase keeps finding |

### A process defect in the manual checks, found at 4b-1

Substep 4b-1 was a pure refactor and its verification asked for an unchanged
generation hash. The hash had changed — but not because of the refactor.
Substep 4a's manual check said *"Select **Zone**, nudge a parameter, **Save**"*,
which wrote `biome_bands: [0.0, 0.26]` into the shipped `zone.graph.json` and was
never reverted. Different bands, different biome assignment, different terrain.

**A manual check that mutates shipped content invalidates every later baseline
comparison**, silently, and in a direction that looks like a code defect. Every
"hash unchanged" requested between 4a and 4b-1 was measured against a moved
baseline.

Two corrections, applied from here:

1. Any manual check that writes an asset ends with an explicit revert step, or
   operates on a copy.
2. The generation hash is recorded in this document as a dated anchor rather than
   relied on from memory, so "unchanged since when" has an answer. `verify.rs`
   deliberately commits no golden hash — a file CI re-baselines reflexively is
   worse than no check — but an anchor a human compares against is a different
   thing from a gate a machine enforces.

Incidental, found in the same pass: five crate `Cargo.toml`s carried a stray
`private = { ignore = true }` above `[package]`. Cargo treats unknown top-level
keys as warnings, so it was inert. Removed; unrelated to this phase.

### Substep 4b — landed

| # | Substep | Outcome |
|---|---|---|
| **4b-1** | Border analysis takes a per-column id source | **Landed.** `analyze_borders` is generic over an id closure; `sample_id` public as the one-graph source. No behaviour change |
| **4b-2** | Per-column zone selection | **Landed.** `WorldEvaluator` holds one `ZoneGraph` per manifest entry; a column's zone id selects which zone graph assigns its biome, in both the bulk path and the border scan. `ChunkEvaluation` exposes resolved `zone_ids` / `biome_ids` instead of two raw caches — with several zone graphs there is no single cache to look a biome up in. `biome_column` and `evaluate_chunk` share one `resolve_columns`, so the climate sim and terrain cannot disagree about a column's biome |
| **4b-3** | Multi-zone selection coverage | **Landed.** Two zones with disagreeing biome tables; asserts each column's biome follows its own zone and that the assignment reaches voxel material. Carries a vacuity guard requiring the test chunk to actually straddle a zone border |

**A content defect exposed by making zone ids load-bearing.** `band_id(k, ids)`
falls back to the band index when `ids` is short, so `world.graph.json`'s
`zone_bands: [0.0]` with `zone_ids: []` had been assigning zones **0 and 1**
across the world all along — `derive_tags` was already recording it. Nothing
consumed it, so it was inert. The moment zone ids selected a zone graph, half the
world resolved to a zone with no graph and fell back to biome 0.

Fixed in content (`zone_ids: [0, 0]`, making the long-standing behaviour
explicit) and guarded in code: `log_validation` now errors at generator
construction for any zone id the World graph can assign that no manifest zone
implements. That check exists because the failure mode is *silent* — different
terrain and nothing else — and per-column detection is not an option.

This is the §0.4 species from the other direction: not a stale tag read
forward, but an inert value becoming load-bearing while the content that was
fine while it was inert is not.

**An authoring gap found in the same pass.** `params.rs` gives `WorldOutput` a
band list and **no zone-id editor**, while `ZoneOutput` gets a per-band biome
picker. So the assignment that just proved load-bearing cannot be authored
in-engine at all. Added to S5.

### Substep 5 — landed

| # | Substep | Outcome |
|---|---|---|
| **5a** | Detail graphs become editable | **Landed.** `GraphSlot::Detail(u16)`; a detail graph gets its own slot, emitted immediately after its biome. Roadmap §4.2's row was corrected to T0 in 0b and this is the T0→T1 step |
| **5b** | `WorldOutput` zone picker | **Landed.** `GraphCatalogs` bundles the zone and biome catalogs (two bare `&[(u16, String)]` arguments would swap silently); `id_combo` serves both. A band can only be assigned an id that has a graph, so the S4b content defect is now unrepresentable rather than merely diagnosed |
| **5c-1** | Manifest scalars editable | **Landed.** `sea_level` and per-biome params in a World-settings form. The editor holds the manifest and is its single writer — `WorldManifest::write` mirrors `read` |
| **5c-1b** | Manifest edits invalidate | **Landed.** See below |
| **5c-2** | New Zone, Attach Detail | **Landed.** Both graph registrations are in-engine; all four structural actions share one guard and one reload-from-disk. Deleting a biome now also removes its detail file, which the old hand-patched path orphaned |
| **5c-3** | Seed onto the manifest | **Landed.** P1's "same seed + graphs + coordinates" is now a property of one file. `--verify-generation` without `--seed` uses the world's real seed instead of 0 |

**A gap the manual check found, not the diff.** Design §4's invalidation table
opens with `Manifest → All loaded chunks`, and nothing implemented it:
`slot_for_graph_file` matched every file the manifest *references* and never the
manifest itself, so saving it logged "ignoring unreferenced graph file" about the
one file everything else is referenced from. Invisible until `sea_level` became
the first manifest field whose change had to regenerate — `create_biome` had been
writing the manifest all along, but the *graph* file it also wrote is what
triggered regeneration. Fixed with `GraphSlot::Manifest`.

**A dependency 5c-2 had to route around.** A newly-inserted `GraphRef` has no
pins until resolved, and `Graph::connect` validates against pin count — so a zone
template cannot be built by wiring one on a canvas. The template resolves the
ref against the World boundary *before* connecting. Live canvas resolution of
`GraphRef` / `LibraryRef` (deferred at Substep 1) is therefore still open; it is
needed for an author to add a cross-graph read by hand rather than get one from a
template.

### Generation hash anchor

| Anchor | Value | Notes |
|---|---|---|
| Through 5c-2 | *(seed 0)* | `--verify-generation` defaulted to seed 0 |
| From 5c-3 | `0x762455f194348d88` | Seed now comes from the manifest (54321). Reproduce with `--verify-generation 64`; `--seed 0` still yields the previous value |
| From S11 | `0x4b7242f2a2c2c6de` | Content wave 1 (three biomes, two zones). Held unchanged through S12-S17 |
| **From 17f** | **`0x12e6a0644184099f`** | `DEFAULT_COUNT` 64 -> 512. Reproduce with `--verify-generation` (no argument) |

The 5c-3 change is expected, not a regression: verifying determinism at a seed
the world is never played at was the weaker check.

The 17f change is a *sample* change, not a generation change: the same world,
hashed over 512 chunks instead of 64. See "The anchor could not see stage 8"
below for why it had to move.

### Deferred, added during Substep 5

| Item | Status | Trigger |
|---|---|---|
| Live canvas resolution of `GraphRef` / `LibraryRef` — a newly-inserted ref node shows no pins and cannot be wired | Open | Needed before an author can add a cross-graph or library read without a template. Wants the editor to know the library registry and sibling boundaries |
| `TerrainGenParams` carries ~18 fields (`base_height`, `hill_*`, `ridge_*`, `cave_*`, `water_level`) that predate the graph system and appear to reach nothing in generation | Open, unverified | An audit pass; if confirmed dead, deletion is a cheap simplification and removes a panel section that implies control it does not have |
| Deleting a *zone* has no in-engine action (New Zone exists; Delete Zone does not) | Open | Not needed to author the reference world; add if a zone ever needs removing without hand-editing |

### Substep 6a — landed, with a design correction

| # | Substep | Outcome |
|---|---|---|
| **6a** | `LibraryRef` evaluates in a density chain | **Landed.** `Evaluator` carries the kernel map; both the fill path and the pointwise path evaluate a `StandardCaveNoise` reference. `DomainWarp`'s displacement is now shared between the two paths rather than duplicated. Found while reading: `WorldEvaluator::with_libraries` existed and was **never called**, so no generator ever had a library registry |
| **6a-fix** | Reference nodes bind at placement | **Landed.** See below |

**A design correction the author raised, and was right about.** Substep 1 made
`LibraryRef` insertable from the catalog; it did not make it *authorable*. An
inserted node carried an editable numeric `library id`, arrived with no pins,
invalidated its graph, and could only acquire pins through a save-and-reload —
which for a biome graph meant broken terrain in between.

This is the same defect S5b fixed for `WorldOutput`: **a number field can name
something that isn't there, and a picker can't.** The correction goes further and
is better: the node menu now lists *libraries and graphs by name*, built from
`GraphCatalogs`, and placing one binds it and resolves its pins in the same
action. An unbound reference is now unreachable from the editor rather than
merely discouraged. `NodeCategory::Graph` separates the two menus.

This retires the "live canvas resolution" item deferred at Substep 1: with
insert-time binding there is no unresolved state to resolve.

**Two bugs of the same species found in the process.** `load_world_graphs` (the
editor's load path) never resolved library pins — only the generator's path did —
so restarting did not help either. That is the third instance this phase of *two
resolution paths, one updated*, after `out[1]` in S4a and the S4b zone selection.

**A transcription-format lesson.** One edit in 6a-fix was given as prose rather
than a BEFORE/AFTER block — "push `entries.push(…)` inside the loop" — and it was
the single line that did not land, leaving the library menu empty. Every paired
block in the same substep transcribed correctly. Prose instructions do not get
transcribed reliably; the format is not ceremony.

### Substep 6b — landed

| # | Substep | Outcome |
|---|---|---|
| **6b** | Chunk-Y seam regression pinned | **Landed.** Three tests: a `LibraryRef` samples pointwise above the chunk window; a library in a density chain leaves the seam cell reading as buried fill rather than surface material; and the chain is deterministic. The second is the recorded hazard's regression test — it fails with `MaterialId(6)` (grass at the seam) if the pointwise path stops traversing the library |

Incidental: a stray editor auto-import
(`use image::codecs::png::CompressionType::Default;`) had shadowed the `Default`
*trait* across `world_eval.rs`'s whole test module, breaking every
`..Default::default()`. Not a transcription error — a quick-fix that offered the
wrong `Default`.

### Substeps 6c / 6d — deferred, with a trigger

**Not built: `ChunkTags.library_refs` population, libraries in the manifest, and
a Library editor slot.**

The reasoning is the one this phase has applied throughout. `library_refs` exists
to make design §4's `LibraryGraph[L]` invalidation row narrow, and that row needs
a *library edit* to react to. Today libraries are kernel-backed: the asset
declares an id, a name, a boundary and a kernel name, and the computation is
native code. Nothing an author edits in-engine changes what a library computes,
and library files are not watched. So the tag would be a field written by
generation, persisted, and read by nothing.

**Trigger: authored (non-kernel) library graph bodies.** Once a library has a
graph body an author edits, editing it must invalidate the chunks that used it,
and the tag has its consumer. The manifest `libraries` list and the editor slot
land in the same substep, since a library you cannot open is a library you cannot
author.

Deferring costs nothing structurally: `library_refs` is already in the blob
format as a count-prefixed list (v8), so populating it later needs no format
change, and adding `libraries` to the manifest is an additive schema change with
the v1 migration path already established.

**What this does not defer:** the density gap itself is closed (6a, 6b), which is
what the content gate's cave-bearing biome needs. Roadmap §11's "authored library
graph bodies" remains a real deliverable, now sequenced after the event bus
rather than before it — the bus has two waiting consumers and an exit gate;
authored bodies block nothing downstream.

### Substep 7 — landed. Exit gate closed.

| # | Substep | Outcome |
|---|---|---|
| **7a** | The bus, and subscriber #1 | **Landed.** `WorldEvent` emitted at the mutation door — one seam that already knows intent, origin and success — into an outbox a pump publishes onto `bevy_ecs`'s `Events<WorldEvent>`. The mutation-and-event log's event counts are the first subscriber, filling the column post-phase-10 recorded as having no referent |
| **7b** | Subscriber #2 | **Landed.** `graph_hot_reload_system` writes `GraphChanged` and stops there; `WorldRegenCoordinator` subscribes and queues. `UiState.pending_graph_reload` is gone |

**Found while wiring it:** nothing called `Events::<T>::update()`. A full Bevy
`App` registers that via `add_event`; this engine owns its schedule, so the two
pre-existing event types (`RegenerateWorld`, `RemeshAll`) were registered but
never aged — and never read or written either. The bus adds the update system.

**Why 7b is the substep that matters.** 7a added a reader. 7b removed a
*relationship*: the watcher used to push onto a queue parked on `UiState` and the
regeneration coordinator used to drain it — two systems reaching through a third
party's state to talk to each other, with regeneration state living on a UI
struct. Now the watcher announces and does not know regeneration exists, and the
queue lives on the coordinator that owns it. That is what §6.7 means by
"subscribers, not call sites".

**Gate: "Event bus carrying mutation and generation events, with at least two
independent subscribers" — met.** The two subscribers read the same stream,
filter differently, and neither knows the other exists; removing either leaves
the other working.

### Exit gates — status at this point

| Gate | Status |
|---|---|
| Reference world: 3+ biomes, 2 zones, structures, river | ✗ — infrastructure done (S3–S6), content is S11 |
| Every asset authored in-engine; zero hand-edited JSON | ~ — authoring paths now exist for graphs, zones, detail graphs, manifest scalars, seed, and library references; unproven until S11 authors the world through them |
| Cross-chunk placement passes order-independence tests | ✗ — not started |
| Blueprint format; a captured structure places through worldgen | ✗ — not started |
| **Event bus with two independent subscribers** | **✓ (S7)** |
| Column inspector and biome map preview | ✗ — S8, S9 |
| Graph validation CLI green in CI; asset lint clean | ✗ — S10 |
| Asset standards in the design doc | ✗ — S29 |
| D8 recorded with measurements | ~ — inside-the-supported-range evidence gathered (S2 series); decision at S30 |
| Generation-frontier worst case measured | **✓ measured, attributed, partially mitigated** (S2 series) |
| §7.4 checklist | ✗ — S31 |

### Substep 8 — column inspector, landed

| # | Substep | Outcome |
|---|---|---|
| **8a** | Column pipeline stages | **Landed.** Climate channels, zone, biome, border distance and neighbour, reported from the same `resolve_columns` / `analyze_borders` generation runs on |
| **8b** | Voxel-domain stages | **Landed.** Own-biome density via the production pointwise sampler, surface, composited material column, pond level. Runs on the worker pool via `pool().install`, blocking — the determinism checker's precedent for an explicit operator action |
| **8c** | Column, not chunk slice | **Landed.** Three corrections from the first manual check |

**Three defects the manual check found, all of a kind — a tool reporting
something true about a narrower thing than its label claimed:**

1. **The inspector only ever looked at chunk Y 0.** The wrapper hardcoded
   `IVec3::new(cx, 0, cz)`, so it reported world Y 0–31 while meadow's surface
   remaps to Y 12–52 — for many columns, a window that did not contain the
   surface. Now it samples density across the whole streamed vertical span
   (pointwise sampling reads above its own chunk window happily) and composites
   only the chunk holding the surface.
2. **Stage 7 answered a question nobody asked.** `composite_fluid` reports the
   biome-authored *pond* level from a `FluidOutput` terminal — which no shipped
   biome has. The ocean is applied later at the storage boundary from
   `sea_level`; poured water is a runtime override. A row labelled "Fluid"
   reporting only the first of three reads as broken when it is empty. Renamed
   to `pond_level`, with ocean derived against sea level and the panel stating
   that the inspector reports *generated* state.
3. **Bare ids.** `zone 0` / `biome 0` is not an answer to "which one is this" —
   the same defect as the numeric library id and the numeric zone band. Names
   are resolved app-side (`ColumnInspection`), keeping `nodegraph-eval` dealing
   in ids and knowing nothing about manifests.

Worth noting the pattern: all three were invisible in review and obvious in use.
The manual check is not a formality on tool substeps — it is the only test a
preview tool has.

### Substep 9 — biome / zone map, landed

| # | Substep | Outcome |
|---|---|---|
| **9** | Biome / zone map preview | **Landed.** A 128² top-down grid of zone and biome ids at arbitrary scale, sampled pointwise through the same `sample_id` path `resolve_columns` and `analyze_borders` use — so the preview cannot disagree with the world it previews, and a test pins that against the column inspector. Click to recentre, Camera to follow, units-per-sample as the zoom, a categorical palette and a named legend |

Nothing here generates a chunk, which is the tool's reason to exist: at the scale
where "where are my biomes" is a real question, generating the answer is not an
option. Both preview tools the exit gate names by name now exist.

### Substep 10a — graph validation CLI, landed

| # | Substep | Outcome |
|---|---|---|
| **10a** | `--validate-graphs` + CI | **Landed.** Manifest resolution, duplicate registrations, graph `kind` against manifest position, per-graph `Graph::validate`, reference targets, and assignment coverage in both directions. Errors fail, warnings report, `--strict` promotes. Wired into CI beside `--verify-generation`. `assignable_ids` is now one public function rather than a third copy of `band_id`'s index-fallback rule |

Every check corresponds to a failure this project has actually hit: a `kind`
disagreeing with its manifest position (shipped for two versions), a band table
assigning an id nothing implements (half the world silently falling back to
biome 0), a registered biome nothing assigns (`biome_test`, still warned about),
and a reference that does not resolve.

**The validator was caught inventing a finding**, which is why it was asked to
fail once. It validated graphs as read from disk, but a `GraphRef` has no pins
until resolved, so every edge into one reported "output pin 0 out of range". The
engine resolves before it evaluates and so must the validator.

### A process defect in the revert instructions

Manual-check revert steps had been saying `git checkout -- <asset>`. **`git
checkout` reverts to HEAD, which is the pre-phase commit** — so a later
substep's cleanup silently undid an earlier substep's asset repair. This is how
`world.graph.json`'s `kind` returned to `Biome` after Substep 1 fixed it, and it
went unnoticed because the accompanying `WorldOutput` state remained
functionally equivalent (one region, zone 0) and the hash held.

Corrected: revert steps now name the value to restore rather than invoking
`git checkout` on files the phase has deliberately changed. Recorded as an open
recommendation that the phase's work be committed before S11 begins writing
content, so `git checkout` means "back to the last good state" rather than
"back to before the phase".

### Deferred lint, with reasons

**Unknown fields in graph assets.** `biome_meadow.graph.json` carried
`"clamp": true` on a `Remap` node — a field `RemapParams` does not have. Serde
ignores unknown fields by default, so it parsed happily and the first editor save
dropped it: authored intent discarded silently, with nothing to report it. The
fix is *not* `deny_unknown_fields`, which would make every future field addition
break older assets; it is a lint that warns, and that needs schema introspection
to walk raw JSON against the known shape. Worth doing deliberately.

### Substep 10b — material reference lint, landed

`NodeKind::material_refs` collects every `MaterialId` a node's parameters name;
the validator checks them against `materials.ron` and reports each missing id
once per asset. The prefab-name check is deferred to blueprints, where prefab
names become real — `PlacePrefab` appears in no shipped graph and
`resolve_prefabs` has no live caller, so a lint for it today would guard nothing.

### Suspected defect, recorded before content can depend on it

**`DensitySubtract` may implement union-with-complement, not difference.**

The node is documented as "CSG difference (`max(a, -b)`)" and implemented that
way in both the fill and pointwise paths. But the sibling combinators establish
the convention: `Union` is `max`, `Intersect` is `min`, and solid is `> 0`. Under
that convention, `A \ B` is `min(a, -b)` — solid where A is solid *and* B is not.
`max(a, -b)` is solid where A is solid *or* B is empty, which carves nothing:
with `a = 5` and a cave at `b = 0.8`, `max(5, -0.8) = 5`.

No shipped graph uses it, so it is untested and unwired — and it is exactly the
node a cave biome would reach for. **Substep 11 deliberately routes around it**,
carving with `Threshold → Multiply → Subtract`, which uses only nodes the shipped
content already exercises. Confirming the semantics (and fixing the node or the
documentation) is its own substep, because changing a density combinator changes
generation and wants a test and a hash comparison of its own.

### Substep 11 — content wave 1, landed

The reference world's first wave, authored in-engine:

| | |
|---|---|
| Zones | 2 — `zone.graph.json` (0), `zone_highlands.graph.json` (1), split by a World-graph climate threshold at 0.1 |
| Biomes | 3 — Meadow (0), Rocky (1), Ocean (2), each with a detail graph |
| Below sea level | **Ocean** — surface remapped to y 4–18 against sea level 24, sand/gravel/limestone bed |
| Cave-bearing | **Rocky** — `World Position → standard_cave_noise → Threshold(0.8) → ×200 → Subtract` carving the terrain density |
| Validation | `--validate-graphs --strict` passes with no warnings — the `biome 2 unassigned` finding is gone because zone 1 assigns it |
| Anchor hash | **`0x4b7242f2a2c2c6de`** |

**What this proves beyond the content itself.** Each piece exercised a mechanism
built earlier in the phase and confirmed it works on real assets rather than in a
fixture: per-column zone selection (S4b-2) at the zone border; the border closure
resolving biome ids through each column's own zone; a library kernel in a density
chain with the pointwise sampler able to follow it (S6a/S6b), verified by the
column inspector showing no unsampleable rows; foliage dropped on submerged
columns by the storage boundary; and the map and inspector answering their
questions on a world that finally has more than one of anything.

**Authoring gap found by using the tools.** Renaming a biome or zone had no
in-engine control — the display name *is* the file name, so `biome_test` became
`biome_ocean` by moving the file and editing the manifest's `graph` field by
hand. That is precisely the hand-edited JSON the gate forbids, and Create and
Delete already exist beside it. Closed in S11b.

### Substep 12 — measured, and a finding from a log line

Figures and the first 0.3.0→0.4.0 comparison in `perf-baseline.md`. Headline:
generation barely moved (+4 % on `generate` mean) while meshing tripled (3.5×) —
three biomes, a second zone and the first 3D noise in shipped content cost almost
nothing next to what caves cost the mesher.

**Seam finalization persists generated content as player-edit overrides.**

Found by the author noticing "Saved 218 modified chunks" in the log after a run
with no player edits. `handle_finalize_seam` writes its demotions into
`voxel_diffs` and sets `persist_dirty`. The code says so plainly — *"worldgen-
finalization entries, not player edits, but the override guard treats 'present in
voxel_diffs' as 'already decided' either way, so sharing the bucket is safe"* —
and it is safe. What was never counted is the cost:

- **9,700 seam finalizations** in one three-minute run, each marking its chunk
  persist-dirty. Autosave then wrote 218–354 chunks per 30-second cycle.
- At the ~0.1 ms/chunk recorded in 0.3.0 that is 22–35 ms, which is exactly the
  observed `post` stage max of **30.21 ms**.
- Design §9's "save files stay small; untouched chunks store no overrides" stops
  holding once nearly every visited chunk carries overrides.
- `jobs.rs:10–15`'s chunk-I/O trigger — *"any measurement showing writes above
  ~1 ms"* — is now armed from a second direction.

**Why it is not simply a bug.** Seam demotions genuinely differ from what
per-chunk `generate_chunk` produces, so recording them as a delta is consistent
with the diff model. And they *are* re-derivable: `world::seam` documents the
pass as idempotent and order-independent, and the player-edit protection comes
from the `voxel_diffs` guard rather than from persistence. So the demotions need
not be persisted at all — the chunk could re-derive them on load once its
neighbours are resident, as it does the first time.

The catch is that `seam_finalized` is persisted precisely to stop that
re-derivation, so dropping the override write means dropping the flag too, which
is a blob-format bump and a save wipe.

**Natural home for the fix is 0.6.0**, where per-layer save versioning, fluid
tombstones, and moving chunk I/O off the main thread are already scheduled — this
belongs to that cluster rather than beside it.

**Decided: record, fix at 0.6.0.** §7.3 budgets are tracked at 0.4.0 and gate
from 0.5.0; saves are wipeable through 0.5.x, so nothing accumulated is lost. The
cost accepted knowingly is a ~30 ms hitch roughly every 30 seconds during
exploration, and a save file that grows with every chunk visited rather than with
every chunk edited.

Carried to 0.6.0 with the format cluster:

| Item | Shape of the fix |
|---|---|
| Seam demotions persisted as overrides | Stop writing them to `voxel_diffs` and stop persisting `seam_finalized`; re-derive on load once neighbours are resident, as the first load already does. Needs an idempotence test and a `BLOB_VERSION` bump |
| Chunk I/O on the main thread | Trigger armed from two directions now (eviction writes, autosave writes) |
| Fluid persisted as a full-field snapshot | Design §9's stated trigger — "when save-file size on ocean-heavy worlds becomes a problem" — is met: an ocean-heavy world exists and fluid is the largest resident CPU layer at 25.6 MB |

### Substep 12b — `sim` attributed, and two suspects closed

| span | mean | max | verdict |
|---|---|---|---|
| `fluid` | 0.02 | 0.12 | **closed** — the unbounded `active.len()` scan recorded in post-phase-10 §4 is not a cost at this radius |
| `params` | 0.01 | 0.17 | **closed** — the `EngineParams` deep-clone is not a cost |
| `climate` | **2.12** | **15.17** | the owner |
| `save` | 0.00 | 0.00 | no autosave landed inside a frontier frame this run; the mechanism is established from the earlier `post 30.21 ms` reading |

Two items carried out of 0.3.0 as `sim` suspects are disproven by measurement
rather than argued away. That is worth as much as finding the owner.

**The owner is not the culprit.** `climate_tick_system` samples biome per cloud
cell through `biome_id_at_world` → `biome_column` → `resolve_columns`, and since
Substep 4b-2 that bulk-evaluates the World graph **and every zone graph** over a
whole 32² chunk — 3,072 column evaluations — to read a single cell. It was
already wasteful at 2,048 before multi-zone; adding a second zone raised it by
half again.

So the measured cost belongs to a shared path this phase made more expensive, not
to the quarantined subsystem that happens to call it most. Cloud hydrology stays
quarantined (roadmap §3.12, "zero engineering before 0.5.0"); the fix is to make
the lookup pointwise, which `sample_id_map` already does.

**Carried to 0.5.0's fix-or-remove decision on cloud hydrology:** its cost scales
with zone count, because it samples biome per cloud cell. Worth knowing when that
decision is taken.

### Substep 12c — pointwise biome lookup, and the series closed

`biome_id_at_world` now samples two graph walks instead of bulk-filling every
zone graph over a chunk to read one cell.

| | before | after |
|---|---|---|
| `climate` mean | 2.12 ms | **1.90 ms** |
| `climate` max | 15.17 ms | **8.06 ms** |

**The prediction was wrong about magnitude.** I expected the mean to fall toward
zero; it fell 10 %. The bulk lookup was roughly half the *tail* and a small
fraction of the sustained cost — the remainder is inside the climate simulation,
which is quarantined (roadmap §3.12) and a non-goal this version. Recorded for
0.5.0's fix-or-remove decision, along with the note that its cost scales with
zone count.

The fix is kept regardless: strictly less work for the same answer, verified by
the map/inspector/pointwise three-way agreement test, and the anchor hash held.

**Not attributed to 12c:** the frontier worst case also fell (77.3 → 40.5 ms,
14/742 → 6/674 frames over) — but that run covered 1.1 M triangles against the
previous 1.8 M, and Substeps 2c/2d established how much these vary between pans.
Claiming it would be exactly the mistake those substeps were spent learning.

**Substep 12 series closed.** Measured (12), attributed (12b), one bounded
mitigation (12c). Two recorded suspects disproven, one owner identified and
correctly routed to a non-goal, one new finding (seam-override persistence)
recorded with its 0.6.0 home and its fix shape.

**Confirmed with a harder number.** After the Substep 12 measurement runs, with
seven player edits total across the session:

```
saves/default/world.vxdb       17.2 MB
saves/default/world.vxdb-wal    7.5 MB
```

~25 MB of save file for a world nobody edited. The resident `overrides` layer
was 6.69 MB at the same moment; the save is larger because it accumulates every
chunk ever *visited*, not only those currently resident.

Worth recording that the instruments briefly appeared to exonerate it: the
frontier table's `save` span read 0.00 and the `post` stage read 0.01. Neither is
evidence — the frontier sample only covers frames with an open streaming deficit,
and the stage table is a one-second aggregate, so an autosave firing on a settled
frame twenty seconds earlier appears in neither. A behaviour that stops being
visible without a cause is a change in measurement, not a fix. The decision to
record and fix at 0.6.0 stands.

### Substep 13 — cross-chunk generation designed. `engine-design.md` v1.10.

Written ahead of building it, per the phase's sequencing rule. Three decisions,
all confirmed:

**Margin-band world-absolute derivation, not a staged pipeline pass.** A feature
carries a world-absolute anchor, a bounded extent, a priority key and a payload;
every chunk that can see it re-derives it identically and stamps only the part
inside its own window. Order independence is a property of the construction, the
way `JitteredGrid`'s seam continuity already is — not a claim defended after the
fact. A staged pass would need participating chunks co-resident and would
reintroduce exactly the order sensitivity P1 forbids.

**Rivers are excluded from that model.** A river modifies the *density field* at
stages 3–4, not stamped voxels at stage 8. Carving before material, walkability
and slab smoothing means every downstream stage sees the carved surface, which
is what keeps rivers clear of §5's multi-distance-smoothing tensions — notably
that scatter anchors to pre-smoothing surfaces, so a channel lowered after
placement would leave props hanging over it. This makes the rivers substep
smaller than the roadmap implies.

**The predicted trigger for declared job dependencies did not arrive.**
`jobs.rs:10–15` and post-phase-10 §2 both named "the staged cross-chunk
generation pass that structures and rivers require". Under this design there is
no such stage: every chunk stays a pure function of `(seed, graphs, coords)` with
nothing to wait on. Building the machinery anyway would be designing against an
imagined consumer — the exact thing the original omission avoided, one version
later. **Trigger revised to the region graph** (0.5.0), whose per-chunk
connectivity summaries joined across borders is a genuine "B consumes A's output"
edge rather than a readiness predicate. Recorded in `engine-design.md` §12 and to
be re-recorded at `jobs.rs` when that file is next touched.

### Plan revision after S13

§12's ordering assumed cross-chunk infrastructure (S13–S14) preceded blueprints
(S15–S17) and structures (S18). With no staged pass to build and the feature
model's payload being a blueprint reference, the dependency runs the other way:

| # | Substep |
|---|---|
| **14** | Blueprint format — types, serde, registry, version field |
| **15** | In-world capture + mutation-door stamping |
| **16** | Blueprint authoring UI |
| **17** | Feature derivation **and** structures, together — the model's only consumer |
| **18** | Generation-order-independence harness + structure tests |
| **19** | Rivers (density-stage, per S13) |

Feature derivation moves into the structures substep rather than preceding it,
for the same reason job dependencies are not being built: an abstraction lands
with its consumer.

### Substep 14 — blueprint format, landed

`voxel-core/src/blueprint.rs`: `Blueprint` (on-disk), `ResolvedBlueprint` (the
form the stamping path consumes), `BlueprintShape`, `DestructionPolicy`,
`BLUEPRINT_VERSION = 1`. Five tests. `assets/blueprints/rock.blueprint.json`
converted from the prefab asset.

Four format decisions, all taken because this is the last moment they are free:

| # | Decision | Reason |
|---|---|---|
| B1 | Materials by **name** (`"granite"`, never `2`) | Applies **D5**'s direction at the only point still free. A registry reorder silently reinterprets every numeric asset; an unknown name is a hard error. |
| B2 | Blueprint-local shape vocabulary, not `ShapeId` | **D1** (octant mask, 0.6.0) reshapes the runtime enum. An asset embedding it changes shape when the runtime does. Evidence the cost is real: `rock.prefab.json` carried a `"rotation"` field `Voxel` dropped versions ago and nothing noticed. |
| B3 | Sparse cell list; footprint **derived** | Two representations of one fact drift apart. |
| B4 | v1 omits sway weights, variant sets, placement rules | Their *shape* is unsettled. With a version field, **adding** a field later is cheap; **changing** one is not. |

`BlueprintShape` covers `{Cube, SlabBottom, SlabTop}`, which is the whole solid
runtime set (`ShapeId` adds only `Empty`) — so the vocabulary is total today and
the translation layer costs nothing until D1.

### Substep 14b — `PrefabTemplate` deleted, landed

`PlacePrefab` → `PlaceBlueprint`, `place_prefabs` → `place_blueprints`,
`resolve_prefabs` → `resolve_blueprints` (now taking a `&MaterialRegistry`),
`EvalError::UnresolvedPrefab` → `UnresolvedBlueprint`. `nodegraph-ir/src/prefab.rs`
and `assets/prefabs/` deleted. Hash unchanged, as expected: no shipped graph
contains the node.

Two facts found while scoping it:

- **`resolve_prefabs` had no caller anywhere in the workspace.** `PlacePrefab`
  has therefore never resolved in the running engine — evaluating one would have
  returned `UnresolvedPrefab`. That is what made the signature change free, and
  it means 14b wires nothing new; the consumer arrives at S16/S17.
- **The variant rename is a wire change** (`NodeKind` serializes by variant
  name). Free only because the node appears in zero shipped assets — recorded
  because that window is now closed.

Naming note: `assets/prefabs/` (deleted) and `assets/prefabs.ron` (kept) were
unrelated concepts sharing a word — the latter is the scatter-instance registry
behind `PrefabId`, which §11's blueprint work does not touch. Q4's "no third
prefab concept" applies to voxel templates; the scatter registry is a fourth
thing that happens to be spelled the same, and 0.6.0's D5 registry work is where
that name should be revisited.

### Substep 15 — capture and mutation-door stamping, landed

`world/blueprint.rs`: `capture(world, name, min, max, registry) -> Blueprint` and
`stamp_batches(blueprint, origin) -> Vec<(chunk, edits)>`, plus
`WorldMutation::StampBlueprint` (intent 10), `WorldEvent::BlueprintStamped`
(index 8), and `world::split_world_voxel`. Four tests.

Five decisions:

| # | Decision | Reason |
|---|---|---|
| C1 | A dedicated `StampBlueprint` intent, not N `EditVoxelBatch` commands | The door's value is that *intent* is legible at the seam — the same reason `FinalizeSeam` and `CommitFluidPlans` exist despite both being expressible as batches. One authored act = one log row, one event, one thing undo must treat atomically at 0.5.0, one message to replicate. |
| C2 | Stamping is **additive**; it never clears | A blueprint carries only solid cells (absence *is* how the format says empty). A rock on a hillside must not punch a box out of the hill. Consequence: capture → stamp reproduces the original only where the destination is air. |
| C3 | Capture **refuses to truncate** | A region reaching outside the resident set errors, naming the missing chunk. Silent truncation is the `load_world_graphs`/`out[1]` failure mode: a wrong answer that looks like an answer. |
| C4 | Capture anchors at the AABB min corner | Stamping at `p` puts the captured min corner at `p`. S16 can move the anchor. |
| C5 | One `split_world_voxel` helper | `interaction.rs` open-coded the `div_euclid`/`rem_euclid` pair and its own comment says `scatter_edit_system` does too. This would have been the third copy. |

The handler delegates to `handle_edit_voxel_batch` per chunk rather than
reimplementing the write, so override bookkeeping, delta→full promotion, persist
marking and border invalidation stay in one place. That finally gives
`MutationOutcome::extend` a live consumer and its `#[allow(dead_code)]` came off
— it had carried a "used as multi-chunk handlers land (Substeps 1b/1c/6)"
comment since Phase 8 without one arriving.

Not user-visible: nothing issues a stamp until S16, so this substep had tests and
no manual check. Said rather than substituting a check that proves nothing.

**Two process notes.** The one test failure was a transcription typo (a cell
authored at x=31 twice instead of 31 and 32), diagnosed by running the suite
rather than reasoning from the panic — the panic said "1 solid, expected 2",
which is equally consistent with a capture defect. Second: the editor
auto-inserted three unused `bevy_ecs` imports into the transcribed files, the
same class as the `image::codecs::png::CompressionType::Default` import that
shadowed the `Default` trait during S6. Worth watching on every transcribed file.

### Substep 16 / 16b — blueprint authoring UI, landed

A `Blueprints` section on the engine panel: arm a tool, then click in the world.
`voxel_core::Blueprint` gained `save` and `list`; `world::blueprint::blueprint_dir`
names the library; `blueprint_authoring_system` services the panel.

| # | Decision | Reason |
|---|---|---|
| E1 | Corners come from the cursor, not typed fields | Six `DragValue`s would be a debug form. The engine has a pick ray. |
| E2 | Capture writes the file immediately | Disk is authoritative, the rule hot reload already runs on. An unsaved in-memory blueprint is a second source of truth with no way to tell which won. |
| E3 | The blueprint name is validated because it becomes a filename | A text field that turns into a path needs a guard; `../../evil` is not a blueprint name. Tested. |
| E4 | The stamp lands one voxel *above* the clicked voxel | Matches `place_blueprints` on the generation side, so a hand-stamped blueprint sits where a scattered one would. |
| E5 | The `MutationOutcome` is discarded, deliberately | A stamp fills only `mesh_invalidated`, and `meshing_tick_system` acts on `mesh_dirty` every frame — what `interaction::edit_voxel` already relies on. Recorded because "discarded" and "forgot" look identical in a diff. |

**The design error, and the correction.** S16 as first written had the panel act
on the currently-picked voxel when a button was clicked. That is unusable:
aiming and clicking are the same mouse, so a button that acts on "wherever the
cursor points" can never be pressed while pointing anywhere useful. The user
caught it before transcribing the manual check.

16b replaced it with an **armed tool**: a button arms, the next in-world click
performs, right-click cancels. Two further decisions fell out:

| # | Decision | Reason |
|---|---|---|
| F1 | Set region is one button consuming two clicks | Box selection is a two-click gesture everywhere else; two arm buttons would make one region take four interactions. |
| F2 | The armed state gets a screen HUD, not only a panel line | While a tool is armed you are looking at the world. A mode with no indicator outside the panel that set it is a mode you forget you are in. |

The gating came free: `picking_system` already sets `pick.anchor = None` while
`egui_wants_pointer`, so the click that arms a tool can never also trigger it.

`interaction::scatter_edit_system` was deleted — left/right click were an
unlabelled, unconfigurable debug binding for scatter place/remove. The
`PlaceScatter` / `RemoveScatter` intents stay: they are door intents with tests,
and S21's foliage authoring wants them behind a real tool rather than a bare
click.

**A rename sweep leaked, found before S16.** The `prefab`→`blueprint`
find/replace from 14b reached `voxulacrum-app/src/prefabs.rs` — the *scatter*
registry, which 14b explicitly excluded — and repointed `prefabs_path()` at
`assets/blueprints.ron`, which does not exist. `load_prefab_registry` never
fails: it logs at error level and falls back to the built-in table. So the engine
ran on the fallback with green tests, because the drift test reads the correct
file through `include_str!` at compile time. Restored from HEAD, which was safe
only because that file had no intentional Phase 11 change in it — verified by
diffing every line before recommending the checkout.

Two things to carry: `assets/prefabs.ron` (scatter registry, behind `PrefabId`)
and `assets/blueprints/` (voxel templates) keep distinct names, because one
character between two unrelated concepts is how this happened. And a rename
sweep gets a `git status` spot-check afterwards.

### Substeps 16c–16e — gizmo, tool ergonomics, rotation. Landed.

**16c** — the capture region is drawn in the world (`DebugLinePass::set_selection`,
a `blueprint_selection_gizmo_system`). Two decisions: the gizmo is a separate
system because `blueprint_authoring_system` has early returns a tail would skip;
and `set_selection` caches its corners and early-outs, because unlike the pick
highlight a selection changes on a click rather than every frame.

**16d** — Shift repeats an armed tool, Esc cancels one, and a set region can be
trimmed one voxel at a time per axis bound.

| # | Decision | Reason |
|---|---|---|
| H1 | Shift is a bound `Held` action, not a raw key read | Reading `held_keys` directly would rebuild the unconfigurable binding the deleted scatter tool was criticized for, one layer down. |
| H2 | Actions named `CancelTool` / `RepeatTool` | The mechanism is "armed tool", not "blueprint stamp". The next armed tool gets both free. |
| H3 | Esc cancels the *arming*, not the selection | `Clear` discards work; a key that silently did would be risky to press. |

The axis trim is not a convenience. A corner can only land on a voxel the pick
ray actually hits, so without it a region can never be trimmed to a shape whose
faces are not all clickable surfaces — which is most shapes.

**16e** — `voxel_core::Yaw`, applied in `stamp_batches` and carried on
`WorldMutation::StampBlueprint`.

Rotation is **Y quarter-turns only, and that is a shape-vocabulary constraint.**
All three `BlueprintShape`s are invariant under Y rotation, so a yaw is a pure
coordinate permutation with no shape remapping. Rotating about X or Z would turn
a `SlabBottom` into a vertical half-cell the vocabulary cannot spell — so
**arbitrary orientation is blocked on D1** (octant-occupancy mask, 0.6.0), where
a rotation becomes a bit permutation. Y rotation is free *today* precisely
because no shape is horizontally asymmetric; adding a stair before D1 breaks
that, and nothing in the compiler will say so.

Rotation lives on the placement, not the template: `stamp_batches` applies the
yaw, so the stored `ResolvedBlueprint` stays the authored one and a rotated stamp
is not a second template. `place_blueprints` on the generation side deliberately
does **not** take a yaw yet — no graph node supplies one, and S17 adds it when
structures need per-instance rotation.

**Two gaps recorded rather than built.**

- `ResolvedBlueprint::extent()` returns the **unrotated** extent. S17's margin
  band needs the rotated one to size correctly. Not added now because nothing
  reads it and an untested sizing helper is worse than none — but S17 must not
  miss it.
- **`InputMap::default()` has no drift test** against `assets/input.ron`, unlike
  `MaterialRegistry::load_initial` and `PrefabRegistry::load_initial`, which both
  do. That gap let a `input.ron`-only edit desynchronize the two silently during
  16d. Added to the deferred-lint list.

One more leaked word from the 14b rename was found and fixed in `input.rs`'s
`load_or_default` doc comment ("materials/blueprints pattern" → "prefabs"). A
workspace-wide grep for the remaining occurrences of "blueprint" outside
blueprint code came back clean.

### Substeps 16f / 16g — blueprint preview, landed

An isometric preview of the selected blueprint at its stamp rotation, painted
with egui shapes rather than rasterized to a texture: a blueprint is a few
hundred quads, so `Shape::convex_polygon` is less code than a `ColorImage`, is
resolution-independent, and needs no upload or cache key. The cached cells are
**unrotated** — rotating redraws without touching disk — and material colours
resolve in the system, which holds the registry, so the panel stays presentation.

16g fixed three things the first version got wrong, each with a distinct cause:

| Symptom | Cause | Fix |
|---|---|---|
| Muddy, doubled voxel edges | Every cell drew all three faces including buried ones, each with a 0.6 px stroke AA-blending against its neighbours | Cull any face whose neighbour is a full cube; drop the stroke entirely |
| Hairline seams between coplanar faces | Each polygon antialiases against the background at its shared edge — culling cannot reach this | Grow each quad 1.5 % about its centre so neighbours overlap |
| Large blueprints lagged the engine | A volume was drawn as a volume | Culling turns it back into a surface; past 4096 cells the preview reports its size instead of rendering |
| Preview orientation disagreed with the world | The preview used a fixed projection while the camera orbits | Compose the stamp yaw with the camera's snapped quarter turn, **negated** |

The negation is the part worth keeping: orbiting the camera one turn left makes a
fixed object appear to turn one turn right. It reads `target_rotation` rather
than `rotation`, so the preview snaps with the key instead of spinning through
the camera's ease. `Yaw` gained `steps` / `from_steps` / `then` for it.

Refusing past a size limit rather than degrading is deliberate: a stutter is not
legible, a message is.

**The blueprint authoring loop is closed** — capture, trim per axis, name, save,
preview, rotate, stamp, repeat — in-engine, previewable, every write through the
mutation door. That is P11 ("authoring is a loop") satisfied for the first
content type in this version.

### Substeps 17a–17c — cross-chunk feature model, built and wired

S13 designed it; 17a built it (`nodegraph-eval/src/feature.rs`), 17b made it
authorable (`NodeKind::PlaceStructure`), 17c wired it into `evaluate_chunk` as
stage 8. The hash held at every step, because no shipped graph declares a
structure yet — which is the property that proved the hook inert.

| # | Decision | Reason |
|---|---|---|
| K1 | The surface and zone arrive through a callback | Keeps `feature.rs` pure and testable against a synthetic surface, and mirrors `analyze_borders`' `id_at`. |
| K2 | The anchor's Y is a **pointwise surface query**, never a neighbour's voxels | This is the whole model. §5's fourth smoothing tension draws exactly this line: a surface sample is a function of world position, so every chunk agrees; a neighbour read is not. |
| K3 | The margin band uses a **rotation-invariant** reach | A quarter turn permutes `(dx, dz)` magnitudes, so `max(\|dx\|, \|dz\|)` holds for all four yaws. **This resolves the `rotated_extent` S16e deferred** — the band never needed a per-yaw extent, it needed one bound covering every yaw. |
| K4 | Protected volumes beat priority | §5: protected cells are ones "later features do not overwrite", and features apply in priority order. Reads backwards until you see it is what lets an authored vault outlive whatever the world puts on top of it. |
| M1 | An unresolved blueprint is a hard error | A source with no template places nothing and says nothing — the `prefabs.ron` fallback shape, where a wrong world looks like a working one. |
| M2 | Sources sorted by content, not slotmap order | Source index is the last tiebreak in the feature ordering. Priority is a full 64-bit hash so collisions are vanishingly rare, which is exactly why resting determinism on them would be a bug nobody reproduces. |
| M3 | `sample_zone_and_biome_at` factored out, not copied | Two implementations of "which zone is this column" is two answers waiting to disagree. The probe's surface scan likewise mirrors `inspect_column`'s. |

**A performance property that came from a stream-ordering decision.** 17a draws
the density roll *before* calling the probe, so a rejected cell never pays for a
column evaluation — and a probe costs a full biome-layer evaluation. At density
0.25 with a 48-voxel cell a chunk typically probes zero or one column. That
ordering was written for the hash contract (a rejected cell must not shift the
stream for later ones) and turns out to be what keeps stage 8 inside the frontier
budget. It must not be reordered.

**Two process defects, both mine, both repeats.**

1. I gave an import as prose rather than a BEFORE/AFTER block, and it did not
   transcribe — the identical defect as 6a-fix. Standing correction adopted:
   **every import is a block, no exceptions.**
2. I assumed `composite_terrain` returned a bare `ChunkBuffer` when it returns an
   `Arc`, without reading the signature. The re-read rule covers files I intend
   to edit; it now covers every signature a package *calls*.

Also corrected: `derive_features` took `Fn` where the caller needed to accumulate
an error. Widened to `FnMut`, which §5 wanted anyway — a memoizing probe holds
state, and memoization is explicitly permitted.

### Substep 17e / 17f — the anchor could not see stage 8

**17e** added the end-to-end seam test: two adjacent chunks are evaluated
separately, and every cell of a structure straddling their boundary is asserted
present in whichever chunk owns its world position, with a guard that fails
loudly if no structure crosses (the sample would otherwise pass vacuously).
`derive_features` only *locates* the crosser; the assertions read the two
independently generated terrains, which is the part under test.

**17f** raised `DEFAULT_COUNT` from 64 to 512, and this is the finding worth
keeping.

After S17d shipped a structure source and structures were visibly generating,
the aggregate hash did not move. The first answer given was arithmetic — the
64-chunk sample is a 128-voxel footprint, cell size 96 at density 0.35 expects
well under one instance — which was correct but was not evidence. The user
pushed, and the experiment settled it: the app and assets were copied to a
scratch directory (`asset_root` prefers a `shaders/` sibling of the exe, so a
portable copy works), the copy's structure density set to `0.0`, and both run.

| sample | structures on | structures off |
|---|---|---|
| 64 (default) | `0x4b7242f2a2c2c6de` | `0x4b7242f2a2c2c6de` — **identical** |
| 128 | `0xdd4f0da73b0000ec` | `0x2d525c7ca47c0096` |
| 512 | `0x12e6a0644184099f` | `0xcc4b8a5f915b90ba` |
| 1024 | `0x86a4a8ed28423332` | `0xa1247eb2d9812e76` |

Generation was correct and stage 8 was in the hashed path. The anchor was blind:
no structure landed in the sampled box, so it was bit-identical with the feature
on and off. **A regression check that cannot see the subsystem it is supposed to
anchor.**

512 covers a 384-voxel footprint (~5-6 instances) and runs in 0.85 s. But the
principle matters more than the number: **raising the count buys probability, not
a guarantee.** A sampled hash is the wrong instrument for sparse content — it
samples a fixed box, and anything on a coarse world grid is seen by luck. Dense
per-voxel generation is what it is good for. Sparse systems need a property
asserted directly, which is what 17a's derivation tests and 17e's seam test are.

**Standing procedure, adopted here:** when a substep ships content that *should*
move the anchor and the anchor does not move, that is the finding — chase it,
don't explain it. The disable-and-compare run above is the procedure, and it took
four minutes. It also means "hash unchanged" in 17c/17d/17e was a weaker claim
than it was presented as: all three were correctly unchanged, but for 17d the
hash moving was offered as the proof content had landed, and its not moving
should have triggered this investigation immediately.

### Substep 18 — generation-order independence, in CI

`verify.rs` already generated every sampled chunk twice with one generator, in
parallel. That proves repeatability *within* a generator and under parallelism;
it cannot see state accumulating **across coordinates** — chunk A generated after
B differing from A generated before it.

Added: a strided subset of the sample regenerated in **reverse order,
sequentially, by a freshly loaded generator**, compared per coordinate. Reverse
catches ordering, sequential removes the parallelism that would mask it, and a
second generator instance catches anything cached in the first. The seed override
is honoured on the reload, or the check would compare two different worlds.

A subset (`ORDER_SAMPLE = 64`, strided so it spans all four chunk-Y layers)
rather than the full 512: accumulated state shows on any coordinate, so one
mismatch fails, and a full sequential sweep would cost several times the parallel
pass to prove the same thing.

This is a standing check rather than a code reading because §5 names the exact
mechanism that would break it: **memoization is an optimisation, never a
correctness mechanism.** Per-cell feature derivation may be cached so many chunks
sharing a cell derive it once, and dropping that cache must change speed and
nothing else. Nothing memoizes today; this is what will notice when something
does.

With 17e (the seam property) and 18 (the order property) both asserted every run,
P1's guarantee for cross-chunk content no longer rests on the aggregate hash
happening to sample a structure.

### Substep 19 — rivers. Design (v1.11), then 19a.

R1-R4 answered as recommended. `engine-design.md` v1.10 -> **v1.11** carries the
design, written before the code per the version's sequencing rule.

| # | Decision | Reason |
|---|---|---|
| R1 | Flow direction from a dedicated **elevation-potential field**, not the generated surface | A structure source's density roll gates its probes, so a chunk typically probes none. A river network has **no gate** — every chunk would pay a column probe per nearby node, every time. A potential sample costs what `SurfaceNoise` costs. The discrepancy is closed by *content*: author the world graph's elevation from the same potential and the two agree by construction. |
| R2 | The channel **floor is the network's**, interpolated from node elevations | Carving into the generated surface reintroduces the pointwise-surface cost R1 exists to avoid. Consequence, stated so it is not discovered later: **a river cannot follow terrain it did not shape.** |
| R3 | The channel authors its own per-column pond level | Stage 9 fills below `sea_level`; a river above it would be a dry ditch. Reuses `FluidOutput` rather than adding machinery. |
| R4 | Branching network, carve + fill, crossing chunk and biome borders. **No** meanders, deltas, waterfalls, erosion, variable flow, or sim coupling | Each is a separate decision, and none is blocked by this model. |

Two properties the construction gives rather than the tests hoping for:
**acyclicity is structural** (every link strictly decreases potential, so a cycle
would need a node lower than itself), and **width derives from potential**
rather than Strahler order, which would need an unbounded upstream traversal.

**Cost is bounded by the resolve/sample split.** Working out one cell's segment
costs nine potential samples; per column that would be ~81. The network resolves
**once per chunk** — a chunk touches a small block of cells — and each column
then tests distance against the few segments near it. That resolution holds no
cross-chunk state, so it does not engage §5's rule about memoization.

**19a landed** with five tests. The seam test initially failed on its own guard —
every comparison passed, but on `None`s, because the chosen `cell_size`/`width`
put no channel across the sampled strip. Rather than guess again, the workspace
was copied to a scratch directory and a parameter sweep run there: three cell
sizes x three widths x eight seeds, **72 configurations, every one reporting the
two chunks agreeing on every sampled column.** The single test asserts continuity
at one configuration; the sweep says that is not an artifact of the one chosen.
24/12 yields 118-256 channel columns on the seam at every seed; 48/12 yields
between 0 and 1, which is what failed.

**Technique worth keeping:** copying the workspace to the scratchpad and
iterating there turns "guess a parameter and ask the user to run it" into a
measurement, at the cost of one build. Same move as the structures-on/off hash
comparison. It is now the default whenever a package's *values* are in question
rather than its shape.

### Substeps 19b / 19c — rivers wired into the density stages

**19b** moved `RiverParams` from `nodegraph-eval` to `nodegraph-ir`, the split
`NoiseParams` already has: a node's parameters are IR (serialized, edited,
diffed), the field they describe is evaluation. 19c needs it because a `NodeKind`
variant cannot carry a type from a crate the IR does not depend on.

**19c** added `NodeKind::River` (Density in, Density out) with both evaluation
paths implemented. Both matter: the bulk fill produces the chunk's field, and the
**pointwise sampler** is what chunk-Y seam continuation and the structure probe
read. A river visible in one and not the other would put structures on terrain
that is about to be carved out from under them.

The network is cached per evaluator (`RefCell<Vec<(NodeId, Arc<RiverNetwork>)>>`)
because resolving costs nine potential samples per cell and `sample_density` is
called once per voxel of a vertical span. Interior mutability because that method
takes `&self`. This is the resolve-once/sample-many split § 5 specifies, at the
evaluator rather than the chunk, which is the same lifetime.

**Open: carved rivers look poor.** Reported after the manual check, and expected
at this point - no tuning pass has happened. The likely causes, in order of
suspicion:

1. **R2's stated consequence, showing up as promised.** The bed sits at a fixed
   elevation range (14-46) derived from the potential, not from the terrain, so
   where terrain runs far above the bed the channel reads as a gorge and where it
   runs below, the river does nothing at all. This is the designed behavior; what
   is untuned is whether the bed range and the world's actual surface heights are
   in any sensible relation. That is content, not code.
2. **Steepest descent over eight neighbors gives angular paths.** Each link is one
   of eight directions, so a "river" is a chain of octilinear segments. Smoothing
   the polyline, or interpolating the path rather than the endpoints, is a
   possible refinement - but it is a change to the model, not a parameter.
3. **The quadratic floor profile with `depth: 4`** may simply be too deep and too
   sharp-walled for the world's scale.

Cause 1 is a content-tuning question and cause 3 is a parameter; both belong in
the tuning substep. Cause 2 would be a design revision and should not be reached
for until 1 and 3 are ruled out - the cheapest explanation first.

---

## Reorientation after the river research (state of play)

Two research documents landed at the S19 pause: `docs/macro-terrain-research.md`
and `docs/river-generation-research.md`, answering `docs/river-research-brief.md`.
Both are the reference; this section records only the decision and what happens
next.

### What the research established

**The brief's open question resolves yes.** `ColumnEvaluator::sample_column(id,
world_x, world_z)` is world-absolute and valid at arbitrary world XZ. Bed and
flow topology can come from one wired input.

**The diagnosis was one level too shallow.** The defect is not that the bed fails
to follow the terrain; it is the **direction of dependence**. Every technique
that produces convincing rivers has the terrain derive from the network. Two
independent fields cannot be reconciled into one. "A river cannot follow terrain
it did not shape" is a specification, not a limitation.

**Three blockers, all graph plumbing, none about terrain design:**

| # | Blocker |
|---|---|
| B1 | The column domain implements four node kinds and no arithmetic, so a control channel cannot be scaled into world-Y units |
| B2 | `GraphRef` errors in the **density** evaluator, so a biome graph containing one fails to evaluate the whole chunk |
| B3 | No `SurfaceField -> Density` path, so a per-column value cannot enter a density chain |

**B2 is the finding worth keeping.** Design § 4 describes cross-graph dataflow as
the mechanism "that makes the hierarchy actually compose". It composes in the
column domain only. No shipped biome graph contains a `GraphRef`, so nothing has
exercised it. Same pattern as the `prefabs.ron` fallback and the anchor that
could not see stage 8: **a claimed capability with no exercise is not a
capability.** S17 and S19 were built directly alongside this and never tripped it.

**The world is also too short.** 128 voxels with `sea_level = 24` gives at most
~72 voxels of fall, and ~36 for the biome covering most of the world. That is an
arithmetic fact, not an aesthetic one: a river needs somewhere to fall.

### The decision

Scope: Phase A is five substeps, Phase B six (including a world-height change and
a chunk-skip bound), Phase C eight - roughly nineteen on top of the twelve already
remaining. Phase B also rewrites all three biome graphs, invalidating the content
proof, the hash anchor and the 0.4.0 perf comparison together.

Agreed:

1. **Phase A lands in 0.4.0 regardless of rivers.** Mostly porting; the edge map
   is worth ~3.5x on every pointwise consumer; B2 is a live architectural defect
   that should not survive a release themed on worldgen completeness. It is
   independently valuable, which is the test.
2. **Phases B and C become 0.5.0**, with macro terrain as that version's
   headline. **Rivers come off 0.4.0's deliverables** as a roadmap amendment.
3. **The S19 river code stays but does not ship enabled.** Step 1 of the
   recommended sequence moves it to the column domain, so it is not wasted - but
   shipped content must not carry a `River` node that produces ravines.

### Immediate next actions, in order

1. **Commit.** Nothing on this branch is committed. Commit message drafted in
   session; regular per-substep commits from here on, at the user's request.
2. **Verify B1 / B2 / B3 against the code directly.** They are load-bearing, the
   cost figures are explicitly estimates, and this phase has twice been wrong by
   trusting an unread signature (`composite_terrain`'s `Arc`, the terminals'
   descriptor pin counts). B2 is the cheapest check and changes the most if wrong.
3. **Phase A step 0a**: precompute the `(node, pin) -> edge` map. Contained,
   benefits every pointwise consumer, unrelated to rivers.

### Carried forward

- Anchor hash `0x12e6a0644184099f` (`--verify-generation`, 512 chunks).
- Remaining pre-existing substeps: 21 (foliage 2/3), 22-24 (preview tools),
  25-27 (editor ergonomics), 28 (content wave 2), 29 (asset standards),
  30 (D8 decision + measurement), 31 (close).
- Roadmap amendments now owed at close: rivers off 0.4.0, macro terrain as the
  0.5.0 headline, plus the six deferred from Q6.
- Deferred lints: `InputMap::default()` has no drift test against
  `assets/input.ron`; `PlaceStructure` outside a ZoneGraph is a runtime error
  rather than a validation error; unknown-field lint for graph assets.

## Verifying B1 / B2 / B3, and measuring step 0a

Read against the code, not the research summary. All three hold; one is worse
than stated, and step 0a's premise does not survive measurement.

### B1 - confirmed

`ColumnEvaluator::fill_node` (column_eval.rs) matches exactly four kinds -
`SurfaceNoise`, `WorldOutput`, `ZoneOutput`, `GraphOutput` - and falls to
`WrongGraphDomain` for everything else. `sample_column` matches the same four.
There is no arithmetic in the column domain.

### B3 - confirmed

`PinType::can_coerce` is literally `matches!((from, to), (Scalar, Density) |
(Curve, Scalar))`. `pin.rs`'s own `new_types_are_strict` test already asserts
`!is_compatible(SurfaceField, Density)`. No per-column value can enter a density
chain today.

### B2 - confirmed, and it is an authoring trap rather than a missing feature

The evaluator side is as described: `Evaluator::evaluate` fills every node in
topological order with no skip, and `fill_node` and `sample_density` both return
`UnresolvedGraphRef` for `GraphRef`. `ColumnEvaluator::evaluate` has the skip
(with the comment explaining that a `GraphRef` has no single cached output); the
density evaluator has no equivalent.

What the research did not say is that the two resolution paths disagree about
which graphs may contain one:

| Path | Resolves `GraphRef` pins on |
|---|---|
| `WorldEvaluator::with_cross_graph_resolved` (generation) | zone graphs only |
| `load_world_graphs` (editor) | every graph after the world - zones, **biomes, details** |

So the editor renders a `climate` pin on a `GraphRef` dropped into a biome graph,
lets it be wired, and saves it. Generation then fails that chunk with a warn.
Nothing catches it in between: `Graph::validate` has no rule about `GraphRef` by
graph kind, and neither does `--validate-graphs`. The comment on
`with_cross_graph_resolved` still says biome-graph imports "arrive with the biome
density rework (Substep 7)", which is where the gap came from.

This is the same shape as the `prefabs.ron` fallback: a capability that reads as
present, fails silently, and has no exercise proving otherwise.

### A second defect, found while reading and confirmed by test

`ColumnEvaluator` resolves a `GraphRef` read by *name*, and gets that name from
`graph_ref_output_name(graphref, pin)`, which indexes the `GraphRef` node's
**output** pins. The bulk path (`input_surface`) passes `from.pin`. The pointwise
path (`sample_input_surface`) passes `pin` - the *destination's input pin index*.

The two agree only when the source output index equals the destination input
index, which is why nothing has caught it: the shipped world graph exposes one
boundary output and everything wires pin 0 to pin 0.

Confirmed rather than reasoned. A world graph exposing `aridity` (output 0) and
`temperature` (output 1), with a zone graph wiring output 1 into `ZoneOutput`,
gives **418 of 1024 columns disagreeing** between bulk and pointwise. Bulk reads
temperature; pointwise reads aridity.

Class matters more than the token: bulk and pointwise disagreeing is the seam
defect, and pointwise is what neighboring chunks use to agree about a shared
column. B1's fix - arithmetic in the column domain - is exactly what produces
multi-input column nodes fed by multi-output `GraphRef`s, so this would have
started firing during Phase A.

### Step 0a - measured, and the premise does not hold

The research estimated the linear edge scan at ~93% of pointwise probe cost and
the `(node, pin) -> edge` map at ~3.5x. Measured on a scratch workspace
(`voxel-core` + `nodegraph-ir` + `nodegraph-eval`, release, criterion, 1024
samples per iteration), against the graphs actually shipped plus a synthetic
chain to isolate scaling:

| bench | edges | scan | indexed | change |
|---|---|---|---|---|
| `sample_column` / world.graph.json | 2 | 64.8 us | 87.6 us | **+35%** |
| `sample_density` / biome_meadow | 12 | 291 us | 323 us | **+11%** |
| `sample_density` / synthetic chain | 9 | 145 us | 194 us | **+33%** |
| `sample_density` / synthetic chain | 33 | 778 us | 695 us | -11% |
| `sample_density` / synthetic chain | 129 | 6.89 ms | 2.82 ms | **-59%** |

The quadratic is real and the index removes it - the 129-edge case is 2.4x
faster. But the crossover is around 25-30 edges, and **no graph in the tree is
within a factor of two of that.** Below it the index loses: nothing beats
scanning two contiguous edges, and the indexed lookup costs a version check plus
two dependent loads.

Two layouts were measured, not one. A `SecondaryMap<NodeId, Vec<Option<PinRef>>>`
was slower still at the large end (129 edges: 3.26 ms) because every lookup chased
a per-node heap allocation. The contiguous form - one `Vec` of slots plus a
per-node `(start, len)` window - is what the 2.82 ms above refers to.

Two hypotheses about where pointwise cost actually goes were tested and rejected:

- **Noise construction.** `sample_density` rebuilds a `FastNoiseLite` per noise
  node per sample. Measured: construct+sample 94.2 us / 1024, sample alone
  93.7 us / 1024. Construction is free; the fractal sample is ~90 ns and is what
  `biome_meadow`'s ~284 ns per sample is made of (six noise evaluations, the
  domain warp's two counted three times through the diamond).
- **The scan share.** At `biome_meadow`'s size the edge scan is a small single-
  digit fraction, not 93%.

One thing the reading did turn up that *is* worth fixing whenever the index
lands: `UpstreamGraphs::surface_sample` constructs a whole `ColumnEvaluator` per
sampled column. Adding index construction to `ColumnEvaluator::new` would put an
allocation on that path, per column, which is how an optimization becomes a
regression.

### Recommendation

Defer step 0a to 0.5.0 and land it against the graphs that need it. The engine's
own rule is that a metric with no deficit behind it is not a measurement; this
measurement found no deficit at current sizes and a large one at sizes that do
not exist yet. Landing it now buys a real regression - small in absolute terms
against an 8.5-9.0 ms generate job, but a regression - in exchange for a win
nothing can observe.

What should land now is the pointwise bench itself. It is the instrument that
will say when the deficit arrives, and it is the durable half of this work.

### Outcome: step 0a landed anyway

Landed at the user's call after seeing the measurement above, as substeps 20b
(the bench) and 20c (the index). Tests pass and the generation hash is unchanged,
which is the property that matters: the index resolves the same edges by a
different route, so any hash movement would have meant the two disagree
somewhere and `agrees_with_a_linear_scan_over_every_pin` had missed it.

The A/B on the development machine, against a saved baseline in one session:

| case | edges | change |
|---|---|---|
| `sample_column/world` | 2 | +26% |
| `sample_density/biome_meadow` | 12 | +11% |
| `sample_column/zone_via_graph_ref` | 3 | +14% |
| `sample_density/chain_9edges` | 9 | -1% |
| `sample_density/chain_33edges` | 33 | -16% |
| `sample_density/chain_129edges` | 129 | **-58%** |

The user's run reported 33-50% improvement on *every* case including the 9-edge
one. That is not attributable to this change - the index saves a scan
proportional to edge count, so a 9-edge graph cannot gain what a 129-edge graph
gains. The baseline had been taken under load: two runs of identical code in that
same session differed by up to 146%, and the post-change intervals tightened from
about +/-10% to +/-1%. Net of a uniform machine factor the two machines agree on
the shape. **The table above is the figure to cite**, not the raw second run.

One supporting change went in with it. `UpstreamGraphs::surface_sample`
constructed a whole `ColumnEvaluator` per sampled column; the upstream evaluator
is now built once at `insert`. Without that, the index would have added an
allocation to a per-column path - the mechanism by which an optimization becomes
a regression.

Two edge scans remain, both in `world_eval.rs` free functions over a `&Graph`
with no evaluator to hang an index on, and both once per chunk rather than per
column. Indexing them would cost a build to save a scan.
