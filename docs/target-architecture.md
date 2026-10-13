# Voxulacrum Target Architecture

Oct 6, 2026 · @Jack Woods

This is the target architecture for Voxulacrum as a general engine for isometric voxel games, settled in our design discussion and checked against the v0.4.0 branch (commit 2b2cf1e), `engine-design.md` v1.11 and `roadmap.md` v1.2. It keeps most of what the code and those documents already decided and changes six things: voxel scale, rotated grids, the one-binary editor rule, the engine/game boundary, the order of the format work, and when moving grids arrive. The Migration plan tab sequences the work.

## At a glance

&#91;embedded content: engine, kit, game and mod layers · client, shared and server code\]

Dependencies only point down. The facade is the single crossing point, and inside the engine the client and server both depend on the shared simulation, never on each other.

## Targets

The minimum spec is a 6 to 8 core CPU with integrated graphics, holding 30 fps at reduced settings. Every expensive technique therefore needs a cheaper fallback, and the raster path is the baseline. The roadmap's §7.3 budgets (16.6 ms CPU at default zoom) remain the target for recommended hardware.

| Target | Value | Notes |
| --- | --- | --- |
| Minimum hardware | 6 to 8 cores, integrated GPU, 30 fps | The current perf laptop's Core Ultra 7 155H has an integrated Arc GPU, so min-spec can be measured on it by forcing the low-power adapter |
| Hosted world (no dedicated server) | 4 players, up to 8 | Host runs client and server in one process |
| Larger sessions | Dedicated server | Same server crate, no renderer |
| World size | Practically infinite | i32 voxel coordinates at 0.5 m reach about 1,000,000 km in each direction |
| Simulation distance | 8 to 10 chunks, set per world | Vertical range bounded separately, e.g. ±4 chunks |
| Default view distance | 4-chunk radius at widest zoom | Chunks are 16 m (32 voxels of 0.5 m) |
| Simulation tick | 30 Hz | Player sim already runs at 30 Hz |
| Physics | 60 Hz (2 substeps per tick) | Vehicles and stacked grids need smaller steps |
| Fluids, block ticks, growth | 10 Hz or lower | Fluids already run at 10 Hz |
| Network snapshots | Up to 30 Hz nearby, lower far away | Priority accumulator decides |
| Visible dynamic grids (server defaults) | 10 large, 40 medium, 100 small | Tiny debris (1 to 8 voxels) is not a grid; see Grids and assemblies |
| Platforms | PC first, controller supported, consoles kept possible | Consoles need ahead-of-time compiled WebAssembly and a separate graphics backend |

No gameplay code assumes a tick rate. Durations are written in seconds or derived from one tick-rate constant, so changing 30 Hz later is configuration, not a rewrite.

## Principles

Twelve of the roadmap's fourteen pillars carry over unchanged or extended. Two are revised: P8 (from no diagonal geometry to no head-on views) and P9 (the editor lives in one binary).

| Pillar | Status | What changes |
| --- | --- | --- |
| P1 Determinism in generation, divergence tolerance in simulation | Kept, extended | Determinism split into two explicit tiers (Runtime spine) |
| P2 The graph is the world generator | Kept | Graphs gain a resolution parameter for LOD |
| P3 Changes localize | Kept |  |
| P4 Generated/authored split is save and wire format | Kept | Plus per-region generation parameters (D4) |
| P5 One mutation door | Kept, generalized | The door becomes the Commands verb for every system, not only voxels |
| P6 Client/server boundary is compiler-enforced | Kept, pulled earlier | Integrated server moves forward to M3 (runtime spine) |
| P7 Interim shapes are recorded | Kept |  |
| P8 No diagonal geometry, ever | **Revised** | Becomes **no head-on views**: the camera always sees three faces of an axis-aligned block (a top and two sides) and never looks straight along a world axis, where blocks collapse into flat two-face shapes. The rule covers the camera's resting views only: an eased rotation passing through an axis-aligned angle, or one object briefly facing the camera, is fine. The world's voxel geometry stays cubes only; models, entities and dynamic grids may take any shape or rotation. Pixel-perfect snapping and quantized light leave the pillar and become per-game choices (P19) |
| P9 The editor lives inside the engine, one binary | **Revised** | One renderer and in-engine authoring stay. The editor becomes its own binary on the engine library, and playtesting a game or mod launches it as a separate process over a debug protocol |
| P10 Performance is budgeted | Kept | Min-spec named now (Targets), not at the end |
| P11 Authoring is a loop | Kept |  |
| P12 Every capability has a game-facing API | Kept, enforced | Enforced by a facade crate and CI, not review |
| P13 Visibility is queryable world state | Kept | Dynamic grids become occluders too |
| P14 One scheduler, one clock family | Kept | Tick framework moves forward to M3 (runtime spine) |

Five principles are added:

