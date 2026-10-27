# Voxel Engine Foundation Design Document

**Status:** Baseline reference, v1.18
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

- v1.12 — Recorded M2's settled content architecture in §12: **packages and content namespaces**. Content loads as data-only packages (`package.ron` beside fixed-layout content), each with its own namespace; bare names resolve in the holding content's namespace and qualified names reach into any loaded package. The built-in `voxulacrum` package declares the standard libraries beside their kernels. Dependencies load in dependency order, then by id. Tables whose ids saves store (materials, prefabs, foliage) stay with the world's own package until the save-embedded mapping lands. A package's `play.ron` carries its Play-mode policy, and games build on the facade crate alone.

- v1.13 — Designed the **runtime spine** in §12 ahead of building it (roadmap M3, R1). Crates and sides, with two compiler-enforced dependency rules: server-core has no renderer or windowing dependency, and the render crate has no simulation dependency. One **30 Hz tick** with a `SimTick` counter, sub-rates keyed by tick number, a fixed phase order, state that changes only inside a tick, separate simulation and presentation time, and stateless seeded randomness; scheduled, random and block ticks are designed with a trigger for building them. The **client/server boundary**: commands applied at tick boundaries, one update per tick drained by cursor, and `Arc`-shared chunk snapshots in the shape M7 serializes. The **render world** and extract, with the minimal camera description. **Observer residency.** Two **determinism tiers**, with Tier 1 adding `libm` and no fused multiply-add to the rules. Records `target-architecture.md` N4 (integrated server), N5 (tick rates) and N6 (determinism tiers) as settled. Brought into line with it: §1's host principle and §12's editor section, which now follow roadmap P9's editor binary; §12 Runtime architecture, now two schedules; §12 Chunk streaming, now observers and areas of interest; §12 Determinism, now Tier 1; §12 Tick and scheduling framework; §12 Networking's tick rate and its sequencing by milestone; §12 Engine versioning, now by milestone, with the rubric's eleventh row; and the glossary.
- v1.14 — Designed the **engine object model** in §12 ahead of the items that build its parts (roadmap M3, B8). One word per concept, retiring the overloaded uses of region, layer, tag, block and prefab. Definitions of the seventeen primitives, with where the code is today. A **catalog of asset types**, each with its owner (core, kit or game), registry, side, simulation and presentation fields, override rule and milestone; game content (items, recipes, loot) is built from generic data tables. Assets with both kinds of field load as a shared **definition** and a client **appearance** under one name, and overriding always replaces a whole record. Every registry and code hook listed as an **extension point** with its deliveries, side and determinism tier, including the **worldgen kernel registry** and its rules. **Override rules**: assets by name, never by path; a package replaces only names from packages it depends on, and unrelated packages replacing one name is a load error; tags merge; shared content is covered by the join handshake and presentation is not. **Package kinds** `Library` and `ResourcePack` added beside `Game` and `Mod`. The **shader reach** ladder, with who may use each rung and what it promises. The **camera boundary**, adopting the Runtime spine's description, with everything particular to a camera moved to kit camera modules. Records the three v2.1 decisions roadmap §8 deferred to this item: the foliage stack is interim (§6 now says so), the camera is a module, and there is one object model. §2's material registry now points to it, and §12's packages section corrects its claim that nothing persists library ids: chunk tags do, along with zone and biome ids. Glossary extended.
- v1.15 — Recorded the **cross-platform determinism job** (roadmap M3, T7) in §12 Determinism: what its manifest hashes, the content and scenario checks that keep a comparison from passing vacuously, and which platforms gate. The rule that hashes use the persistence encoding now says the generated fluid, scatter and detail layers have encodings for hashing alone. Runtime spine's interim shapes gain the harness living in the app binary, which keeps the Linux runner installing windowing and GPU packages until R3c moves it to the headless server; the issue had named R3b for that.
- v1.16 — Settled **Tier-1 math in generation** (roadmap M3, R7) in §12 Determinism. glam's `libm` feature turned out not to be enough: its `Quat`, `Vec4` and other SIMD-backed types sum dot products in a different order on SSE2 than on NEON, so the rule now bans those types in Tier-1 code as well, and tree skeletons rotate with a scalar quaternion of their own. The rules are enforced by clippy lists in `crates/nodegraph-eval/clippy.toml`, which a CI step denies. The determinism manifest gains a `kernels` section. Runtime spine's interim shapes retire R7's row and gain one for the lists covering only `nodegraph-eval` until server-core exists (R3b).
- v1.17 — Recorded the **in-place decoupling of the world code** (roadmap M3, R3a) in Runtime spine's interim shapes. The world code now reads no editor setting: its streaming budgets, fluid limits and autosave interval are a `ServerConfig` it owns, and the terrain-generation parameters the editor carried are gone, since generation read none of them. Four rows are added, each with the item that retires it: regeneration's render rebuild (R4), streaming's mesher and view-test coupling (R6), `ServerConfig` built from defaults (R3c), and the two client modules that live under `world/` (R3b).
- v1.18 — Built the **tick framework** (roadmap M3, R2) and recorded what it settled in §12 Runtime spine. `TICK_RATE_HZ` is defined once and can be overridden at build time, which is how CI runs the tests again at 20 Hz; a frame-jitter test holds the state at tick 3,000 to one hash under three frame-time sequences. The fluid throttle is decided from world state, dropped ticks include time past the 0.25 s frame clamp, and the tick inspector publishes once per second like the rest of the Performance panel. Runtime architecture's interim schedule now runs the tick schedule from `Simulation`. Runtime spine's interim shapes retire R2's row and gain five: world time on the client until R5, commands applied between ticks until R5, fluid changes drained per frame until R4, residency outside the tick until R6, and a tick counter that restarts each session. The driver's pause and time scale are recorded as not built.

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
- **The engine is the host** (revised v1.13, following `roadmap.md` P9). Authoring happens inside the engine, on its one renderer: the editor is its own binary built on the engine library, and playtesting launches a game as a separate process the editor attaches to. Each world has one chunk store, owned by its server; clients hold snapshots of it. The headless server is the engine's server-core without a renderer, not a second engine (§12, Runtime spine).
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

The first bullet is superseded: identity is a namespaced name, and saves carry a name-to-id mapping (§12, "Registry identity and save-embedded mapping"). The material's full shape, with its properties, its split into a definition and an appearance, and the rules for replacing it, is under "Engine object model" in §12 (added v1.14).

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

The engine ships a set of standard libraries in its built-in package, `voxulacrum`, so content references them as `voxulacrum:<name>`:

- `StandardCaveNoise` — 3D Worley + ridged noise composite
- `SurfaceLayering` — depth-conditional material cake (grass/dirt/stone)
- `ExposureLayering` — surface layering varied by face exposure (top-facing vs side-facing surfaces get different materials/tints; useful for snow caps, exposed rock on cliff faces, moss on shaded sides)
- `BiomeBorderFade` — standard fade kernel used by biomes at their edges
- `PoissonPlacement` — point distribution for scatter

Standard libraries are **backed by `LibraryKernel`** — an enum matching each standard library's name to a native implementation, which also declares each library's boundary (`LibraryKernel::boundary`), so a declaration cannot drift from the code it describes. A package may author further libraries in the `.library.json` format, naming a kernel: the asset captures the library's boundary declaration and identity, and the actual computation runs in native code. This is the pragmatic starting shape: libraries participate in the graph editor, the reference/cycle-detection system, and the invalidation table, without paying the cost of full graph-body library implementation for functions that biomes don't currently customize.

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

**Interim shape** (added v1.14, following roadmap §8 v2.1). This section describes the foliage stack as built, and that stack is interim: it gets no new work. Voxel trees and structures become clips, non-cube props become fixtures and sub-voxel models, detail paint and scatter become registered layers, and grass and canopies become sprites, depth sprites and billboard clumps (§12, "Engine object model", which lists the migration under its interim shapes).

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
    pub tint: u8,       // brightness nudge (0.85..1.15); a value, not a palette index
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

The engine uses **`bevy_ecs`** as its runtime (an ECS library extracted from the Bevy engine, used without the rest of Bevy). The engine is not a Bevy app; the client owns its own `winit` event loop and `wgpu` device, and each side embeds `bevy_ecs` as its schedule and resource system.

There are two schedules (revised v1.13; see "Runtime spine" below). The **server** runs the tick phases once per simulation tick. The **client** runs one frame schedule per frame, ordered by **`FrameStage`**:

```
FrameStage::Input     — gather window events; build the next InputFrame and commands
FrameStage::Tick      — drive the server's ticks (integrated client only)
FrameStage::Sync      — drain server updates; update chunk views; schedule and poll meshing
FrameStage::Extract   — fill the render world, including finished meshes
FrameStage::Render    — run the render schedule on the render world: upload, render graph, egui, present
FrameStage::PostFrame — client housekeeping
```

These orderings are the engine's scheduling contract: a new system declares its side, and the tick phase or frame stage it runs in. The stage names may change as M3 builds them; the order is the contract. Resources (the chunk store, graph registry, material registry, render context, etc.) live as ECS resources accessed through system parameters, each in the world of the side that owns it.

**Interim shape:** until the render world and the client/server boundary land (roadmap M3, R4 and R5), one schedule runs both sides on the main thread: `Input → Simulation → Meshing → UniformWrite → Render → PostFrame`. In `Simulation`, an exclusive system runs the tick schedule as many times as the frame's time owes (R2); the player step, fluids and autosave are its phases. Climate, time of day, picking and the editor tools run in `Simulation` once per frame; streaming, seam finalization, the client's application of the change feed (`chunk_change_system`) and meshing run in `Meshing`, in that order.

