# Voxel Engine Foundation Design Document

**Status:** Baseline reference, v1.11
**Scope:** World generation, chunk data model, foliage, water, isometric pixel-art rendering
**Purpose:** Authoritative goalpost for engine architecture. Every system described here is foundational — implementations may be incremental, but the data model and architectural shape are settled. Where the current implementation diverges from a documented target, the target stays; the divergence is called out as interim shape with future migration.

**Revision history:**
- v1.0 — Initial baseline.
- v1.1 — Replaced 45° slope geometry with half-height slabs. Slopes are removed from the engine entirely. Added walkability mask and traversal smoothing distance as foundational concepts.
- v1.2 — Documented previously-implicit foundational systems: ECS runtime (bevy_ecs + FrameStage schedule), chunk persistence (rusqlite + zstd), mesh disk cache (graph-hash-keyed), and chunk streaming. Added material registry pattern. Clarified that `ChunkOverrides` is the canonical in-memory representation of player edits that the persistence layer serializes.
- v1.3 — Documented settled architectural additions from Phases 2 through 7: cross-graph dataflow (`GraphRef` + `GraphOutput`), named boundary pins for libraries, per-biome parameter sidecar, standard libraries as authored assets with `LibraryKernel` backing, and the manifest-driven world structure. Reframed traversal smoothing distance in §5 to reflect the coupled tensions Phase 6 surfaced (multi-distance smoothing requires terrain modification, foliage reordering, and cross-chunk generation infrastructure). Revised LOD strategy in §11 for the orthographic isometric camera model. Renamed the pin type `ScatterPoints` → `Positions` to match the general-mechanism naming pattern used throughout. Renamed `FluidProvider` → `FluidOutput` to match code. Added explicit interim-vs-target notes for three items where current implementation is scheduled to migrate: the mesh vertex format (current `TerrainVertex` → target `FaceVertex`), save format versioning (current wipe-on-bump → target per-layer versioning), and fluid persistence (current full-snapshot → target diff-with-tombstones).
- v1.4 — Clarified §9 to document the content-authoring-vs-play mode model the engine implements. Added an explicit "Modes: content authoring vs. play" subsection to §9 stating that authoring and play do not coexist: authoring-mode regeneration is authoritative and wipes overrides by design; play-mode graphs are read-only. Removed the "worldgen edits don't destroy player work" property from §9's rationale, which described a coexistence case that is not a design goal. Updated the Stable Instance IDs paragraph to distinguish play-mode chunk reload (applies tombstones and additions) from authoring-mode regeneration (wipes). Removed property #3 from the fluid persistence interim justification since it described the same coexistence case. Introduced the mutation command API as the enforcement mechanism for the mode separation; the API becomes real in Phase 8.
- v1.5 — Expanded §12 Networking with the settled decisions from the companion `networking-architecture-proposal.md`: authoritative-state model over lockstep, determinism-in-generation as the bandwidth story, per-chunk base-content hash for self-healing terrain divergence, single mutation door as the sole write entry point, server-only fluid simulation, fixed-timestep player sim as a shared pure function, snapshot interpolation for other entities, per-observer streaming generalizing from the current camera-driven model. Documented the phase sequencing that follows Phase 8: player character (networking-aware but standalone) → server-core crate extraction → observer-set streaming → loopback milestone → LAN and internet transport. The proposal itself remains a companion document labeled PROPOSAL until the networking arc completes.
- v1.6 — Reconciled §5 with Phase 9's settled outcome: the walkability mask no longer exists as a resident runtime artifact. Walkability is computed transiently inside worldgen for slab smoothing; runtime movement, collision, and room detection query geometry directly (`WorldView::solid_interval`); future AI pathfinding consumes the same geometric queries. Documented the cross-chunk seam finalization pass (`world::seam`, persisted `seam_finalized`) as part of slab smoothing. Promoted the underground visibility system (`underground-visibility-design.md`: air-side face rule, cost-priority march, clarity radius) to settled status in §11 camera occlusion. Added §12 subsections for engine versioning & release policy and performance budgets, both anchored by the companion `roadmap.md`, which is authoritative for version sequencing and compatibility boundaries. Named `roadmap.md` a companion document.
- v1.7 — Added §12 subsections establishing three previously-undocumented foundational commitments: **developer and authoring tooling as a first-class engine surface** (every authorable content type in-engine, previewable, hot-reloadable; maturity model and per-version targets in `roadmap.md` §4), **the game-facing API surface** (the boundary between engine capability and game rules, enumerated in `roadmap.md` §6), and **particles/VFX and non-player entity visuals** as acknowledged architectural gaps that must be designed before they are built. Corrected the performance-budget cross-reference to `roadmap.md` §7.3 following that document's renumbering, and restated 1.0 readiness as the roadmap's rubric rather than a single shipped title.
- v1.8 — Incorporated the settled conclusions of an external architecture review (`engine-architecture-reference.md`; full disposition in `roadmap.md` §21). Added two foundational principles to §1: **visibility is queryable world state, not a rendering effect** (occlusion decisions must live where the renderer, the interaction system, and the simulation all read the same classification), and **one scheduler, one clock family** (all deferred work on a single job system; all periodic simulation on one tick framework). Added §12 subsections for the **region graph** (enclosure/connectivity promoted from a per-frame rendering computation to persistent multi-consumer world state), the **gameplay/render lighting split** with render light sampled from a per-chunk 3D texture rather than baked into vertex attributes, the **tick and scheduling framework**, **registry identity and save-embedded name mapping** (recording a latent correctness defect in the current numeric-ID persistence), and **per-region generation parameters** as the storage precondition for world-scale change. Noted §10's `light_level_index` as an interim shape pending that lighting decision. Recorded the open architectural decisions the review surfaced — octant shape encoding, grid-handle abstraction, 4-way camera rotation, colored light — as dated decisions in `roadmap.md` §8 rather than settling them here.

- v1.9 — Folded in 0.3.0's settled outcomes. §1's "one scheduler" principle became real: a **job system** subsection in §12 records the single scheduler, its `(class, distance)` priority, its one global concurrency budget, and — with named triggers — the two things it deliberately omits (declared job-to-job dependencies, which have no consumer until the staged cross-chunk generation pass; chunk I/O *writes*, which are sub-millisecond and whose relocation would introduce a save/load ordering hazard). §4 gained the constraint that **tag matching is backward-looking**, which bounds the invalidation table: a ZoneGraph edit changes biome *assignment*, so tags predating it cannot identify the affected chunks. §4 and §12 record that **hot reload is save-triggered** and that disk is authoritative for the world while the editor is authoritative for its canvas — closing a defect where regeneration mixed an in-memory graph with on-disk siblings. §5 documents that stage 9's fluid reaches stage 10's foliage as a downstream filter at the storage boundary rather than a `SurfaceFilter` input, with the migration trigger. §10 records the mesh cache key as **content-addressed** (an interim shape stronger than the documented input-addressed target) and specifies that it must exclude the snapshot's edge and corner border cells. §12's performance budgets are restated **against CPU work rather than frame time**, with release-only measurement and read-at-rest as architectural requirements — learned from a "recurring frame hitch" that proved to be a debug build measured with a metric that inverts under vsync. §12 streaming records the **altitude-aware view test**. Recorded figures now live in the companion `perf-baseline.md`.