- **P15 Mechanism in the engine, policy in the game.** The engine never hardcodes a material, channel, controller constant or creature; games register them.
- **P16 The game is the first package.** The prototype game loads through the same package and facade API a mod would use, with no privileged access.
- **P17 Authoritative data and derived data are separate.** Meshes, light textures, GPU trees and LOD are caches rebuilt from world data at any time, including after a GPU device loss.
- **P18 Never delete data you don't understand.** Content from a missing or disabled package is kept as placeholders and opaque blobs, and restored when the package returns.
- **P19 No house style.** Pixel-perfect snapping, palette quantization, outlines, texel-locked resolution and the lighting model are presentation settings a game chooses, never assumptions any other engine system depends on.

## Engine, kit, game and mods

Dependencies point one way: mods depend on a game's mod API, the game on the optional gameplay kit, the kit on the engine core. Nothing below a layer knows the layer above exists. The editor and CLI depend on the engine; the launcher and account services depend on nothing here.

| Layer | Contains | Lives in |
| --- | --- | --- |
| Engine core | Voxel world, grids, worldgen, simulation, physics integration, rendering, audio, input, networking, packages, scripting host | Engine repo, behind one facade crate |
| Gameplay kit | Opinionated but reusable systems: character controller, isometric camera controller, interaction and brushes, gameplay tags, attributes and effects, legged gait controller, inventory framework | Engine repo, built only on the facade |
| Game | Content, rules, tuning, its own mod API | Its own repo, loaded as a package |
| Mods | Content and WebAssembly modules against a game's mod API | Package archives |
| Editor and CLI | Authoring panels, debug protocol client, build/run/package commands | Engine repo, separate binaries |
| Launcher and accounts | Installs, auth, sessions | Separate product, built late |

**Where new code goes**, asked in order: would a different game want it exactly as is (core)? Would many games want it with different rules (core mechanism, game policy)? Is it reusable but opinionated (kit)? Otherwise it is game code. Code is promoted upward once it proves general, never moved down.

**Enforcement:**

- The game lives in its own repository and points at a local engine checkout with a Cargo `[patch]` during co-development.
- Games depend on one facade crate. CI fails a game build that depends on any other engine crate.
- The engine builds and tests with no game present. Engine tests use throwaway content under an engine-reserved namespace.
- An engine testbed project demonstrates every engine feature without game help. The reference world from roadmap 0.4.0 becomes a second sample package.
- A non-blocking CI job builds the game against the engine's latest commit, so breakage shows early without the game steering the engine.
- Anywhere the engine names a game type, it is replaced by a registry the game fills at load.

## Runtime spine

**Client, server, shared.** Every engine API is tagged with one side. Shared code is the simulation (ECS, world, physics, AI, networks, rules) and runs on both, which is what makes prediction possible. Client-only code presents (render, audio, UI, input devices). Server-only code holds authority (validation, interest management, auth checks). Cargo features or separate crates enforce it, so a dedicated server binary contains no wgpu, winit or egui. **Singleplayer is an integrated server:** client and server run in one process over an in-memory channel, so every feature is multiplayer-correct from the day it is written.

**Five API verbs.** Every system is expressed through the same primitives, which makes them work identically from native game code and WebAssembly mods, and makes them recordable and networkable.

| Verb | Meaning | Already in the code |
| --- | --- | --- |
| Registries | Types defined at load | Materials, prefabs, libraries, graphs (with built-in fallbacks) |
| Resources | Global state | 62 bevy resources inserted in `main.rs` |
| Components and queries | Per-entity data | Only `Player` and `PlayerInputBuffer` today |
| Events | Something happened | `WorldEvent` bus, published one frame late |
| Commands | A validated request to change state | `MutationCommand` with origin and mode: the model for all systems |

**Time.** One fixed 30 Hz tick with a tick counter that timestamps networking, replays and scheduled updates. Sub-rate clocks derive from it rather than running free; this replaces `PlayerClock`, `FluidClock` and `ClimateClock` (roadmap §6.24). Render frames interpolate between ticks. Time scale and pause apply per domain (simulation, audio, UI, effects). Gameplay randomness comes from seeded streams per system and entity, never a global generator.

**Determinism tiers.**

- *Bit-identical on every machine:* world generation, voxel commands (extract, merge, bake), fluids, logic networks. Integer or fixed-point math, and the `libm` crate for transcendental functions. This matters more here than in most engines, because clients generate terrain locally and verify it by hash (networking proposal §2.2): every cross-machine float difference costs an authoritative refetch. `skeleton.rs` currently calls std `powf` and `ln` directly, and `sin` and `cos` through glam's quaternion rotations.
- *Server-authoritative, same-machine reproducible:* physics, AI, animation. Corrections fix clients; same-machine determinism serves replays and tests.

**Threading.** Simulation runs fixed ticks on the job system. Once per frame an extract step copies what rendering needs (changed transforms, new meshes, visual state) into a separate render world, and rendering is pipelined: the GPU draws frame N while simulation prepares N+1. Systems never touch render state directly. Today simulation and rendering share the main thread inside one bevy schedule run, and `render_present_system` is a 340-line exclusive system.

**Coordinates and precision.** i32 voxel and chunk coordinates are kept: at 0.5 m per voxel they reach about 1,000,000 km, so the i64 suggested earlier in our discussion is unnecessary. The precision problem is f32 positions, which lose millimeter precision beyond about 8 km. Rendering stays camera-relative f32. Physics uses f32 inside **physics scenes per activity cluster**: players and active grids within a few hundred meters share a scene with its origin near the cluster's center, scenes merge as players approach and split as they separate, and separate scenes run on separate cores.

