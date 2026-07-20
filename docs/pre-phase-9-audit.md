# Pre-Phase-9 State Audit — Player Character (Networking-Aware, Single-Player)

Planning reference for **Phase 9 (Player Character)**. Companion to
`post-phase-8-audit.md` (close-of-phase state), `engine-design.md` v1.5 (target),
and `networking-architecture-proposal.md` (target-shape constraint).

Substep 0 deliverable: a current-state snapshot of every subsystem Phase 9's
twelve implementation substeps touch, plus the constraints and audit gaps that
should be surfaced *before* code planning rather than discovered mid-substep.
No code is planned here.

All paths are in `crates/voxulacrum-app/` unless noted. Line numbers are
snapshots at branch `mc-revision` (HEAD `757d5b6`) and will drift.

> **Scope note on the worktrees.** `.claude/worktrees/bold-nobel-a54071` and
> `quizzical-davinci-273796` are separate stale branches (`claude/*`, HEAD
> `ac10205`) whose trees still contain `meshing/marching_cubes.rs` — a mesher the
> main line has already replaced with `cube_mesher.rs`. They are **not** part of
> the Phase-9 line; ignore them.

---

## 1. Vertex format (`TerrainVertex`) and all its consumers — Substep 1

### 1.1 The current type

`rendering/pipelines.rs:8–35`. **64-byte, baked-color** interim shape (design
§10 "Interim shape: `TerrainVertex` with baked color"):

```rust
#[repr(C)]
pub struct TerrainVertex {
    pub position: [f32; 3],   // 12
    pub normal: [f32; 3],     // 12  — full float normal, not packed i8
    pub color: [f32; 3],      // 12  — registry color BAKED at mesh time
    pub ao: f32,              //  4  — always 1.0 today (cube mesher has no AO)
    pub material_id: u32,     //  4
    pub cell_flags: u32,      //  4  — greedy-debug bit only; 0 in cube mesher
    pub _pad_vert: [u32; 2],  //  8  — pad to 64
}
```

`layout()` (same file) declares six vertex attributes at `shader_location`
0–5. The target `FaceVertex` (design §10) is ~32 bytes, drops baked `color`,
packs `normal` to `i8`, and adds `face_axis`, `occlusion_class`,
`biome_tint_index`, `variant_index`, `light_level_index`, `enclosure_factor`,
`edge_flag`, `sway_weight`, `ao_factor`. Substep 1 replaces the type, the
layout, and every consumer below.

### 1.2 Every writer, reader, and shader that touches the format

**Writer (the only mesher):**
- `meshing/cube_mesher.rs:79–156` `generate_chunk_mesh` — the sole producer.
  Emits one quad per exposed face; writes `color` from `mat_config.colors[mat]`,
  `ao: 1.0`, `cell_flags: 0`. Face iteration order (`FACES`, lines 13–32) is
  `+X, -X, +Y, -Y, +Z, -Z` — **this is the natural source of `face_axis`** for
  Substep 1 (the loop already knows `face_idx`). `voxel-core` already has a
  `FaceAxis` enum (`coord.rs:106–121`) with exactly these six oriented values.

**GPU upload:**
- `world/mod.rs:112–160` `upload_mesh_result` — `bytemuck`-casts
  `&[TerrainVertex]` into a vertex buffer. Format-size-agnostic (uses
  `bytemuck::cast_slice`), so it needs no logic change, only the type swap.

**Pipelines consuming `TerrainVertex::layout()`** (three):
- `create_terrain_pipeline` (`pipelines.rs:37`, uses `terrain.wgsl`)
- `create_terrain_wireframe_pipeline` (`pipelines.rs:106`, `terrain.wgsl`)
- `create_shadow_pipeline` (`pipelines.rs:175`, `shadow.wgsl`)

**Shaders reading the vertex struct** (`shaders/` at repo root, loaded from disk
at runtime by `pipelines.rs:649–655` `read_shader`, hot-reloadable):
- `terrain.wgsl` — `VertexInput` at lines 50–57 mirrors all six attributes;
  `fs_main` reads `color` directly (line 183) and branches on `debug_mode` for
  material-id / AO / normals / greedy views (lines 151–167). **Tri-tonal axis
  lighting (Substep 8) lands here**, keyed on the new `face_axis` byte.
- `shadow.wgsl` — depth-only; reads `position` (needs the layout, ignores the
  rest). Must stay compatible with the new stride.