- v1.10 — Designed **cross-chunk feature generation** in §5 ahead of building it, per the 0.4.0 sequencing rule. Structures and anything else straddling chunk boundaries are derived **world-absolutely over a margin band** — a generalization of the shipped scatter model — rather than by a staged pipeline pass: every chunk that can see a feature re-derives it identically and stamps only the part inside its own window, so generation-order independence is a property of the construction rather than a claim defended afterward. Records priority resolution by a world-absolute key, the **protected-volume flag** region transformation must respect, memoization as an optimisation that must never change output, and the cost ceiling that follows from margin-band scaling. **Rivers are explicitly excluded from that model**: a river modifies the density field at stages 3–4 rather than stamping voxels at stage 8, which is what keeps it clear of the multi-distance-smoothing tensions §5 records. §12's job system records that the **predicted trigger for declared job dependencies did not arrive** — the feature model has no cross-chunk stage — and revises the trigger to the region graph, whose cross-border summary joins are a genuine job-to-job edge. §5's fourth smoothing tension is corrected to note that the feature model does not supply what cross-chunk staircases need, since those are a function of generated terrain on both sides rather than of world position.
- v1.11 — Designed **river networks** in §5 ahead of building them, per the 0.4.0 sequencing rule. A coarse world grid of hash-jittered nodes, each linking to the lowest-potential neighbour, forming a forest whose **acyclicity is structural** (every link strictly decreases potential). Flow direction comes from a dedicated **elevation-potential field** rather than the generated surface, because a river network - unlike a structure source, whose density roll gates its probes - has no gate and every chunk would otherwise pay a column probe per nearby node. The channel **floor is the network's, interpolated from node elevations**, not carved into the terrain, which avoids the same pointwise-surface cost; the stated consequence is that a river cannot follow terrain it did not shape. Width derives from potential rather than Strahler order, which would need an unbounded upstream traversal. The channel authors its own per-column pond level through the existing `FluidOutput` mechanism, so a river above `sea_level` holds water. The pass **resolves the network once per chunk** and tests distance per column, which is what bounds the cost; that memoization holds no cross-chunk state and so does not engage §5's rule that memoization must never change output.

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
- **Visibility is queryable world state, not a rendering effect** (added v1.8). Whenever the engine hides geometry — underground visibility, occlusion fades, any future slice mode — the decision lives in world state that the renderer, the interaction system, collision, and audio all read from the same place. If the renderer hides a ceiling but the raycaster still collides with it, players target blocks they cannot see and the game feels broken in ways they cannot articulate. This rules out occlusion implemented purely in shaders: a shader may *consume* the classification but may not *be* it.
- **One scheduler, one clock family** (added v1.8). All deferred and background work — generation, meshing, lighting, region detection, pathfinding, structures, chunk I/O, simulation — runs on a single job system with declared dependencies and priorities. All periodic simulation derives from one tick framework rather than per-system ad-hoc clocks. Two schedulers means two tuning surfaces that must agree by hand and never do.

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
- **Slabs are produced by the smoothing pass, not authored by players directly.** During worldgen, the slab smoothing pass converts cube-step boundaries into slab transitions where the walkability computation (§5) permits. Player tools place full cubes; designers can author slabs in prefabs explicitly.
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

### Interim shape: monolithic version + wipe-on-bump

The current implementation ships a single monolithic `BLOB_VERSION` byte per chunk blob plus a global `VOXEL_FORMAT_VERSION` marker whose bump wipes all saves and the mesh cache on next launch. This is a **pre-release interim policy**, not the target. The doc's per-layer versioning target above is intentionally preserved so that the following release-blocking property holds: at first release, saves survive engine upgrades via layer-scoped migrations that read older layer versions and write the current one.

The wipe policy is defensible while the engine is pre-release because save invalidation is cheap (no user commitment yet) and per-layer version machinery adds implementation overhead that provides no user-facing benefit before players accumulate save state. The migration to per-layer versioning becomes mandatory before any release that promises save-file compatibility across engine upgrades. Retrofitting per-layer versioning onto a monolithic blob is a format redesign; do it deliberately at the last planned `BLOB_VERSION` bump before that release, not discovered mid-release.

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
World (manifest: sea_level, seed, zone/biome/library registrations)
├── WorldGraph (climate channels, zone selector)
├── Zones (registered via manifest)
│   ├── ZoneGraph[Z1]
│   │   ├── BiomeGraph[B1]
│   │   ├── BiomeGraph[B2]
│   │   └── DetailGraph[B1], DetailGraph[B2]
│   └── ZoneGraph[Z2] ...
└── Libraries (reusable subgraphs, registered via manifest)
    ├── LibraryGraph["StandardCaveNoise"]
    ├── LibraryGraph["SurfaceLayering"]
    └── ...
