# Pre-Phase-3 State Audit

**Status:** Substep 0 deliverable. Reference-only — no code changes.
**Date:** 2026-06-15.
**Supersedes as starting reference:** `docs/post-phase-2-audit.md` (close-of-Phase-2 snapshot).
**Authoritative targets:** `docs/engine-design.md` v1.2 (§3 chunk model, §5 stage order, §9 overrides, §10 slab faces, §11 material continuity), the Phase 3 system prompt.

This audit grounds Substeps 1–8 in the *actual* state of the code as read on
2026-06-15, and surfaces the gaps where the current implementation diverges from
the Phase 3 target. It is a factual snapshot plus three explicit decisions the
user must make before code planning begins.

---

## §0. Scope of Phase 3 (recap, for grounding only)

Phase 3 expands the chunk model toward the design-doc four-layer architecture,
formalizes `ChunkOverrides`, and adds two generation passes: the **walkability
mask** (§5 stage 6) and **slab smoothing** (§5 stage 7). New layers ship as
**empty scaffolding** (Empty-Scaffolding Principle — not relitigated here). The
single content-gen exception is ocean fill for the FluidLayer
(`Submerged` below `sea_level`), and only if it lands trivially. Phase 3 ships
ONE biome (`BiomeId(0)`), ONE zone (`ZoneId(0)`). Determinism is mandatory: every
new pass needs a determinism test. `BLOB_VERSION` bumps 4 → 5 with
wipe-migration.

---

## §1. Current chunk model surface

### 1.1 Runtime `Chunk` (`world/chunk.rs`)

The live runtime container is **not** the design-doc data `Chunk`. It carries
mesh + persistence bookkeeping interleaved with the voxel payload:

```
pub struct Chunk {
    position: IVec3,                       // design doc wants ChunkCoord
    storage: Arc<ChunkStorage>,            // design doc wants voxels: ChunkStorage (owned)
    mesh_dirty: bool,
    mesh_seq: u64,
    mesh: Option<ChunkMesh>,
    generation: u64,
    persist_dirty: bool,
    edit_list: Option<Vec<VoxelEdit>>,
    mesh_debounce: Option<Instant>,
}
```

Construction is uniform across all three generation sites (see §6), always
`Chunk::new(pos, Arc::new(storage))`. The `storage` is shared via `Arc` because
the mesher reads it on worker threads while the main thread holds the chunk.

### 1.2 Storage tiers (`world/storage.rs`)

- `ChunkStorage` enum: `Uniform { voxel }` | `Populated(Box<PopulatedChunk>)`.
  Collapses to `Uniform` when homogeneous (`try_collapse`).
- `PopulatedChunk { material_id: PalettedBitArray, lighting: Option<Box<LightingData>>, simulation: Option<Box<SimulationData>>, flora: Option<Box<FloraData>> }`.
- `PalettedBitArray { palette: Vec<u32>, bits_per_entry: u8, data: Vec<u64> }` —
  packs `u32`-encoded voxels.

**The optional layer fields (`lighting`, `simulation`, `flora`) already model a
multi-layer chunk, but they are pure scaffolding** — see §4 and §1.4.

### 1.3 Missing design-doc types

None of the following exist anywhere in the workspace yet; all must be defined
in Phase 3:

- `ChunkCoord` (design doc uses it as the chunk key; code uses `IVec3`).
- `LocalPos` (intra-chunk coordinate; code uses raw `usize`/`u16` indices).
- `FaceAxis` (needed for decal/override keys).
- `ChunkTags`, `ChunkOverrides`, `WalkabilityMask`, `FluidLayer`/`FluidCell`,
  `DetailTexel`, `ScatterInstance`/`StableInstanceId`, `DecalEntry`.

### 1.4 ShapeId vocabulary (`voxel-core/src/shape.rs`) — slabs already exist in the *data* model

```
pub enum ShapeId { Empty = 0, Cube = 1, SlabBottom = 2, SlabTop = 3 }
```

- `SlabBottom` = solid half in Y range [0.0, 0.5]; `SlabTop` = [0.5, 1.0].
- `is_solid()`, `is_slab()`, `from_raw()`, `MAX_DISCRIMINANT = 3`,
  `BIT_BUDGET = 3` (packed format reserves 3 bits, room for stairs/chamfers).
- **The slab shapes exist in the voxel data model today.** What is missing is
  *rendering* them (§3). The `is_slab()` doc-comment still reads "used by the
  preview mesher" — a stale reference; the preview crate was deleted in Phase 2.

---

## §2. Edit infrastructure

`VoxelEdit { index: u16, voxel: Voxel, moisture: Option<u8>, flora_id: Option<u16>, flora_growth: Option<u8> }`
(Copy). The runtime edit list lives on `Chunk::edit_list: Option<Vec<VoxelEdit>>`.

Edit application + persistence call sites:

- `World::apply_edit(chunk_pos, edit)` — `mod.rs:164`, appends to `edit_list`.
- `apply_edits_to_storage(&storage, &filtered)` — `persistence.rs:399`, replays
  edits onto a freshly generated storage at load.
