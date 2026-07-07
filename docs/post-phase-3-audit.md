# Post-Phase-3 State Audit

**Status:** Reference snapshot — close of Phase 3
**Date:** 2026-06-17
**Scope:** Read-only inventory of the engine as it stands at the end of Phase 3,
taken after all eight Phase 3 substeps landed (coordinate/shape primitives, the
typed sidecar layers, the data-model `Chunk` / `LoadedChunk` split, `ChunkTags`,
the walkability mask, slab smoothing + slab rendering, the v5 multi-layer save
format, and the dead-code verification sweep). It is the counterpart to
`post-phase-2-audit.md` / `pre-phase-3-audit.md` and is the intended
starting-state reference for whatever Phase 4 turns out to be.

This is a *factual* audit. It records what each Phase 3 substep delivered and the
current shape of the affected subsystems. Items still open or deliberately
deferred are gathered under **§9 (Carried forward)** so a later phase can pick
them up — but nothing here proposes a change.

---

## 0. What Phase 3 changed (one-paragraph delta vs. Phase 2)

The chunk model was rebuilt around the design-doc architecture. New coordinate
and identity primitives landed in `voxel-core` (`ChunkCoord`, `LocalPos`,
`FaceAxis`). The runtime `Chunk` was **split into two types**: a pure data-model
`Chunk` (voxels + typed sidecar layers + tags + overrides) and a runtime
`LoadedChunk` wrapper that holds it alongside mesh/persistence bookkeeping. The
former `Vec<VoxelEdit>` edit list was **replaced** by the canonical
`ChunkOverrides` diff set; `VoxelEdit`, `apply_edits_to_storage`, and
`deduplicate_edits` are gone. The empty `PopulatedChunk` lighting/simulation/flora
Option tiers were removed and re-homed as **typed sidecar layers** on the data
`Chunk` (`DetailLayers`, `ScatterStore`, `FluidLayer`, `DecalLayer`, `LightData`),
all shipping empty per the Empty-Scaffolding Principle. Two generation passes were
added inside the shared `generate_chunk_storage` path: the **walkability mask**
and **slab smoothing** (the latter demoting walkable surface cubes to
`SlabBottom`), and the cube mesher gained **slab face rendering** so the result is
visible. `ChunkTags` now tags every chunk `ZoneId(0)` + `[BiomeId(0)]`. The save
format bumped **v4 → v5** to a sectioned, deterministic multi-layer blob (voxels +
overrides + tags) with a `VOXEL_FORMAT_VERSION` wipe. A closing dead-code sweep
removed the now-justified `#![allow(dead_code)]` blocks. During verification a
**Substep-3 streaming load-queue inversion** was found and fixed (§8).

---

## 1. Crate layout

The workspace (`Cargo.toml`) is unchanged from Phase 2 — still **6 members**.
Phase 3 added no crates and removed none; all new code lives in `voxel-core`
(primitives) and `voxulacrum-app` (chunk model, passes, persistence).

| Crate | Phase 3 change |
|---|---|
| `voxel-core` | Added `ChunkCoord`, `LocalPos`, `FaceAxis` (coord.rs); `ShapeId` slab vocabulary unchanged. |
| `nodegraph-ir` / `nodegraph-eval` / `nodegraph-hotreload` / `nodegraph-editor` | No change. |
| `voxulacrum-app` | The data-model `Chunk` / `LoadedChunk` split, the typed layer modules, `ChunkTags`, `ChunkOverrides`, the walkability + slab-smoothing passes, slab rendering, and the v5 save format. |

Dependency direction is still clean and acyclic: `voxel-core` is the root;
`nodegraph-*` build on it; `voxulacrum-app` sits at the top.

---

## 2. New coordinate & identity primitives (`voxel-core`)

The design-doc types that §1.3 of the pre-Phase-3 audit flagged as missing now
exist:

- `ChunkCoord` — the chunk key (was raw `IVec3`). The data `Chunk` keys on it
  (`coord: ChunkCoord`); `IVec3` is still the streaming/runtime currency, with
  `From<IVec3>`/`Into<IVec3>` bridges at the boundary.
- `LocalPos` — intra-chunk coordinate (`x/y/z: u8`), `to_index()` / `from_index()`
  both `const`. Derives `Copy/Clone/Eq/PartialEq/Ord/PartialOrd/Hash/Debug/Default`.
  It is the key type across the override and layer maps.