```

The world's structure is materialized on disk as a manifest file (`world.manifest.json`) plus the individual graph and library files it references. See "Manifest-driven world structure" below.

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
    FluidOutput,            // fluid initialization terminal; "what fluid at this position?"
    // Selection types
    BiomeId,
    ZoneId,
    Curve,                  // editable spline f32 → f32
    // Placement types
    Positions,              // set of candidate positions (formerly named ScatterPoints;
                            //   renamed to describe the general mechanism, not the scenario)
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

### Cross-graph dataflow

Graphs in the hierarchy reference each other's outputs via a `GraphRef` node and a `GraphOutput` marker. This mechanism is what makes the hierarchy actually compose: a `ZoneGraph` reads its parent `WorldGraph`'s climate channels rather than re-deriving climate internally; a `BiomeGraph` reads whatever inputs it declares by name from its containing `ZoneGraph` and grandparent `WorldGraph`.

- **`GraphOutput { name, pin_type }`** — a terminal node placed inside the producing graph. Every named output a graph exposes to consumers is marked by a `GraphOutput`. A `WorldGraph` typically has one `GraphOutput` per climate channel plus one for `zone_id`. A `BiomeGraph` has one `GraphOutput` for terrain density and one for material.
- **`GraphRef { target, output_name }`** — a source node placed inside the consuming graph. It resolves `target` (a typed identifier like `GraphRefTarget::World` or `GraphRefTarget::Zone(id)`) against the manifest, then reads the named `GraphOutput` from that target graph at evaluation time.

Cycles in the graph-reference DAG are forbidden and caught at validation time. The manifest declares which graph is `WorldGraph`, which `ZoneGraph`s exist, and which `BiomeGraph`s each zone contains, so target resolution is a manifest lookup rather than a search.

Evaluation memoizes upstream outputs per chunk column (see §5 `ColumnCache`), so a `GraphRef` resolves once per column per graph reference regardless of how many nodes downstream read from it.

### Named boundary pins for libraries

A `LibraryGraph` declares its boundary — typed named inputs and typed named outputs — at authoring time. Every `LibraryRef` node that references the library exposes those declarations as its own pin descriptors dynamically, so the visual graph shape adapts to the library's boundary rather than being hardcoded on the referring node.

- **Library boundary declaration** — the library file's header specifies `{ inputs: [{ name, pin_type }, ...], outputs: [{ name, pin_type }, ...] }`. Boundary changes to a library are breaking changes to every graph that references it (a validation error surfaces at the referring `LibraryRef` node).
- **`LibraryRef` dynamic pins** — at graph load time, the referred library's boundary is resolved and the node's pin descriptor is computed. The resolved boundary is cached on the referring node's serialized params so that subsequent loads don't re-walk libraries; edits to the library re-cache during the library's own reload flow.

Cycles among library references are forbidden and caught at validation time. Cross-graph references (see above) and library references share cycle-detection infrastructure.

### Per-biome parameter sidecar

Biomes are graphs, but some biome-scoped values are not natural graph outputs — traversal smoothing distance is a single scalar that governs the slab-smoothing pass; fade radius is a boundary-blending constant; future parameters (fluid density, fog color, wind strength) will follow the same shape. These values are stored in a **typed scalar-metadata sidecar** per biome, addressable by name.

```rust
BiomeParams { entries: HashMap<String, Scalar> }
```

The biome manifest entry declares parameter values; the evaluator threads the biome's `BiomeParams` into any pass that consumes them (slab smoothing reads `traversal_smoothing_distance`; the material composition pass reads any material-domain params; etc.). A `BiomeParam(name)` node kind exposes parameters to `BiomeGraph` internal nodes, so authors can drive graph behavior from the sidecar without hardcoding constants.

Parameters are free-form (any biome can declare any parameter name) with documented defaults. Structured schemas are future work; they are not a blocker for adding new parameters.

### Standard libraries

The engine ships a set of standard libraries as authored assets under `assets/libraries/`:

- `StandardCaveNoise` — 3D Worley + ridged noise composite
- `SurfaceLayering` — depth-conditional material cake (grass/dirt/stone)
- `ExposureLayering` — surface layering varied by face exposure (top-facing vs side-facing surfaces get different materials/tints; useful for snow caps, exposed rock on cliff faces, moss on shaded sides)
- `BiomeBorderFade` — standard fade kernel used by biomes at their edges
- `PoissonPlacement` — point distribution for scatter

Standard libraries follow the same authoring format as any other library. They are **backed by `LibraryKernel`** — an enum matching each standard library's name to a native implementation. The authored asset captures the library's boundary declaration and identity; the actual computation runs in native code. This is the pragmatic starting shape: libraries participate in the graph editor, the reference/cycle-detection system, and the invalidation table, without paying the cost of full graph-body library implementation for functions that biomes don't currently customize.

When authors want to write a library body from scratch (rather than customize a native kernel's parameters), the same file format extends to hold a graph body instead of a kernel reference. That extension is a compatible addition, not a rewrite.

### Manifest-driven world structure

A world is defined on disk by a **manifest file** (`world.manifest.json`) that names:

- `sea_level` — the global scalar Y at which oceans fill (see §7). This lives on the manifest rather than as a `WorldGraph` output because a single scalar constant across the world is not natural graph data, and threading it through as a graph output would introduce a `GraphRef` in every biome without providing any biome variation.
- `seed` — the world seed.
- `world` — the path to the `WorldGraph` asset (`.graph.json`).
- `zones` — a list of `ZoneGraph` asset references, each with an identity (`ZoneId`) and a path.
- `biomes` — a list of `BiomeGraph` asset references, each with an identity (`BiomeId`), a path, an optional `detail` path pointing to its `DetailGraph`, and an optional `params` block (the per-biome parameter sidecar).
- `libraries` — a list of `LibraryGraph` asset references, each with an identity (`LibraryGraphId`) and a path.

The manifest is the resolution root: `GraphRef` targets and `LibraryRef` targets resolve against it. Editing the manifest invalidates the entire loaded world (structural change); editing individual graphs invalidates per the table below.

### Edit invalidation table

Edits to different graph types invalidate different chunk sets. This is the primary mechanism for fast iteration.

| Edit target | Invalidates |
|-------------|-------------|
| `Manifest` (structural change: `sea_level`, seed, graph registrations) | All loaded chunks |
| `WorldGraph` (climate, zone selector) | All loaded chunks |
| `ZoneGraph[Z]` | All chunks tagged with Zone Z or within Z's fade range |
| `BiomeGraph[B].density` | All chunks tagged with Biome B + fade buffer |
| `BiomeGraph[B].material` | Material-only re-pass; density cache kept |
| `BiomeGraph[B].fluid_output` | Fluid initialization re-pass |
| `BiomeGraph[B].params` (sidecar values) | Depends on the parameter; `traversal_smoothing_distance` triggers a slab-smoothing re-pass on B's chunks |
| `DetailGraph[D]` | Detail-only re-pass; voxel data untouched |
| `LibraryGraph[L]` | Every graph importing L applies its own rules |

Each chunk's `ChunkTags` makes invalidation a tag-set lookup, not a full-world scan.

**Tag matching is backward-looking, and that bounds the table above** (added v1.9). Tags describe the generation that already ran, so matching on them is sound only for edits that cannot change what the tags would become:

- A **BiomeGraph** or **DetailGraph** edit changes how an already-assigned biome *looks*. A chunk tagged with that biome still will be, so narrow matching is correct.
- A **ZoneGraph** edit changes which biome each column *is assigned*. The chunks needing regeneration are exactly those whose assignment changes — and their current tags describe the assignment being replaced. The row above ("chunks tagged with Zone Z or within Z's fade range") is therefore **only reachable once multiple Zone graphs exist and zone assignment is itself stable across the edit**; with a single Zone graph governing the whole world, a Zone edit invalidates every loaded chunk.
- A **WorldGraph** edit changes climate, hence zone assignment, hence everything — already "all loaded chunks".

Ignoring this is not a subtle failure: moving a biome band boundary leaves every reassigned chunk untouched, because each is still tagged with the biome it is leaving.

### Hot reload is save-triggered (added v1.9)

Worldgen regeneration is driven by **a graph file changing on disk**, and by nothing else. The in-engine editor's Save writes the file; an external text editor writes the same file; both take the identical path, and regeneration always reloads the whole hierarchy from the manifest.

This is what makes P2's "the manifest is the resolution root" true in practice. Regenerating from an edited graph held in memory while its siblings came from disk produced a world matching neither — and it degraded worst for the higher-tier graphs, where a ZoneGraph's `GraphRef(WorldGraph)` resolved against the on-disk World while the editor held unsaved World edits. Loading the whole hierarchy from one source makes that state unrepresentable rather than merely avoided.

The cost is that the authoring round trip is edit → save → see result rather than edit → see result. P11's bar is that an author need not restart the engine, hand-edit a file, or guess at a result; a save keystroke is none of those.

---

## 5. Generation Pipeline

Chunk generation runs in **strict pipeline order**. Each stage reads from previous stage outputs and the source of truth graphs; no stage may reorder.

### Stage order

```
1. WorldGraph evaluation
   - Per-column: climate vector (temp, humidity, continentalness, erosion, weirdness)
   - Per-column: zone_id + zone_border_distance
   - (`sea_level` is read from the manifest and made available as a `Scalar` input to any graph that needs it via `GraphRef`; it is not a graph output.)

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

6. Walkability computation (engine pass, transient)
   - Per-voxel boolean: "the player can stand on top of this voxel"
   - A voxel is walkable if it is solid, the voxel above it is empty, and the voxel two above is empty (headroom).
   - Computed transiently as an input to Stage 7 and discarded. It is NOT a resident
     runtime artifact (revised v1.6; see "Walkability" below).

7. Slab smoothing (engine pass, not graph-driven)
   - Reads the transient walkability computation from Stage 6.
   - For each walkable voxel adjacent to a walkable neighbor at a different height, inserts slab steps to halve the height transition.
   - Cliff faces and non-walkable surfaces are not smoothed; they remain sharp cube-stepped.
   - Smoothing distance is a per-biome parameter (see "Traversal smoothing distance" below).
   - Authored slabs in prefabs are respected and treated as fixed.
   - Chunk-boundary transitions are resolved by a cross-chunk **seam finalization pass**
     (`world::seam`): demotions at chunk borders are finalized once neighbor data exists,
     recorded through the override/diff mechanism, and marked by a persisted
     `seam_finalized` flag so reloads neither re-derive nor revert them (this is also
     what protects player edits near borders from being overwritten by seam re-derivation).