## World data model

The voxel becomes a 0.5 m cube and a **block** is a 2×2×2 group of voxels. Players interact and the world generates at block scale by default; the voxel is the true storage, collision and simulation resolution. This answers roadmap D1 with more than the octant mask it proposed: an octant mask gives one material per block with 8 occupancy bits, while 0.5 m voxels give each octant its own material and let fixtures sit at half-block offsets.

**The current shapes become voxel arrangements.** `SlabBottom` is the lower four voxels, `SlabTop` the upper four, and stairs (6 of 8), corners (5 or 7 of 8) and vertical slabs (4 of 8) arrive for free. `ShapeId`, its rotation rules and slab-specific face culling retire. Slab smoothing becomes a half-step detail pass that writes voxels. Existing 1 m content converts exactly by doubling.

**Storage is two-level** so natural terrain costs what it does today:

- A per-chunk block layer (one palette entry per block), plus a 4,096-bit split mask and a running popcount per word.
- Split blocks store 8 voxel entries in a rank-packed array sharing the same palette. A block that becomes uniform collapses back.
- The existing `PalettedBitArray` (entries never straddle a u64; its growth should round widths to 1, 2, 4, 8 or 16 as voxel-core's PaletteStorage does, instead of jumping from odd widths to 16) and the `Uniform` fast path are kept.
- **Occupancy bitmasks** (solid and opaque, as u64 columns) are added. Meshing, collision, ambient occlusion and light propagation run on them as bit operations. Today `is_solid` decodes the palette per voxel.
- The split flag is stored so it could recurse later (0.25 m) without a format change.

**Chunks stay 32³ cells**, now 32³ voxels = 16 m (16³ blocks). 32 cells plus a one-voxel border is 34 bits, which fits a u64 column for binary meshing; 32³ blocks would not. 8³-voxel bricks (4 m) sit inside chunks for GPU structures. Chunks are cubic and stream vertically; the manifest's fixed `min_y`/`max_y` column (128 voxels by default) and whole-column loading go away.

**Three kinds of placeable thing:**

| Kind | Example | Storage |
| --- | --- | --- |
| Material | Cobblestone, planks, dirt | A material per voxel; a full block is 8 equal voxels, collapsed |
| Fixture | Crafting table, door, machine | Indivisible footprint in voxels; each footprint voxel holds "part of fixture X at offset N" so any voxel finds its anchor in one step; voxel-aligned placement, 1 m snapping preferred on full surfaces |
| Sub-voxel model | Torch, fence, flower | Model plus collision shape anchored at a voxel |

**Registries are string-keyed** (`namespace:name`) with numeric IDs assigned at load. Every save embeds its name-to-ID mapping (roadmap D5). This closes the defect where saves store bare `MaterialId` values. Palette entries reference interned `(block, properties)` records rather than a dense global state-ID space, which stays manageable under heavy modding.

**Layers are registrable.** Chunks already carry parallel layers (voxels, detail, scatter, fluids, decals, lighting, tags, overrides). The list becomes extensible so a game or mod can register a sparse per-chunk layer (multipart connectors, heat, gas) with its own version and serializer. Per-layer versioning and per-region generation parameters (D4) land in the same format change.

**Unknown content is preserved.** Palette entries from a missing package load as visible placeholders keeping their original name and properties; unknown components and block-entity data round-trip as opaque blobs.

## Grids and assemblies

Roadmap D3 is decided as **adopt, including moving grids**. The world is one grid among many: a `VoxelGrid` has its own chunks (same palette format and mesher) plus a transform, and the terrain is simply the large static, axis-aligned one. Systems take a grid handle instead of the world singleton: raycasting, collision, meshing, region detection, pathfinding, targeting and block updates. The handle discipline lands with the format freeze (M4); moving grids follow in M9 (living world) rather than post-1.0.

**Motion modes** are switchable at runtime, keeping velocity: *static* (terrain), *kinematic* (animation or a controller sets the transform; it pushes players but is not pushed), *dynamic* (physics owns it) and *powered* (dynamic with joint motors tracking an animated pose, for stagger and hit reactions only).

**Size classes and default server budgets:**

| Class | Representation | Default limit |
| --- | --- | --- |
| Debris (1 to 8 voxels) | Instanced cubes with box colliders, no chunk storage or meshing | Hundreds; baked or despawned when settled |
| Small grid | Full grid, simplified colliders | 100 |
| Medium grid | Full grid | 40 |
| Large grid or assembly | Full grid, per-brick colliders | 10 |

**Grid operations**, all engine commands that are deterministic, server-validated and replicated as one small command each:

- **Split:** a connected-components pass when an edit disconnects part of a grid (or the terrain); each piece becomes a grid that inherits velocity.
- **Extract:** cut a region out of the world into a new dynamic grid. Unbreakable voxels stay; fixtures go whole or not at all; their ports detach from networks; riders attach through relative frames; adjacent water flows into the hole.
- **Merge:** add one grid's voxels into another, allowed only when lattice-aligned (rotation within tolerance of a 90° multiple, integer offset).
- **Bake:** write a settled, possibly rotated grid into the world by mapping each world voxel's center into grid space (no holes). It replaces only air, fluid and plants by default, snaps small rotations first, and places fixtures only if they align.

**Assemblies** are composite entities: a root with a tree of parts (grids, models, sprites, bare bodies, effect anchors) joined by physics joints, each part with its own components and tags. They are defined in data, including IK chains and a locomotion controller. A giant creature's lifecycle is a sequence of mode switches: kinematic while alive, powered when staggered, dynamic on death, sleeping once settled, then baked into terrain.

**Riders and relative frames.** An entity standing on a grid stores its position relative to that grid, attaching on contact and detaching on jump or leave. Logic networks live in a grid's local space, so a ship carries its own machines.

**Protecting big builds.** Per-grid cost tracking (physics time, colliders, contacts) visible to players; automatic collider simplification to merged brick boxes; anchoring (freeze to static), packing (save as a voxel clip item) and degradation that reduces substeps for distant grids before anything is frozen.

## Voxel clip format

One format carries any piece of voxel content: worldgen structures, assembly parts, extract output and bake input, blueprints and copy-paste, packed vehicles, editor prefabs, large-build network transfer, and test fixtures. It grows out of the current blueprint format (JSON schema v1: anchor, sparse cells with material names, `DestructionPolicy`, `protected_volume`, yaw-only rotation), which already gets the important thing right by storing material names rather than IDs.

| Section | Contents |
| --- | --- |
| Header | Format version, dimensions, origin and anchor, alignment parity, content hash |
| Palette | Namespaced names and properties, never numeric IDs |
| Voxels | The chunk's two-level layout, chunked internally for large clips, compressed |
| Mask | Structure void (leave what is there) versus air (clear it) |
| Fixtures | Type, anchor, orientation, footprint |
| Block-entity data | Keyed by anchor, schema-versioned per type |
| Ports and networks | Optional internal topology, reconnected on placement |
| Entities | Optional, positions relative to the clip |
| Sockets | Named attachment points with facing and type, for jigsaw-style structure assembly and assembly joints |
| Custom layers | Registered serializers for package-defined layers |
| Metadata | Author, tags, thumbnail, required packages with versions, destruction policy, protected-volume flag |

- **Transforms:** all 48 orientations (24 rotations plus mirroring). Block properties declare semantic types ("this is a horizontal direction") so the engine rotates `facing=north` correctly; fixtures rotate footprints and port faces; restricted fixtures make a transform invalid with a reason.
- **Alignment parity** is recorded because a clip with half-block-offset fixtures must keep that offset relative to the block lattice when placed.
- **Placement modes:** replace all, fill only air, or keep existing solids, always honoring the mask, with remapped block-entity data and reconnected ports.
- **Storage:** compressed binary, streamable for very large clips, plus a JSON or RON text form for small clips so test fixtures and hand-authored structures diff cleanly in git.
- **Import:** MagicaVoxel `.vox` through the importer registry.

## World generation

The node-graph system is the engine's most mature subsystem and its architecture stays: the World, Zone, Biome, Detail and Library graph kinds, manifest resolution, `GraphRef` and boundary pins, world-absolute seeding, the margin-band feature model, the river network design, save-triggered hot reload, `--validate-graphs` and the determinism CI job. Generation keeps writing 1 m blocks; the 0.5 m voxel scale adds an optional detail pass rather than 8× the evaluation work.

| Gap today | Target |
| --- | --- |
| Fields are fixed at 32 cells, one cell per voxel (`EvalContext`, `field.rs`) | A resolution parameter per evaluation, used for 1 m blocks in 16 m chunks and for coarse LOD rings |
| Every node fills a full 32³ field; no coarse sampling | Optional coarse density sampling with trilinear interpolation, tunable per graph and switched off near players if budget allows |
| Every Y-chunk in a column re-runs the World and Zone graphs and the border scan | A per-column cache shared by the chunks of a column |
| A feature anchor probe runs a full 32³ biome evaluation per candidate cell | Memoized probes, which the design already permits as long as output is unchanged |
| Library kernels are a closed five-entry Rust enum; only cave noise evaluates inside a graph | Authored library graph bodies (roadmap 0.4.0) plus a kernel registry packages can extend |
| One climate value split by threshold bands; at most two biomes blend per column | Multi-parameter climate (temperature, humidity, continentalness, erosion) with nearest-neighbor biome selection and multi-biome blending |
| No macro layer: each biome invents its own absolute height | Lazily evaluated macro tiles for continents, ranges and drainage, sampled by local graphs (the plan in `macro-terrain-research.md`) |
| Platform `powf` and `ln`, plus `sin` and `cos` via glam, in `skeleton.rs` | `libm`, because clients regenerate terrain and verify by hash |
| `fastnoise2` is a dependency but imported nowhere | Removed; per the phase 11 handoff, its native build is also what keeps the app crate from building in scratch workspaces |

**Where generation runs.** The networking proposal's model stays the default: clients generate terrain locally from seed and graphs, verify each chunk by hash, and fetch authoritatively on mismatch. Servers that want a private seed (so players can't compute ore and structure locations) can switch to sending downsampled and full chunk data instead. Worldgen packages are therefore shared-side by default.