The choice of `bevy_ecs` is an implementation detail of the runtime — it does not affect the data model, the graph hierarchy, or any other system described in this document. Any compatible ECS could substitute. The choice is documented here because every cross-cutting system (hot reload, streaming, persistence) integrates through a schedule and must know its ordering.

### Runtime spine (added v1.13)

How the engine runs: which crate holds what and which side runs it, how time advances, what crosses between client and server, how rendering gets its data, and which results must be identical on every machine. This section records three decisions from the October 2026 design review as settled (`target-architecture.md` N4, N5 and N6): **singleplayer is an integrated server**, with client and server in one process, so every feature is multiplayer-correct the day it is written (roadmap P6); **simulation runs on one 30 Hz tick**; and **determinism comes in two tiers**. Roadmap M3 builds it. The shapes it replaces are listed under "Interim shapes" at the end.

#### Crates and sides

Every engine API belongs to one side. **Shared** code is the world and the simulation, and runs on both sides: that is what makes client prediction possible (M7), and what lets clients generate terrain locally. **Client** code presents: rendering, audio, UI, input devices and cameras. **Server** code holds authority: command validation and interest management.

| Crate | Side | Holds |
|---|---|---|
| `voxel-core`, `nodegraph-ir`, `nodegraph-eval`, `nodegraph-hotreload` | Shared | Voxel and chunk primitives; the graph IR, evaluator and file watcher |
| `voxulacrum-server-core` | Shared, with server-only code in a `pub(crate)` module | World and chunk state, generation glue, fluids, persistence, the mutation door, the event bus, residency, the tick framework, `player_step`, the job system, packages and registries, and the boundary types |
| `voxulacrum-render` | Client | wgpu, the render passes and the render world |
| `voxulacrum-app` | Client | Window, input, camera modules, editor panels, extract and mesh scheduling |
| `nodegraph-editor` | Client | The graph canvas |
| `voxulacrum` | Facade | The only surface games use; re-exports server-core's boundary types |
| `voxulacrum-server` | Server (binary) | The headless loop, over server-core alone |

Two dependency rules make the boundaries compiler-enforced rather than conventions:

- **Server-core never depends on `wgpu`, `naga`, `winit`, `raw-window-handle`, `egui` or `eframe`**, directly or transitively. CI checks the resolved dependency graph and builds server-core on a Linux runner with no windowing or GPU packages installed, which a Windows build cannot prove.
- **The render crate does not depend on server-core.** Render code therefore cannot name a simulation type, which is what "rendering reads only the render world" means in practice.

Shared and server code share one crate until M7, because no server-only code exists before then; the split comes when M7 needs a client build without it. This departs from the design review's three crates (shared, client and server), one of which would have had nothing in it. `bevy_ecs` is allowed on every side, with its default features off. The editor binary (roadmap P9) is a client like any other.

#### Time

One tick-rate constant, `TICK_RATE_HZ = 30`, and a tick counter, `SimTick(u64)`, that identifies every tick and later timestamps network messages, replays and scheduled updates. Code writes durations in seconds, or in ticks derived from that constant, and never in a bare 30; the tests also run at 20 Hz to keep that true. The build-time variable `VOXULACRUM_TICK_RATE_HZ` overrides the constant, and CI runs the workspace tests a second time with it set to 20 (added v1.18). A frame-jitter test (`ecs/frame_jitter.rs`) runs the tick schedule to tick 3,000 under three frame-time sequences, constant 16.7 ms, constant 7 ms and seeded jitter from 5 to 60 ms, and requires the same hash of player and fluid state from each.

**Sub-rates are keyed by tick number, never free-running.** Fluids step on every third tick (10 Hz). Physics will run two substeps per tick (60 Hz) once it exists (M8). Block updates and growth run at 10 Hz or lower. A sub-rate system that would exceed its budget defers a whole step, and decides that from world state, never from frame timing. The fluid throttle that drops ticks when a frame runs long is the failure this rules out. Fluids instead step on every other fluid tick while more than `ServerConfig`'s `throttle_active_cells` are active, counted at that tick (added v1.18).

**A tick runs its phases in a fixed order:**

1. Apply queued commands through the mutation door, sorted by client and sequence number.
2. Step each player with its input for this tick.
3. Run scheduled, random and block ticks (designed below; not built yet).
4. Run sub-rate systems by tick number: fluids when `tick % 3 == 0`.
5. Advance world time: time of day and wind are functions of the tick.
6. Update residency: apply observer changes, insert finished generation results, evict.
7. Autosave every N ticks.
8. Publish this tick's events and changes to the outbox.

Logic networks join in the order `target-architecture.md` gives them (M9): topology updates after phase 1, stepping and write-back between phases 3 and 4. Further field layers such as heat and gas join phase 4 (M10).

**Server state changes only inside a tick.** Between ticks it is read-only, so every tick boundary is a consistent snapshot: it is what the client reads, and what a joining client receives in M7.

**Driving ticks.** The integrated client adds `min(frame_dt, 0.25) × scale` to an accumulator each frame and runs ticks while the accumulator holds a whole tick, at most five per frame. A remainder past the cap is dropped and counted: dropped ticks mean the simulation is running slow, and the tick inspector shows them. Whole ticks of frame time past the 0.25 s clamp count as dropped too. The headless server instead sleeps until each tick's deadline and logs overruns. The driver has no `scale` or pause yet, since nothing uses them; the first consumer, an editor pause or a time-scale setting, adds both.

**Render interpolation.** Extract interpolates between the last two ticks by `alpha = accumulator / tick_dt`, for transforms (the player already works this way) and for world time.

**Time domains.** *Simulation* time is ticks. Pause and time scale apply here, and scale changes how many ticks run per real second, never `dt` per tick, so fixed-step results don't change. *Presentation* time is the client's frame clock: cloud animation, water shader time, mesh debounce and UI animation. The cloud and vapor automaton (`world/climate.rs`) is presentation until the cloud hydrology fix-or-remove decision says otherwise. Audio and UI time arrive later, on the presentation side.

**Randomness** comes from stateless streams, `hash(world_seed, system, tick, key)`. There is no shared generator, so the order in which systems run cannot change a draw.

The **tick inspector** on the Performance panel shows the tick number, ticks run this frame, dropped ticks and the cost of each phase (`roadmap.md` §4.4). Like the panel's other figures it publishes once per second, since a per-frame count read at 60 fps is unreadable: ticks run in the last second and the most in one frame, ticks dropped in the last second and this session, and each phase's mean and longest run per tick. Phase cost is measured server-side (`TickTimings`), by timing systems between the phases, so the headless server reads the same figures (added v1.18).

#### Scheduled, random and block ticks

Designed now, built with their first consumer.

- **Scheduled ticks:** a per-chunk queue ordered by `(due_tick, seq)`. Entries persist with their chunk, so the queue is a chunk layer and needs registrable layers (roadmap D4). Entries that fell due while their chunk was unloaded run on its first tick back, in queue order.
- **Random ticks:** each tick, for each resident chunk inside the simulation range, `K` cells chosen by `hash(world_seed, chunk, tick, i)`. The distribution is uniform within a chunk and doesn't depend on iteration order.
- **Block ticks** (a neighbor changed): the door's changed cells, batched per tick, with a per-tick budget and a cascade depth limit.
- **Region ticks:** a coarse sub-rate for distant simulation; the abstract entity tier's clock (`roadmap.md` §6.23).

**Trigger:** the first block that needs a delayed update, or D4's registrable layers. Building the queues before then would be designing against an imagined consumer, the same reason the job system defers declared dependencies (see "The job system" below).

#### The client/server boundary

In M3 the client calls server-core directly, in process. M7 sends the same types over a channel, first an in-memory loopback and then QUIC, so the types are designed to be serialized even though nothing serializes them yet.

**Client to server:** `ClientCommand { client, seq, kind }`.

| Kind | Carries |
|---|---|
| `Edit` | A Play-origin mutation intent |
| `Author` | An Authoring-origin mutation intent: how the editor stamps, pours and regenerates |
| `Input` | The `InputFrame` for the next tick |
| `SetInterest` | The client's observer area ("Residency" below) |
| `Admin` | Mode switch, save, regenerate |

No kind carries `System` origin. System writes (fluid commits, chunk insertion, seam finalization) begin inside server-core, so the door's runtime origin check becomes a compile-time one at the boundary.

**Commands apply at the next tick boundary.** In singleplayer an edit therefore lands up to one tick (33 ms) after it is issued; M7's client prediction covers remote play.

**Server to client:** one `ServerUpdate { tick, events, changed, acks, players, world_time }` per tick, in a tick-stamped outbox that the client drains by cursor. A frame that ran three ticks reads three updates; a frame that ran none reads none. bevy's double-buffered `Events` stay an intra-schedule tool on each side: they age per schedule run, so across the boundary they would lose the first tick's events whenever a frame ran several ticks.

- `events`: the world events this tick published (`roadmap.md` §6.7).
- `changed`: `(chunk, version, layers, faces, origin)` for every chunk whose data changed this tick: the layers that changed, the faces whose boundary cells changed, and the origin of the command that changed them. It names data, never render work. Which neighbors re-mesh and whether an edit is debounced are the client's decisions, and `faces` and `origin` are what it decides them from (revised in R4, which replaced the door's render-shaped rebuild lists with this feed).
- `acks`: each command accepted, or rejected with a reason.
- `players`: each player's state at this tick.

**Snapshots.** `server.chunk(coord)` returns a `ChunkSnapshot`: the chunk's version, voxels, fluid, scatter, detail paint and tags, plus the generation output only rendering uses (tree segments and canopy lobes). In process every part is an `Arc` clone of copy-on-write data, so a snapshot costs almost nothing. It is also the shape M7 sends when a client subscribes to a chunk: the generated-base hash plus deviations ("Networking" below). The client builds the mesher's padded window from snapshots.