- `deduplicate_edits` — `persistence.rs:423` (last-write-wins per index).
- `build_chunk_edits(chunk) -> Option<ChunkEdits>` — `persistence.rs:440`, reads
  `chunk.edit_list` + `persist_dirty`.
- `streaming.rs:267` overlays filtered edits at load, then sets
  `chunk.edit_list` (`streaming.rs:283`).

**Relevance to Phase 3:** `VoxelEdit` is the *current* override mechanism and is
voxel-only (plus moisture/flora hints). The design-doc `ChunkOverrides` (§9) is a
strictly richer, multi-layer diff set (voxel_diffs, voxel_removed,
scatter_removed/added, detail_diffs, fluid_diffs, decal_diffs). Substep 2
formalizes `ChunkOverrides` as scaffolding; it does **not** yet have to replace
`VoxelEdit` — the two can coexist, with `VoxelEdit` remaining the live path and
`ChunkOverrides` the empty forward-looking structure. **Decision point flagged in
§7 (b).**

---

## §3. Meshing / slab path — CRITICAL GAP

`meshing/cube_mesher.rs` (109 lines, read in full):

- `generate_chunk_mesh(snap: &ChunkSnapshot, mat_config) -> (Vec<TerrainVertex>, Vec<u32>)`.
- **No slab support whatsoever.** Line 68 comment: *"Any solid shape renders as
  a full cube in Phase 0; slab faces are Phase 3."* Every `is_solid()` voxel
  emits a full-cube face set from the `FACES` array — no `ShapeId` branching, no
  half-height quads, no midline faces.

Consequently the design-doc §10 slab face rules (SlabBottom top face at Y+0.5,
SlabTop bottom face at midline, half-height side quads) are **entirely
unimplemented in the renderer.**

**This directly conflicts with Substep 6's done-signal**, which requires
*visible* slab rendering after the smoothing pass. The Phase 3 prompt scopes out
"mesher rewrite beyond slab support," which implies **adding slab support to the
mesher is in scope**. The magnitude of that work (and whether it belongs in
Substep 6 or earlier) is **Decision point §7 (c)**.

`ChunkSnapshot` (`world/chunk.rs`) is the mesher input: a 34³ padded
`Box<[Voxel; SNAP_VOLUME]>` extracted from a chunk + its `ChunkNeighbors` via
`resolve_voxel(neighbors, x, y, z)`. **This neighbor-access pattern is the
template Substep 5 should reuse** for walkability boundary lookups (the
walkability rule needs the voxel above and two-above, which crosses the +Y chunk
boundary at the top layer).

---

## §4. Lighting path — pure scaffolding, no bake exists

- `LightingData { light_sun: Box<[u8; CHUNK_VOLUME]>, light_emit: Box<[u8; CHUNK_VOLUME]> }`.
- `PopulatedChunk::lighting` is **always `None` at construction**
  (`storage.rs:326`, `:400`).
- `lighting_mut()` (`storage.rs:335`) is the only writer, and a workspace-wide
  grep shows it has **zero callers**. `light_sun`/`light_emit` are referenced
  only in their own definition.

**Conclusion:** there is no skylight or emissive bake. Lighting is an empty
Option field — consistent with the Empty-Scaffolding Principle. The mesher
therefore lights with a fixed/flat value, not baked data. Phase 3 does not add
lighting (design doc §5 stage 11 is out of Phase 3 scope); this is recorded so no
substep assumes baked light exists.

---

## §5. Persistence format (BLOB_VERSION = 4)

`world/persistence.rs` constants:

- `BLOB_VERSION: u8 = 4` (line 39 — "chunk payloads now store packed u32 Voxels").
- `VOXEL_FORMAT_VERSION: u64 = 1` (line 43) — wipe-migration gate stored as DB
  meta; mismatch on open wipes the chunk table.
- Tags: `TAG_DELTA = 0`, `TAG_FULL_UNIFORM = 1`, `TAG_FULL_POPULATED = 2`.

Blob byte layout: `[BLOB_VERSION: u8][tag: u8][payload]`.

- `TAG_DELTA`: per-edit `index:u16 + packed voxel:u32` (6 bytes each).
- `TAG_FULL_UNIFORM`: single packed voxel.
- `TAG_FULL_POPULATED`: `PalettedBitArray::serialize_to_bytes` →
  `[palette_len:u16][palette:u32…][bits_per_entry:u8][word_count:u32][data:u64…]`.

Reader rejects any `raw[0] != BLOB_VERSION` (`persistence.rs:103`). Roundtrip
tests (`roundtrip_delta`, `roundtrip_full_uniform`, `roundtrip_full_populated`,
`rejects_unknown_blob_version`) landed in Phase 2 Step 5 and all pass.

**Phase 3 impact:** BLOB_VERSION bumps to **5**. Because the gate already
hard-rejects mismatched versions and `VOXEL_FORMAT_VERSION` already drives
wipe-migration, the bump is a clean wipe — no in-place migration code needed. The
multi-layer payload (Substep 7) extends `TAG_FULL_POPULATED` (or adds new tags)
to serialize whichever layers are non-empty. **Empty layers must serialize to
nothing** to keep the common case (material-only) byte-compatible in spirit with
v4.