- `FaceAxis` — `{ PosX, NegX, PosY, NegY, PosZ, NegZ }`, used as the decal/override
  face key. It has no numeric accessor, so persistence carries its own
  `face_to_u8` / `face_from_u8` (§7).
- `ShapeId` (`Empty/Cube/SlabBottom/SlabTop`) is unchanged from Phase 2 — the slab
  shapes already existed in the data model; Phase 3 added their *rendering* and a
  pass that *produces* them.

---

## 3. The chunk model — data `Chunk` vs. runtime `LoadedChunk`

The central Substep-3 decision (§7a of the pre-Phase-3 audit) was settled in favor
of a **clean split** (`world/chunk.rs`):

```
pub struct Chunk {                      // data-model container (design doc §3)
    coord: ChunkCoord,
    voxels: Arc<ChunkStorage>,
    detail_layers: DetailLayers,        // Tier 1 foliage paint
    scatter_instances: ScatterStore,    // Tier 2/3 discrete scatter
    fluids: FluidLayer,
    decals: DecalLayer,
    lighting: LightData,
    tags: ChunkTags,                    // ZoneId(0) + [BiomeId(0)] in Phase 3
    overrides: Option<ChunkOverrides>,  // canonical player-edit model
}

pub struct LoadedChunk {                // runtime wrapper (live set)
    data: Chunk,
    mesh_dirty: bool, mesh_seq: u64, mesh: Option<ChunkMesh>,
    generation: u64, persist_dirty: bool, mesh_debounce: Option<Instant>,
}
```

- `Chunk::new(pos, Arc<ChunkStorage>)` default-constructs every layer empty, tags
  `ZoneId(0)`/`BiomeId(0)`, and sets `overrides: None`.
- `voxels` stays behind an `Arc` because the mesher reads it on worker threads
  while the main thread holds the chunk — the Phase-2 sharing model is preserved.
- `LoadedChunk` carries the meshing/persistence bookkeeping that used to be
  interleaved on the old runtime `Chunk`; `ChunkSnapshot::extract`,
  `ChunkNeighbors`, and `border_dirty_neighbors` all read through
  `chunk.data.voxels` / `chunk.data.coord`.

The empty `PopulatedChunk` lighting/simulation/flora Option tiers that Phase 2
carried (§4 of the pre-Phase-3 audit) were **removed** in Substep 3a;
`PopulatedChunk` is now just `{ material_id: PalettedBitArray }`. Baked lighting
re-homed to the typed `LightData` layer (§5), still empty.

---

## 4. Player edits — `ChunkOverrides` replaces `VoxelEdit`

Substep 2 (with the §7b decision resolved to **replace, not coexist**) made
`ChunkOverrides` the live edit path (`world/overrides.rs`):

```
pub struct ChunkOverrides {
    voxel_diffs: HashMap<LocalPos, Voxel>,          // cleared cell = Voxel::EMPTY
    voxel_removed: HashSet<LocalPos>,               // reserved; empty in Phase 3
    scatter_removed: HashSet<StableInstanceId>,     // reserved
    scatter_added: Vec<ScatterInstance>,            // reserved
    detail_diffs: HashMap<(DetailLayerId, LocalPos), DetailTexel>,  // reserved
    fluid_diffs: HashMap<LocalPos, FluidCell>,      // reserved
    decal_diffs: HashMap<(LocalPos, FaceAxis), DecalEntry>,         // reserved
}
```

- Phase 3 populates only `voxel_diffs`; the other six fields are reserved
  scaffolding per the design doc §9 and stay empty until their systems land.
- Methods: `is_empty()`, `voxel_override_count()` (drives the delta-vs-full
  persistence choice), `set_voxel(index, voxel)` (last-write-wins per cell for
  free).
- `World::apply_edit(chunk_pos, index, voxel)` (`mod.rs:171`) is the new edit
  entry point — it routes into `overrides.voxel_diffs`. The old
  `World::apply_edit(chunk_pos, edit)` signature, `VoxelEdit` the type,
  `apply_edits_to_storage`, and `deduplicate_edits` are **gone**. The only
  remaining `VoxelEdit` reference is a historical doc-comment in `overrides.rs`.

---