**Stats.** `server.stats()` feeds the job, streaming, mutation-log and tick panels.

**Enforcement.** `World`, `LoadedChunk` and the streaming manager are `pub(crate)` inside server-core; the boundary module is its only public surface. If the app compiles, it went through the boundary.

**The editor** is a client. Its canvas and panels run client-side, and its world changes are `Author` commands. The graph-file watcher runs on the server, because disk is authoritative for the world ("Hot reload" below). The column inspector and field probe evaluate graphs with the client's own `WorldGenerator`, which every client holds anyway because clients generate terrain locally.

#### Render world and extract

Rendering reads only a **render world**: a separate `bevy_ecs` world owned by the render crate. Simulation systems never touch it, and render systems touch nothing else.

- **Per-frame data:** the camera description, lighting and time of day, wind, clip bounds, the visibility mask, player render state, debug lines and HUD data.
- **Per-chunk entries**, keyed by chunk coordinate: the terrain mesh, water mesh, scatter instances, detail paint, and tree capsules and canopy lobes. Each records the chunk version it was built from. They are derived data (roadmap P17): any of them can be rebuilt from a snapshot at any time, including after a GPU device loss.

**Chunk versions** come from one counter the world owns. Every change the door applies stamps the chunk with the next value, so a version is never reused, even by a chunk that unloads and loads again. Versions are runtime state and are not saved. The startup set is published to the feed as loads, so the client builds it the way it builds a streamed chunk (R4).

**Extract** runs once per frame, after the client drains its updates. It copies the per-frame data and applies `changed` to the per-chunk entries, marking stale ones for rebuild. It never scans every loaded chunk.

**The camera description** has four fields core code reads: view, projection (with view-projection), frustum, and area of interest. A camera module adds a block of its own for its presentation; the isometric module's holds the texel snap offset, rotation and pitch. Nothing outside that module reads the block (roadmap P20). This is the minimal description the engine object model's camera boundary adopts (roadmap B8). Render code still makes orthographic assumptions after M3; V8 (M5) removes them.

**Pipelining**, with the GPU drawing frame N while simulation prepares N+1, is not built in M3. A self-contained render world is what keeps it possible.

#### Residency

The server keeps a chunk resident because some **observer** needs it. An observer is an id and an **area of interest**: a center, a horizontal radius and a vertical range. The local player is observer 0.

- The resident set is the union of all observers' areas plus every chunk with active fluid. Entity load anchors and simulation bubbles join it later.
- Generation priority stays `(class, distance²)`, measured to the nearest observer.
- Residency changes happen in tick phase 6.

On the client, a camera module computes its area of interest and sends it with `SetInterest` when it changes; the isometric camera derives its area from the altitude-aware view test ("Chunk streaming" below). The client's **chunk view** reacts to chunks loading, changing and unloading: it takes snapshots, schedules meshing and drops render entries. A headless server takes its observers from its command line or script.

#### Determinism tiers

**Tier 1: bit-identical on every machine.** Generation (every layer), the mutation door's handlers (voxel edits, blueprint stamping, seam finalization) and fluid simulation. Clients regenerate terrain and verify it by hash ("Networking" below), so every cross-machine difference costs an authoritative refetch. The rules are under "Determinism" below; a CI job compares Windows and Linux hashes on every push (T7).