**Cache serialization (round-trips the format to disk):**
- `meshing/cache.rs` — `CompactVertex` (lines 28–35) stores `pos_x/y/z`
  (u8 at 2 steps/voxel), `normal_index` (u8), `material_id` (u8). `to_compact`
  / `from_compact` (72–105) convert to and from `TerrainVertex`, re-deriving
  `color` from the `colors` table on load and hardcoding `ao: 1.0`,
  `cell_flags: 0`. `CACHE_VERSION` is currently **12** (`cache.rs:14`).

### 1.3 Cache invalidation — how Substep 1 forces a one-time rebuild

`compute_cache_key` (`cache.rs:~145–160`) hashes **only** the snapshot's packed
voxels + the material `colors` table (SeaHasher). It does **not** include a
graph hash, world seed, mesher-version, or registry hash — design §10's ideal
key. In practice invalidation still works because (a) any graph/material change
alters the baked voxels or colors that feed the key, and (b) `CACHE_VERSION`
gates the format. **For Substep 1, bumping `CACHE_VERSION` (12 → 13) is the
invalidation lever**: `load_cached_mesh` deletes any file whose `version`
mismatches (`cache.rs:189–192`). The `CompactVertex` schema must also grow to
carry whatever new per-vertex bytes survive the round-trip (at minimum
`face_axis`; `enclosure_factor`/`edge_flag`/`sway_weight` are written as their
computed value or 0).

> ⚠ **Audit gap to surface in 1-planning.** The compact-vertex round-trip is
> *lossy by design* (it re-derives color and discards AO). Any new `FaceVertex`
> byte that is **not** a pure function of `(material_id, position, normal)` must
> be added to `CompactVertex` explicitly or it will silently reset to 0 on a
> cache hit. `enclosure_factor` (Substep 9) is the dangerous one — it is a
> function of world topology, not of the vertex alone, so it must be persisted
> in the compact form, not recomputed.

### 1.4 The design-doc divergence this closes

Design §10 lists the capabilities the migration unlocks (palette/time-of-day
shifts without re-meshing, per-vertex biome tint, `enclosure_factor` fog,
edge-flag outlines, quantized light, debug attribute views, sway weight, ~2×
smaller mesh data). None are built; all wait on the format. Substeps 8 (tri-tonal
via `face_axis`) and 9 (`enclosure_factor`) are the first two Phase-9 consumers.

---

## 2. Walkability mask — current code paths — Substep 2

### 2.1 The type already exists (Phase 3), fully transient

`world/walkability.rs`. `WalkabilityMask { bits: Option<Box<[u64; 512]>> }` —
one bit per voxel, `None` when nothing is walkable (the common buried/air case,
O(1)). Public API: `from_storage` (pure compute), `is_walkable(x,y,z)`, `get`,
`is_empty`, `count`. The walkability predicate (`is_walkable_at`, lines 89–102)
matches design §5: solid **Cube or SlabBottom** top face, two clear cells above,
above-chunk treated as air. Determinism test present (`determinism_recompute_matches`).

### 2.2 Its one and only consumer, and where it dies

`world/slab_smoothing.rs:37` — `smooth_slabs` calls
`WalkabilityMask::from_storage(storage)`, uses it to pick cube→slab demotions,
and drops it when the function returns. **That is the entire lifetime.** No
`LoadedChunk` (or `Chunk`) field holds a mask; it is recomputed and discarded
per chunk generation. This is exactly the "transient since Phase 3" state the
Phase-9 prompt describes, and confirms the design's intended second consumer
(player movement) has never arrived.

Call chain today:
`world_generator.rs:195` `generate_chunk` → `slab_smoothing::smooth_slabs`
(internally builds the mask) → mask dropped. `generate_chunk` returns a
`GeneratedChunk { storage, tags, detail_layers, scatter, fluids }` — **no mask
field**.

### 2.3 What Substep 2 must add