---

## §6. Generation / streaming / regen call paths

All chunk production funnels through `WorldGenerator::generate_chunk_storage(pos)
-> ChunkStorage`, which crosses the eval→storage boundary via
`StorageBoundary::materialize` (`world_generator.rs:100`,
`storage_boundary.rs:50`). Three call sites:

| Path | Site | Threading |
|------|------|-----------|
| Startup fill | `mod.rs:59` (`World::generate`) | `pool.install` + `par_iter` |
| Background regen | `mod.rs:427` (`generate_world_background`) | shared `gen_pool`, `par_iter` |
| Streaming load | `streaming.rs:248` | `pool.spawn` per chunk |

Each then builds `Chunk::new(pos, Arc::new(storage))`. Streaming additionally
overlays saved edits before constructing the chunk (`streaming.rs:248–283`).

**Phase 3 impact:** the new generation passes (walkability mask, slab smoothing)
must run **inside or immediately after `generate_chunk_storage`** so all three
paths get them for free — exactly as `materialize` is shared today. Walkability
needs neighbor access, which `generate_chunk_storage` does *not* currently have
(it produces one chunk in isolation). **This is a structural consideration for
Substep 5/6:** boundary-dependent passes either run at the snapshot stage (where
neighbors exist, like the mesher) or accept that top-of-chunk walkability is
approximate until neighbors load. Flagged for Substep 5 planning, not a blocking
decision now.

---

## §7. Decisions required before Substep 1 code planning

Three gaps need an explicit user call. Each is presented as a decision, not a
recommendation — the user drives.

**(a) Runtime-`Chunk` vs. data-model-`Chunk` reconciliation (drives Substep 3).**
The live `Chunk` (`world/chunk.rs`) interleaves mesh/persistence bookkeeping with
the voxel payload and keys on `IVec3 position` / `Arc<ChunkStorage> storage`. The
design-doc `Chunk` is a pure data container keyed on `ChunkCoord` with owned
`voxels: ChunkStorage` plus the layer fields. Options:
  - Extend the existing runtime `Chunk` in place with new layer fields (keeps one
    type, mixes concerns).
  - Introduce a separate data `Chunk` and have the runtime wrapper hold it (clean
    separation, more plumbing).
This is the central Substep 3 decision and should be settled before then.

**(b) `ChunkOverrides` vs. `VoxelEdit` coexistence (drives Substep 2).** Does
Phase 3 leave `VoxelEdit` as the live edit path and add `ChunkOverrides` purely
as empty scaffolding, or begin routing edits through `ChunkOverrides.voxel_diffs`
now? Empty-Scaffolding Principle favors the former; confirm.

**(c) Slab rendering in the mesher (drives Substep 6 — CRITICAL).** The mesher
has zero slab support (§3). Substep 6's done-signal requires visible slabs.
Either slab face emission is added to `cube_mesher.rs` as part of Substep 6, or
the done-signal is redefined (e.g., validated by data/test rather than
visually). The prompt's "no mesher rewrite beyond slab support" wording implies
slab support is in scope; confirm the intended boundary so Substep 6 is sized
correctly.

---

## §8. Inherited Phase 2 deferred items (still open)

- Moisture/flora **serialization** is deferred — `VoxelEdit` carries
  `moisture`/`flora_*` hints but the persistence layer dropped the
  `EDIT_FLAG_MOISTURE/FLORA_*` encoding in Phase 2 Step 5 (replaced by a
  documenting comment). The `simulation`/`flora` storage tiers remain empty
  Option fields, consistent with scaffolding.
- `format_version` DB meta write was removed in Step 5;
  `VOXEL_FORMAT_VERSION` remains the authoritative wipe gate.

None of these block Phase 3; they are recorded so a later substep doesn't
mistake the absence for a regression.

---

## §9. Summary of what Phase 3 must build vs. what already exists

| Concern | Exists today | Phase 3 work |
|---------|-------------|--------------|
| Slab shapes in data model | ✅ `ShapeId::SlabBottom/SlabTop` | — |
| Slab rendering | ❌ (cube only) | **Add to mesher (§7c)** |
| `ChunkCoord`/`LocalPos`/`FaceAxis` | ❌ | Define (Substep 1) |
| Layer Option fields on storage | ✅ (empty) | Formalize as typed layers (Substep 1) |
| `ChunkOverrides` | ❌ (only `VoxelEdit`) | Define scaffolding (Substep 2) |
| `ChunkTags` | ❌ | Define + trivially populate Zone0/Biome0 (Substep 4) |
| Walkability mask | ❌ | Implement + determinism test (Substep 5) |
| Slab smoothing pass | ❌ | Implement + determinism test (Substep 6) |
| Lighting bake | ❌ (empty field) | Out of Phase 3 scope |
| Persistence multi-layer | ❌ (v4 material-only) | v5 + wipe (Substep 7) |
| Shared gen call path | ✅ `generate_chunk_storage` | New passes hook here (§6) |

---

*End of pre-Phase-3 state audit. Awaiting signoff before Substep 1 code
planning. No code has been written.*