**Tier 2: reproducible on one machine.** `player_step` now; physics, AI and animation later (Rapier's `f32` scenes, M8). This needs a fixed tick, inputs per tick and no wall clock; `f32` is allowed. Server corrections fix clients, and same-machine reproducibility serves replays and tests.

**Not deterministic, by design:** residency timing, since which tick a generated chunk becomes resident depends on when its job finishes; and presentation.

#### Interim shapes

| Today | Target above | Migrates in |
|---|---|---|
| Time of day and wind are client state in `SimulationManager`, advanced each frame to the simulation time the frame draws (ticks run plus `alpha`), and the editor's time controls set them directly | World time is server state, advanced in tick phase 5 and sent as `ServerUpdate.world_time`; the time controls become `Admin` commands | R5 |
| Edits, pours and the editor's tools reach the door from the frame, between ticks, as soon as they are issued | Commands queue and apply in tick phase 1 | R5 |
| Changes collect in a feed on `World` that `chunk_change_system` drains once per frame, however many ticks ran | `ServerUpdate.changed`, one per tick, drained by cursor | R5 |
| Residency (`streaming_tick_system`) runs once per frame, outside the tick | Tick phase 6 | R6 |
| `CurrentTick` is not saved, so every session starts at tick 0 | Saved with the world, so due ticks survive a reload | The first scheduled tick (D4's registrable layers) |
| One `FrameStage` schedule runs simulation and rendering on the main thread | Server tick phases; a client frame with extract; a separate render world | R4, R5 |
| Terrain meshes are a client resource (`ChunkMeshes`), what needs re-meshing is tracked by `MeshingPipeline`, and the other per-chunk buffers live in the render passes, which render code reads directly | Per-chunk render entries in the render world, each tagged with its chunk version | R4 |
| Residency follows one isometric camera, and streaming drives meshing | Observers; the client chunk view | R6 |
| The whole engine is one app crate. The world code under `world/` reads no editor setting and imports no client module, apart from the two couplings in the rows below this one (R3a) | The crates above | R3 |
| Streaming (`world/streaming.rs`) submits to the mesher, checks it before evicting, and evaluates the isometric view test itself; `ServerConfig`'s load and unload margins size that test | Residency from observers; the margins belong to the camera's area of interest | R6 |
| `ServerConfig` (`world/config.rs`: streaming budgets, fluid limits, autosave interval) is built from defaults in the app, and nothing loads it from a file | The headless server fills it from its command line | R3c |
| The cloud automaton (`world/climate.rs`) and the visibility flood (`world/room.rs`) are client modules that live under `world/` | They stay in the app crate when the world code moves | R3b |
| Clippy's Tier-1 lists cover `nodegraph-eval` only; the mutation door's handlers and fluid simulation, still in the app crate, keep the rules by review | Server-core's Tier-1 modules under the same lists. Clippy's configuration is per crate, so its Tier-2 modules (the player step) allow the two lints per module, or move to a crate of their own | R3b |
| The app binary writes the determinism manifest (`--determinism-manifest`), so the Linux CI runner installs windowing and GPU packages to build it | `voxulacrum-server` writes it, on a runner that installs nothing | R3c |
| The underground visibility flood runs on the client each time the player's head cell changes | Visibility as shared world state, through the region graph | S1 (M5) |

The visibility flood stays client-side through M3 knowingly. The renderer and the player's targeting both run on the client, so they agree, as §1's visibility principle requires; a server that validates targeting (M7) needs the region graph's shared classification.

### Chunk streaming

The world is effectively infinite; only chunks some observer needs are resident in memory. An observer (the local player, a remote client, later an AI agent or a debug camera) declares an area of interest, and the server's resident set is the union of those areas plus every chunk with active fluid (revised v1.13; see "Residency" under Runtime spine).

- **Area of interest** is set per observer, by its camera or controller.
- **Chunk lifecycle**: chunks outside every area evict; chunks inside one load (from persistence if they exist, otherwise generate via the pipeline; §5).
- **Eviction policy** is LRU among chunks outside every area, with a hysteresis band to avoid thrashing at the boundary.
- **Loaded chunks track their resident state** for systems that need to iterate the resident set (mesher, fluid simulation, AI pathfinding).
- **Background workers** handle chunk generation and meshing without blocking the main thread. Both submit to the single job system below (revised v1.9); neither owns a pool or spawns threads of its own.

Streaming is foundational: every subsystem must tolerate chunks appearing and disappearing on a frame-by-frame basis. Code that assumes all chunks are resident is incorrect.

**Interim shape:** one observer, the isometric camera, whose view the streaming manager evaluates itself; the same manager also submits dirty chunks to the mesher. Roadmap R6 splits it into server residency and a client chunk view.

**The view test is altitude-aware** (added v1.9). Under the isometric projection a ground displacement `d` along the away-axis shifts a point up-screen by `d·sin(pitch)`, while an altitude `Δy` shifts it by `Δy·cos(pitch)` — so altitude is screen-equivalent to `Δy/tan(pitch)` of ground. A residency test comparing a column's *footprint* against the view band therefore misses columns whose footprint lies outside it but whose raised geometry is plainly on screen. The test compares the column's projected **span**. It is column-granular by necessity: a per-chunk test would be tighter, but omitting a middle Y layer leaves the layer above it permanently unmeshable, since meshing requires both Y neighbours resident. The test belongs to the isometric camera module, which turns it into that camera's area of interest (revised v1.13).

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

Mandatory in **Tier 1**: generation, the mutation door's handlers and fluid simulation, whose results must be bit-identical on every machine, not only on one (revised v1.13; tiers under "Runtime spine" above). Specific rules:

- Determinism is stated over the full generation input set: same seed + same graphs + **same per-region parameters** + same coordinates → bit-identical output (revised v1.8; see "Per-region generation parameters" below).
- All RNG seeded from explicit context: `hash(world_seed, layer, node_id, world_pos, purpose)`. Simulation draws add the tick.
- No use of chunk coordinates alone for RNG (breaks at fade boundaries).
- No f32 in persistent state where ordering matters (fluid mass is u16).
- No thread-order-dependent generation, and no wall clock, thread identity or job completion order in any result.
- Iteration over hash maps stabilized via sorted keys where output ordering matters.
- **No platform math library** (added v1.13). Transcendental functions (`sin`, `cos`, `tan`, `powf`, `exp`, `ln`, `log2`, `log10`, and the rest of their family, `cbrt` and `powi` included) come from the `libm` crate, and glam's math goes through its `libm` feature, because platforms' math libraries don't promise the same last bit. `sqrt`, `abs`, `floor`, `ceil`, `round` and `trunc` are exact everywhere and allowed.
- **No SIMD-backed glam types** (added v1.16): `Quat`, `Vec3A`, `Vec4`, `Mat2`, `Mat3A`, `Mat4`, `Affine2` and `Affine3A`. Their SSE2 and NEON backends sum a dot product in different orders, so a `normalize` or `slerp` can differ in the last bit between x86_64 and aarch64, which the `libm` feature does not reach. `Vec2`, `Vec3` and `Mat3` are scalar and allowed. Rotations use `nodegraph-eval`'s scalar `Rotation`, a unit quaternion with each operation written out in a fixed order.
- **No fused multiply-add**, and no parallel floating-point reductions, whose results depend on how the work was split (added v1.13).
- Hashes are computed over the persistence encoding, which is fixed-endian and key-sorted (added v1.13). Voxels, tags and edit records use the blob's own sections. A chunk's generated fluid, scatter and detail paint are never saved, since the chunk regenerates them, so their encodings exist for hashing alone, built from the same entry writers as the override sections (`persistence::write_fluid_layer` and its siblings; added v1.15).

Clippy enforces the math rules (R7, added v1.16): `crates/nodegraph-eval/clippy.toml` lists every banned float method in its `f32` and `f64` forms under `disallowed-methods`, and the SIMD-backed glam types under `disallowed-types`, and CI denies both lints on the crate's every target. Clippy ignores a listed path it cannot resolve, without a warning, so the lists are checked against the pinned toolchain and again whenever it moves. Server-core's Tier-1 modules adopt them (R3b; Runtime spine, "Interim shapes").

**The cross-platform job** (T7, added v1.15). Every push writes a manifest of Tier-1 hashes on Windows, Linux and macOS (`--determinism-manifest`) and compares them (`determinism-diff`), which names the first differing chunk and layer. The manifest holds every layer of the 512 chunks `--verify-generation` samples, a fluid pour stepped 200 times, and a run of voxel edits ending in a rotated blueprint stamp, each hashed after every batch. A `kernels` section (added v1.16, R7) hashes tree skeletons and Poisson-disk point sets directly, since a difference smaller than a voxel changes no chunk hash and no shipped graph places by Poisson disk. The run fails, before any comparison, if its sample holds no chunk with fluid cells, trees, structures, two biomes or caves, or if a scenario did not do what it is for (water crossing a seam, a chunk promoted to a full save, a stamp across chunks), since a comparison proves nothing about content it does not contain. Windows against Linux fails the build; macOS reports but does not gate. `--verify-generation` keeps its own check, which compares a run with itself.

Tier 2 (the player step; later physics, AI and animation) needs only same-machine reproducibility. Simulation as a whole is divergence-tolerant (`roadmap.md` P1): the server owns it, and clients render what it sends.

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
- **Fixed-timestep player sim as a pure function** `step(state, input, &world) -> state` in shared server-core code, at the 30 Hz simulation tick. Both client and server run the identical function; this is the one place client and server share simulation.
- **Snapshot interpolation for other entities** at ~100–150 ms behind real time. No extrapolation — for a builder game, late is better than wrong.
- **Per-observer streaming.** `ChunkStreamingManager` generalizes from camera-driven to per-observer subscription sets; the local player is observer 0.

**Sequencing** (revised v1.13, by roadmap milestone):

1. Player character built networking-aware but standalone. Shipped, with the mutation door, the fixed-timestep player sim and the semantic action input layer. Fluid diff with tombstones moved to the format freeze (M4).
2. Server-core crate extraction — a compiler-enforced client/server boundary. Pure refactor (M3).
3. Observer-set streaming — `ChunkStreamingManager` generalizes (M3). M3 also runs singleplayer through the same boundary in process, with nothing serialized (Runtime spine above).
4. Loopback milestone — client and server as separate processes on one machine over in-memory channel. This is where 90% of protocol issues surface (M7).
5. LAN transport, artificial latency/loss testing, internet exposure (M7).

Legacy summary retained for cross-reference: server is authoritative for generation, fluid simulation, and override application; clients receive chunk deltas, not full chunks; determinism guarantees enable client-side terrain generation. See the companion proposal for wire model, desync taxonomy, and channel/transport recommendations.

### Engine versioning and release policy (added v1.6)

The companion `roadmap.md` is authoritative for sequencing, milestone exit gates, and compatibility boundaries. Since v1.13 this section plans by milestone, as the roadmap has since v2.0. The architectural commitments:

- **Pre-1.0 scheme**: `0.MINOR.PATCH`. A minor version is cut when a roadmap milestone completes or compatibility changes, whichever comes first; patches are fixes only. Version numbers are assigned when a release is cut, never planned in advance. Post-1.0, a major version is a breaking change to save compatibility, asset formats, or network protocol that migrations cannot bridge.
- **Save compatibility boundary**: wipe-on-bump (§3 interim) is permitted until the format freeze (M4) completes. Per-layer versioning lands there as the final planned blob-format redesign. From M11, saves migrate forward; at 1.0, save compatibility is a user-facing promise.
- **Protocol version = build version** (from M7): mismatched clients are refused, never partially accommodated.
- **1.0 is a readiness bar, not a shipped title.** `roadmap.md` §5 defines it as an eleven-domain rubric (world authoring, tooling maturity, runtime capability, game-facing API, multiplayer, performance, data durability, extensibility, coherence, documentation, distribution). A demonstration title is evidence that the rubric passes, never a substitute for it.
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

**Settled in v1.13** as part of the runtime spine (see "Time" and "Scheduled, random and block ticks" under Runtime spine above): one 30 Hz tick with a `SimTick` counter, sub-rates keyed by tick number, a fixed phase order, separate simulation and presentation time, and stateless seeded randomness. Scheduled, random, block and region ticks are designed there and built with their first consumer.

**Built in M3** (R2, added v1.18): the three clocks are gone. `world/tick.rs` holds the rate, the counter, the tick schedule and its phases; the client's `TickDriver` and `PresentationClock` live in `ecs/clocks.rs`.

### Registry identity and save-embedded mapping (added v1.8)

`MaterialId` and every other registry ID are **runtime interning details**. The authoring *and persistence* identity is a stable namespaced string (`voxulacrum:oak_slab`), and **every save embeds its own name↔ID mapping** so that a registry reorder, insertion, or mod-added entry cannot silently reinterpret existing worlds.

This revises the v1.0 statement that "stable IDs are the wire format." Numeric IDs remain the in-memory and on-wire encoding *within a session or a version-locked connection*; what changes is that a persisted or transmitted payload always travels with the mapping needed to interpret it.

**This records a live defect.** The current implementation persists bare numeric IDs with no mapping. Nothing breaks today because the registry has not been reordered, and nothing will break until it is — at which point every existing save silently reinterprets. The fix is scheduled with the format redesign; it is cheap now and data-corrupting later.

### Packages and content namespaces (added v1.12)

Content loads as **packages**: a directory with a `package.ron` manifest at its root and its content beside it in a fixed layout (`materials.ron`, `prefabs.ron`, `foliage.ron`, `play.ron`, `graphs/`, `libraries/`, `blueprints/`, `species/`). The manifest names the package's id, its namespace (the id unless it names another), its save directory under `saves/`, its version, the engine versions it supports, the world it starts and its dependencies. Packages are data-only until the WebAssembly runtime; one that declares code modules is refused.

- **Every registry entry is `namespace:name`.** A bare name in content resolves in the namespace of the content holding it; a qualified name reaches into another loaded package. This is the in-memory half of the identity rule above; the save-embedded mapping is the other half.
- **The engine package.** A built-in package, `voxulacrum`, declares one standard library per native kernel, beside the kernel's own code, so the boundary cannot drift from the computation and content never repeats the declaration. Its id and namespace are reserved.
- **Dependencies** load from `packages/<id>/` before the packages that need them, in dependency order and then by id, so the same list loads the same way everywhere. Missing, cyclic, wrong-version and namespace-sharing dependencies are refused.
- **Tables whose ids saves store stay with the world's own package.** Materials, prefabs and foliage are authored with numeric ids that voxels, scatter instances and paint edits persist, so two packages cannot merge them until the save carries its mapping; a dependency that ships one is refused. Libraries, blueprints and species merge across packages. Chunk tags do persist library ids, numbered after the libraries loaded before them, so adding a dependency renumbers them; they also persist the manifest's numeric zone and biome ids. Tags only drive invalidation, which is why nothing has broken, but D3's mapping must cover them (corrected v1.14).
- **Play policy is package data.** A package's `play.ron` holds the player's body, movement and reach, the spawn rule and the build tool. The controller, targeting and tools are engine mechanisms; a package without `play.ron` has no Play mode.
- **Games build on the facade crate alone,** in their own repositories, and CI checks it (`boundary-check`, including `--manifest-path` for a workspace outside the engine).

### Engine object model (added v1.14)

What the engine is made of, as content and game code see it: the primitives, the asset types, the registries that are its extension points, and the rules by which packages add to and replace each other's assets. Several items lock parts of this in (the render world's camera description in R4, material definitions and textures in V5 and V7, registration from game code in B9 and B10, and the save format at M4), so it is designed once, here, before any of them invents its own answer (roadmap B8). This section settles concepts and rules. Each item that builds a type owns its data layout. Where the code differs today, the difference is listed under "Interim shapes" at the end.

#### Terms

Several words had come to mean more than one thing. In this document each means one:

- **Region** is a world region (defined below). A node of the region graph is an **air region**; the region graph keeps its name.
- **Layer** is a chunk layer. Detail paint's `DetailLayer` is a channel inside the detail-paint layer.
- **Tag** is a registry tag (defined below). §3's per-chunk records of contributing zones, biomes and libraries are **chunk tags**.
- **Block** is a spatial unit, never a type. What a voxel is made of is its material.
- **Prefab** retires with the interim foliage stack (§6). A data-defined entity is an **entity type**; a piece of voxel content is a clip.
- Where a package sits among the packages a world loads is its **load order**.
- A type's **owner** is the layer of the engine/game split that provides it (`target-architecture.md`, "Engine, kit, game and mods"): the **core** (the engine behind the facade), the **kit** (opinionated systems in the engine repository, built on the facade alone, which a game may use or replace), or the **game** (its own repository, loaded as a package).
- A type's **side** is shared, client or server, as under "Crates and sides" in the Runtime spine above.

#### Primitives

Each definition states the target. The line after it says where the code is today, where that differs.

- **Voxel.** The unit of storage, collision and simulation: a 0.5 m cube (§8 D1). A voxel holds one palette entry, which is either a material with that material's properties, or a part of a fixture. **Properties** are a material's states: a few named values with declared semantic types (an axis, a horizontal direction), which is what lets a clip rotate them correctly (D9). Visual variants are not properties: they are presentation, chosen by position hash (V7), and never stored. A voxel is geometry and nothing else (§2); fluid, paint and scatter live in their own layers.
  *Today:* a 1 m cell holding a `ShapeId` and a `MaterialId`, with no properties.
- **Block.** A 2×2×2 group of voxels on the 1 m lattice. Players build and generation writes at block scale by default, and a block whose eight voxels match is stored once (two-level storage, D2). Texture coordinates anchor to this lattice (V5). A block is not a type and has no registry.
  *Today:* the voxel is 1 m, so a block and a voxel coincide.
- **Chunk.** A cube of 32³ voxels (16 m) within a grid: the unit of residency, generation, persistence, meshing and change reporting. Chunks are cubic and stream vertically (D10). Each carries chunk layers and a version, which the server's `changed` feed reports (Runtime spine).
  *Today:* 32³ 1 m cells, loaded in fixed vertical columns.
- **Layer.** One kind of per-chunk data with its own coverage, update cadence, serializer and version (§3): voxels, fluid, detail paint, scatter, decals, light and scheduled-tick queues, and package-defined layers such as heat or gas. Each splits into generated content and overrides (§9). Layers are registrable (D4).
  *Today:* a fixed set, versioned together under `BLOB_VERSION`.
- **Grid.** A set of chunks with a transform. Every world system addresses a grid through a handle rather than reaching a world singleton (§8 D3). The terrain is the large, static, axis-aligned grid. Detached grids may be static (D8, M4) or move, as kinematic, dynamic or powered grids (M9).
  *Today:* one implicit world.
- **Clip.** A piece of voxel content outside any grid: a palette by name, voxels in the chunk's two-level layout, a structure-void mask, fixtures, block-entity data, ports, sockets, optional entities and metadata (D9). One format carries worldgen structures, blueprints, copy and paste, packed vehicles, extract output and test fixtures, and it transforms through all 48 orientations.
  *Today:* blueprint v1, yaw-only, with cells naming their material.
- **Fixture.** An indivisible object occupying a footprint of voxels, such as a door, a table or a machine (D11). Each footprint voxel records its fixture and its offset in it, so any voxel finds the anchor in one step, and breaking any of them removes the whole fixture. A fixture type declares its footprint, port table and orientation rules. Placement is voxel-aligned.
  *Today:* none.
- **Sub-voxel model.** A model with a collision shape, anchored at one voxel, for things smaller than a voxel or not made of cubes: a torch, a fence post, a flower (D11). Sub-voxel models replace the procedural scatter props.
  *Today:* `PrefabShape` props in the scatter layer.
- **Entity.** Anything dynamic that isn't voxels. Entities live in one global ECS store rather than in chunks, are found through a separate spatial index, persist by region and keep a stable id across save, load and network (roadmap §6.3). An entity type defines one in data (M8).
  *Today:* the player, and nothing else.
- **Assembly.** A composite entity: a root and a tree of parts (grids, models, sprites, bare bodies, effect anchors) joined by physics joints, each part with its own components, defined in data with its IK chains and locomotion controller (M9).
  *Today:* none.
- **Region.** A world region: an aligned group of chunks within one world instance. It is the unit that owns per-region generation parameters (§8 D4), entity persistence, the region tick and region transformation (roadmap §6.26). D5 sets its size.
  *Today:* none; generation is a function of seed, graphs and coordinates only.
- **World instance.** One world: a seed, a generation configuration and its grids, with its own residency and persistence, sharing the session's registries with any other instance (roadmap §6.27). Persistence is keyed by instance from M4 (D5), and sub-dimensions build on it in M10.
  *Today:* one world per save directory.
- **Package.** The unit that content, and later code, loads as: a directory with a `package.ron` manifest and its content in a fixed layout ("Packages and content namespaces" above). Its kind decides what it may do ("Package kinds" below).
  *Today:* data-only; code modules are refused until M11.
- **Registry.** The table of one asset type, keyed by namespaced name. Numeric ids are assigned at load as an interning detail, entries come from packages in load order, and any save that stores ids embeds the name-to-id mapping ("Registry identity and save-embedded mapping" above). Every registry is an extension point ("Extension points" below).
  *Today:* materials, prefabs, foliage, libraries, blueprints and species. The first three are numbered by hand in their files.
- **Tag.** A named set of entries from one registry, such as `sandbox:flammable` over materials, so a rule can match a category instead of listing names. A tag is an asset in its own right. Unlike other assets, a tag merges: each package's entries add to it (override rules below).
  *Today:* none.
- **Event.** A typed notice that something happened. Events go to subscribers, never to call sites (roadmap §6.7). The server publishes each tick's events in that tick's `ServerUpdate` (Runtime spine). An event type declares its side and, from M7, whether it replicates. Game code subscribes from M3 (B9) and may declare event types of its own.
  *Today:* `WorldEvent`, nine variants, all local, published one frame late.
- **Command.** A validated request to change state. It carries an origin and is applied only through the mutation door, at a tick boundary (roadmap P5). The door is where validation, the mode gate, persistence marking, event emission and, from M7, replication happen. Every system's writes take this shape, not only voxel edits (`target-architecture.md`, "Five API verbs"). A client sends commands as the `ClientCommand` kinds (Runtime spine).
  *Today:* `MutationCommand` with `Authoring`, `PlayTime` and `System` origins, applied at once rather than at a tick boundary.

#### Asset types

Every type content can name, grouped as in B8's starting catalog. Voxels, blocks, chunks, grids, regions and world instances are runtime structures rather than assets: content reaches them through the types below, so a packed grid is a clip and a world instance starts from a world definition.

A type with both simulation and presentation fields loads as **two records under one name**: a **definition**, shared and covered by the join handshake's content hashes, and an **appearance**, client-only. One authored entry may hold both. Overriding always replaces a whole record, never single fields, so the two halves are what let a texture pack restyle a material without being able to change how it behaves. A side of "shared + client" below means that split.

The override column uses these terms; the rules behind them are under "Override and layering rules" below.

- **Replace:** a package that depends on the asset's package may replace the whole record by name.
- **Replace; appearance open:** the definition as Replace; the appearance may also be replaced by a resource pack.
- **Open:** presentation only, replaceable as Replace and also by resource packs.
- **Merge:** each package's entries add to it, and a package that depends on its owner may replace it whole.
- **Add only:** new entries only. An existing entry's meaning is part of a save or wire format, so replacing it would reinterpret data.
- **Game only:** added or replaced by the game package alone.
- **None:** never replaced.

The milestone is when a type is built. For a type that exists, it is the item that next reshapes it. A row marked *interim* is replaced by the type it names and gets no new work.

**World data**

| Type | Owner | Registry | Side | Simulation fields | Presentation fields | Override | Milestone |
|---|---|---|---|---|---|---|---|
| Material | core | materials | shared + client | solid, opaque, hardness, permeability, supports flora, gameplay light emission, physics material, properties, tags | color, textures, palette ramp, surface function, filtering, variants, edge sharpness, surface sounds | Replace; appearance open | Exists; save mapping M4 (D3); split into definition and appearance M5 (V5) |
| Material properties (block states) | core | none: declared on their material | shared | named values with semantic types | none | with their material | M4 (D2, D9) |
| Visual variant | core | none: part of a material's appearance | client | none | per-face alternatives chosen by position hash | with the appearance | M5 (V7) |
| Physics material | core | none: fields of a material definition or a collider | shared | friction, restitution, buoyancy | none | with its owner | M8 |
| Chunk layer type | core mechanism; kit and games register | layers | declared per layer, shared by default | coverage, serializer, version | none | Add only | M4 (D4) |
| Fixture type | core mechanism; game content | fixtures | shared + client | footprint, anchor, port table, orientation rules, block-entity schema | model, textures | Replace; appearance open | M4 (D11) |
| Sub-voxel model type | core mechanism; game content | sub-voxel models | shared + client | collision shape, anchor rule, destruction policy | model, sway | Replace; appearance open | M4 (D11) |
| Clip | core | clips | shared | voxels, mask, fixtures, block-entity data, ports, sockets, entities, protected-volume flag | thumbnail | Replace | M4 (D9) |
| Registry tag | core | tags, one set per registry | shared | member names | none | Merge | M8, or earlier with its first consumer |
| World definition | core | worlds | shared | seed, sea level, graphs, zones, biomes, default region parameters | none | Replace | Exists as `world.manifest.json`; named M4; several per package M10 |
| Detail channel and species (`foliage.ron`), *interim* | core | foliage | shared + client | ids, density meaning | color | Replace; appearance open, once D3 lets a dependency ship it | Exists; a registered layer M4 (D4); replaced by sprites and billboard clumps M6 |
| Scatter prop (`prefabs.ron`), *interim* | core | prefabs | shared + client | id, footprint | procedural shape, color, scale | None; it retires | Exists; replaced by sub-voxel models M4 (D11) and sprites M6 |

**Visual.** Every visual type is client-side presentation, except a decal type, whose placements are world state.

| Type | Owner | Registry | Side | Fields | Override | Milestone |
|---|---|---|---|---|---|---|
| Palette | core | palettes | client | colors for the palette pass | Open | Exists, by path; named M5 (V2) |
| Shader | core | shaders | client | WGSL source | Game only | Exists, by path; named M5 (V3) |
| Texture | core | textures | client | image, resolution, frames, filtering hint, optional normal and emissive maps | Open | M5 (V7) |
| Surface function | core mechanism; game content | surface functions | client | shader function filling albedo, normal, emission and material id | Replace | M5 (V5) |
| Lighting model | core mechanism; built-ins in core | lighting models | client | shader function and its parameters | Game only | M5 (V5) |
| Render pass | core mechanism; game content | render passes | client | shader, insertion point, inputs | Replace; a mod only at the post-process points | M5 (V6) |
| Presentation preset | core; example presets in the kit | presentation presets | client | render resolution, snapping, filtering, anti-aliasing, palette pass, outlines, lighting model, post-process profile | Open | M5 (V2) |
| Post-process profile | core | none: part of a presentation preset | client | post-pass parameters | with its preset | M5 (V2) |
| Sprite, depth sprite, billboard clump | core | sprites | client | image, depth and normal maps, pivot | Open | M6 |
| Particle effect | core | particle effects | client | emitter, spawn rules, budget class | Open | M6; game-facing M8 |
| Decal type | core | decals | shared + client | definition: faces it may mark; appearance: texture | Replace; appearance open | M6 |
| Sky and atmosphere profile | core | atmospheres | client | sky gradient, sun, clouds, fog | Open | M6 |
| Model | core | models | client | mesh, material slots, skeleton (glTF) | Open | M8; sub-voxel models name one from M4 |

**Generation.** Everything here is shared and Tier 1, because clients generate terrain themselves and verify it by hash.

| Type | Owner | Registry | Side | Fields | Override | Milestone |
|---|---|---|---|---|---|---|
| Graph (world, zone, biome, detail) | core | graphs | shared | nodes, parameters | Replace | Exists, by path; named M4 |
| Graph library | core | libraries | shared | boundary, and a body or a kernel | Replace; the engine package's kernel-backed declarations: None | Exists |
| Worldgen kernel | core mechanism; packages register | kernels | shared | native function, boundary, version | Replace; the engine's own: None | M4 (B10) |
| Biome | core | biomes | shared + client | definition: graph, detail graph, parameters; appearance: tint, atmosphere | Replace; appearance open | Exists as manifest entries with numeric ids; named M4 |
| Zone | core | zones | shared | graph | Replace | Exists as manifest entries with numeric ids; named M4 |
| Blueprint, *interim* | core | blueprints | shared | anchor, cells, destruction policy, protected-volume flag | Replace | Exists; becomes a clip M4 (D9) |
| Tree species | core | species | shared | skeleton generator parameters | Replace | Exists; the generator outputs clips M4 (D9) |
| Macro tile layer | core | none yet | shared | continent, range and drainage parameters | Replace | Unscheduled (`macro-terrain-research.md`) |

**Entities and behavior**

| Type | Owner | Registry | Side | Fields | Override | Milestone |
|---|---|---|---|---|---|---|
| Play policy (`play.ron`) | kit | none: one per package | shared | body, movement, reach, spawn rule, build tool | Replace | Exists; kit controller parameters M5 (E2) |
| Entity type (entity templates) | core mechanism; game content | entity types | shared + client | definition: components, collider, replication, persistence and tier rules; appearance: model or sprite, animations, sounds | Replace; appearance open | M8 |
| Component type | core mechanism; registered from code | components | declared per component | schema, saved or not, replication rule, version | Add only | M8 |
| Animation clip and graph | core | animations | shared + client | definition: state machine, root motion, events, hitbox tracks; appearance: pose tracks | Replace; appearance open | M8; IK chains M9 |
| Assembly definition | core mechanism; game content | assemblies | shared + client | definition: parts, joints, IK chains, locomotion controller; appearance: part visuals | Replace; appearance open | M9 |
| AI agent profile | core | agent profiles | server | size, step, jump, drop, capabilities | Replace | M9 |
| Behavior definition | kit | behaviors | server | a behavior tree, utility scorer or state machine in data | Replace | M9 |
| Interaction kind | kit | interaction kinds | shared | targeting and placement rules | Replace | M8 |
| Logic channel and network type | core mechanism; game content | channels, network types | shared | channel name; solver, solve mode, unload policy | Add only | M9 |
| Reaction table | core schema; game content | a data table | shared | ignition, heat capacity, phase change per material | Replace | M10 |
| Data table type | core mechanism; declared by the kit or game | one registry per declared type | declared per type | the record schema; records follow it | Add only for the type; Replace for its records | M8 |

**Audio and UI.** Everything here is client-side presentation. Sounds play from replicated events, never from a second simulation (roadmap §6.14).

| Type | Owner | Registry | Side | Fields | Override | Milestone |
|---|---|---|---|---|---|---|
| Sound | core | sounds | client | audio data, streaming hint | Open | M6 |
| Sound event | core | sound events | client | sounds, variation, bus, voice limit | Open | M6; game-facing M8 |
| Music state and soundscape | core | music states, soundscapes | client | director states, tracks, ambience beds | Open | M8 |
| UI layout | core mechanism; game content | UI layouts | client | layout, data bindings, named extension points | Replace | M8 |
| UI theme | core | themes | client | styles | Open | M8 |
| String table | core | strings | client | text per locale | Open; a resource pack may add a locale | M8 |
| Font | core | fonts | client | font data | Open | M8 |
| Icon and cursor | core | icons | client | images | Open | M8 |

**Packaging and configuration**

| Type | Owner | Registry | Side | Fields | Override | Milestone |
|---|---|---|---|---|---|---|
| Package manifest | core | none: one per package | read by both | id, namespace, kind, version, engine range, dependencies, modules | None | Exists (B6) |
| Input action | core mechanism; games register | input actions | shared: action names travel in the `InputFrame` | name, value type, context | Add only | M5 (E1) |
| Input bindings | core | none: one default set per package, then the player's own | client | default keys and buttons per action | Open; the player's rebinding wins | Exists, by path (`assets/input.ron`); M5 (E1) |
| Parameter preset, *interim* | core | none: files under `params/` | client | the engine's tuning parameters | None; not package content | Exists; becomes presentation presets and settings M5 (V2) |
| WebAssembly module | core | none: listed in its package's manifest | declared per module | code | None; replaced by its package's next version | M11 |
| Capability | core | none: requested per module | per module | own data folder, listed network domains, clipboard | None | M11 |

**Game content.** These belong to a game (roadmap §3, "The engine/game boundary"), so the engine has no type for them. Each is built from a generic engine type:

| Content | Built from |
|---|---|
| Items | a data table whose record type the kit's inventory framework declares (roadmap §6.19), plus an entity type for an item lying in the world |
| Recipes and crafting | a data table the game declares |
| Loot tables | a data table using the kit's weighted selection |
| Spawn tables | a data table the kit declares (roadmap §6.9), read by spawning |
| Creature behaviors | behavior definitions and entity types |
| Progression, quests and dialogue | data tables and string tables |
| Combat rules and balance | game code and data tables |

#### Extension points

Every registry is an extension point, and so are the code hooks that have no registry of their own. Each is filled through one or more **deliveries**:

- **Data:** content files in a package. Available now for the types that exist.
- **Native:** a Rust crate built on the facade, which is how a game's own code arrives. Systems and event subscriptions from M3 (B9); registration into registries from M4 (B10).
- **WebAssembly:** a module in a package, against a game's mod API (roadmap §6.9). From M11, and never for anything that must be compiled into the engine.

A data-only package can fill only what data can describe; a type whose entries carry code (a layer's serializer, a kernel, a system) needs native code or a module. The **tier** is the determinism tier the type's results must meet (Runtime spine, "Determinism tiers"); "none" is presentation, which only has to look right.

| Registry or hook | Delivery | Side | Tier | From |
|---|---|---|---|---|
| Materials | data, native, WebAssembly | shared + client | 1 (definition); none (appearance) | exists |
| Tags | data, native | shared | that of the registry tagged | M8 |
| Chunk layers | native, WebAssembly | declared per layer | 1 for layers generation or the door writes; declared otherwise | M4 (D4, B10) |
| Fixtures, sub-voxel models | data; native for fixture behavior | shared + client | 1 | M4 (D11) |
| Clips | data; native for capture | shared | 1 | M4 (D9) |
| World definitions, graphs, biomes, zones | data | shared | 1 | exists |
| Graph libraries | data | shared | 1 | exists |
| Worldgen kernels | native, WebAssembly | shared | 1 | M4 (B10) |
| Blueprints, species, foliage, prefabs (interim) | data | shared | 1 | exists |
| Palettes, textures, sprites, particle effects, atmospheres, models, presentation presets | data | client | none | M5 to M8 |
| Shaders, surface functions, lighting models | data (WGSL, validated with naga) | client | none | M5 (V3, V5) |
| Render passes | data (WGSL), native | client | none | M5 (V6) |
| Decals | data | shared + client | 1 (placement); none (appearance) | M6 |
| Camera modules | native | client | none | M5 (V8) |
| Entity types, animations, assemblies | data, native | shared + client | 2 (definition); none (appearance) | M8, M9 |
| Components | native, WebAssembly | declared per component | 2 | M8 |
| Systems | native from M3, WebAssembly | declared per system | 2; a system changes world state only through commands | M3 (B9) |
| Event types | native, WebAssembly | declared per type | 2 | M8 |
| Command kinds | native, WebAssembly | server-validated | 2; the door's voxel handlers stay 1 | M8 |
| Interaction kinds | data, native | shared | 2 | M8 |
| Agent profiles, behaviors | data; native for custom behavior nodes | server | 2 | M9 |
| Logic channels, network types | data for channels; native for solvers | shared | 1 | M9 |
| Data table types and their records | native or WebAssembly declare a type; data fills it | declared per type | declared per type | M8 |
| Input actions | data, native | shared | 2 | M5 (E1) |
| Input bindings | data | client | none | exists |
| Console commands and variables | native | declared per command | none | M5 (T1) |
| Sounds, sound events, music, soundscapes | data | client | none | M6, M8 |
| UI layouts, themes, strings, fonts, icons | data; native for UI layouts | client | none | M8 |
| Platform services | native | client | none | M11 |

**The worldgen kernel registry.** A kernel is native computation behind a library boundary ("Standard libraries", §4). A registered kernel must be a pure function of its inputs and the seed: no state between calls, no wall clock, no thread identity. It follows the Tier 1 rules under "Determinism" above (`libm` for transcendental functions, no fused multiply-add, no parallel float reductions), and it carries a version. The version feeds the content hash a client checks at join, so changing what a kernel computes without bumping it is a defect the handshake cannot catch. The engine's own kernels are the engine package's and cannot be replaced. WebAssembly kernels (M11) additionally run with NaN canonicalization on and relaxed SIMD off, the two places WebAssembly floating point is allowed to differ between machines.

#### Override and layering rules

- **Assets are referenced by namespaced name, never by file path.** A path appears only inside a package, where its fixed layout maps files to the names they define. A reference in content, in a save or on the wire is always a name (the numeric id is its interned form within a session).
- **Load order** is the existing one: dependencies before the packages that need them, then by id ("Packages and content namespaces" above). Resource packs load after every other package, in the order the player chooses.
- **A package adds names only in its own namespace.** Writing a bare name defines it there.
- **A package may replace a name from a package it depends on**, directly or through other dependencies, by writing its qualified name. The replacement takes the whole record ("Asset types" above). Because a replacing package always loads after the package it replaces, the later one in load order wins. What the dependency requirement adds is that two packages replacing the same name, where neither depends on the other, is a load error naming both packages and the name, rather than a winner picked by id order. A game that wants both resolves it by depending on both and replacing the name itself.
- **Kinds limit what a package may replace** ("Package kinds" below). A library replaces nothing; a resource pack replaces only appearances and Open types.
- **Tags merge.** Each package's entries add to a tag in load order. A package that depends on the tag's owner may instead replace the tag whole, which it states explicitly.
- **Add-only and None types are never replaced**, by any package.
- **Replacing keeps identity.** A replaced record keeps its name, so its saved id still maps to it (D3). Removing the package that replaced it restores the original. Removing the package that defined it leaves placeholders that keep the name (roadmap P18, D7).
- **Simulation must match the server; presentation need not.** For every shared registry, the server's join handshake carries a content hash of the resolved records, after every replacement. Appearances and client-only types are outside it, so a client's resource pack never fails a join and never changes what the world does.
- **Until D3 lands**, materials, prefabs and foliage stay with the world's own package ("Packages and content namespaces" above), so nothing replaces them yet.

#### Package kinds

| Kind | Starts a world | Loads as | Adds names | Replaces | Side |
|---|---|---|---|---|---|
| `Game` | yes, the one it names | the root package, or a dependency of the mods for it | yes | names from its dependencies | any |
| `Mod` | no | a package the player enables, or a dependency | yes | names from its dependencies | any |
| `Library` | no | a dependency only | yes | nothing | any |
| `ResourcePack` | no | a client-side layer the player orders | appearances and Open types | appearances and Open types from any loaded package | client only |

**Why `Library`.** Shared content and code (a block set, a kernel collection, a UI kit) need a kind that others depend on without it changing them. Because a library cannot replace anything, taking a newer library can add names to a world but never alter one, and a library never shows up in a player's mod list as something to switch on.

**Why `ResourcePack` is a kind rather than a flag.** Every rule that applies to it differs from the others: its load order comes from the player rather than from dependencies, it is client-only, it may replace only presentation, it is outside the handshake's content hash, and it can be swapped while a world runs, reloading client registries only. A flag on `Mod` would need each of those as an exception. A resource pack names no dependencies; a replacement naming something no loaded package defines is reported and skipped, so an out-of-date pack degrades rather than refusing to load.

Only a `Game` may name a world. The finer per-module sides and client requirements of `target-architecture.md` ("Packages, fault isolation and API stability") arrive with code modules in M11, inside these kinds.

#### Shader reach

How far a package can change rendering, as a ladder from least to most invasive. Each rung is a different promise, so each has its own users and its own stability.

| Rung | Games | Mods | Resource packs | Stability |
|---|---|---|---|---|
| 1. Presets and parameters (V2) | yes | yes | yes | stable from M8 |
| 2a. Surface functions (V5) | yes | yes | no | a versioned function signature, stable from M8 |
| 2b. Lighting functions (V5) | yes | no | no | as 2a |
| 3. Passes at named insertion points (V6) | at every point | before post and after post only | no | point names stable from M8; the inputs each point offers are versioned |
| 4. Replacing a default shader by name | yes | no | no | internal: no promise, and expected to break on engine updates |
| 5. Replacing a whole render stage | not offered | no | no | none |

**Why the line falls here.** Rungs 1 to 3 are what P19 needs: strict pixel art, textured 3D and hybrids are presets, surface functions and lighting models over one pipeline, and a surface function fills albedo, normal, emission and material id while the engine keeps doing lighting, shadows, edges and the palette pass around it. That keeps the pipeline free to change underneath, which is why those rungs can carry a promise. Mods stop short of lighting and of passes inside the scene: their WGSL is validated with naga, and confining it to surface functions and post-process passes keeps a mod from breaking an invariant other systems depend on, such as the visibility classification, which a shader may consume but never be (§1). Resource packs change data only, never code. Rung 4 exposes engine shader source that changes with every release, so a game may use it at its own risk and nothing else may. Rung 5 is not a surface at all: a game that needs a stage replaced is showing a missing hook, and the answer is the hook (roadmap P12), not a fork of the renderer behind the facade.

#### Camera boundary

The core makes no camera assumption (roadmap P20). It takes a **camera description** per view and nothing else, which is the minimal description the Runtime spine's render world already records:

- **View** and **projection**, with the view-projection.
- **Frustum**, for culling.
- **Area of interest**: a center, a horizontal radius and a vertical range, which the client sends as its observer area (`SetInterest`).

Core systems read only these. Streaming takes the area of interest through observers ("Residency" above); culling and shadows take the frustum; picking takes the view-projection; level of detail and audio take positions from the description, never from a projection type. One view is active through M5; render-to-texture (minimaps, portraits, item icons) adds views, each with its own description.

**Camera modules** are kit code, registered natively on the client ("Extension points" above). A module turns input and game state into a description, and keeps a block of its own that only it reads. Everything that makes a camera a particular kind of camera lives there:

- framing and projection choice (isometric, dimetric, orthographic, perspective);
- texel snapping and the sub-pixel offset it hands the upscale pass;
- rotation steps and their easing;
- cutaway and underground visibility policy, meaning which world-state classification to apply and when, since the classification itself is world state (§1);
- follow behavior, such as the floor-anchored follow of roadmap §6.11;
- the audio listener position;
- deriving its area of interest, such as the isometric camera's altitude-aware view test ("Chunk streaming" above).

The isometric camera is the kit's default module, and P8's rule of no head-on resting views is its rule, not the engine's. Observers are not cameras: a headless server takes its observers from its command line or script. The testbed's perspective camera (V8) is what keeps the boundary honest.

#### Interim shapes

| Today | Target above | Migrates in |
|---|---|---|
| One material record mixes presentation fields (`color`, `render_delegated`) with simulation fields; `hardness`, `permeable`, `supports_flora` and `sharpness` are loaded but no system reads them | A definition and an appearance under one name | V5 (M5) |
| Materials, prefabs and foliage are numbered by hand and come only from the world's own package | Any package, identity by name, with replacement | D3 (M4) |
| Chunk tags persist numeric zone, biome and library ids with no mapping | Covered by the save's mapping | D3 (M4) |
| `world.manifest.json` names graphs by path, and zones and biomes by number | World definitions, graphs, zones and biomes named | M4; no item owns it yet |
| Shaders, palettes, input bindings and parameter presets are engine files read by path under the asset root, in no package | Named assets in the engine package, added to and replaced as the catalog says | V3, V2 and E1 (M5) |
| `PackageKind` has `Game` and `Mod`, nothing branches on it, any kind may name a world, and no package can replace another's asset | Four kinds and the override rules above | Before V7 (M5), whose gate is the first client-only override |
| Library kernels are the closed `LibraryKernel` enum | The kernel registry | B10 (M4) |
| The foliage stack: procedural `PrefabShape` props, skeleton trees drawn as capsule impostors, canopy cores, the tier-1 detail-paint schema and `render_delegated` | Clips (D9) for voxel trees and structures, fixtures and sub-voxel models (D11) for props, registered layers (D4) for detail and scatter, sprites, depth sprites and billboard clumps for grass and canopies | M4, M6 |
| One isometric camera in the app; streaming evaluates its view itself, and render code assumes an orthographic projection | Camera modules in the kit over the camera description | R4 and R6 (M3), V8 (M5) |

The foliage stack gets no new work in the meantime (roadmap §8, v2.1). The skeleton generator stays, as a generator that outputs clips.

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

### Editor on the engine (revised v1.13)

- The editor is its own binary built on the engine library (`roadmap.md` P9). It authors on the engine's one renderer; it is not a separate application with a renderer of its own.
- It is a client of the runtime spine: its world changes are `Author` commands through the mutation door, the same door hot reload and play use.
- Playtesting launches the game as a separate process that the editor attaches to over a debug protocol (M8).
- The editor uses egui, hosted by the engine. egui stays editor tooling; it is not the game UI stack.
- A graph-type picker selects which graph type is being edited.

**Interim shape:** the editor still lives inside the one `voxulacrum-app` binary. It becomes its own binary with the debug protocol (M8).

---

## 13. Glossary

- **Air region**: a connected volume of air, one node of the region graph. Always qualified, because a bare "region" is a world region.
- **Anchor voxel**: the integer cell that owns a foliage instance for purposes of player interaction.
- **Appearance**: the client-only half of an asset that has both kinds of field, holding its presentation. Replaceable by resource packs; outside the join handshake's content hash.
- **Area of interest**: the region an observer needs resident: a center, a horizontal radius and a vertical range. The server's resident set is the union of all observers' areas plus chunks with active fluid.
- **BiomeGraph**: graph type defining terrain density, material, fluid output, and fade behavior for one biome.
- **BiomeParams**: per-biome typed scalar-metadata sidecar. Addressable by name from the biome manifest entry and by `BiomeParam(name)` nodes inside a `BiomeGraph`. Home for `traversal_smoothing_distance` and future biome-scoped scalars.
- **Block**: a 2×2×2 group of 0.5 m voxels on the 1 m lattice. A spatial unit, never a type.
- **Camera description**: what the core takes from a camera, per view: view, projection with view-projection, frustum and area of interest.
- **Camera module**: kit code that turns input and game state into a camera description, and owns everything particular to one kind of camera. The isometric camera is the default one.
- **ChunkTags**: per-chunk metadata identifying which Zone, Biomes, and Libraries contributed to it. Drives invalidation. Called chunk tags, to keep them apart from registry tags.
- **Clip**: one format for any piece of voxel content outside a grid: structures, blueprints, copy and paste, packed vehicles, test fixtures.
- **CSE cache**: chunk-scoped cache that ensures each node's output is computed at most once per chunk evaluation.
- **Data table**: a record type the kit or a game declares with a schema, side and tier, filled by packages with named records. What game content such as items, recipes and loot is built from.
- **Definition**: the shared half of an asset, holding its simulation fields. Covered by the join handshake's content hash.
- **DetailGraph**: graph type defining foliage paint and scatter placement, distinct from terrain generation.
- **DetailLayer**: 2D paint map over a chunk's XZ footprint storing per-column foliage density.
- **DetailTexel**: a single cell in a DetailLayer: species, density, tint, flags.
- **Determinism tier**: Tier 1 results are bit-identical on every machine (generation, the mutation door's handlers, fluids); Tier 2 results are reproducible on one machine (the player step, later physics and AI).
- **Enclosure factor**: per-vertex value indicating how enclosed a position is by surrounding terrain. Used for fog and audio.
- **Extension point**: a registry or code hook a package or game fills, through data, native code on the facade, or a WebAssembly module.
- **Extract**: the once-per-frame client step that fills the render world from server updates and snapshots.
- **Face axis**: which of the 6 cardinal directions a face belongs to. Determines palette ramp. No diagonal values exist.
- **Fade factor**: 0–1 weight for blending between neighbor biomes at boundaries.
- **Fixture**: an indivisible object occupying a footprint of voxels; breaking any footprint voxel removes it whole.
- **FluidCell**: a single cell's fluid state: type, mass, flags.
- **FluidFillMode**: chunk-level default for fluid presence. Enables ocean fast path.
- **FluidOutput**: `BiomeGraph` terminal for water bodies. Renamed from `FluidProvider` to describe the terminal position, not the role.
- **FrameStage**: ordered schedule of the client's per-frame work: Input, Tick, Sync, Extract, Render, PostFrame. Until M3's render world and boundary land, one FrameStage schedule (Input, Simulation, Meshing, UniformWrite, Render, PostFrame) runs simulation and rendering together.
- **Generated content**: chunk data reproducible from seed + graph. Not serialized in detail.
- **GraphOutput**: terminal node placed inside a producing graph, marking a named typed output that other graphs (via `GraphRef`) can consume.
- **GraphRef**: source node placed inside a consuming graph, referencing a target graph and one of its `GraphOutput` names. Resolved via the manifest at evaluation time.
- **LibraryGraph**: reusable subgraph referenced by any other graph type. Declares typed named inputs and outputs at its boundary.
- **LibraryKernel**: enum matching each standard library's identity to a native implementation. Backs standard libraries whose authored file captures identity and boundary but delegates computation to native code.
- **LibraryRef**: node referencing a `LibraryGraph` by ID. Exposes the referenced library's boundary as dynamic pin descriptors on the referring node.
- **Load order**: the order a world's packages load in: dependencies first, then by id, then resource packs in the player's order.
- **Manifest**: `world.manifest.json`. Names `sea_level`, seed, and the paths + identities of the WorldGraph, ZoneGraphs, BiomeGraphs (with optional DetailGraph paths and `BiomeParams`), and LibraryGraphs. Resolution root for `GraphRef` and `LibraryRef`.
- **MaterialRegistry**: data-driven registry mapping stable `MaterialId` values and human-readable names to material definitions. Loaded at startup; extensible by mods.
- **Mesh disk cache**: persistent cache of meshed chunks keyed by chunk coordinate, graph hash, mesher version, registry hash, and world seed.
- **Observer**: anything the server keeps chunks resident for, identified by an id and an area of interest. The local player is observer 0.
- **Overrides**: player-authored edits serialized separately from generated content. Canonical in-memory shape is `ChunkOverrides`; persisted directly.
- **Owner**: which part of the engine/game split provides a type: the core, the kit, or the game.
- **Pin type**: typed connection between graph nodes. Enforces compatibility at edit time.
- **Positions**: pin type representing a set of candidate positions (used by scatter placement and any other consumer of a candidate-position set). Formerly named `ScatterPoints`; renamed to describe the general mechanism rather than the scenario.
- **Registry tag**: a named set of entries from one registry, which packages add to rather than replace.
- **Render world**: the render crate's own `bevy_ecs` world, filled by extract; the only state render code reads.
- **Resource pack**: a client-only package kind that may replace only presentation, loads in the player's order, and is outside the join handshake.
- **ScatterInstance**: a single placed foliage/prop instance with anchor, sub-offset, rotation, prefab reference.
- **ScatterStore**: per-chunk collection of ScatterInstances indexed by type.
- **Sea level**: global Y read from the manifest, determining ocean fill.
- **Server-core**: `voxulacrum-server-core`, the shared crate holding the world, generation glue, simulation, persistence, the mutation door and the tick framework. It has no renderer or windowing dependency.
- **Slab**: a half-height voxel shape. Either `SlabBottom` (lower half solid) or `SlabTop` (upper half solid). All slab faces are cardinal-axis-aligned.
- **Slab smoothing**: engine pass during worldgen that inserts slab steps at walkable cube-step transitions, controlled by traversal smoothing distance.
- **Streaming**: load/evict policy keeping only chunks inside some observer's area of interest resident. Background workers handle generation and meshing.
- **Sub-voxel model**: a model with a collision shape, anchored at one voxel, for things smaller than a voxel or not made of cubes.
- **Sub-voxel offset**: fractional position within an anchor voxel, packed as signed bytes.
- **Tick**: one step of simulation, 1/30 s, identified by its `SimTick` number. All periodic simulation derives from it.
- **Traversal smoothing distance**: per-biome worldgen parameter controlling how aggressively slab smoothing distributes steps across distance. Produces emergent gradual slopes without diagonal geometry.
- **Seam finalization**: cross-chunk pass (`world::seam`) resolving slab-smoothing demotions at chunk borders once neighbor data exists; recorded through the override/diff mechanism and marked by a persisted `seam_finalized` flag.
- **Walkability**: transient worldgen computation identifying voxels the player can stand on; input to slab smoothing, then discarded. Runtime movement, collision, and room detection use direct geometric queries (`WorldView::solid_interval`) instead of a resident mask (revised v1.6).
- **World region**: an aligned group of chunks that owns per-region generation parameters, entity persistence, the region tick and region transformation. What a bare "region" means.
- **WorldGraph**: top-level singleton graph defining climate, zone selection, and global constants.
- **ZoneGraph**: graph type defining biome distribution, cave rules, structures, and rivers within a Zone.

---

## Document Maintenance

This document is the **baseline reference** for the engine's foundational data model and architecture. Changes to anything described here require explicit revision of this document. Implementation details (specific algorithms, library choices, code organization) may evolve freely; data shapes, pipeline stages, and architectural boundaries do not.

When new systems are designed (combat, AI, multiplayer, modding), they extend this document rather than replacing it. The systems described here are load-bearing; everything else grows on top.