**Region transformation** keeps its roadmap shape: generation becomes a function of seed, coordinates and persistent per-region parameters (D4), stored with the delta.

## Simulation

**Tick phases.** Each 30 Hz tick runs in a fixed order, which also makes behavior deterministic:

1. Apply queued commands (player actions, last tick's outboxes) through the mutation door.
2. Update logic network topology from those edits (merge, split, recompaction).
3. Run local events and scheduled ticks for awake fixtures; random ticks over resident regions.
4. Step logic networks in parallel, one job per network.
5. Write network results back through fixture bindings into outboxes.
6. Step field layers (fluids, later heat and gas).

**Logic between blocks** runs at voxel scale, because fixtures can sit at half-block offsets.

- **Ports** are voxel faces with a registered channel type ("power", "kinetic", "items" are game-registered, never engine-defined) and a role. Fixtures declare port tables that rotate with them; connector voxels derive ports from their state.
- **Local logic:** neighbor-changed events batched per tick, per-chunk scheduled-tick queues saved with the chunk, sleep and wake for fixtures, and a per-tick update budget with a cascade depth limit.
- **Networks** (Create-style kinetics, power, pipes): topology formed from ports in the world, then simulated detached from it as a compact graph. Two-connection runs compact into one edge, so a 200-voxel axle costs one edge. Splits are detected by searching from both ends of a removed edge and stopping when the smaller side is exhausted. Network types are registered with their own solver, solve mode (on topology change, fixed rate, or both) and unload policy (freeze, keep simulating, or catch up). Topology persists in its own store; chunks hold node handles.

**Parallel updates.** The roadmap's 8-pass parity coloring (§6.28) is correct when every chunk writes only itself and reads neighbors, which is the rule here: cross-chunk writes go through outboxes. (The 27-color scheme from our discussion is only needed when a tick writes directly into neighbors.) Cellular automata use double buffering where memory allows. In Rust, the parity scheme is one small audited `unsafe` abstraction with a safe API.

**Fluids.** The current model stays: a mass-conserving cellular automaton on integer u16 mass, planned read-only then committed in a deterministic order, with sleep and wake and the O(1) `Submerged` fast path. It keeps running on 1 m cells, with capacity now computed from how many of the block's 8 voxels are open. Three fixes come first (see the migration plan): capacity underflow, mass creation in submerged chunks, and asymmetric caps at chunk seams. It moves onto the generalized update model as that model's first consumer. A shallow-water pipe model for large open bodies is a later option, not a requirement.

**Region graph** (roadmap §6.22) is adopted as written, at block resolution with face coverage masks now derived from voxel occupancy. Its seven consumers (occlusion, spawn shelter, interior flag, audio, coarse pathfinding, abstract creature movement, fill light) plus camera anchoring are unchanged.

**Light.** `GameplayLight` and `RenderLight` stay separate (§6.25). Gameplay light is a quantized sky and block channel at block resolution, with per-face transmission derived from voxels so a 0.5 m wall still blocks it. Render light is sampled from a per-chunk 3D texture (D2), so placing a torch never remeshes.

## Entities, physics, animation and AI

**Entities.** `bevy_ecs` stays. The roadmap's entity design stays too: a global archetype store rather than chunk-owned entities, a loose spatial grid that doubles as network interest management, stable IDs, region-serialized persistence and the realized, abstract and statistical tiers (§6.3, §6.23). Added: data-defined prefabs, per-component metadata (saved or not, replication rule, schema version), parent and attachment relationships to voxel anchors and grids, and **load anchors** so an entity can keep a radius loaded around itself, with server limits.

**Physics.** Rapier provides rigid bodies, joints (multibody joints for leg and tail chains), motors and contact events with impulse magnitude, inside the per-cluster f32 scenes. Queries (raycast, shape cast, overlap) cover voxels, entities and grids together; voxel raycasts use DDA over occupancy masks. The existing `player_step` stays as the core of the kit's character controller: it is already a pure, server-portable function with swept per-axis AABB collision. It gains moving-grid riding, swimming, 0.5 m auto-step and parameters in place of the constants in `player/sim.rs`.

**Animation.** One system for skeletal (glTF) and sprite-flipbook animation; animation graphs with state machines and blend trees; animation events. Procedural animation is first-class: skeletons bind to any part (grid, model or sprite), IK solvers (two-bone, FABRIK, CCD) with joint limits are engine primitives, and procedural and clip poses layer. Foot placement IK matters more than usual with 0.5 m steps. The kit provides the legged gait controller; for networking, clients rebuild procedural motion from the root transform, gait phase and foot-plant events.

**AI.** Hierarchical pathfinding over the region graph (§6.10), refined at voxel resolution near the agent, with agent profiles (size, step, jump, drop, capabilities), flow fields for many agents with one goal, incremental invalidation by chunk version, navigation in a grid's local space, and asynchronous path requests. Perception uses batched voxel line-of-sight and game-emitted noise stimuli. AI runs on the server with a per-tick time budget and distance-based level of detail. The engine supplies blackboards and scheduling; the kit supplies behavior trees, utility AI and state machines in data. Giant agents use coarse chunk-level navigation and let locomotion handle detail.

**Combat accuracy.** The server keeps about one second of hitbox history and checks attacks against where targets were on the attacker's screen (lag compensation). The engine provides the history and rewind queries; the kit provides hit-registration rules.

## Rendering

The rendering core stays, reframed as configurable stages rather than a house style (P19): scene targets with an adjustable resolution, optional camera and entity snapping with sub-pixel compensation, a choice of upscale filters, the optional palette pass (CIELAB nearest or OkLCh stepping), the surface-class outline pass, the two-pass player silhouette, the visibility mask behind underground cutaways, the single-render planar reflection, capsule impostors (a technique games may use, not a requirement for trees) and GPU-generated grass. What changes is how geometry reaches the GPU, how a game extends the pipeline, and that no stylization stage is mandatory.

| Today | Target | Why |
| --- | --- | --- |
| One quad per exposed face, 32-byte `FaceVertex`, no AO | Binary greedy meshing on occupancy masks, 8-byte packed quads expanded in the vertex shader | 0.5 m voxels multiply faces; merged quads keep flat terrain at today's quad count |
| 256 MB disk mesh cache | Measured after the mesher changes, likely retired | It exists because meshing is slow; binary meshing runs well under a millisecond per chunk |
| Per-chunk vertex and index buffers, one `draw_indexed` each, CPU frustum culling | One suballocated arena, multi-draw indirect, then GPU culling with two-phase Hi-Z | Draw count stops scaling with chunk count, which matters on integrated GPUs |
| Tri-tonal brightness fixed to world axes (top 1.0, ±X 0.75, ±Z 0.60) while Q/E already rotates the camera | Selectable lighting models: camera-relative face tiers for flat stylized looks, or normal-based lighting with soft shadows and ambient occlusion for textured 3D | The same face reads differently after each rotation today |
| Material = one flat color in a 256-entry buffer | Material definitions with world-space UVs anchored to the 1 m lattice, optional palette ramps, per-material texture filtering, flags, and a surface-function hook | Half blocks and stairs show the correct quarter of a block texture |
| Fixed pass list; `MainScenePassNode` hardwires its sub-draws | Render graph with named insertion points (after opaque, after lighting, before post, after post, before UI) | Games and mods add passes without forking the renderer |
| No shader includes; `GlobalUniforms` copy-pasted into each shader | A shader library with includes (voxel sampling, lighting and ramp quantization, projection, depth reconstruction, wind) | One definition per struct |
| Outline depth edge uses one threshold (default 0.0005) whatever the drop height | Depth-scaled edge intensity, optionally quantized to palette steps, per-material scale | Edges encode how far a ledge drops |
| No LOD | Zoom-band LOD rings that use the generator's coarse resolution | Widest zoom stays cheap |
| 8192² Depth32 shadow map (about 256 MB) and full-resolution reflection | Shadow tiers (off, blob, 2048, 4096, ray-traced) plus a render-scale setting that lowers texels per voxel | Integrated-GPU minimum spec |

**Extension hooks for games.** Surface functions are the main one: a small shader function fills albedo ramp index, normal, emission and material ID, and the engine does lighting, shadows, edges and any enabled palette mapping around it, which keeps the pipeline free to change underneath. Lighting is a second hook: a game picks a built-in lighting model or supplies its own lighting function. Mod shaders are validated with naga and limited to surface functions and post-process passes. Per-entity render flags cover shadows, outline, highlight, x-ray silhouette and cutaway participation.

**New renderable types:** dynamic grids with per-instance transforms and oriented-box culling; sprites, depth sprites (per-pixel depth and normals so they intersect voxels correctly) and billboard clumps for canopies, which fit the sprite pipeline planned as foliage Part C; GPU particles; projected decals; render-to-texture for minimaps, portraits and item icons; an entity ID buffer for pixel-accurate picking; and the existing debug lines grown into a general debug-draw API.

**Presentation style is a game choice.** The engine ships no house style. Every stylization stage is a per-game setting that can also be switched at runtime, so the styles worth comparing are presets over one pipeline and can be tried side by side in the testbed. Three example presets:

| Setting | Strict pixel art | Textured 3D, pixel-art textures | Hybrid with sub-pixel drift |
| --- | --- | --- | --- |
| Render resolution | Locked texels per voxel | Native, with a render-scale slider | Texel resolution |
| Camera and entity snapping | Both snapped | Neither | Camera snapped; entities and sprites may move by sub-pixel amounts |
| Texture filtering | Nearest | Nearest, mipmapped | Nearest |
| Anti-aliasing | None | MSAA or TAA allowed | Edge-aware upscale only |
| Palette pass | On | Off | Optional |
| Outlines | On | Optional | Optional |
| Lighting model | Camera-relative face tiers, quantized | Normal-based, soft shadows, ambient occlusion | Either |

The camera accepts any pitch and yaw that satisfies P8, with true isometric (35.26°) and 2:1 dimetric as presets and 90° rotation steps with eased transitions. Rotated grids and 3D models draw like any other geometry; under strict pixel art they alias, which that game accepts or designs around. Nothing outside the renderer depends on the chosen style, because visibility, targeting and collision read world state (P13).

## Input, UI, audio, networking, persistence and platform

| System | Today | Target |
| --- | --- | --- |
| Input | Raw events, `InputMap` bindings from `input.ron`, 22-variant `GameAction` enum; keys and scroll only; no gamepad; rebinding means editing the file and restarting | Device layer (keyboard, mouse, gamepad via `gilrs`, haptics, cursor modes, IME as a separate text channel) under an action layer: registered actions with value types, a context stack, modifiers and triggers (hold, tap, chord), runtime rebinding with conflict detection, prompt glyphs per device, tick-stamped inputs for prediction and replay |
| Picking | Inverse view-projection plus voxel DDA, reach-limited in play | Same, across all grids, plus the entity ID buffer for sprites |
| Game UI | One element: the material swatch | Retained, declarative UI: flexbox and grid layout (`taffy`), styles and themes, data binding to view-models, UI as data with named extension points for mods, controller focus navigation, world-space anchors, pixel-art scaling, AccessKit |
| Tool UI | egui panels (parameters, graph editor, probes, inspectors) | Stays egui, editor-only, never shipped as game UI |
| Text | egui text | Shaping and fallback for every script (`parley` or `cosmic-text`); Fluent string tables from the first game screen |
| Audio | None | Mixer graph with buses and ducking, data-defined sound events, voice limits with virtual voices, a separate audio thread; listener at the player with screen-space panning; occlusion by voxel raycast and reverb estimated from rays (the region graph supplies interior state); soundscapes from environment queries; adaptive music. `kira` is a reasonable base |
| Networking | None; the proposal is complete and stays authoritative-state | QUIC via `quinn` (reliable streams plus datagrams), the in-memory transport for the integrated server, per-component replication rules, priority-based bandwidth budgeting, prediction for the local player and voxel edits, interpolation for others, lag compensation, typed versioned messages, server validation helpers, a network condition simulator |
| Persistence | SQLite with WAL, zstd with a trained dictionary, overrides-only, key-sorted deterministic blobs, wipe on version bump | Kept as the store. Added: per-layer versions, embedded registry mapping, region parameters, world-instance keys, entity and player data, per-record checksums with quarantine, atomic writes, rolling backups, hitch-free saving from copy-on-write snapshots |
| Platform | winit, Fifo present, paths next to the executable | Display modes and caps, OS-standard save and config paths, focus handling, a platform-services trait (achievements, presence, cloud saves) with a no-op default, crash dumps, opt-in telemetry |
| Tools | In-engine diagnostics (frame timings, job inspector, streaming, cache, memory, determinism checker, mutation log); `--verify-generation`, `--validate-graphs` | Kept and exposed through a console with registered commands and variables, a debug protocol for the editor, headless mode, replays, golden-image and determinism tests, network soak bots |

**Environment** (time of day, weather, the environment query API for biome, temperature, light, sky exposure and enclosure) is engine-owned and feeds spawning, audio, AI and rendering. `world/climate.rs` is the cloud and vapor automaton behind cloud shadows; it runs every frame through `climate_tick_system` despite a stale module-level `dead_code`; it belongs to the roadmap's fix-or-remove decision on cloud hydrology.

## Packages, fault isolation and API stability

**Packages.** Everything the engine loads at runtime is a package with a manifest (id, version, kind game or mod, engine API range, exported mod API version, dependencies, content namespace, modules). The current `world.manifest.json` is the seed of this: it already resolves the world graph, zones and biomes by reference (libraries load separately).

- **Sides are per module**, not per package: shared (registries both sides need), server (behavior, admin tools), client (UI, shaders, sound packs). Each package also declares a client requirement: required, optional with placeholder fallbacks, none (server-only plugins), or client-only.
- **Join handshake:** the server sends required packages with content hashes; clients check a content-addressed cache; data-only packages may download automatically, WebAssembly modules prompt once per package and source with the capabilities they request, and native code never downloads. Then the server sends its registry mapping.
- **Capabilities** (own data folder, listed network domains, clipboard) default to none and are enforced by the WebAssembly runtime.
- **Load order** is dependency order, then sorted by id, so the same mod list loads the same way everywhere. File overrides go through a layered virtual file system by priority.
- **Code:** the game's own code compiles natively through the facade; mods run as WebAssembly components with WIT interfaces (Wasmtime), ahead-of-time compiled per platform at packaging so consoles stay possible.

**Fault isolation**, so every failure has a defined blast radius:

| Failure | Response |
| --- | --- |
| Mod trap or runaway loop | Commands from that call are discarded (calls are transactional); epoch interruption caps time per call; repeated faults disable the mod and notify |
| Panic in a job | Caught at the job boundary; the chunk is retried, then shown as a placeholder and never saved over good data |
| Corrupt saved record | Checksum mismatch quarantines it, restores from backup or regenerates, logs everything |
| Missing package | Placeholders and opaque blobs (P18); a warning before loading a world with packages missing |
| Malformed network input | Size limits, bounded decoding, fuzz-tested parsers, rate limits; the sender is disconnected, the server never panics |
| GPU device lost | Device recreated and rebuilt from world data (P17) |
| Over budget | Lag attribution names the system, mod or grid; offenders are throttled first; memory pressure lowers view distance |

Plus a safe-mode launch with packages disabled, and crash reports that bundle the dump, logs, package list, settings and the last few seconds of recorded commands.

**API stability.** Six surfaces are versioned independently: the Rust facade, the WIT interfaces, content schemas, the network protocol, the save format and the debug protocol. Each has three tiers: stable (semver, deprecation before removal), unstable (behind an `unstable` feature or WIT gate) and internal (not exported). CI runs `cargo-semver-checks` on the facade and snapshots the public API, WIT files and JSON schemas. The facade never exposes third-party types (`glam`, `rapier`, `wgpu`), public structs and enums are `#[non_exhaustive]`, and IDs are newtypes. This sharpens roadmap §7.2: the API promise applies per surface from the moment the facade exists (M2), rather than from a fixed version.

## Decision register

D1 to D8 are the roadmap's forcing-date decisions; N1 to N15 are new. "Recommended" rows change a pillar or were left to my recommendation, so they wait for your sign-off before being folded into `engine-design.md`. Milestones M1 to M11 are defined in the Migration plan tab; version numbers are assigned only when a milestone completes.

| ID | Decision | Status | Milestone |
| --- | --- | --- | --- |
| D1 | Shape vocabulary: 0.5 m voxels with 2×2×2 blocks, superseding the octant-mask option | Decided | M4 |
| D2 | Render light sampled from a per-chunk 3D texture, not vertex attributes | Decided | M5 |
| D3 | Grid handles for every world system, including moving grids | Decided | M4 handles, M9 motion |
| D4 | Per-region generation parameters in the save format | Decided | M4 |
| D5 | String-keyed registries with a per-save name mapping | Decided | M2 in memory, M4 on disk |
| D6 | Colored (RGB) block light or one channel plus per-material palette response | Open | M5 |
| D7 | 4-way camera rotation: keep (Q/E already rotates), with camera-relative face tiers | Decided | M5 |
| D8 | Greedy meshing: binary greedy on all faces, 8-byte packed quads | Decided | M5 |
| N1 | Engine/game separation: facade crate, game in its own repo, game as the first package | Decided | M2 |
| N2 | Revise P9: editor is its own binary on the engine library; playtesting runs the game out of process | Decided | M2 |
| N3 | Revise P8 to no head-on views: the camera always shows three faces of a block; any geometry or rotation is allowed | Recommended | M5 |
| N4 | Integrated server (client and server crates over an in-memory channel) pulled forward | Decided | M3 |
| N5 | Tick rates: 30 Hz simulation, 60 Hz physics substeps, 10 Hz fluids | Decided | M3 |
| N6 | Determinism tiers, with `libm` in generation | Decided | M3 |
| N7 | Physics: Rapier in f32 scenes per activity cluster | Decided | M8 |
| N8 | Keep i32 voxel coordinates (no i64) | Recommended | M4 |
| N9 | Cubic vertical streaming with no fixed world height | Decided | M4 |
| N10 | Worldgen runs client-side by default; private-seed servers send chunk data | Recommended | M7 |
| N11 | Mods as WebAssembly components, ahead-of-time compiled; package boundary drawn at M2 instead of just before 1.0 | Decided | M2 boundary, M11 runtime |
| N12 | No house style: strict pixel art, textured 3D and hybrid are presets of one configurable pipeline, lighting model included; the game chooses | Recommended | M5 |
| N13 | Logic at voxel scale through ports, events and a pluggable network service | Decided | M9 |
| N14 | Tiny debris as instanced cubes, not grids | Recommended | M9 |
| N15 | Inventory units deferred to the game layer | Decided | Game |

## Open questions

- **D6, colored light.** True RGB costs three to four times the light field and cannot be retrofitted; one channel plus per-material palette response may suit a quantized look better. Decide with the light layout in M5.
- **Vertical simulation range.** How many chunks above and below a player simulate (±4 is a placeholder), and whether the world has any vertical bound for gameplay reasons.
- **Launcher scope.** Game-specific or a generic launcher for any title on the engine. Recommended: design it generic, build it late, launch with an offline auth provider until then.
- **Editor UI toolkit beyond egui.** egui stays for tools; whether a more designer-friendly shell is ever wanted can wait because the editor talks to the game over a protocol.
- **Presentation style for the first game.** Strict pixel art, textured 3D with pixel-art textures, or a hybrid. The engine supports all three as presets, so this is settled by playtesting in the testbed rather than in advance.
- **Mesh disk cache.** Keep or retire, decided by measurement once binary meshing exists.
- **Cloud hydrology.** The roadmap's fix-or-remove decision stands; nothing here depends on it.