8. ZoneGraph structures
   - Features derived world-absolutely over a margin band; each chunk stamps the
     part inside its own window, with priority resolution (see "Cross-chunk
     feature generation" below). No neighbour output is read, so placement is
     order-independent by construction.
   - Rivers are *not* here: they modify density at stages 3–4, before material,
     walkability and smoothing. Same subsection.

9. Fluid initialization
   - All empty voxels ≤ sea_level → ocean fill mode
   - BiomeGraph.fluid_output outputs → settled fluid cells
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

**How stage 9 reaches stage 10** (added v1.9). Fluid initializing before foliage exists so foliage can respect submersion, but the coupling is a *downstream filter*, not an input to the foliage evaluation. `SurfaceFilter` and scatter placement remain fluid-blind; the finished fluid field is applied at the eval→storage crossing, where paint texels and scatter instances whose surface the field submerges are dropped.

This is deliberate — it keeps the evaluator independent of a storage-domain layer — but it is strictly weaker than a graph-level input: a downstream filter can only *suppress* foliage, never *select* different foliage. **Migration trigger:** the first DetailGraph that wants to place aquatic species where a column is submerged, rather than place nothing, forces submersion to become a real `SurfaceFilter` input.

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

### Cross-chunk feature generation (added v1.10)

Structures, and anything else whose footprint straddles chunk boundaries, are generated by **world-absolute derivation with margin-band ownership** — a generalization of the scatter model the engine already ships (§6, `PROP_MARGIN`), not a new pipeline phase.

**The mechanism.**

- A **feature** carries a world-absolute anchor, a bounded extent, a priority key, and a payload (a blueprint reference; later, other placeable kinds).
- Features are derived per **feature cell** — a coarse world-space grid — from `hash(world_seed, cell_x, cell_z, purpose)`, exactly as `world_cell_seed` derives scatter. A feature cell's contents are a pure function of world position and the seed.
- A chunk being generated enumerates the feature cells whose extent can reach its window, collects the candidates, orders them by priority key, and **stamps only the part of each that falls inside its own window**.
- **No chunk reads another chunk's output.** Every chunk that can see a feature re-derives it identically. Generation-order independence is therefore a property of the construction, not a claim to be defended after the fact — the same reason `JitteredGrid` is seam-continuous while `PoissonDisk` is not.

**Why not a staged pipeline pass.** The obvious alternative — per-chunk evaluation, then a cross-chunk pass over a staging set, then finalization — requires the participating chunks to be co-resident, and reintroduces exactly the order sensitivity P1 forbids, to be argued away case by case afterward. Margin-band derivation has neither problem and has a working precedent in the engine today. It also means there is no cross-chunk *stage*, and therefore no job-to-job dependency edge (see §12, the job system).

**Priority resolution.** Overlapping features are ordered by a key derived world-absolutely from the feature's identity, and applied in that order. Because the key does not depend on which chunk is asking, every chunk resolves an overlap the same way. A feature may mark the cells it occupies as a **protected volume**, which later features do not overwrite and which region transformation (§12) must respect — the mechanism that lets an authored vault survive a self-modifying world.

**Memoization is an optimisation, never a correctness mechanism.** Per-cell derivation may be cached so that the many chunks sharing a cell derive it once. Dropping the cache must change speed and nothing else. Stating this explicitly is deliberate: a cache whose presence changes output is how a pure function becomes load-order dependent, which §10's mesh-cache key learned the hard way.

**The cost ceiling, and what falls outside it.** Per-chunk work scales with the number of feature cells in the margin band, i.e. `((extent + chunk_dim) / cell_dim)²`. This is comfortable for features spanning a few chunks and does not scale to a feature spanning hundreds. **Rivers are therefore not features.** A river is a modification to the *density field*, not a stamped template, and it belongs to the density stages (3–4) rather than to stage 8: carving before material (5), walkability (6) and slab smoothing (7) means every downstream stage sees the carved surface. That ordering is what keeps rivers clear of the multi-distance-smoothing tensions recorded below — in particular that scatter anchors to pre-smoothing surfaces, so a channel lowered after placement would leave props hanging over it. A river network is derived per column from a coarse node graph and sampled pointwise like any other density source.

### River networks (added v1.11)

Rivers are **not features** (see above): the margin-band cost ceiling does not survive a feature spanning hundreds of chunks. A river is a modification to the *density field* at stages 3-4, carved before material (5), walkability (6) and slab smoothing (7) so every downstream stage sees the carved surface.

**The network.** A coarse world grid, one **node** per cell at a hash-jittered position. Each node samples an **elevation potential** - a dedicated low-frequency 2D field, not the terrain surface - and links to whichever of its eight neighbouring cells' nodes has the lowest potential. A node with no lower neighbour is a terminus. The links form a forest of paths, and **acyclicity is structural**: every link strictly decreases potential, so a cycle would require a node lower than itself.

**Why a potential field rather than the terrain surface.** Flowing downhill on the actual generated surface is the honest ideal and is unaffordable at stage 3. A structure source pays a column probe only for cells that survive its density roll, so a chunk typically probes none; a river network has no such gate and every chunk would pay a probe per nearby node, every time. A potential sample costs what `SurfaceNoise` costs. The discrepancy this introduces - a river's downhill is the potential's downhill, not the terrain's - is closed by *content* rather than by luck: author the world graph's elevation channel from the same potential and the two agree by construction.

**The bed is the network's, not the terrain's.** Carving a channel "into the surface" would reintroduce the pointwise-surface cost the potential field exists to avoid. Instead the channel **floor** is interpolated along a segment from its two endpoint node elevations, and density is subtracted above that floor within the width profile. The river is self-consistent: its bed is where the network says it is. The consequence is worth stating plainly - **a river cannot follow terrain it did not shape** - which reads as rivers cutting through hills rather than winding around them. That suits a stylized voxel world; it would not suit a realistic one.

**Width** derives from a node's potential rather than from a Strahler order, because computing stream order requires an unbounded upstream traversal. Lower potential means further downstream means wider, monotonically, at bounded cost and with no traversal at all.

**Water.** The channel authors a per-column pond level from the same interpolated elevation the floor came from, through the existing `FluidOutput` mechanism. Stage 9's ocean fill handles everything at or below `sea_level`; a river above it holds water because it authored a level, not because the ocean reached it.

**Cost, and the shape that makes it bounded.** Resolving a cell's segment requires that cell's node plus the nine potential samples that pick its downstream link. Doing that per column would be ~81 samples per column and is not affordable. The pass therefore **resolves the network once per chunk** - a 32-voxel chunk touches at most a 4x4 block of coarse cells - and then each column tests distance against the handful of resolved segments near it. That is roughly 150 potential samples and 9 distance tests per column, per chunk.

This per-chunk resolution is memoization *within a single chunk's derivation*: it holds no cross-chunk state, so it does not engage the rule that memoization must never change output. Every chunk resolves the cells it touches identically because the network is a pure function of world position and seed - the same construction that makes feature derivation order-independent.

**Not in 0.4.0:** meanders, deltas, waterfalls, erosion, variable flow, and any coupling to the fluid simulation. Each is a separate decision; none is blocked by this model.

### Walkability (revised v1.6)

Walkability — "the player can stand on top of this voxel" — is a **worldgen-internal, transient computation**, not a resident per-chunk artifact. Earlier revisions specified a persistent `WalkabilityMask` shared by slab smoothing, AI pathfinding, and player movement. Phase 9 implemented that shape and then removed it: the resident mask produced a chunk-Y seam defect, a streaming performance regression, and a poor fit for continuous collision (tunneling at speed, drift). The settled model:

A voxel is walkable if:
- It is solid (Cube, SlabBottom, or SlabTop).
- The voxel directly above is empty.
- The voxel two above is empty (headroom for the player).
- Its top surface is horizontal (Cube top face or SlabBottom top face at the midline). SlabTop voxels are not walkable on their top face because their "top" is at the cell ceiling with no headroom; they are walkable from beneath when used as overhang flooring.

Consumers divide by when they run:
- **Slab smoothing (Stage 7)** — computes walkability transiently during generation, uses it to decide which boundaries to smooth, and discards it.
- **Player movement and collision** — query geometry directly at runtime via `WorldView::solid_interval` (slab-aware solid-interval queries). Auto-step validity derives from the same queries.
- **Room detection / underground visibility** — the same direct geometric queries.
- **AI pathfinding (future)** — consumes the geometric query layer. If profiling ever justifies a cached navigation artifact, it will be a purpose-built nav structure designed then — not a revival of the generation-time mask, whose invalidation-on-edit and residency costs were the reason it was removed.

### Traversal smoothing distance

A per-biome worldgen parameter controlling whether the slab smoothing pass converts cube-step transitions into slab half-steps.

| Value | Behavior | Visual result |
|-------|----------|---------------|
| 0 | No smoothing | Sharp cube steps; terraced terrain |
| ≥ 1 | Smooth single-voxel transitions | Half-step inserted at 1-cube height changes |

The parameter influences whether the smoother demotes cube voxels adjacent to walkable-height transitions into `SlabBottom` voxels. A meadow biome might use `1` (softened). A canyon biome might use `0` (sharp, terraced). A flat plains biome uses `0` (no smoothing needed; terrain is already level).

**Multi-distance smoothing — target reserved for a future revision.** Earlier revisions of this section anticipated that values above 1 would distribute half-steps across multi-cube height differences ("long landings between half-steps; reads as gradual slope"). Implementation surfaced four coupled tensions that make the naive distributive approach unworkable:

1. **Correct multi-distance smoothing is terrain morphology, not step insertion.** Producing gradual slopes over distances greater than one cube requires lowering surfaces (min-cone erosion or similar), which contradicts the engine's "cliff faces stay sharp" principle from §1 unless erosion is scoped precisely enough to skip cliffs — a per-region decision the smoothing pass alone can't make.
2. **Bounded radius produces mid-slope artifacts.** A distance-N pass applied to a height difference of N+K cubes produces an unsmoothed segment somewhere on the slope. There is no non-arbitrary place to put it.
3. **Scatter placement runs before smoothing.** Foliage instances (§6) anchor to pre-smoothing surface positions. Multi-distance smoothing that actually lowers the surface would leave props floating above the new surface. Fixing this requires reordering scatter after smoothing, which couples the two systems tightly.
4. **Cross-chunk seam smoothing.** Multi-cube staircases that straddle chunk boundaries need neighbour context that per-chunk generation does not have. Note that the cross-chunk feature model above does *not* supply it: that model works because a feature is derived world-absolutely and never reads a neighbour's output, whereas a staircase spanning a border is a function of the *generated terrain* on both sides. The existing seam finalization pass (§5 stage 7) is the shape that fits — residency-gated, idempotent, order-independent — and multi-distance smoothing would extend it rather than the feature model.

Multi-distance smoothing is a design decision that couples across four subsystems and depends on infrastructure the engine doesn't yet have. It is reserved as future work; the parameter type and per-biome sourcing infrastructure are in place so that when the four tensions are resolved, the behavior can activate without renaming or re-plumbing.

The **half-step-only behavior at value ≥ 1** produces the current stable "softened terraces" visual style. The "feeling of gradual slopes" over multi-cube distances is the aspiration; it does not currently ship.

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

- **`sea_level`** (manifest constant) — defines ocean fill. Every empty voxel at or below this global Y receives water at generation time; chunks entirely at or below sea level use the O(1) `FluidFillMode::Submerged(WATER)` fast path. Lives on the manifest (see §4) rather than as a `WorldGraph` output because it is a single scalar constant across the world with no biome variation, and threading it through as a graph output would introduce a `GraphRef` in every biome for no gain.
- **`BiomeGraph.fluid_output`** — a biome's terminal for water bodies. Produces the biome's contribution to `FluidLayer` at generation time; used for authored lakes, ponds, and biome-specific water sources. `fluid_output` is a **terminal name** ("this is the biome's fluid output"), not a **role name** ("this thing provides fluids") — the same reconciliation pattern as `Positions` in §4.
- **`ZoneGraph.rivers`** — defines cross-biome water channels. Rivers are structurally coupled to terrain modification (carved riverbed) and cross-chunk generation infrastructure; both are future work.

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

- **Save files stay small**: untouched chunks store no overrides. Persistent state is the delta from graph output, not a snapshot of the world.
- **The graph remains the source of truth** for the default world. A fresh world starts from graph output alone; every player edit persists as a delta from that source.
- **Player edits are represented uniformly** across voxels, scatter, detail, fluid, and decals via `ChunkOverrides`.

### Modes: content authoring vs. play

The engine operates in one of two modes at any given time. The two do not coexist:

- **Content authoring mode.** The developer edits graphs in the integrated editor. Each graph edit invalidates affected chunks per the invalidation table in §4; regeneration is authoritative and produces a fresh world from the current graph. Any player-shaped state present in memory or on disk (voxel overrides, scatter tombstones, fluid diffs) is a stale artifact of a previous run and is intentionally cleared by regeneration. There are no players to protect from graph iteration.
- **Play mode.** The graph is fixed. Players make edits (place/remove voxels, scatter, water); each edit lands in the appropriate `ChunkOverrides` layer and persists. Regeneration does not run; graphs are read-only at runtime. A chunk unloading and reloading re-runs generation from the (fixed) graph and re-applies persisted overrides on top.

The `ChunkOverrides` diff model exists to serve play mode's persistence needs and to keep save files small, not to reconcile graph edits against player state at runtime. **A general-purpose "editing graphs during play preserves player edits" property is not a design goal** — the engine does not implement the reconciliation (subtracting removed-generated from newly-generated, remapping stable IDs across seed changes, resolving material-change conflicts on player-edited voxels, etc.) that such coexistence would require.

The mode separation is enforced by the **mutation command API**: every world-mutation call site (editor graph edit, player voxel edit, player fluid disturbance, streaming override apply) declares its origin — dev-authoring or play-time — and the runtime rejects the combination that would produce coexistence rather than silently entering it.

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

Generated scatter instances need IDs that survive chunk reload in play mode:

```
StableInstanceId = hash(world_seed, world_pos, prefab_id, sequence_in_anchor)
```

**In play mode**, when a chunk unloads and reloads, the engine regenerates from the (fixed) graph, subtracts `scatter_removed` from the freshly-generated set, and unions `scatter_added`. Because IDs are deterministic from world position + seed, tombstones stay valid across the unload/reload cycle.

**In authoring mode**, graph-edit regeneration is authoritative and any `scatter_removed`/`scatter_added` state is cleared alongside the rest of the chunk overrides.

### Interim shape: fluid persistence full-snapshot

The current implementation serializes `fluid_diffs` as a full snapshot of the chunk's live fluid field at save time (including generated ocean cells on sea-level-straddling chunks), and the loader overlays that snapshot on top of freshly-generated fluid at load time. This is an **interim shape**, not the target. The doc's diff-against-generated model above is intentionally preserved so that the following properties hold at the target:

1. **Save files stay small** — the full-field snapshot serializes hundreds of untouched generated cells per straddle chunk; the target diff serializes only genuine deviations.
2. **Player-drained water stays drained** across chunk unload/reload in play mode — the target diff includes tombstones for removed generated cells; the interim overlay has no tombstone concept and cannot represent removal, so drained water re-derives on load.

The migration to a diff-with-tombstones shape lands when player-water interaction becomes a real concern, or when save-file size on ocean-heavy worlds becomes a problem — whichever comes first. The wire format extension is compatible: adding a tombstone list to the existing `fluid_diffs` structure doesn't require a `BLOB_VERSION` bump (once per-layer versioning lands per §3).

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
    pub light_level_index: u8,      // 1 byte — quantized; INTERIM, superseded by
                                    //   3D-texture light sampling (§12 lighting split, v1.8)
    pub enclosure_factor: u8,       // 1 byte — for fog and audio occlusion
    pub edge_flag: u8,              // 1 byte — material boundary for outline shader
    pub sway_weight: u8,            // 1 byte — wind animation (0 for terrain)
    pub ao_factor: u8,              // 1 byte — baked corner AO
    pub _padding: u8,
}
// ~32 bytes per vertex
```

### Interim shape: `TerrainVertex` with baked color

The current implementation ships a **64-byte `TerrainVertex`** with baked color per vertex rather than the 32-byte `FaceVertex` above. Registry color is resolved and written into the vertex at mesh time; the shader reads color directly. This is an interim shape, not the target. The `FaceVertex` target above is intentionally preserved so that the following capabilities can land as a single format migration rather than being paid for incrementally:

- **Palette / time-of-day shifts** without re-meshing the world (target: shader composes color from `material_id` + `occlusion_class` + `biome_tint_index` at draw time).
- **Biome tint** carried per-vertex (target: `biome_tint_index` byte).
- **Enclosure factor** for cave fog (§11) and audio occlusion (§12) that the doc-stated "no additional data required" premise depends on.
- **Edge-flag outlines** at material boundaries.
- **Quantized light levels** as a per-vertex byte.
- **Debug attribute views** that swap which vertex byte drives color.
- **Sway weight** for foliage (already relevant for §6 Tier 3 rendering).
- Roughly 2× smaller mesh data on disk and in GPU memory.

The migration to `FaceVertex` is scheduled as its own substep before any of the above features are built. Doing it incrementally would break the mesh disk cache multiple times; doing it once means one migration and all subsequent §11 features land against a stable format. The `CACHE_VERSION` machinery in the mesh cache already handles the invalidation.

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

**Interim shape: the key is content-addressed, not input-addressed** (added v1.9). The implementation hashes the chunk's actual voxel content rather than the graph/mesher/registry/seed inputs above. This is *stronger* for correctness — the failure mode this section warns about cannot occur, because a stale mesh would have to hash to the content it no longer matches — at the cost of a weaker hit rate, since two chunks with identical content still key separately.

What the key hashes is load-bearing and was got wrong once: it must cover the chunk interior **plus the six face-adjacent border planes, and must exclude the border's edges and corners**. The mesher culls against face-adjacent voxels only and never reads a diagonal, while meshing waits on exactly the six face neighbours being resident — so including edge and corner cells made the key depend on whether unrelated diagonal neighbours happened to be loaded, which varies run to run. Measured before the exclusion: 1,400 of 4,758 lookups on a warm second run over an identical route were re-keys, each rebuilding a mesh already on disk and deleting the file it replaced. After: zero.

Convergence on the input-addressed key is unscheduled; it would raise hit rate but forfeit the correctness property, and is worth doing only alongside a measured need.

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

The biggest UX problem in isometric voxel games is camera occluding the player. The settled solution (v1.6; implemented in Phase 9, designed in the companion `underground-visibility-design.md`, which is authoritative for the details) is structural rather than heuristic:

- **Air-side face rule**: every terrain face fronts a well-defined air cell; a face renders only if the player has flood-reached that air. "No surface leakage" is a property of the classification, not a rule to remember.
- **Cost-priority march**: the visibility flood stores flood-cost per cell; a face discards if reached air closer to the player sits behind it along the view direction — culling nearer chambers that would otherwise obscure the player's space.
- **Clarity radius**: bounds what renders in full color versus what the flood merely knows about, with screen-space exclusion circles for the void presentation.
- **Room detection** via bounded 6-connected flood yielding a continuous undergroundness `u ∈ [0,1]` (threshold-based, not binary — binary flood-escape rejects any room with a doorway).
- **Above-ground occlusion** uses a two-pass player stencil (pre-foliage mark, post-foliage composite) against the `Depth24PlusStencil8` scene target.

Mesh-level cost: runtime-computed classification, not a baked attribute. No mesh storage change. Deferred polish (fringe/dim clarity state, cave-mouth `u`-blending) is scheduled in `roadmap.md`.

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

The engine uses an **orthographic isometric camera at pixel-art resolution** (roughly 16 px per world voxel). This flattens the near/far distinction that perspective LOD strategies depend on: a voxel at the back of the visible region occupies the same pixel footprint as a voxel at the front, and a mesh simplification pass that reduces vertex count without changing on-screen pixels is not obviously a win.

The consequence: **distance-based LOD as described in perspective rendering contexts does not straightforwardly apply.** LOD in this engine takes different shapes:

- **View-radius culling** is real and load-bearing. Chunks outside the streaming radius do not render at all; the streaming system loads and unloads based on camera position. The unload boundary provides hysteresis against churn (see §12).
- **Zoom-level LOD** (if zoom-out capability is added) becomes a real LOD axis: at a distant zoom, individual voxels compress into fewer pixels and simplification becomes visually justified. This is the axis on which "simplified distant meshes from heightmap + biome color" would apply — the assumption is *rendering scale*, not *view distance*.
- **Per-chunk mesh detail** stays uniform within the streaming radius at a given zoom level. A distant chunk at the edge of the streaming radius renders with the same vertex data as a nearby chunk.
- **Foliage LOD** follows the same shape: Tier 3 billboards deferred at the current zoom level; if zoom-out is added, billboards activate below a threshold pixel size.

**What this replaces:** the earlier revision of this section described billboarding of distant tree instances and simplified heightmap meshes for far chunks as if they were driven by distance from a perspective viewer. The engine's camera does not produce that distance gradient. When zoom levels or additional camera modes land, this section revises to describe LOD as a scale-driven strategy across zoom bands.

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
- **Background workers** handle chunk generation and meshing without blocking the main thread. Both submit to the single job system below (revised v1.9); neither owns a pool or spawns threads of its own.

Streaming is foundational: every subsystem must tolerate chunks appearing and disappearing on a frame-by-frame basis. Code that assumes all chunks are resident is incorrect.

**The view test is altitude-aware** (added v1.9). Under the isometric projection a ground displacement `d` along the away-axis shifts a point up-screen by `d·sin(pitch)`, while an altitude `Δy` shifts it by `Δy·cos(pitch)` — so altitude is screen-equivalent to `Δy/tan(pitch)` of ground. A residency test comparing a column's *footprint* against the view band therefore misses columns whose footprint lies outside it but whose raised geometry is plainly on screen. The test compares the column's projected **span**. It is column-granular by necessity: a per-chunk test would be tighter, but omitting a middle Y layer leaves the layer above it permanently unmeshable, since meshing requires both Y neighbours resident.

### The job system (added v1.9)

The realization of §1's "one scheduler" principle. One scheduler owns *when* and *how many* for every continuous background consumer — generation and meshing today; lighting, region detection, pathfinding and structures later — and deliberately does **not** own *what comes back*: each consumer keeps its own result channel, so adding a consumer never widens the scheduler's knowledge of payload types.

- **Priority is `(class, distance²-to-observer)`.** Class dominates, so an interactive job — currently an edit-driven re-mesh — preempts bulk streaming regardless of where the camera is. Within a class, nearest first.
- **Concurrency is one global budget**, not a per-kind partition. Static partitioning capped generation at a fraction of the pool while the remainder sat reserved for meshing that was usually idle. Per-kind *submission* limits remain, but they are backpressure — bounding how much outstanding work a consumer may hold — not concurrency.
- **Jobs are introspectable**: queue depth, in-flight count, completion count and per-kind timing are recorded by the jobs themselves through shared atomics, which is what makes a scheduler inspector possible at all. Closures on a bare pool cannot be inspected.

**What it deliberately is not, with triggers:**

- **No declared job-to-job dependencies.** The edge everyone reaches for — "a chunk may not mesh until its six face neighbours exist" — is a *world-state readiness predicate*, not a job edge: a neighbour can be resident without any generation job having run this session.

  **The predicted trigger did not arrive** (revised v1.10). v1.9 named "the staged cross-chunk generation pass that structures and rivers require" as the first genuine edge. §5's cross-chunk feature model has no such stage: features are derived world-absolutely and stamped per chunk, and rivers modify density in place, so every chunk remains a pure function of `(seed, graphs, coords)` with nothing to wait on. Building the machinery anyway would be designing against an imagined consumer, which is what the original omission avoided.

  **Revised trigger: the region graph.** Per-chunk connectivity summaries joined across chunk borders is a genuine "B consumes A's output" edge rather than a readiness predicate, and it is the first one on the ladder. Until then the omission stands.
- **Chunk I/O writes remain on the main thread.** Reads already run inside generation jobs. Writes measure well under a millisecond per chunk, and moving them would require the database behind a shared handle plus a per-chunk ordering rule to stop a reload reading a record whose save has not landed — a correctness hazard introduced in order to move work that is not on any hot path. Trigger: per-layer save versioning, or any measurement showing writes above ~1 ms.

### Determinism

Mandatory across all systems. Specific rules:

- Determinism is stated over the full generation input set: same seed + same graphs + **same per-region parameters** + same coordinates → bit-identical output (revised v1.8; see "Per-region generation parameters" below).
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

A detailed networking architecture proposal exists as a companion document (`networking-architecture-proposal.md`), authored ahead of implementation so player-facing systems can be built against the shapes networking will require. That proposal remains labeled PROPOSAL until the networking arc completes; the settled decisions summarized below become authoritative for player-phase and networking-phase planning:

- **Authoritative-state model**, not lockstep. The server owns the simulation; clients render replicated state and predict only their own avatar. Divergence is a bounded rendering artifact, not a fatal desync.
- **Determinism earns its keep in generation, not simulation.** Clients generate terrain locally from `(seed, graphs)`; the server ships only deviations (`ChunkOverrides` + fluid diff). The generated/authored split (§9) is already wire-shaped; the persistence blob encoders (§3) are the wire encoding for chunk payloads.
- **Per-chunk base-content hash** on subscribe; mismatch triggers authoritative-fetch fallback. Cross-machine generation divergence (SIMD variants, compiler drift) is self-healing, not fatal.
- **Single mutation door.** The mutation command API (Phase 8) is the sole entry point for world writes; server-side, it validates and broadcasts authoritative deltas; client-side, it applies predictively with server-corrected reconciliation.
- **Fluid stays server-only.** Clients render received fluid deltas and interpolate cosmetically. Client-side fluid simulation is not built.
- **Fixed-timestep player sim as a pure function** `step(state, input, &world) -> state` in shared server-core code. Both client and server run the identical function; this is the one place client and server share simulation.
- **Snapshot interpolation for other entities** at ~100–150 ms behind real time. No extrapolation — for a builder game, late is better than wrong.
- **Per-observer streaming.** `ChunkStreamingManager` generalizes from camera-driven to per-observer subscription sets; the local player is observer 0.

**Sequencing** (planned across phases 9 through 13+):

1. Player character built networking-aware but standalone. Prerequisites (mutation door, fluid diff/tombstones, fixed-timestep player sim, semantic action input layer) land alongside it.
2. Server-core crate extraction — a compiler-enforced client/server boundary. Pure refactor.
3. Observer-set streaming — `ChunkStreamingManager` generalizes.
4. Loopback milestone — client and server as separate processes on one machine over in-memory channel. This is where 90% of protocol issues surface.
5. LAN transport, artificial latency/loss testing, internet exposure.

Legacy summary retained for cross-reference: server is authoritative for generation, fluid simulation, and override application; clients receive chunk deltas, not full chunks; determinism guarantees enable client-side terrain generation. See the companion proposal for wire model, desync taxonomy, and channel/transport recommendations.

### Engine versioning and release policy (added v1.6)

The companion `roadmap.md` is authoritative for version sequencing, per-version feature gates, and compatibility boundaries. The architectural commitments:

- **Pre-1.0 scheme**: `0.MINOR.PATCH`. A minor version is a completed arc with exit gates and explicit non-goals; patches are fixes only. Post-1.0, a major version is a breaking change to save compatibility, asset formats, or network protocol that migrations cannot bridge.
- **Save compatibility boundary**: wipe-on-bump (§3 interim) is permitted through 0.5.x. Per-layer versioning lands at 0.6.0 as the final planned blob-format redesign. From 0.10.0, saves migrate forward; at 1.0, save compatibility is a user-facing promise.
- **Protocol version = build version** (from 0.7.0): mismatched clients are refused, never partially accommodated.
- **1.0 is a readiness bar, not a shipped title.** `roadmap.md` §5 defines it as a ten-domain rubric (world authoring, tooling maturity, runtime capability, game-facing API, multiplayer, performance, data durability, extensibility, documentation, distribution). A demonstration title is evidence that the rubric passes, never a substitute for it.
- **A release is not done without**: workspace version bump, changelog entry, close-of-version audit, design-doc revision for newly-settled architecture, green determinism suite, and recorded perf baselines. Version drift between `Cargo.toml`, the changelog, and announcements is a release-process bug.

### Performance budgets (added v1.6)

Performance is budgeted, not aspirational. Budgets are tracked from 0.3.0 (via per-`FrameStage` timing instrumentation surfaced in the engine's Performance panel) and gate releases from 0.5.0. Current budget values and their measurement procedure live in `roadmap.md` §7.3; recorded figures live in `perf-baseline.md`. The architectural commitment here is that every subsystem is attributable — a recurring frame hitch without an identified owner is a release blocker — and that streaming throughput scales with the observed area rather than being fixed constants.

**The budget is stated against CPU work, not frame time** (added v1.9). Under a vsync-limited present mode, wall-clock frame time measures *blocking* and moves inversely to engine cost: a faster engine fills the swapchain sooner and waits longer in acquire. The instrumentation therefore isolates the present block and reports `schedule span − present` as the number the budget applies to. Two corollaries, both learned by getting them wrong:

- **Performance is measured in release builds only.** An unoptimized build ran roughly 7× slower here, which is not a slow version of the truth but a different number entirely.
- **One-second aggregates must be read at rest.** Sampled immediately after a camera move they describe the fill, not the resting state.

The 0.3.0 baseline was taken after diagnosing a long-standing "recurring at-rest frame hitch" that proved not to exist: it was a debug build measured with wall-clock frame time. **A budget nobody can observe a violation of is not a budget** — which is why the instrumentation is architecture here rather than tooling.

**Throughput also scales with the *declared supported* range, not the permitted one.** Zoom limits that let a user reach configurations the engine cannot hold at budget make "within budget at all supported zooms" unfalsifiable. The supported range is declared by measurement and recorded in `perf-baseline.md`.

### The region graph (added v1.8)

Enclosure and connectivity are **persistent world state with many consumers**, not a rendering byproduct. A parallel field over air voxels assigns each air cell to a connected region; per-chunk connectivity summaries record which boundary faces link which internal regions and are joined across chunk borders. Updates are incremental — never global refills. Regions carry derived state: enclosed or sky-connected, revealed, volume, boundary faces.

It is built properly rather than opportunistically because it has at least eight consumers: occlusion (per the visibility principle in §1), creature spawn shelter checks, interior/exterior determination for weather and temperature, audio occlusion and reverb, hierarchical pathfinding (coarse routing over regions, fine path within), abstract creature movement between regions, interior fill light for revealed regions (see below), and camera Y anchoring to a region floor.

It runs at **voxel resolution using face coverage masks**. A system with eight consumers cannot afford a finer subdivision's cost multiplier. It degrades on large irregular caves, overhangs, and half-built structures, which is why occlusion remains a hybrid of region state and the air-side face rule (§11) rather than relying on regions alone.

The engine currently computes a bounded visibility flood and room detection per frame and discards the result. Promoting that computation to persistent shared state is scheduled in `roadmap.md`.

### Lighting: gameplay field vs. render field (added v1.8)

The voxel light field exists regardless of how the renderer works, because an authoritative server must answer questions no GPU shadow map can: can a creature spawn here, does this crop grow, is snow melting, is the player in darkness. The two are therefore **named separately** — `GameplayLight` and `RenderLight`. Conflating them is how a server ends up disagreeing with what players see.

- **Gameplay light** is quantized sky and block channels at voxel resolution, updated over several ticks, exposed as a world query. Because the renderer need not consume it directly, it can be comparatively cheap.
- **Render light is sampled from a per-chunk 3D texture, not baked into vertex attributes.** With vertex-baked light, placing a torch is a re-mesh event; with a 3D texture (one-voxel border for correct interpolation, roughly 4–6 KB per chunk) light changes stop touching meshes at all, interpolation is smooth rather than banded, the baked field survives as a low-spec quality setting, and any later real-time sun-shadow term is additive rather than a pipeline rewrite.
- **Interior fill light** resolves the conflict between cutaway occlusion and directional lighting. A hidden ceiling that still casts shadow leaves the revealed room a black hole; one excluded from casting floods it with sunlight. Neither is acceptable. A per-region `revealed` flag driving an ambient boost reads as "the camera is showing you inside," and is tunable per biome or structure.

**Interim shape:** §10's `FaceVertex` carries `light_level_index` as a per-vertex byte, which is the vertex-baked model. The target above supersedes it. The decision and the migration are dated in `roadmap.md` §8 (D2) and land before the lighting bake is built — building the bake first would mean building it twice. Whether block light is single-channel or RGB is a separate dated decision (D6) because it is a data-layout choice, not a later feature.

### Tick and scheduling framework (added v1.8)

One tick framework, per the scheduling principle in §1. It supersedes the current ad-hoc per-system clocks (`PlayerClock`, `FluidClock`, `ClimateClock`) and provides what simulation systems need beyond a fixed step: scheduled block updates, random ticks over resident regions with a defined distribution, sub-rate clocks derived from the primary tick rather than free-running, and a coarse region tick for distant simulation. It is replicated-clock-aware from the start so server and client agree on tick identity.

### Registry identity and save-embedded mapping (added v1.8)

`MaterialId` and every other registry ID are **runtime interning details**. The authoring *and persistence* identity is a stable namespaced string (`voxulacrum:oak_slab`), and **every save embeds its own name↔ID mapping** so that a registry reorder, insertion, or mod-added entry cannot silently reinterpret existing worlds.

This revises the v1.0 statement that "stable IDs are the wire format." Numeric IDs remain the in-memory and on-wire encoding *within a session or a version-locked connection*; what changes is that a persisted or transmitted payload always travels with the mapping needed to interpret it.

**This records a live defect.** The current implementation persists bare numeric IDs with no mapping. Nothing breaks today because the registry has not been reordered, and nothing will break until it is — at which point every existing save silently reinterprets. The fix is scheduled with the format redesign; it is cheap now and data-corrupting later.

### Per-region generation parameters (added v1.8)

Generation is currently a pure function of `(seed, graphs, coords)`. That is sufficient for a world whose rules never change, and insufficient for one that can be reshaped after it has been generated and edited — a region-scale transformation in the Terraria-hardmode sense. The generalization:

```
stored_chunk = (region_parameters, edit_delta)
resolved     = generate(seed, coords, region_parameters) ⊕ edit_delta
```

Generation becomes a function of **persistent, mutable per-region parameters** stored alongside the existing edit delta. A regional transformation is then *change the parameters, regenerate the base, reapply the delta* — rather than loading and rewriting every affected chunk's absolute contents. This keeps retroactive change affordable, keeps saves small, reduces replication of a world-scale event to a parameter change plus a progress front, and makes transformations re-runnable and revertible because parameters are data rather than baked results. The costs — generation must stay deterministic and versioned, and seed and parameters can never be discarded — are properties the engine already commits to (§1, §12).

The generated/authored split (§9) already provides the delta half. **The missing half is the persistent per-region parameter store, and adding it after the save format freezes is a format redesign.** It is therefore scheduled at the last planned format change even though the feature that consumes it ships later. Determinism (§12) is restated accordingly: same seed + same graphs + **same region parameters** + same coordinates → bit-identical output.

### Tooling as a first-class engine surface (added v1.7)

Authoring and diagnostic tooling is foundational architecture, not convenience work. The commitment:

- **Every authorable content type is authored in-engine.** Terrain graphs, biomes and zones, materials, palettes, blueprints and structures, foliage, input maps, entity definitions, and audio sets are all edited through engine panels, hot-reload without restart, and surface errors as messages rather than misbehavior. Requiring an author to hand-edit an asset file outside the engine is a defect in that content type's tooling.
- **Every authorable change is previewable before it reaches the world.** The author must be able to see the result and inspect why it looks the way it does — field probes, column inspection, biome mapping, isolated preview scenes, parameter scrubbing, and regeneration diffs are the general mechanisms. This is what makes the graph-as-world-generator model (§4) tractable at content scale rather than merely correct.
- **Every subsystem on the critical path explains itself at runtime.** Streaming, meshing, persistence, determinism, mutation, and networking each expose live state, timings, and validation sufficient to get from symptom to cause without writing new code.
- **Tooling ships with the feature it serves.** A subsystem is not complete when its runtime behavior is correct; it is complete when the content it consumes can be authored, previewed, and diagnosed.

The maturity model (T0 raw → T3 diagnosable), per-tool inventory, and per-version targets live in `roadmap.md` §4.

### The game-facing API surface (added v1.7)

The engine's purpose is to host titles, so every engine capability must be callable by game code without reaching into engine internals. The boundary:

- **Engine-side**: world query and mutation, entity and actor framework, character control and physics, interaction and targeting, blueprints, events, registries and configuration, navigation primitives, camera, UI framework, input, audio, VFX, world time and weather, game-state persistence, replication patterns, inventory mechanism, and the developer surface (console, inspectors) that game code registers into.
- **Game-side, permanently**: progression, crafting recipes, combat rules, specific creature behaviors, narrative, and the art direction of a particular title.
- **A capability the engine "has" but that a game feature cannot call is not delivered.** Each surface is documented and, from 0.9.0, covered by a deprecation policy. Conformance is proven empirically by a fixed set of small game features built without modifying engine crates (`roadmap.md` §6.30) and kept building in CI.

Full per-capability detail, current state, and version assignment live in `roadmap.md` §6.

### Acknowledged gaps: particles/VFX and entity visuals (added v1.7)

Two systems that a shipping title requires are absent from every section of this document and are recorded here so they are designed rather than improvised:

- **Particles and VFX.** Block-break debris, fluid splashes, dust, footstep puffs, ambient motes, and weather precipitation. The constraint from §11 applies without exception: particles are quantized and palette-driven, with no smooth gradients or sub-pixel motion that would break the pixel-art downscale. Emitters attach to entities, voxel positions, and events; they are pooled and budgeted; and they replicate as events rather than per-particle state.
- **Non-player entity visuals.** The player is a capsule rendered by a bespoke pass. Any entity beyond it needs a decided representation — voxel models, billboarded sprites, or a hybrid — plus animation, and the same tri-tonal/face-axis discipline (§11) that terrain obeys, so entities read as part of the same world rather than as overlaid art.

Both are scheduled in `roadmap.md` (VFX at 0.5.0; entity visuals at 0.9.0) and both require a design pass that extends this document before implementation begins.

### Hot reload

- File watcher (`notify`) on graph files. **A file changing on disk is the only trigger** (revised v1.9).
- On change: resolve the changed file to its hierarchy slot *through the manifest*, reload the whole hierarchy from disk, walk `ChunkTags` to find affected chunks (subject to §4's backward-looking constraint), queue regeneration.
- The in-engine editor's Save writes the file; an external edit writes the same file. Both therefore take one path, rather than converging on one by construction.
- **Disk is authoritative for the world; the editor is authoritative for its canvas.** The watcher does not push graphs back into the editor, so there is no direction in which the two can disagree about what the world is generated from.

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
- **BiomeGraph**: graph type defining terrain density, material, fluid output, and fade behavior for one biome.
- **BiomeParams**: per-biome typed scalar-metadata sidecar. Addressable by name from the biome manifest entry and by `BiomeParam(name)` nodes inside a `BiomeGraph`. Home for `traversal_smoothing_distance` and future biome-scoped scalars.
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
- **FluidOutput**: `BiomeGraph` terminal for water bodies. Renamed from `FluidProvider` to describe the terminal position, not the role.
- **FrameStage**: ordered schedule of per-frame work in the bevy_ecs runtime. Stages run in fixed order: Input, Simulation, Meshing, UniformWrite, Render, PostFrame.
- **Generated content**: chunk data reproducible from seed + graph. Not serialized in detail.
- **GraphOutput**: terminal node placed inside a producing graph, marking a named typed output that other graphs (via `GraphRef`) can consume.
- **GraphRef**: source node placed inside a consuming graph, referencing a target graph and one of its `GraphOutput` names. Resolved via the manifest at evaluation time.
- **LibraryGraph**: reusable subgraph referenced by any other graph type. Declares typed named inputs and outputs at its boundary.
- **LibraryKernel**: enum matching each standard library's identity to a native implementation. Backs standard libraries whose authored file captures identity and boundary but delegates computation to native code.
- **LibraryRef**: node referencing a `LibraryGraph` by ID. Exposes the referenced library's boundary as dynamic pin descriptors on the referring node.
- **Manifest**: `world.manifest.json`. Names `sea_level`, seed, and the paths + identities of the WorldGraph, ZoneGraphs, BiomeGraphs (with optional DetailGraph paths and `BiomeParams`), and LibraryGraphs. Resolution root for `GraphRef` and `LibraryRef`.
- **MaterialRegistry**: data-driven registry mapping stable `MaterialId` values and human-readable names to material definitions. Loaded at startup; extensible by mods.
- **Mesh disk cache**: persistent cache of meshed chunks keyed by chunk coordinate, graph hash, mesher version, registry hash, and world seed.
- **Overrides**: player-authored edits serialized separately from generated content. Canonical in-memory shape is `ChunkOverrides`; persisted directly.
- **Pin type**: typed connection between graph nodes. Enforces compatibility at edit time.
- **Positions**: pin type representing a set of candidate positions (used by scatter placement and any other consumer of a candidate-position set). Formerly named `ScatterPoints`; renamed to describe the general mechanism rather than the scenario.
- **ScatterInstance**: a single placed foliage/prop instance with anchor, sub-offset, rotation, prefab reference.
- **ScatterStore**: per-chunk collection of ScatterInstances indexed by type.
- **Sea level**: global Y read from the manifest, determining ocean fill.
- **Slab**: a half-height voxel shape. Either `SlabBottom` (lower half solid) or `SlabTop` (upper half solid). All slab faces are cardinal-axis-aligned.
- **Slab smoothing**: engine pass during worldgen that inserts slab steps at walkable cube-step transitions, controlled by traversal smoothing distance.
- **Streaming**: load/evict policy keeping only chunks within observer radius resident. Background workers handle generation and meshing.
- **Sub-voxel offset**: fractional position within an anchor voxel, packed as signed bytes.
- **Traversal smoothing distance**: per-biome worldgen parameter controlling how aggressively slab smoothing distributes steps across distance. Produces emergent gradual slopes without diagonal geometry.
- **Seam finalization**: cross-chunk pass (`world::seam`) resolving slab-smoothing demotions at chunk borders once neighbor data exists; recorded through the override/diff mechanism and marked by a persisted `seam_finalized` flag.
- **Walkability**: transient worldgen computation identifying voxels the player can stand on; input to slab smoothing, then discarded. Runtime movement, collision, and room detection use direct geometric queries (`WorldView::solid_interval`) instead of a resident mask (revised v1.6).
- **WorldGraph**: top-level singleton graph defining climate, zone selection, and global constants.
- **ZoneGraph**: graph type defining biome distribution, cave rules, structures, and rivers within a Zone.

---

## Document Maintenance

This document is the **baseline reference** for the engine's foundational data model and architecture. Changes to anything described here require explicit revision of this document. Implementation details (specific algorithms, library choices, code organization) may evolve freely; data shapes, pipeline stages, and architectural boundaries do not.

When new systems are designed (combat, AI, multiplayer, modding), they extend this document rather than replacing it. The systems described here are load-bearing; everything else grows on top.
