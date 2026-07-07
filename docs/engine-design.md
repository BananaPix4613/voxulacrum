# Voxel Engine Foundation Design Document

**Status:** Baseline reference, v1.2
**Scope:** World generation, chunk data model, foliage, water, isometric pixel-art rendering
**Purpose:** Authoritative goalpost for engine architecture. Every system described here is foundational — implementations may be incremental, but the data model and architectural shape are settled.

**Revision history:**
- v1.0 — Initial baseline.
- v1.1 — Replaced 45° slope geometry with half-height slabs. Slopes are removed from the engine entirely. Added walkability mask and traversal smoothing distance as foundational concepts.
- v1.2 — Documented previously-implicit foundational systems: ECS runtime (bevy_ecs + FrameStage schedule), chunk persistence (rusqlite + zstd), mesh disk cache (graph-hash-keyed), and chunk streaming. Added material registry pattern. Clarified that `ChunkOverrides` is the canonical in-memory representation of player edits that the persistence layer serializes.

---

## Table of Contents

1. [Engine Identity and Constraints](#1-engine-identity-and-constraints)
2. [Voxel Format](#2-voxel-format)
3. [Chunk Data Model](#3-chunk-data-model)
4. [World Generation Graph Architecture](#4-world-generation-graph-architecture)
5. [Generation Pipeline](#5-generation-pipeline)
6. [Foliage System](#6-foliage-system)
7. [Water System](#7-water-system)
8. [Anchor Model for Player Interaction](#8-anchor-model-for-player-interaction)
9. [Authored vs. Generated Diff Model](#9-authored-vs-generated-diff-model)
10. [Meshing and Mesh Data](#10-meshing-and-mesh-data)
11. [Isometric Pixel-Art Rendering Requirements](#11-isometric-pixel-art-rendering-requirements)
12. [Cross-Cutting Concerns](#12-cross-cutting-concerns)
13. [Glossary](#13-glossary)

---

## 1. Engine Identity and Constraints

The engine is a **2.5D isometric voxel engine** with stylized pixel-art rendering. The world is rendered at high internal resolution and downscaled with stylized filtering to produce a hand-drawn pixel-art aesthetic.

### Core characteristics

- **Voxel geometry** uses full cubes and half-height slabs (top-half and bottom-half). All faces are cardinal-axis-aligned. No diagonal geometry.
- **World is effectively infinite** via streamed chunks. Changes must localize.
- **Sandbox**: players can place and remove voxels, foliage, and other world elements.
- **Multi-biome**: regions differ fundamentally in shape, material, foliage, and atmosphere.
- **Hot-reload**: world generation can be edited at runtime via an in-engine node graph editor, with affected chunks regenerating in place.

### Why no diagonal geometry

The engine deliberately rejects 45° slope shapes despite the appeal of "smooth" terrain. The reasons are foundational to every other system:

- **Iso-orthographic projection is hostile to 45° slopes.** Slope faces project to screen-space angles that are close to but not exactly aligned with cube grid angles. The eye reads this near-parallelism as visual error rather than as intentional geometry.
- **Pixel-art final downscale produces literal staircases at any non-axis-aligned edge.** Slopes attempt to introduce continuous diagonals into a medium that physically cannot render them at output resolution. Slab steps embrace the medium's nature.
- **The grid is the visual language.** Slopes erase the grid identity that makes voxel pixel-art read as deliberate construction rather than as approximated nature.
- **Material clashes vanish.** Slab faces are cardinal-axis cubic faces with one material each. No diagonal triangles, no mid-face material seams, no combinatorial slope-shape vocabulary.
- **Traversal becomes simpler, not more complex.** Half-slab auto-stepping is the proven model (Minecraft, Stardew Valley, Terraria, every grid-based game with vertical movement). It works better than slope interpolation.

Gradual inclines are produced not by per-voxel diagonal geometry but by **distributing slab steps across distance** — a worldgen parameter, not a geometric feature. See §5 (Slab smoothing).

### Foundational principles

- **Data model decisions are settled now.** Implementations grow incrementally; data shapes do not.
- **Determinism is mandatory.** Same seed + same graph + same coordinates → bit-identical output. All randomness derives from explicit seeded RNGs.
- **The engine is the host.** The editor is a panel inside the engine, not a separate application. There is one `main` function, one renderer, one chunk store.
- **The graph is the world generator.** Editing the active graph edits the live world.

---

## 2. Voxel Format

Each voxel is a packed structure representing a unit of solid geometry. Every face of every shape is cardinal-axis-aligned.

```rust
#[repr(C)]
pub struct Voxel {
    pub shape: ShapeId,         // 3 bits — small shape vocabulary
    pub material: MaterialId,   // 16 bits
    pub flags: u8,              // 8 bits: light source, water-logged, etc.
    // No rotation field — slabs do not require rotation.
}
```

### Shape vocabulary

- `Empty` — air
- `Cube` — full solid cube, fills entire cell
- `SlabBottom` — solid in the lower half of the cell, empty above
- `SlabTop` — solid in the upper half of the cell, empty below

That is the complete shape set. Four shapes total, three bits sufficient with one reserved for future use (e.g., a `SlabBoth` shape if material-splitting a full cube ever proves useful).

### Stacking and composition

A full-height surface that mixes two materials at a half-cube boundary is represented as two voxels: a `SlabBottom` of material A in the lower cell and a `SlabTop` of material B in the upper cell. The result fills both cells exactly; the renderer draws an interior cardinal-axis face between them. This supports per-half-cube material layering on cliffs and exposed banks without geometric complexity.

### Critical constraints

- **Voxels are pure geometry.** No foliage data, no fluid data, no decals.
- **Slabs are produced by the smoothing pass, not authored by players directly.** During worldgen, the slab smoothing pass converts cube-step boundaries into slab transitions where the walkability mask permits. Player tools place full cubes; designers can author slabs in prefabs explicitly.
- **There is no rotation field.** All shapes are rotation-invariant under the cardinal grid. Top and bottom slabs are distinct shapes rather than a rotated single shape because their face attribution differs (a `SlabTop`'s top face is at the cell's top; a `SlabBottom`'s top face is at the cell's midline).
- **No diagonal faces ever exist.** The mesher will not produce them. The shader does not branch on them. The face_axis attribute (§10) has exactly 6 valid values, one per cardinal direction.

### Material registry

`MaterialId` is a stable index into a **MaterialRegistry** — a data-driven registry loaded at engine startup from an asset file (RON or JSON). The registry maps stable IDs and human-readable names to material definitions (palette assignments, light response, sound profile, gameplay flags).

- **Stable IDs are the wire format.** Voxels store `MaterialId` directly. Save files reference materials by ID, not by name.
- **Name resolution happens at load time.** Graphs reference materials by name (`"stone"`, `"grass_top"`); the loader resolves names to stable IDs and stores the resolved IDs in the in-memory graph.
- **The registry is consumed by everything that needs to interpret materials**: the worldgen evaluator, the mesher (for `material_id` baking), the renderer (for palette lookup), the audio system (for surface sounds), and mod loaders (which can register additional materials).
- **The initial registry content** ships as engine data and may be extended by mods. Material identity is not hardcoded in any system; everything goes through the registry.

---

## 3. Chunk Data Model

A chunk is a 32×32×32 region carrying four parallel data layers. Storage is sparse where coverage is typically low and dense where it is typically high.

```rust
pub struct Chunk {
    pub coord: ChunkCoord,
    pub voxels: ChunkBuffer<Voxel>,         // dense: 32³ array
    pub detail_layers: DetailLayers,        // foliage paint (Tier 1)
    pub scatter_instances: ScatterStore,    // foliage instances + structures (Tier 2/3)
    pub fluids: FluidLayer,                 // water, lava, future fluids
    pub decals: DecalLayer,                 // surface marks (reserved, may start empty)
    pub lighting: LightData,                // baked skylight + block light
    pub tags: ChunkTags,                    // biome, zone, library refs for invalidation
    pub overrides: ChunkOverrides,          // player edits, persisted separately
}

pub struct ChunkTags {
    pub zone: ZoneId,
    pub biomes: SmallVec<[BiomeId; 4]>,
    pub library_refs: SmallVec<[LibraryGraphId; 8]>,
}
```

### Why four layers, not one

Each layer has different:
- **Update cadence** — voxels rarely change; fluids tick constantly.
- **Coverage characteristics** — voxels are dense; fluids and decals are typically sparse.
- **Edit invalidation** — editing biome density invalidates voxels; editing tall grass density invalidates only detail layers.
- **Rendering pipeline** — voxels mesh; foliage instances; fluids may use a different mesh strategy.

Conflating them into one structure (e.g., putting fluid level in voxel flags) couples unrelated systems and prevents the fast paths each system needs.

### Chunk storage on disk

Chunks serialize as a tagged composite. Each layer has its own version field. Layers default to empty when absent so older saves load forward-compatibly.

```
ChunkFile {
    header: { version, coord, world_seed },
    voxels: VoxelLayerV1 | VoxelLayerV2 | ...,
    detail_layers: DetailLayersV1 | absent,
    scatter_instances: ScatterStoreV1 | absent,
    fluids: FluidLayerV1 | absent,
    decals: DecalLayerV1 | absent,
    overrides: ChunkOverridesV1 | absent,
}
```

**Persistence implementation:** chunks are stored in a single per-world **SQLite database** (`rusqlite`), with `zstd` compression on each chunk row (optionally trained on a corpus dictionary for better compression of similar chunks). This is the engine's chosen persistence layer; future changes to the storage format affect serialization, not the chunk data model.

**`ChunkOverrides` is the canonical representation of player edits**, in-memory and on-disk. The persistence layer serializes `ChunkOverrides` directly; there is no separate "edit log" format. When a chunk loads, the generator runs from the graph + seed and the deserialized `ChunkOverrides` is applied on top. When a chunk evicts from memory, only its `ChunkOverrides` plus any tag changes are written back — generated content is reproducible and not persisted.

### Generated vs. authored split

Within each layer, content divides into **generated** (reproducible from seed + graph; not serialized in detail) and **overrides** (player edits; serialized fully). See §9 for the diff model.

---

## 4. World Generation Graph Architecture

The world generator is composed of **five graph types** in a strict hierarchy. Each graph has its own root output pins and its own subset of available nodes.

### Graph types

| Graph Type | Quantity | Role |
|------------|----------|------|
| `WorldGraph` | exactly one | Climate fields, zone selection, sea level, global constants |
| `ZoneGraph` | one per Zone | Biome distribution within zone, cave rules, structure pool, rivers |
| `BiomeGraph` | one per Biome | Terrain density, material, fluid sources, fade behavior |
| `DetailGraph` | one per detail system (usually per biome) | Foliage paint + scatter placement |
| `LibraryGraph` | any number | Reusable subgraphs referenced from any graph above |

### Hierarchy

```
World
├── WorldGraph (climate, zone selector, sea_level)
├── Zones (registered)
│   ├── ZoneGraph[Z1]
│   │   ├── BiomeGraph[B1]
│   │   ├── BiomeGraph[B2]
│   │   └── DetailGraph[B1], DetailGraph[B2]
│   └── ZoneGraph[Z2] ...
└── Libraries (reusable subgraphs)
    ├── LibraryGraph["StandardCaveNoise"]
    ├── LibraryGraph["SurfaceLayering"]
    └── ...
```

### Pin types

```rust
pub enum PinType {
    // Scalars and vectors
    Scalar,                 // single f32 or per-position f32
    Vec3,
    // Field types
    Density,                // 3D scalar field; solid where > 0
    SurfaceField,           // 2D scalar over chunk XZ footprint
    Material,               // material provider; "what block at this position?"
    FluidProvider,          // fluid initialization; "what fluid at this position?"
    // Selection types
    BiomeId,
    ZoneId,
    Curve,                  // editable spline f32 → f32
    // Placement types
    ScatterPoints,          // set of candidate positions
    PlacementMask,          // boolean field for "can place here?"
    SpeciesWeights,         // weighted variant selection
    // Output types
    PaintOutput,            // writes to DetailLayer
    ScatterOutput,          // writes to ScatterStore
    Terrain,                // final voxel field (BiomeGraph terminal)
    Assignments,            // prop/structure bindings
}
```

### Coercion rules

- `Scalar → Density` (constant field)
- `Curve → Scalar` (sample requires explicit Scalar input)

No other implicit coercions. Strictness is a feature.

### LibraryGraph mechanics

A `LibraryGraph` declares typed inputs and outputs at its boundary. A `LibraryRef` node inside any other graph references a library by ID, exposes its inputs as pin inputs, and exposes its outputs as pin outputs. Cycles are forbidden (validation catches them).

Standard libraries to ship:
- `StandardCaveNoise` — 3D Worley + ridged noise composite
- `SurfaceLayering` — depth-conditional material cake (grass/dirt/stone)
- `ExposureLayering` — surface layering varied by face exposure (top-facing vs side-facing surfaces get different materials/tints; useful for snow caps, exposed rock on cliff faces, moss on shaded sides)
- `BiomeBorderFade` — standard fade kernel
- `PoissonPlacement` — point distribution for scatter

### Edit invalidation table

Edits to different graph types invalidate different chunk sets. This is the primary mechanism for fast iteration.

| Edit target | Invalidates |
|-------------|-------------|
| `WorldGraph` (climate, zone selector, sea_level) | All loaded chunks |
| `ZoneGraph[Z]` | All chunks tagged with Zone Z or within Z's fade range |
| `BiomeGraph[B].density` | All chunks tagged with Biome B + fade buffer |
| `BiomeGraph[B].material` | Material-only re-pass; density cache kept |
| `BiomeGraph[B].fluid_provider` | Fluid initialization re-pass |
| `DetailGraph[D]` | Detail-only re-pass; voxel data untouched |
| `LibraryGraph[L]` | Every graph importing L applies its own rules |

Each chunk's `ChunkTags` makes invalidation a tag-set lookup, not a full-world scan.

---

## 5. Generation Pipeline

Chunk generation runs in **strict pipeline order**. Each stage reads from previous stage outputs and the source of truth graphs; no stage may reorder.

### Stage order

```
1. WorldGraph evaluation
   - Per-column: climate vector (temp, humidity, continentalness, erosion, weirdness)
   - Per-column: zone_id + zone_border_distance
   - Global: sea_level

2. ZoneGraph evaluation
   - Per-column: biome_id + biome_border_distance
   - Per-column: neighbor biomes for fade blending

3. BiomeGraph density
   - Per-voxel: density value
   - Boundary cells: blend with neighbor biome densities by fade factor

4. ZoneGraph cave subtraction
   - Per-voxel: cave density subtracted from terrain density

5. BiomeGraph material
   - Per-voxel: material assignment based on density, depth, surface proximity

6. Walkability mask computation (engine pass)
   - Per-voxel boolean: "the player can stand on top of this voxel"
   - A voxel is walkable if it is solid, the voxel above it is empty, and the voxel two above is empty (headroom).
   - Produces a sparse per-chunk artifact consumed by Stage 7 and reused by AI pathfinding and player movement at runtime.

7. Slab smoothing (engine pass, not graph-driven)
   - Reads walkability mask from Stage 6.
   - For each walkable voxel adjacent to a walkable neighbor at a different height, inserts slab steps to halve the height transition.
   - Cliff faces and non-walkable surfaces are not smoothed; they remain sharp cube-stepped.
   - Smoothing distance is a per-biome parameter (see "Traversal smoothing distance" below).
   - Authored slabs in prefabs are respected and treated as fixed.

8. ZoneGraph structures
   - Deferred placement; may straddle chunks
   - Cross-chunk template stamping with priority resolution

9. Fluid initialization
   - All empty voxels ≤ sea_level → ocean fill mode
   - BiomeGraph.fluid_provider outputs → settled fluid cells
   - ZoneGraph.rivers → settled fluid cells along channels

10. DetailGraph paint + scatter
    - DetailLayers written (Tier 1 foliage paint)
    - ScatterStore.generated populated (Tier 2/3 foliage + props)

11. Lighting bake
    - Skylight flood-fill
    - Block-light BFS from emissive voxels
    - Quantized to discrete levels

12. Override application
    - ChunkOverrides applied last (player edits supersede generated content)

13. Meshing
    - Vertex attributes baked per §10
```

### Per-column caches

Stages 1–2 produce per-column metadata used by every later voxel stage. Cache once per (x, z) per chunk.

```rust
pub struct ColumnCache {
    pub climate: ClimateVector,
    pub zone_id: ZoneId,
    pub zone_border_distance: f32,
    pub biome_id: BiomeId,
    pub biome_border_distance: f32,
    pub neighbor_biomes: SmallVec<[(BiomeId, f32); 4]>,
}
```

### Vertical layering

Above-ground / below-ground is **not** a separate hierarchy level. It is handled inside `BiomeGraph` via Y-conditional density nodes:

```
BiomeGraph "ForestHills":
  surface_band  = YBetween(60, 120) * SurfaceShape(...)
  subsurface    = YBetween(0, 60)   * StoneLayer(...)
  deep          = YBelow(0)         * DeepLayer(...)
  density       = Union(surface_band, subsurface, deep)
```

This keeps biomes self-contained, supports floating-island and arch biomes without special engine support, and allows caves (a Zone-level pass) to carve through any biome consistently.

### Fade blending

- **Density blending**: linear interpolation between biome densities weighted by border distance.
- **Material selection**: smooth threshold (max/min composite) between biome materials.
- **RNG derivation at fade boundaries**: seeded from `(world_seed, biome_id, x, z, purpose)` only. Never from chunk coordinates alone. This guarantees two chunks sharing a fade boundary blend identically regardless of generation order.

### Walkability mask

A derived per-chunk artifact computed after voxel and material generation, before slab smoothing:

```rust
pub struct WalkabilityMask {
    // One bit per voxel position; sparse storage for non-walkable chunks
    bits: BitArray<{ CHUNK_VOLUME }>,
}
```

A voxel is walkable if:
- It is solid (Cube, SlabBottom, or SlabTop).
- The voxel directly above is empty.
- The voxel two above is empty (headroom for the player).
- Its top surface is horizontal (Cube top face or SlabBottom top face at the midline). SlabTop voxels are not walkable on their top face because their "top" is at the cell ceiling with no headroom; they are walkable from beneath when used as overhang flooring.

The mask is computed once and reused by three consumers:
- **Slab smoothing (Stage 7)** — decides which voxel boundaries to smooth.
- **AI pathfinding** — operates on the walkability mask rather than re-deriving it.
- **Player movement** — collision and auto-step logic queries the mask for step-up validity.

Computing it once, in worldgen, before meshing, is what makes pathfinding and movement cheap at runtime.

### Traversal smoothing distance

A per-biome worldgen parameter controlling how aggressively the slab smoothing pass converts cube-step transitions into slab staircases.

| Value | Behavior | Visual result |
|-------|----------|---------------|
| 0 | No smoothing | Sharp cube steps; terraced terrain |
| 1 | Smooth single-voxel transitions only | Half-step inserted at 1-cube height changes |
| 2–4 | Smooth across short distances | Gentle short staircases |
| 5+ | Smooth across long distances | Long landings between half-steps; reads as gradual slope |

The parameter influences how the smoother distributes slab steps across walkable surfaces. A meadow biome might use 6 (gentle, gradual). A canyon biome might use 1 (sharp, terraced). A flat plains biome uses 0 (no smoothing needed; terrain is already level).

This is the mechanism that produces the "feeling of gradual slopes" without any diagonal geometry. The slope is emergent from step distribution, not from per-voxel angles.

---

## 6. Foliage System

Foliage is **instanced geometry rendered in tandem with the voxel world**, not voxel data. It exists in three tiers.

### Tier 1 — Detail scatter (grass, ferns, small flowers)

- Stored as **2D density paint maps** per chunk, anchored to surface voxels.
- Never per-instance; renderer generates blade positions on the GPU at draw time using world-position hashing.
- Continuous density (0–255), not binary presence.

```rust
pub struct DetailLayers {
    pub layers: SmallVec<[DetailLayer; 4]>,
}

pub struct DetailLayer {
    pub layer_id: DetailLayerId,
    pub map: [DetailTexel; 32 * 32],   // one texel per chunk column
}

pub struct DetailTexel {
    pub species: u8,    // variant within layer; 0 = none
    pub density: u8,    // controls blade count rendered
    pub tint: u8,       // palette index
    pub flags: u8,      // trampled, seasonal, wind strength
}
```

### Tier 2 — Discrete scatter (bushes, mushroom clusters, small rocks)

- Poisson-disk placed at chunk-gen time.
- Stored as instance lists per chunk.
- Rendered with hardware instancing.

### Tier 3 — Hero foliage (trees, large prefabs)

- Same storage as Tier 2.
- LOD billboards at distance.
- Prefab references with anchor + footprint metadata.

```rust
pub struct ScatterStore {
    pub by_type: HashMap<ScatterTypeId, Vec<ScatterInstance>>,
}

pub struct ScatterInstance {
    pub anchor: LocalPos,       // owning voxel cell
    pub sub_offset: [i8; 3],    // -128..127 → ~1/128 voxel precision
    pub rotation_y: u8,
    pub scale_variant: u8,
    pub prefab_id: PrefabId,
    pub flags: ScatterFlags,    // player_placed, harvestable, persistent
}
```

### Critical properties

- **Foliage does not occupy voxel cells.** A voxel with tall grass painted on it is still an empty (air) voxel above its solid neighbor.
- **Foliage can overlap.** Tall grass and a tree can share the same anchor voxel; they live in different stores.
- **Sub-voxel positioning is built-in.** Forests look natural, not gridded.
- **Stylistic placement is controlled by DetailGraph**, which has its own pin types optimized for surface placement rather than 3D field evaluation.

### Prefab metadata

```rust
pub struct PrefabMeta {
    pub anchor_offset: LocalPos,     // anchor position within bounding box
    pub footprint: Vec<LocalPos>,    // cells occupied (for placement validation)
    pub on_anchor_destroyed: AnchorDestructionPolicy,
    pub sway_weights: PrefabSwayData, // per-vertex sway for wind animation
}

pub enum AnchorDestructionPolicy {
    DestroyFoliage,    // grass on broken block → gone
    DetachAsEntity,    // tree → falling physics entity
    Reanchor,          // bush slides to new surface
}
```

---

## 7. Water System

Water is a **separate data layer** with mass-conserving cellular automata simulation. Storage is sparse with a fast path for ocean chunks.

### Storage

```rust
pub struct FluidLayer {
    pub fill_mode: FluidFillMode,
    pub cells: HashMap<LocalPos, FluidCell>,
    pub active: HashSet<LocalPos>,
}

pub enum FluidFillMode {
    Empty,                  // default: no fluid anywhere
    Submerged(FluidId),     // default: every empty voxel is full of this fluid
}

pub struct FluidCell {
    pub fluid_id: FluidId,
    pub mass: u16,          // 0–65535, where 65535 = full source-equivalent
    pub flags: u8,          // settled, falling, source
}
```

### Why this shape

- **Ocean fast path**: `fill_mode = Submerged(WATER)` with empty `cells` map costs O(1) regardless of chunk volume.
- **Sparse deviations**: tunnels under oceans, surface lakes, player-poured water all live in `cells` as explicit entries.
- **Deterministic mass**: u16 fixed-point avoids f32 non-determinism in multiplayer/replay.
- **Active set drives simulation cost**: settled fluid doesn't tick.

### Simulation

- Per-tick: each active cell computes flow to neighbors using w-shadow stable-state algorithm.
- Mass-conserving (with small loss below threshold to prevent infinite spread).
- Two-phase update per tick: read-and-compute-deltas, then apply-deltas. Cross-chunk flow stays consistent.
- Cells settle when stable; removed from active set.

### Cross-chunk flow

- Each chunk simulates independently.
- Reads neighbor chunks via the world's chunk lookup; writes only its own cells.
- Two-phase ordering ensures both chunks see identical neighbor state during read phase.

### Generation sources

- `WorldGraph.sea_level` (global Y) — defines ocean fill.
- `BiomeGraph.fluid_provider` — defines lakes, ponds, biome-specific water bodies.
- `ZoneGraph.rivers` — defines cross-biome water channels.

After generation, all cells are flagged `settled`. They unsettle only when disturbed.

---

## 8. Anchor Model for Player Interaction

Every foliage instance and decoration has an **anchor voxel**: the integer cell that owns it for player interaction, persistence, and edit attribution. Visual placement uses sub-voxel offsets; player interaction uses the anchor.

### Properties

- **The anchor is the unit of player edit**, regardless of visual density. Cutting grass on a column with 50 rendered blades is one operation (texel density decrement or zero).
- **Multiple instances per anchor**: a single voxel can own a wildflower scatter, a tall grass texel, and a small rock scatter simultaneously. Each lives in its own store.
- **The anchor is a query, not a render constraint**: renderer never reads anchors; only player tools and persistence do.

### Interaction primitives

- **Target voxel**: returns all instances/texels anchored to that cell from each store.
- **Place foliage**: writes new ScatterInstance with `player_placed` flag and validation against prefab placement rules.
- **Remove foliage**: deletes instance(s) or zeroes detail texel.
- **Modify terrain under foliage**: triggers `on_anchor_destroyed` policy per affected prefab.
- **Brush/radius operations**: bulk versions of single-anchor ops.
- **Cycle target**: when multiple instances share an anchor, tools cycle through them.

### Visual feedback

- Highlight the anchor voxel when player targets foliage.
- This is what gives "voxel control" feel even though visuals are continuous.

---

## 9. Authored vs. Generated Diff Model

Every chunk layer (voxels, detail, scatter, fluids, decals) splits into **generated** content (reproducible from seed + graph) and **authored overrides** (player edits, persisted).

### Why

- **Worldgen edits don't destroy player work**: regenerating a chunk subtracts removed-overrides from new-generated, adds added-overrides.
- **Save files stay small**: untouched chunks store no overrides.
- **The graph remains the source of truth** for the default world, even after extensive player modification.

### Structure

```rust
pub struct ChunkOverrides {
    pub voxel_diffs: HashMap<LocalPos, Voxel>,           // explicit voxel changes
    pub voxel_removed: HashSet<LocalPos>,                // explicit "this was destroyed"
    pub scatter_removed: HashSet<StableInstanceId>,      // generated instances removed
    pub scatter_added: Vec<ScatterInstance>,             // player-placed
    pub detail_diffs: HashMap<(LayerId, LocalPos), DetailTexel>,
    pub fluid_diffs: HashMap<LocalPos, FluidCell>,
    pub decal_diffs: HashMap<(LocalPos, FaceAxis), DecalEntry>,
}
```

### Stable instance IDs

Generated scatter instances need IDs that survive regeneration:

```
StableInstanceId = hash(world_seed, world_pos, prefab_id, sequence_in_anchor)
```

After regeneration, the engine subtracts `scatter_removed` from the freshly-generated set and unions `scatter_added`.

---

## 10. Meshing and Mesh Data

The mesher consumes a chunk's voxel data plus neighbor chunks and emits a triangle mesh with rich per-vertex attributes. Mesh data is the foundation for the stylized rendering pipeline.

### Vertex format

```rust
#[repr(C)]
pub struct FaceVertex {
    pub position: [f32; 3],         // 12 bytes
    pub normal: [i8; 3],            // 3 bytes (packed)
    pub uv: [u16; 2],               // 4 bytes
    pub material_id: u16,           // 2 bytes
    pub face_axis: u8,              // 1 byte — X+, X-, Y+, Y-, Z+, Z- (exactly 6 values)
    pub occlusion_class: u8,        // 1 byte — palette ramp selector
    pub biome_tint_index: u8,       // 1 byte
    pub variant_index: u8,          // 1 byte — selects among hand-authored variants
    pub light_level_index: u8,      // 1 byte — quantized to discrete steps
    pub enclosure_factor: u8,       // 1 byte — for fog and audio occlusion
    pub edge_flag: u8,              // 1 byte — material boundary for outline shader
    pub sway_weight: u8,            // 1 byte — wind animation (0 for terrain)
    pub ao_factor: u8,              // 1 byte — baked corner AO
    pub _padding: u8,
}
// ~32 bytes per vertex
```

### Mesher requirements

The mesher must have **read access to neighbor chunks** during meshing. This is mandatory for:

- AO computation (samples corner neighbors across chunk boundaries)
- Biome tint interpolation (samples neighbor biome IDs at boundary vertices)
- Edge flag detection (compares materials across chunk seams)
- Enclosure factor (samples skylight propagation across boundaries)

Without neighbor access, every chunk seam becomes a visible artifact.

### Greedy meshing policy

- **Top faces**: NOT greedy-merged. Preserved per-voxel for surface detail and per-voxel variation.
- **Side faces**: greedy-merged.
- **Bottom faces**: greedy-merged (mostly invisible in isometric).

This balances triangle count with detail fidelity where it visually matters.

### Slab face handling

Slabs share the cube meshing pipeline with one adjustment: vertical positioning of the top or bottom face is offset by half a voxel.

- A `SlabBottom`'s top face is emitted at the cell midline (Y + 0.5) rather than at the cell top.
- A `SlabTop`'s bottom face is emitted at the cell midline.
- Side faces of slabs are emitted as half-height quads.
- All faces remain cardinal-axis-aligned. `face_axis` has exactly 6 valid values; the shader does not branch on diagonal cases.

The mesher treats slabs as a thin variant of cube meshing with no special vertex attributes or shader paths required.

### Cross-chunk continuity

- AO sampled across boundaries.
- Biome tint interpolated per-vertex from world position, not per-chunk.
- Edge flags consistent across seams.

### Mesh disk cache

The mesher's output (`Vec<FaceVertex>` + index buffer per chunk) is cached to disk to amortize the cost of regeneration across sessions. The cache lives alongside the chunk persistence database.

**Cache key derivation** is the critical correctness concern. A cached mesh is valid only if every input that could affect it is unchanged. The key must capture:

- The chunk coordinate.
- A **graph hash** covering every graph that contributed to this chunk: the WorldGraph, the relevant ZoneGraph, the relevant BiomeGraph(s), the relevant DetailGraph, and any LibraryGraphs referenced. The hash is computed from canonical graph serialization (deterministic node ordering, deterministic parameter ordering).
- A **mesher version** hash bumped whenever the mesher's algorithm or vertex format changes.
- A **registry hash** of any data-driven inputs (material registry, palette set) the mesher reads.
- The world seed.

If any input changes, the cache key changes and the entry is treated as stale. On graph edits, affected chunks' cache entries become unreachable (not deleted; replaced on next regeneration).

**Failure mode to avoid:** keying only on chunk coordinate or only on a "params hash" that doesn't include the graph. The audit risk is that the cache silently serves stale meshes after worldgen edits. The key must capture everything the mesher's output depends on.

---

## 11. Isometric Pixel-Art Rendering Requirements

The renderer downscales high-internal-resolution output to a pixel-art target. The mesh provides the per-vertex data needed for stylized per-face decisions.

### Tri-tonal axis lighting

In isometric, exactly three cardinal axes are visible (top + two sides). Each gets its own palette ramp. The shader selects the ramp by `face_axis`, then samples from the ramp by `light_level_index`.

Because the engine has no diagonal geometry, every face belongs unambiguously to one of the three visible axes — there are no edge cases, no in-between orientations, no shader branches for diagonals. The tri-tonal look is consistent across every surface in the world.

This is the single most important aesthetic decision in the mesh layer. Smooth shading looks generic; quantized per-axis ramps look hand-drawn.

### Quantized lighting

- 5–8 discrete light levels, no float interpolation.
- Per-material light response (water tinted blue at low levels, lava self-illuminates, foliage warmer).
- Hard shadow boundaries from skylight; no soft falloff.

The lighting bake produces `light_level_index` as a discrete u8 per vertex.

### Outlines

Two layers compose:

1. **Screen-space edge detection** (depth + normal Sobel) for silhouette and large geometric edges.
2. **Mesh-baked edge flags** for material boundaries on coplanar surfaces (grass-to-dirt on flat ground, where normals don't change).

Both contribute to the final outline pass.

### Camera occlusion (cutaway / fade)

The biggest UX problem in isometric voxel games is camera occluding the player. Solution combines:

- **Per-face `is_player_facing` flag** computed at runtime: faces between camera and player participate in cutaway/fade.
- **Smooth fade-out** of player-facing walls.
- **Room detection** (flood-fill at runtime): when player is in an enclosed space, hide ceilings.

Mesh-level cost: a runtime-computed flag, not a baked attribute. No mesh storage change.

### Atmospheric layering

Depth cueing in isometric uses world-space Z, not perspective:

- **Per-Y atmospheric tinting**: shader samples world-space depth and tints toward background.
- **Distance desaturation**: far chunks lose saturation.
- **Fog volumes for caves**: driven by `enclosure_factor` baked per vertex.

### Pixel-perfect snapping

- Camera transform snaps to pixel boundaries.
- World units map cleanly to pixels at intended camera scale.
- Final downscale uses nearest-neighbor filtering.
- Internal sampling can be linear; only the final composition is nearest.

### Palette management

- Each material has a palette assignment.
- Palettes vary by biome (`biome_tint_index`) and time-of-day.
- The shader composes `material_id + biome_tint_index + light_level_index + face_axis` into a final palette index.

### Material variants

- Each material has 3–5 hand-authored visual variants.
- `variant_index` selected per-face by world-position hash at mesh time.
- Avoids tiling artifacts; preserves hand-drawn feel.

### LOD

- Near-field chunks render with full vertex data.
- Distant background chunks render as simplified meshes derived from heightmap + biome color, no per-voxel data.

### Player z-sorting

- All voxel geometry produces accurate depth buffer values.
- The player sprite z-tests against the depth buffer normally.
- Slab boundaries produce clean half-voxel depth steps that the sprite renderer can sort against without subpixel ambiguity.

---

## 12. Cross-Cutting Concerns

### Runtime architecture

The engine uses **`bevy_ecs`** as its runtime (an ECS library extracted from the Bevy engine, used without the rest of Bevy). The engine is not a Bevy app; it owns its own `winit` event loop and `wgpu` device and embeds `bevy_ecs` as the schedule + resource system.

Systems are scheduled through a **`FrameStage`** ordering:

```
FrameStage::Input        — gather window events, tick watchers, apply graph edits
FrameStage::Simulation   — game logic, AI, fluid simulation, time-of-day
FrameStage::Meshing      — feed dirty chunks to mesher worker pool
FrameStage::UniformWrite — upload per-frame GPU data
FrameStage::Render       — execute the render graph
FrameStage::PostFrame    — eviction, persistence flush, cleanup
```

This ordering is the engine's scheduling contract; new systems must declare which stage they run in. Resources (the chunk store, graph registry, material registry, render context, etc.) live as ECS resources accessed through system parameters.

The choice of `bevy_ecs` is an implementation detail of the runtime — it does not affect the data model, the graph hierarchy, or any other system described in this document. Any compatible ECS could substitute. The choice is documented here because every cross-cutting system (hot reload, streaming, persistence) integrates through the schedule and must know about `FrameStage` ordering.

### Chunk streaming

The world is effectively infinite; only chunks within a configurable radius of active observers (player camera, AI agents, debug cameras) are resident in memory.

- **Streaming radius** is configurable per observer; the union of all radii determines the resident set.
- **Chunk lifecycle**: out-of-radius chunks evict; in-radius chunks load (from persistence if they exist, otherwise generate via the pipeline; §5).
- **Eviction policy** is LRU within out-of-radius chunks, with a hysteresis band to avoid thrashing at the boundary.
- **Loaded chunks track their resident state** for systems that need to iterate the resident set (mesher, fluid simulation, AI pathfinding).
- **Background workers** handle chunk generation and meshing without blocking the main thread. The generation pipeline (§5) and the mesher both run on `rayon`-backed worker pools.

Streaming is foundational: every subsystem must tolerate chunks appearing and disappearing on a frame-by-frame basis. Code that assumes all chunks are resident is incorrect.

### Determinism

Mandatory across all systems. Specific rules:

- All RNG seeded from explicit context: `hash(world_seed, layer, node_id, world_pos, purpose)`.
- No use of chunk coordinates alone for RNG (breaks at fade boundaries).
- No f32 in persistent state where ordering matters (fluid mass is u16).
- No thread-order-dependent generation.
- Iteration over hash maps stabilized via sorted keys where output ordering matters.

### Save format versioning

- Every chunk layer has its own version field.
- Format header includes engine version.
- Missing layers default to empty (forward compatibility).
- Migration code path required for breaking version changes.

### Networking

If/when multiplayer:

- Server is authoritative for generation, fluid simulation, and override application.
- Clients receive chunk deltas, not full chunks.
- Determinism guarantees same input produces same output across machines.

### Hot reload

- File watcher (e.g., `notify` crate) on graph files.
- On change: parse, diff against running graph, walk `ChunkTags` to find affected chunks, queue regeneration.
- Editor in-engine edits use the same mutation path as disk edits.
- Both paths converge on the same authoritative graph state.

### Audio occlusion

- Reuses `enclosure_factor` baked into mesh vertices.
- Audio system samples at listener position to determine reverb amount.
- No additional data required.

### Debug visualization

The renderer supports debug modes that swap which mesh attribute drives final color:

- Material ID heatmap
- Biome ID heatmap
- Light level visualization
- Enclosure factor visualization
- Chunk boundary overlay
- AO factor visualization

These are toggleable in the engine's debug UI.

### Editor as engine panel

- The editor lives inside the engine binary.
- No separate executable, no separate window event loop, no separate renderer.
- The editor uses egui (or chosen UI framework) hosted by the engine.
- A graph-type picker selects which graph type is being edited.
- Edits flow through the same mutation API as hot-reload from disk.

---

## 13. Glossary

- **Anchor voxel**: the integer cell that owns a foliage instance for purposes of player interaction.
- **BiomeGraph**: graph type defining terrain density, material, fluid sources, and fade behavior for one biome.
- **ChunkTags**: per-chunk metadata identifying which Zone, Biomes, and Libraries contributed to it. Drives invalidation.
- **CSE cache**: chunk-scoped cache that ensures each node's output is computed at most once per chunk evaluation.
- **DetailGraph**: graph type defining foliage paint and scatter placement, distinct from terrain generation.
- **DetailLayer**: 2D paint map over a chunk's XZ footprint storing per-column foliage density.
- **DetailTexel**: a single cell in a DetailLayer: species, density, tint, flags.
- **Enclosure factor**: per-vertex value indicating how enclosed a position is by surrounding terrain. Used for fog and audio.
- **Face axis**: which of the 6 cardinal directions a face belongs to. Determines palette ramp. No diagonal values exist.
- **Fade factor**: 0–1 weight for blending between neighbor biomes at boundaries.
- **FluidCell**: a single cell's fluid state: type, mass, flags.
- **FluidFillMode**: chunk-level default for fluid presence. Enables ocean fast path.
- **FrameStage**: ordered schedule of per-frame work in the bevy_ecs runtime. Stages run in fixed order: Input, Simulation, Meshing, UniformWrite, Render, PostFrame.
- **Generated content**: chunk data reproducible from seed + graph. Not serialized in detail.
- **LibraryGraph**: reusable subgraph referenced by any other graph type.
- **MaterialRegistry**: data-driven registry mapping stable `MaterialId` values and human-readable names to material definitions. Loaded at startup; extensible by mods.
- **Mesh disk cache**: persistent cache of meshed chunks keyed by chunk coordinate, graph hash, mesher version, registry hash, and world seed.
- **Overrides**: player-authored edits serialized separately from generated content. Canonical in-memory shape is `ChunkOverrides`; persisted directly.
- **Pin type**: typed connection between graph nodes. Enforces compatibility at edit time.
- **ScatterInstance**: a single placed foliage/prop instance with anchor, sub-offset, rotation, prefab reference.
- **ScatterStore**: per-chunk collection of ScatterInstances indexed by type.
- **Sea level**: global Y from WorldGraph determining ocean fill.
- **Slab**: a half-height voxel shape. Either `SlabBottom` (lower half solid) or `SlabTop` (upper half solid). All slab faces are cardinal-axis-aligned.
- **Slab smoothing**: engine pass during worldgen that inserts slab steps at walkable cube-step transitions, controlled by traversal smoothing distance.
- **Streaming**: load/evict policy keeping only chunks within observer radius resident. Background workers handle generation and meshing.
- **Sub-voxel offset**: fractional position within an anchor voxel, packed as signed bytes.
- **Traversal smoothing distance**: per-biome worldgen parameter controlling how aggressively slab smoothing distributes steps across distance. Produces emergent gradual slopes without diagonal geometry.
- **Walkability mask**: per-chunk derived artifact identifying voxels the player can stand on. Computed once during worldgen; reused by slab smoothing, AI pathfinding, and player movement.
- **WorldGraph**: top-level singleton graph defining climate, zone selection, and global constants.
- **ZoneGraph**: graph type defining biome distribution, cave rules, structures, and rivers within a Zone.

---

## Document Maintenance

This document is the **baseline reference** for the engine's foundational data model and architecture. Changes to anything described here require explicit revision of this document. Implementation details (specific algorithms, library choices, code organization) may evolve freely; data shapes, pipeline stages, and architectural boundaries do not.

When new systems are designed (combat, AI, multiplayer, modding), they extend this document rather than replacing it. The systems described here are load-bearing; everything else grows on top.