## 5. Typed sidecar layers (`world/layers.rs`) — empty scaffolding

The multi-layer chunk model is now expressed as typed layers rather than opaque
Option tiers. All ship empty (`Default`), with no generation logic and (apart from
persistence round-trip) no consumers yet:

| Layer | Type | Shape |
|---|---|---|
| Tier 1 foliage paint | `DetailLayers { layers: SmallVec<[DetailLayer; 4]> }` | per-column `DetailTexel { species, density, tint, flags }` over the XZ footprint |
| Tier 2/3 scatter | `ScatterStore { by_type: HashMap<ScatterTypeId, Vec<ScatterInstance>> }` | `ScatterInstance { anchor: LocalPos, sub_offset:[i8;3], rotation_y, scale_variant, prefab_id, flags }` |
| Fluids | `FluidLayer { fill_mode, cells: HashMap<LocalPos, FluidCell>, active: HashSet<LocalPos> }` | `FluidCell { fluid_id, mass:u16, flags }`; `FluidFillMode::{Empty, Submerged(FluidId)}` |
| Decals | `DecalLayer { entries: HashMap<(LocalPos, FaceAxis), DecalEntry> }` | `DecalEntry { decal_id, flags }` |
| Lighting | `LightData { skylight: Option<Box<[u8; CHUNK_VOLUME]>>, block_light: Option<…> }` | both `None`; an unlit chunk costs O(1) |

Identity newtypes: `DetailLayerId(u16)`, `ScatterTypeId(u16)`, `PrefabId(u32)`,
`FluidId(u16)`, `StableInstanceId(u64)`. The ocean-fill exception
(`FluidFillMode::Submerged` below sea level) was **not** taken — it did not land
trivially, so it remains scaffolding. `layers.rs` retains a module-level
`#![allow(dead_code)]` (the only one left after the sweep, §8) because the flag
bits, `FluidFillMode::Submerged`, `ScatterStore::by_type`, `DetailLayer::new`, and
`ScatterTypeId` await later-phase systems.

---

## 6. Generation passes — walkability mask & slab smoothing

Both new passes run inside the shared `generate_chunk_storage` path
(`world_generator.rs:103`), so startup fill, streaming, and background regen all
get them for free — the same sharing model as `StorageBoundary::materialize`.

### Walkability mask (`world/walkability.rs`)
- `WalkabilityMask::from_storage(&ChunkStorage) -> Self` computes the standable-cell
  predicate (solid `Cube`/`SlabBottom` top with air above), with above-chunk
  treated as air (intra-chunk only — no neighbor context).
- It is **transient**: no chunk stores a mask. Its only live consumer is the slab
  smoothing pass, which recomputes it. `count()` is a test-only/future-overlay API
  kept under a targeted `#[allow(dead_code)]`; the formerly-dead `empty()` was
  deleted in Substep 8.
- A determinism test (`from_storage` twice ⇒ identical) guards the pass.

### Slab smoothing (`world/slab_smoothing.rs`)
- `smooth_slabs(&mut ChunkStorage)` runs the walkability mask, then demotes each
  walkable surface `Cube` that has a lower walkable horizontal neighbor to
  `SlabBottom` (material/flags preserved). It rewrites **only full `Cube`** voxels,
  uses the same "around the chunk is air" convention, and is **intra-chunk only**.
- A determinism test (smoothing two identical storages ⇒ identical) guards it.

### Slab rendering (`meshing/cube_mesher.rs`)
The §7c critical gap is closed. The mesher now branches on `ShapeId`:
`SlabBottom => Y range (0.0, 0.5)`, half-height side quads and a top face at the
midline. Cube-only chunks render bit-identically to Phase 2 (the slab path only
engages for actual slab voxels). This is the visible done-signal for the
smoothing pass.

---

## 7. Persistence — v5 sectioned multi-layer format (`world/persistence.rs`)

Substep 7 bumped `BLOB_VERSION` **4 → 5** and `VOXEL_FORMAT_VERSION` **1 → 2** (the
bump forces a one-shot save wipe + mesh-cache clear on open; pre-release saves are
not migrated). Backend unchanged: SQLite (WAL) + zstd (optional trained dict).

**v5 blob layout:** `[BLOB_VERSION u8][variant_tag u8]<variant payload><tags section>`