- A `walkability: Option<WalkabilityMask>` field. **Recommend `LoadedChunk`**
  (runtime state), not `Chunk` (the serializable data-model type) — the mask is
  derived and must never persist (design §5 "computed once ... reused"; §"stays
  derived"). `LoadedChunk` is defined at `chunk.rs:80–92`; it already holds the
  other derived-runtime fields (`mesh`, `mesh_dirty`, `mesh_seq`).
- Compute-and-store at generation time. **Timing nuance to surface:** the mask
  `smooth_slabs` builds internally is over *pre-smoothing* storage; the resident
  mask must be recomputed from the *final post-smoothing* storage (SlabBottom is
  still walkable, so the sets differ). Cleanest shape is to have `generate_chunk`
  compute the mask after `smooth_slabs` returns and carry it on `GeneratedChunk`,
  rather than reaching back into `smooth_slabs`.
- Dirty + recompute on voxel edits. The mutation API already has the seam: every
  voxel-editing handler (`handle_edit_voxel_batch`, `mod.rs:225`) marks
  `mesh_dirty`; a parallel `walkability_dirty` flag (or reuse of the mesh-dirty
  signal) triggers a recompute system on the `gen_pool` (Phase-2 threading).
- The slab-smoothing pass keeps computing its own pre-smoothing mask (it needs
  the pre-demotion set); only the *resident* mask is the new artifact. Do not
  try to make smoothing consume the resident mask — the ordering is wrong.

> ⚠ **Boundary caveat (inherited, must not regress).** Both `from_storage` and
> `smooth_slabs` treat above/around-chunk as air (isolated-chunk convention;
> `walkability.rs:11–15`). The resident mask therefore has the same approximation
> at the top two voxel layers of interior chunks. Player collision (Substep 5)
> reading the mask must tolerate this — or the mask consumer must fall back to a
> direct cross-chunk voxel query at chunk-Y seams. Flag during Substep 2/5.

---

## 3. Input handling — current shape — Substep 3

### 3.1 A semantic-action precursor already exists

`input.rs`. The engine is **already** past raw-winit-in-systems: winit events are
buffered (`RawInputBuffer`, `RawInputEvent`, lines 344–386) and translated by
`process_input_system` (393–506) through an `InputMap` (bindings) into an
`InputState` (per-action `pressed/just_pressed/just_released/value`). Systems
consume `InputState`, not winit (`camera.rs:80` `IsometricCamera::update` reads
`GameAction::CameraPan*`). `PointerState` (321–337) carries cursor + L/R button
edges for picking.

This is the bones of Substep 3's semantic layer. What it is **not** yet:

### 3.2 Gaps between the precursor and the Substep-3 target

- **Camera-centric vocabulary.** `GameAction` (lines 14–29) is
  `CameraPanForward/Backward/Left/Right`, `CameraRotate*`, `CameraZoom*`,
  `ToggleUI/GraphEditor/FieldProbe`, `PourWater`. There is **no player
  vocabulary** — no `MoveVector(Vec2)`, `Interact`, `PrimaryAction`,
  `SecondaryAction`, `CycleTool`, `CycleTargetLayer`, `ToggleCutaway`. Substep 3
  adds these.
- **Digital movement, not analog screen-relative.** Movement is four discrete
  `Held` booleans, resolved to a world-space vector *inside the camera* by
  compositing `forward_dir`/`right_dir` from `self.rotation` (`camera.rs:81–94`).
  The Substep-3 target is a single screen-relative `MoveVector(Vec2)` whose
  world transform happens where movement is *applied to the player* (Substep 4/5),
  so a controller stick maps trivially later. The camera's existing
  rotation→world-axis math (`camera.rs:81–88`) is the reference for that
  transform.
- **Bindings are hardcoded, not an asset.** `InputMap::default()`
  (`input.rs:198–230`) is the only source; `main.rs:458` inserts
  `InputMap::default()`. `InputMap` already `#[derive(Serialize, Deserialize)]`
  with a `KeyCodeSerde` string form (`input.rs:70–178`), so the RON-asset load
  Substep 3 wants is a loader + path, not a redesign. Matches the existing
  `materials.ron` / `prefabs.ron` pattern (`assets/`).
- **`ActionMode::Continuous` exists** (scroll) but there is no analog-axis
  action carrying a `Vec2`; `ActionState.value` is a scalar. Substep 3 needs an
  action payload that can carry a 2-vector (either a dedicated `MoveVector`
  action read from WASD-composited-to-vector, or an extension of `ActionState`).

### 3.3 The egui-focus gate (do not break it)

`process_input_system` suppresses keyboard actions when `egui_wants_keyboard`
and scroll/pointer when `egui_wants_pointer` (`input.rs:443–465`); picking and
scatter edits also early-return on `egui_wants_pointer`
(`interaction.rs:45,155`). Player movement/interaction systems Substep 3–12 add
must respect the same gate so the editor panel keeps input focus.

---

## 4. Mutation command API surface (Phase 8 result) — Substeps 4, 5, 11, 12

### 4.1 The single door

`world/mutation.rs` + `world/mod.rs:178–209`. `World::execute(MutationCommand)
-> Result<MutationOutcome, MutationError>`:
- `MutationCommand { origin: MutationOrigin, mutation: WorldMutation }`.
- `MutationOrigin::{Authoring, PlayTime}`; constructors `MutationCommand::authoring`
  / `::play` (`mutation.rs:154–168`).
- `EngineMode::{Authoring, Play}` on `World.mode` (`mod.rs:42`); `execute`
  rejects `origin != mode.required_origin()` with
  `MutationError::WrongMode` **before touching the world** (`mod.rs:182–187`).
- `WorldMutation` intents (`mutation.rs:106–145`): `EditVoxel`,
  `EditVoxelBatch`, `RemoveScatter`, `PlaceScatter`, `PourFluidColumn`,
  `MarkFluidDirty`, `InsertLoadedChunk`, `SwapRegeneratedChunks`.
- Each returns a `MutationOutcome { scatter_rebuild, water_rebuild,
  mesh_invalidated }` (`mutation.rs:81–100`) — chunks whose **GPU-side** buffers
  the caller must rebuild (the world never touches wgpu). Handlers own their own
  persist-dirty / mesh-dirty / override-bucket / border-neighbor bookkeeping.

### 4.2 What Phase 9 consumes, and the state each intent is in

- **`Play` mode is defined but never constructed.** `World::generate` sets
  `mode: EngineMode::Authoring` (`mod.rs:92`); no path assigns `Play`. Substep 4
  adds the explicit transition on player spawn (and reverse on editor takeover).
  The rejection contract is already unit-tested both directions
  (`mod.rs` `mutation_tests`, e.g. lines 832/843 exercise `Play`), so the gate
  becomes load-bearing the moment a `PlayTime` site appears.
- **`EditVoxel` / `EditVoxelBatch` are the block-break/place write path** for
  Substeps 11–12. `EditVoxelBatch` clones storage once per batch
  (`handle_edit_voxel_batch`, `mod.rs:225–275`); `EditVoxel` delegates as a
  batch of one. Both currently `#[allow(dead_code)]` with **no live caller** —
  Phase 9 is their first consumer. Break = `EditVoxel { voxel: Voxel::EMPTY }`
  (or a `voxel_removed` variant — see gap below); place = `EditVoxel { voxel }`.
- **`RemoveScatter` / `PlaceScatter` are live** via `scatter_edit_system`
  (`interaction.rs:147–205`) — today issued as `::authoring`. Substep 12's
  "interact" tool re-issues these as `::play` (foliage interaction, design §8).
- **`PourFluidColumn`** is live via the G-key debug (`systems.rs` fluid
  disturbance). Out of Phase-9 scope for player tools (fluid interaction is
  deferred) but exercises the same door.

> ⚠ **Audit gap — break vs. remove semantics.** `ChunkOverrides` distinguishes
> `voxel_diffs` (explicit change) from `voxel_removed` (explicit destruction)
> (`overrides.rs:28–30`), but `WorldMutation::EditVoxel` only writes
> `voxel_diffs` via `ovr.set_voxel` (`mod.rs:242`). Breaking a block to air by
> writing `voxel_diffs[pos] = EMPTY` works for rendering/persistence today but
> does not populate `voxel_removed`. Decide in Substep 11/12 planning whether
> player block-break needs the `voxel_removed` path (it matters for the eventual
> diff-vs-generated model and networking tombstones) or whether `EMPTY` in
> `voxel_diffs` is sufficient for Phase 9. Recommend documenting the choice, not
> silently picking `EMPTY`.

### 4.3 Origin is provisional everywhere

Post-Phase-8 audit §"Mutation seam coherence": every live site declares
`Authoring`. Phase 9's player-edit systems are the first `PlayTime` sites. The
mode resource lives on `World` (an ECS resource via `VoxelWorld`), so the
transition is `world.mode = EngineMode::Play` guarded by explicit intent.

---

## 5. Existing pick / raycast infrastructure — Substep 11

### 5.1 The god-camera DDA (Phase 5)

`interaction.rs`. `picking_system` (34–51) runs every frame in
`FrameStage::Simulation` after `simulation_tick_system`:
- `compute_pick` (54–81): window px → NDC → unproject through the camera VP
  (`sim.camera.view_projection()`) → world ray.
- `raycast_voxel` (84–128): **Amanatides-Woo DDA**, 1 unit = 1 voxel, up to
  `MAX_PICK_STEPS = 4096` steps, returns the first solid voxel as an `IVec3`
  world-voxel anchor.
- `is_solid_world` (131–142): chunk lookup + `chunk.is_solid(local)`.
- Result stored in `PickState { anchor: Option<IVec3> }` (24–28), and the anchor
  highlight box drawn via `debug_lines.set_highlight` (50).

### 5.2 The anchor highlight (design §8 "ground truth")

`rendering/debug_lines.rs` `DebugLinePass::set_highlight` renders a wireframe box
at the picked anchor using the `DebugLineVertex` pipeline
(`debug_lines.rs:158`). This is the exact "anchor highlight box is ground truth"
mechanism the Phase-9 authoritative decision names — Substep 11 keeps it and
feeds it the reach-constrained pick.

### 5.3 What Substep 11 adds on top

The current DDA is unbounded (4096 steps, no reach limit, no Y-bias, picks
through walls). Substep 11 adds the **player-scoped** version:
- A reach volume (small AABB around the player, e.g. 5³, tunable).
- Candidate filtering to that volume + a bias toward the player's Y over distant
  hits.
- **Face-aware placement** — the DDA must return the *face* it entered the solid
  cell through (the last axis it stepped in `raycast_voxel`, lines 111–125, is
  exactly this datum — currently discarded). `voxel-core::FaceAxis` is the type.
  Placement writes the adjacent empty cell on the player's side of that face.
- Keep the god-camera DDA as a debug/admin tool on its own keybinding.

> ⚠ Note: `picking_system` reads `sim.camera` and runs in `Simulation`. Once the
> camera follows the player (Substep 6) and the player sim runs on a fixed tick
> (Substep 4), pick-ray unprojection must use the **interpolated render camera**
> (what the user sees), while reach-volume filtering uses the **current sim**
> player position. Surface the frame-vs-tick source split in Substep 11 planning.

---

## 6. Camera state today — Substep 6

### 6.1 Where it lives and how it updates

`camera.rs` `IsometricCamera`, owned by `SimulationManager.camera`
(`simulation/manager.rs:43`), a `Resource`. `SimulationManager::tick`
(`manager.rs:67–141`) calls `camera.update(dt, input, params)` **every frame**
and produces a `FrameState` (the per-frame render data resource).

Fields (`camera.rs:6–18`): `target` (logical position, driven by WASD),
`smooth_target` (render position), `zoom`, `rotation`, `target_rotation`,
`aspect`, `pan_speed`, `smooth_speed`.

### 6.2 What interpolation/snap already exist

- **Exponential smoothing** — `smooth_target` lerps toward `target` each frame
  with `alpha = 1 - exp(-smooth_speed·dt)` (`camera.rs:113–116`); rotation
  smooths the same way (118–123). This is *frame-rate smoothing of a free
  camera*, **not** interpolation between two fixed sim ticks.
- **Pixel snap exists and is good** — `snap_camera` (`camera.rs:144–201`)
  projects the world origin into texel space, cancels the sub-texel fraction in
  the projection matrix, and returns a `SnappedCamera { view_proj,
  subpixel_offset }`; the upscale pass shifts UV by `subpixel_offset` to keep
  motion smooth. `manager.rs:77–88` gates it on
  `params.render_pipeline.camera_snap_enabled`. **Substep 6 reuses this
  wholesale** — it is exactly the design §11 "pixel-perfect snapping" requirement.

### 6.3 What Substep 6 must change

- The camera must **follow the player at a fixed iso offset** instead of being
  WASD-driven. `camera.update`'s WASD branch (`camera.rs:80–94`) moves out;
  `target` becomes `player_render_pos + offset`.
- Replace exponential frame-smoothing with **sim-tick interpolation**:
  `render_pos = lerp(player.prev_pos, player.curr_pos, alpha)` where `alpha` is
  the fractional progress in the current sim tick (from the Substep-4
  `PlayerClock` accumulator). The player entity must store `prev`/`curr` sim
  states explicitly (Substep 4/6 coordination point).
- Q/E rotation + scroll zoom stay on the camera (they are view controls, not
  player sim). Movement WASD → `MoveVector` → player.

> ⚠ **Coupling to flag.** `camera.rs` imports `GameAction`/`InputState` directly
> (`camera.rs:3`) and streaming centers on `sim.camera.smooth_target`
> (`systems.rs:445`, `streaming_tick_system`). After Substep 6, streaming
> naturally follows the player because the camera follows the player — the
> `ChunkStreamingManager` stays camera-driven and unchanged (matches the phase
> scope: player-as-observer-0 is *implicit* in Phase 9, generalized in Phase 11).
> Do not rework streaming in Phase 9.

---

## 7. Existing debug panels and dev tools — Substep 9 (cutaway) reuse targets

### 7.1 The cross-section cap pass — the cutaway's geometry ancestor

`rendering/cap_pass.rs` (`CapPass`, a `Resource`). Driven by
`params.cross_section` (`CrossSectionParams`): when enabled, the terrain shader
**discards** fragments outside `[clip_min, clip_max]` (`terrain.wgsl:139–146`),
and `CapPass` draws solid "cap" quads (`CapVertex`, lines 10–42) over the exposed
interior so the cross-section reads as a filled cut rather than a hollow shell.
Clip bounds are computed per-frame from the camera in
`compute_clip_bounds` (`manager.rs:144–197`), rotation-aware.

This is the infrastructure the Phase-9 prompt means by "the cross-section cap
pass already exists as a dev tool — reuse its geometry infrastructure." Substep 9
turns manual, param-driven clipping into **runtime room-detection-driven**
cutaway: the flood-fill result decides what to hide, feeding the same clip/cap
machinery (or a variant of it) rather than the manual `x/y/z_offset` sliders.

> ⚠ The current clip is a global axis-aligned box in world space
> (`manager.rs:158–191`), not a "hide walls between camera and player" volume.
> Substep 9's cutaway is a different selection criterion using the same *drawing*
> primitive. Surface in 9b planning whether cutaway reuses the clip-discard path
> (cheap, shader-level) or needs per-chunk geometry hiding.

### 7.2 Debug attribute views (design §11 / §12) already partly built

`terrain.wgsl` `fs_main` supports `debug_mode` ∈ {material-id, AO, normals,
greedy} (lines 5–9, 151–167), selected by `compute_debug_mode`
(`manager.rs:199–210`) from `DebugParams` toggles. Post-`FaceVertex`, this is the
natural home for the `enclosure_factor` / `face_axis` debug views the Phase-9
verification sweep requires ("enclosure_factor visualization via debug view").

### 7.3 Other dev UI (model against, not reuse)

- `ui/field_probe.rs` — `FieldProbe` panel: dispatches graph-node inspection eval
  on the `gen_pool`, polled in `field_probe_system` (`schedule.rs:68`). A model
  for how a Phase-9 debug panel dispatches background work.
- `ui/panels.rs`, `ui/hierarchy_editor.rs`, `ui/colormap.rs` — egui panels
  hosted by `EguiRenderer` (a `NonSend` resource). The minimal HUD (Substep 12)
  can live as another egui panel or as a dedicated screen-space pass; egui is the
  path of least resistance for a verification-grade HUD.

---

## 8. Rendering pipeline shape — Substeps 7, 8, 10

### 8.1 Pass order (the player + shadow + stencil insertion points)

Render graph is a simple ordered list (`rendering/render_graph.rs`; passes run in
`add_pass` order). `render_present_system` (`systems.rs:577`, ordering at
787–792) builds: **shadow → scene → outline → post_process → palette → upscale**.

The **scene node** (`rendering/main_scene_pass.rs`, `MainScenePassNode::record`,
lines 51–170) draws, within one render pass onto `SCENE`+`NORMAL` with `DEPTH`:
`terrain → detail_paint (grass) → scatter (props) → water`, plus the cap pass.

- **Substep 7 (player mesh + drop shadow)** inserts draws **between scatter and
  water** in `MainScenePassNode` (design §"Player rendering ... between the
  terrain/foliage passes and the water pass"). The drop shadow projects onto the
  first solid ground below the player — a separate small draw.
- **Substep 8 (tri-tonal)** is a `terrain.wgsl` `fs_main` change only, keyed on
  `face_axis` from Substep 1. No new pass.
- **Substep 10 (always-visible stencil)** inserts a stencil pass **after
  terrain/foliage, before water** to guarantee the player silhouette through
  occluders.

### 8.2 ⚠ Stencil requires a depth-format change

The depth target is **`Depth32Float`** (`render_targets.rs:74`; terrain pipeline
depth-stencil at `pipelines.rs:89–95` uses `StencilState::default()` = disabled).
`Depth32Float` **has no stencil aspect**. Substep 10's stencil pass needs
`Depth24PlusStencil8` (universally supported) or `Depth32FloatStencil8` (requires
the `DEPTH32FLOAT_STENCIL8` wgpu feature). **This is a cross-cutting change**:
every pipeline that binds the depth attachment (terrain, wireframe, shadow uses
its own depth, scatter, water, cap) must agree on the new format, and
`render_targets.rs` must allocate a stencil-capable depth texture. Surface this
in Substep 10 planning as a format-migration touching all scene pipelines — do
not discover it mid-substep (cf. Phase-5 mesh-cache format lesson).

---

## 9. ECS entity/component usage today — Substep 4 constraint

`bevy_ecs` is currently used **purely as a resource + schedule container**. The
only `Commands` use (`systems.rs:234`) does `commands.insert_resource(frame)` —
a resource, not an entity. There are **no `#[derive(Component)]` types, no
`.spawn()`, no `Query<...>` over entities** anywhere in the app.

Consequence for Substep 4: **the player is the engine's first real ECS entity.**
The component/query/system-param patterns are unestablished, so Substep 4
introduces them from scratch (`Player`, `PlayerInputBuffer` components; systems
that `Query<&mut Player>`). This is *aligned* with the authoritative decision
("the player is an ECS entity, not a special-case object") but means there is no
in-repo precedent to copy — plan the component layout deliberately.

The pure-function decision is well-supported by precedent: `FluidClock`
(`ecs/resources.rs:147–173`) is the accumulator pattern `PlayerClock` mirrors,
and `fluid_sim.rs` demonstrates the read-compute-apply two-phase shape the
`step(state, input, &world) -> state` player sim should follow.

> ⚠ **No-renderer-coupling tripwire (Phase-10 prep).** `LoadedChunk.mesh` is an
> `Option<ChunkMesh>` holding `wgpu::Buffer` (`chunk.rs:23–27, 88`). The player
> entity and player systems must never reference `LoadedChunk.mesh` or any wgpu
> type (authoritative decision; networking prereq 5). Player sim reads voxel
> geometry via `World::get_chunk(...).is_solid(...)` / `ChunkStorage` and the
> resident walkability mask — all renderer-free. Player *rendering* (Substep 7)
> reads the player entity's transform and produces GPU data on the render side;
> the entity never holds the buffer. If Substep 4/7 planning finds the player
> "wants" a `wgpu::Buffer`, stop and surface it.

---

## 10. Deferred items inherited from Phase 8 — confirm still deferred

Per `post-phase-8-audit.md` §"Deferred", all remain deferred **except** the two
Phase-9 activates:

| Item | Phase-9 status |
|---|---|
| Vertex `TerrainVertex`→`FaceVertex` (drift 1.5) | **Substep 1 — activated** |
| Walkability mask residency | **Substep 2 — activated** |
| Fluid diff-with-tombstones (drift 1.3) | **Still deferred** (player-water is out of Phase-9 scope) |
| Regen override preservation (drift 1.1) | **Still deferred** — reclassified as intended §9 authoring behavior; do **not** add preservation |
| Per-layer save versioning (drift 1.7) | Still deferred (pre-release) |
| Greedy meshing (drift 1.6) | Still deferred (draw-time budget trigger) |
| Eval→storage boundary (2.3), pin vocab (2.4) | Opportunistic only |
| Rivers, terrain modification, non-water fluids, upward fluid flow, per-tick deferral budget, cross-chunk corner leak | Still deferred |

Phase-9-relevant note: the `Full`-branch fluid-carry constraint documented in
Phase 8's Substep 2 rides with the deferred 1.3 migration — player block edits
that promote a chunk past `DELTA_THRESHOLD` while it also holds fluid remain
theoretically lossy, but the promotion threshold (8192 edits) is unreachable by
normal Phase-9 play. Keep an eye on it if brush tools get large-radius.

---

## 11. Substep readiness map

| Substep | Existing scaffolding | Net-new work | Biggest risk |
|---|---|---|---|
| 1 FaceVertex | cube_mesher, cache format, `FaceAxis` enum, `CACHE_VERSION` lever | 32-byte type, shader color composition, `CompactVertex` schema, 3 pipelines | Lossy compact round-trip dropping new bytes (§1.3) |
| 2 Walkability residency | `WalkabilityMask` type + tests, mutation-API dirty seam | `LoadedChunk` field, recompute system on gen_pool, post-smoothing timing | Chunk-Y seam approximation (§2.3) |
| 3 Action layer | `GameAction`/`InputMap`/`InputState`/`PointerState`, serde-ready map | player vocabulary, `MoveVector(Vec2)`, RON load | analog-axis payload shape (§3.2) |
| 4 Player sim | `FluidClock` accumulator pattern, fluid two-phase model | first ECS entity, `player_step` pure fn, `Play` mode transition | no ECS-entity precedent (§9) |
| 5 Collision + auto-step | `ChunkStorage`, resident mask (S2), slab intervals (`cube_mesher::shape_y_interval`) | AABB sweep, slab-honest resolve, auto-step | mask seam at chunk-Y (§2.3) |
| 6 Camera | `snap_camera` (reuse), smoothing scaffolding | follow + sim-tick lerp, prev/curr player state | frame-vs-tick source split (§5.3/§6.3) |
| 7 Player render | `MainScenePassNode` insertion point, scatter-pass instancing model | player mesh + drop shadow draw | no-renderer-coupling on the entity (§9) |
| 8 Tri-tonal | `terrain.wgsl` `fs_main`, debug-mode plumbing | axis-keyed modulation on `face_axis` | depends on S1 landing first |
| 9 Cutaway + rooms | cap pass, clip-discard, `enclosure_factor` byte (from S1) | flood-fill, runtime clip drive, enclosure plumbing | clip-reuse vs. geometry-hide decision (§7.1) |
| 10 Stencil | render graph, scene pass | stencil pass + **depth-format migration** | all-pipeline depth format change (§8.2) |
| 11 Targeting | DDA, anchor highlight, `FaceAxis` | reach volume, Y-bias, face-aware place | discarded DDA face datum; frame/tick source (§5.3) |
| 12 HUD + tools | egui panels, mutation API, scatter edit system | HUD, tool state, `PlayTime` wiring | break-vs-remove semantics (§4.2) |

---

## 12. Questions to resolve before / during substep planning

These are the audit gaps that change what code gets written; each is flagged at
its substep above and collected here.

1. **Player block-break: `voxel_diffs[pos]=EMPTY` or a `voxel_removed` path?**
   (§4.2). Affects the eventual diff-vs-generated model and networking tombstones.
   Recommend deciding explicitly in Substep 11/12, documenting the choice.
2. **Stencil depth format** (§8.2). `Depth24PlusStencil8` (universal) vs.
   `Depth32FloatStencil8` (feature-gated, keeps 32-bit depth precision). Decide in
   Substep 10; it touches every scene pipeline, so ideally confirm the target
   format's availability on the dev GPU early.
3. **Sim rate: 30 Hz or 60 Hz** (§4, authoritative decision leaves it to Substep 4
   "based on movement feel"). Pick during Substep 4; the `PlayerClock` shape is
   identical either way.
4. **Player mesh: iso sprite vs. low-poly 3D** (Substep 7, agent's call). Sprite
   is simpler and z-sorts against the depth buffer (design §11 "player
   z-sorting"); 3D fits the voxel aesthetic. Decide in Substep 7.
5. **Cutaway mechanism: shader clip-discard vs. per-chunk geometry hide** (§7.1).
   Decide in Substep 9b.
6. **Walkability at chunk-Y seams** (§2.3): tolerate the isolated-chunk
   approximation, or have collision fall back to a cross-chunk voxel query at
   seams? Decide in Substep 2/5.

None of these block starting Substep 1 (FaceVertex), which has no dependency on
them. They surface in the substeps noted.

---

*End of pre-Phase-9 audit. Substep 1 (`TerrainVertex` → `FaceVertex`) is next;
re-read `cube_mesher.rs`, `pipelines.rs`, `cache.rs`, and `terrain.wgsl` at
planning time for current line numbers.*