- Variant tags: `TAG_DELTA = 0`, `TAG_FULL_UNIFORM = 1`, `TAG_FULL_POPULATED = 2`
  (full-populated bridges through `PalettedBitArray::{serialize,deserialize}_to_bytes`,
  which is self-delimiting).
- A bounds-checked `Reader` returns `Corrupt("unexpected end of blob")` on overrun;
  free `w_u16/u32/u64` helpers on the write side.
- The override payload writes all **seven** `ChunkOverrides` fields as
  u32-count-prefixed sections, in a fixed order (1 voxel_diffs, 2 voxel_removed,
  3 scatter_removed, 4 scatter_added, 5 detail_diffs, 6 fluid_diffs, 7 decal_diffs).
  Map-backed sections are **key-sorted** for determinism; `scatter_added` preserves
  `Vec` order. `face_to_u8`/`face_from_u8` encode the decal face key.
- The **tags section** (zone u16; biomes u32-count + u16 each; library_refs
  u32-count + u32 each) is appended to every variant.

**Record type:** `ChunkRecord { edits: ChunkEdits, tags: ChunkTags }`.
`build_chunk_record(chunk)` pairs `build_chunk_edits` with `chunk.data.tags`;
`load_chunk_record(pos)` (renamed from `load_chunk_edits`) returns it.
`save_dirty_chunks` batches `Vec<(IVec3, ChunkRecord)>`; `save_chunk_on_unload`
keeps its `(&self, &LoadedChunk)` signature.

**Tests** (in-module `#[cfg(test)]`, since `voxulacrum` is binary-only):
`roundtrip_delta_all_fields` (all seven override fields), `roundtrip_full_uniform`,
`roundtrip_full_populated`, `roundtrip_empty_is_compact` (empty blob = **40 bytes**
= `2 + 7*4 + 2 + 4 + 4`), `serialization_is_order_independent`,
`rejects_unknown_blob_version`.

**Load re-application (important, deferred):** only `voxel_diffs` are re-applied
on load (self-healed into the freshly generated storage at the streaming load
path). The other six override sections and the tags section **round-trip** but are
not yet re-applied to a reloaded chunk's layers — there are no systems to consume
them in Phase 3. Recorded in §9.

---

## 8. Verification sweep & the streaming regression

**Dead-code sweep (Substep 8).** With the new consumers in place, the three
carried `#![allow(dead_code)]` blocks were re-evaluated per item (a binary crate
does *not* exempt `pub` items from `dead_code`):
- `tags.rs` — allow **removed** (fully consumed by persistence + `Chunk::new`).
- `walkability.rs` — module allow **removed**; `empty()` **deleted** (dead even in
  tests); `count()` kept under a narrow `#[allow(dead_code)]` with an explanatory
  doc comment.
- `layers.rs` — allow **kept** (genuine still-unused scaffolding), comment updated
  to explain what persistence now consumes vs. what awaits later phases.

The build came back with **zero `dead_code` warnings**, confirming the analysis.
Two pre-existing rendering allows are untouched and unrelated to Phase 3:
`rendering/water_pass.rs:6` and `rendering/render_graph.rs:87`.

**Streaming regression found and fixed during verification.** Runtime testing
showed only initial-radius chunks loading, the nearest chunk flashing in/out, and
no reloads (the world went empty). Root cause: an **inverted load-queue
predicate** introduced during the Substep-3 chunk-model migration — original
`!world.chunks.contains_key(&pos)` (true when *absent*) was mechanically migrated
to `!world.get_chunk(pos).is_none()` (true when *present*), net-inverting the
condition. Fixed at the `tick()` load queue to `if world.get_chunk(pos).is_none()`.
User confirmed the fix resolved all three symptoms.

---

## 9. Carried forward (open / deferred items)

Neutral list of items explicitly out of Phase 3 scope or deferred by a Phase 3
substep. None block anything; they are recorded so a later phase can pick them up
deliberately.

| Item | Where | Note |
|---|---|---|
| Six non-voxel override sections **round-trip but are not re-applied** on load | `persistence.rs` / streaming load | `scatter_*`, `detail_diffs`, `fluid_diffs`, `decal_diffs`, `voxel_removed` persist; only `voxel_diffs` self-heals into reloaded storage. No consumers yet. |
| Persisted **`ChunkTags` are functionally redundant** in Phase 3 | `persistence.rs` tags section | Every chunk is `ZoneId(0)` + `[BiomeId(0)]`; the section is wired and tested for when multi-biome/library tagging lands. |
| Sidecar **layers are empty scaffolding** (no generation) | `world/layers.rs` | `DetailLayers`, `ScatterStore`, `FluidLayer`, `DecalLayer`, `LightData` default-empty; flag bits / `FluidFillMode::Submerged` / `by_type` / `DetailLayer::new` / `ScatterTypeId` await later phases (the remaining `#![allow(dead_code)]`). |
| **Ocean fill** (`Submerged` below sea level) not taken | `FluidLayer` | The one Phase 3 content-gen exception; did not land trivially, left as scaffolding. |
| Walkability mask & slab smoothing are **intra-chunk only** | `walkability.rs`, `slab_smoothing.rs` | Above/around the chunk is treated as air; top-of-chunk surfaces don't see the +Y neighbor. Acceptable approximation per §6 of the pre-Phase-3 audit. |
| Walkability mask is **transient** (not stored) | `walkability.rs` | Recomputed by slab smoothing; `count()` kept under a narrow allow for future pathfinding/overlay use. |
| **No baked lighting** | `LightData` | Both fields `None`; design doc §5 stage 11 is out of Phase 3 scope. Mesher lights with a flat value. |
| No **manual edit controls** in the running app | input/UI | `World::apply_edit` exists and is unit-correct, but there is no keybind to place/clear voxels yet, so the override path is test-covered but not interactively exercised. |
| Pre-existing rendering allows | `water_pass.rs`, `render_graph.rs` | Unrelated to Phase 3; not touched. |

**Inherited Phase 2 items now resolved:** the `VoxelEdit` moisture/flora
serialization deferral is moot — `VoxelEdit` is gone and the richer
`ChunkOverrides` diff set supersedes it.

---

## 10. Phase 3 outcomes summary

| Substep | Delivered | Primary file(s) |
|---|---|---|
| 1 — primitives & typed layers | `ChunkCoord`/`LocalPos`/`FaceAxis` in `voxel-core`; typed `DetailLayers`/`ScatterStore`/`FluidLayer`/`DecalLayer`/`LightData` + id newtypes | `voxel-core/coord.rs`, `world/layers.rs` |
| 2 — `ChunkOverrides` | Canonical multi-layer diff set **replacing** `VoxelEdit`; `World::apply_edit(pos, index, voxel)` routes into `voxel_diffs` | `world/overrides.rs`, `world/mod.rs` |
| 3 — data/runtime split | `Chunk` (data) + `LoadedChunk` (runtime) split; `PopulatedChunk` reduced to `{ material_id }`; lighting re-homed to `LightData` | `world/chunk.rs`, `world/storage.rs` |
| 4 — `ChunkTags` | `ZoneId`/`BiomeId`/`LibraryGraphId` tags; `single_biome`; every chunk `Zone0`/`Biome0` | `world/tags.rs`, `world/chunk.rs` |
| 5 — walkability mask | `WalkabilityMask::from_storage` + determinism test; runs in shared gen path | `world/walkability.rs`, `world_generator.rs` |
| 6 — slab smoothing + rendering | `smooth_slabs` (cube→SlabBottom demotion) + determinism test; **slab face emission added to the mesher** | `world/slab_smoothing.rs`, `meshing/cube_mesher.rs` |
| 7 — v5 persistence | Sectioned, key-sorted, deterministic multi-layer blob (voxels + 7 override sections + tags); `Reader`; v4→v5 + format-version wipe; roundtrip/order/empty-size tests | `world/persistence.rs`, `world/streaming.rs` |
| 8 — verification sweep | Per-item dead-code analysis (removed tags/walkability allows, deleted `empty()`, kept narrow `count()` allow, kept layers allow); **found & fixed the Substep-3 streaming load-queue inversion** | `world/tags.rs`, `world/walkability.rs`, `world/layers.rs`, `world/streaming.rs` |

---

*End of audit. This is a factual snapshot at the close of Phase 3; no code was
changed in producing it (the streaming-inversion fix in §8 landed during Substep 8
verification, before this audit). It supersedes `pre-phase-3-audit.md` as the
current starting-state reference for Phase 4.*
