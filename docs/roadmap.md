# Voxulacrum Engine Roadmap — 0.3.0 through 1.0

**Status:** Authoritative planning reference, v1.2
**Companions:** `engine-design.md` (v1.8 — foundational data model and architecture), `networking-architecture-proposal.md` (PROPOSAL until the networking arc completes), `underground-visibility-design.md`, phase audits, `architecture-drift-review.md`, `engine-architecture-reference.md` (external review — disposition recorded in §21).
**Purpose:** Make the technical goals visible, fix the versioning discipline, and sequence every core capability the engine needs before it is a *ready product* — an engine another person (or future you) can build a title on without engine surgery.

**What 1.0 means.** Not "a demo shipped." 1.0 is defined by the **readiness rubric in §5** — measurable criteria across eleven domains. The MVT — a co-op multiplayer voxel survival sandbox — is the *evidence* that the rubric passes, not the definition of readiness. An engine that can ship exactly one title and nothing else is not at 1.0.

---

## 1. How to use this document

This document exists because the 0.2.0 sprint drifted: time went to side tangents (cloud hydrology, water reflections) while the release's stated core — infinite world **with biomes** and node-graph content authoring — shipped with the biome promise unmet (one meadow biome). The rules below are the countermeasure. They are process stakes, as binding as the technical ones.

1. **Every minor version has a theme, exit gates, and explicit non-goals.** Work that serves the theme proceeds. Work on a non-goal stops — write it in the version's parking lot and return to the theme. "It was interesting" is not a reason to ship it in this version.
2. **Exit gates are checklists, not vibes.** A version releases when its gates pass. If a gate must be dropped, that is a scope decision recorded in the changelog, not a silent omission.
3. **Debt is scheduled, not ambient.** Every known defect and interim shape here is assigned to a version. New debt either joins the current version (if it blocks a gate) or gets a version assignment before the release ships.
4. **Every version ships tooling for what it builds.** A feature without an authoring path, a preview, and a diagnostic is half-built (§4).
5. **Forcing-date decisions get decided, not deferred by silence.** §8 lists architectural choices whose cost rises sharply at a specific version. Each is decided on or before that version, in writing, with reasons. A decision that arrives by default because nobody made it is the failure mode this list exists to prevent.
6. **Docs are part of the release.** Not done until: workspace version bumped, `CHANGELOG.md` entry written, close-of-version audit exists, newly-settled architecture folded into `engine-design.md`. (0.2.0 shipped with `Cargo.toml` at 0.1.1 and a changelog frozen at Phase 0. That never happens again.)
7. **Phases map to versions.** Each phase declares up front which version it serves. A phase serving no version on this ladder is a tangent by definition.

---

## 2. Technical pillars — the stakes in the ground

Properties that must hold at any scale of game derived from this engine. Every version must strengthen these or leave them intact.

**P1 — Determinism in generation, divergence-tolerance in simulation.** Same seed + graphs + parameters + coordinates → bit-identical terrain on one machine; hash-verified self-healing terrain across machines. Simulation is *not* lockstep-deterministic; the server owns it, clients render replicated state. Every generation pass ships with a determinism test.

**P2 — The graph is the world generator; the manifest is the resolution root.** All world content flows from authored graphs + registries. Editing the active graph edits the live world. No system hardcodes content identity — materials, prefabs, palettes, sounds all resolve through registries.

**P3 — Changes localize.** The world is infinite; every system is per-chunk or per-column, tolerates chunks appearing/disappearing any frame, and invalidates by tag lookup — never world scan. Cost scales with the observed set, not the world.

**P4 — The generated/authored split is the save format AND the wire format.** Persistent state is the delta from generated output; generation is reproducible from recorded inputs. Save files stay small, network payloads stay small, and the persistence encoders *are* the wire encoding. One serialization discipline.

**P5 — One mutation door.** Every world write — editor, player tool, hot-reload, network command, game script, region transformation — goes through the mutation command API with a declared origin and mode. The door is where validation, mode enforcement, persistence marking, mesh invalidation, event emission, and network broadcast live. A write path that bypasses the door is a bug by definition.

**P6 — The client/server boundary is compiler-enforced.** After the server-core extraction, simulation/generation/persistence/mutation live in a crate with no renderer types. Single-player is the client talking to an embedded core over loopback — one code path, not two.

**P7 — Data shapes are settled; implementations are incremental; interim shapes are recorded.** Any divergence between target and implementation is written down with a named migration trigger. Silent drift is the failure mode.

**P8 — The grid is the visual language.** Cardinal-axis geometry, tri-tonal axis lighting, quantized light, pixel-perfect snapping. No diagonal geometry ever. Aesthetic identity is architecture, not a skin.

**P9 — The editor lives inside the engine.** One binary, one renderer, one chunk store. Tooling grows as engine panels and debug views, not satellite apps. (The headless server binary is server-core, not a second engine.)

**P10 — Performance is budgeted, not aspirational.** Budgets (§7.3) tracked from 0.3.0 via in-engine instrumentation, gating from 0.5.0. An unexplained frame hitch is a release blocker.

**P11 — Authoring is a loop, not a build step.** Every authorable thing is editable in-engine, previewable before commitment, and hot-reloadable without restart. The measure is round-trip time: change → see result. If an author must restart the engine, edit a file by hand, or guess at a result, that content type's tooling is incomplete. (0.2.0's caves were hand-authored in raw graph JSON off-session. That is the failure state this pillar prevents.)

**P12 — Every engine capability has a game-facing API.** A capability the engine "has" but that a game feature cannot call is not delivered. Each subsystem in §6 exposes a stable, documented surface usable from game code without reaching into engine internals — and that surface is what the compatibility promises (§7.2) protect.

**P13 — Visibility is queryable world state, not a rendering effect.** *(Added v1.2, from the external review — the sharpest architectural correction it offered.)* Whenever the engine hides geometry — underground visibility, future slice modes, occlusion fades — the decision lives in world state that the renderer, the interaction system, and any other consumer all read from the same place. If the renderer hides a ceiling but the raycaster still collides with it, players target blocks they cannot see and the game feels broken in ways they cannot articulate. This rules out occlusion implemented purely in shaders. The engine's air-side face rule already computes a real classification; the pillar requires that classification be *shared*, not re-derived or ignored by non-rendering consumers.

**P14 — One scheduler, one clock family.** All deferred and background work — generation, meshing, lighting, pathfinding, structures, chunk I/O, region transformation, simulation ticks — runs on a single job system with declared dependencies and priorities, and all periodic simulation derives from one tick framework rather than per-system ad-hoc clocks. Two schedulers means two tuning surfaces that must agree by hand and never do.

---

## 3. State of the engine at 0.2.0 — settled, interim, and open

**Settled** = designed, implemented, documented, trusted. **Interim** = deliberately divergent from target with a recorded migration. **Open** = designed but unbuilt, or not yet designed.

### 3.1 Voxel & chunk data model — settled, with one forcing-date decision

Voxel format (4 shapes, no rotation, packed u32); material registry (RON-driven, stable numeric IDs); 32³ chunks with parallel layers (voxels, detail, scatter, fluids, lighting, tags, overrides); generated/authored split via `ChunkOverrides`. Decal layer reserved and empty.

**Interim:** monolithic `BLOB_VERSION` + wipe-on-bump. Per-layer versioning is release-gating; scheduled **0.6.0** — the last planned format redesign before saves must survive upgrades.

**Latent defect (new, found while triaging the external review):** saves store numeric `MaterialId` values with **no embedded name↔ID mapping**. Any registry reorder, insertion, or mod-added material silently reinterprets every existing save. Fix is cheap now and impossible later; scheduled **0.6.0** with the format redesign (§8, D5).

**Forcing-date decision:** the 4-shape vocabulary versus an octant-occupancy encoding (§8, D1). Decide by 0.6.0.

### 3.2 World generation — settled architecture, incomplete content surface

Five-graph hierarchy, manifest resolution, `GraphRef`/`GraphOutput`, library boundaries, per-biome parameter sidecar, invalidation table — settled and shipped. Slab smoothing plus cross-chunk seam finalization — settled through Phase 9.

**Implementation divergences:**
- Foliage evaluates before fluid and is blind to it (drift 1.4) — grass will paint underwater the moment a biome dips below sea level. Fix in **0.3.0**.
- `StandardCaveNoise` kernel exists but `Evaluator::sample_density` errors on `LibraryRef` in a density chain — libraries are unusable for terrain. Fix in **0.4.0**.
- `PoissonDisk` seeds RNG from chunk coordinates while every sibling derives world-absolute (drift 2.1) — violates P1, produces pattern seams, and silently relocates stable instance IDs if framing ever changes. Fix in **0.3.0** while save wipes are still free.

**Open:** multiple biomes/zones as shipped content, structures, rivers, cross-chunk generation infrastructure, authored library graph bodies — all **0.4.0**. Multi-distance smoothing stays reserved, post-1.0.

**Structural gap (new):** generation is a pure function of `(seed, graphs, coords)`. There is no persistent per-region generation state that can *change over the world's lifetime*, so no region of an existing world can be retroactively reshaped. This forecloses an entire category of survival-sandbox progression (§6.26). The storage decision must be made before the format freezes — **0.6.0** (§8, D4).

### 3.3 Persistence & save model — settled shape

SQLite + zstd, overrides-only persistence, deterministic key-sorted encoders, cross-chunk fluid persistence, authoring-vs-play mode model — settled and shipped.

**Interim:** fluid persisted as a full-field snapshot with no tombstone concept. Migrate to diff-with-tombstones in **0.6.0**, coupled with per-layer versioning: disk and wire are one problem (P4).

**Open:** game-state persistence beyond chunks (**0.9.0**); save migrations proper (**0.11.0**); atomic-write and backup discipline (**0.6.0**) — SQLite gives transaction atomicity but the engine has no world-level backup rotation or partial-write recovery.

### 3.4 Streaming — settled model, one perf gap

Camera-driven radius streaming, LRU + hysteresis eviction, background generation workers — settled.

- **Defect:** generation/meshing throughput is fixed while the streamed area scales with zoom — edges don't stay filled at zoom-out. Fix in **0.3.0**.
- **Drift:** mesh workers are raw `std::thread`, bypassing the rayon-pool model, with hand-duplicated core-budget arithmetic (drift 2.2). Superseded by the unified job system (P14) in **0.3.0**.
- **Open:** per-observer subscription sets — **0.6.0**.

### 3.5 Rendering — strong core, §11 feature surface half-built

**Shipped and settled:** `FaceVertex` 32-byte format, tri-tonal axis lighting, pixel-perfect snapping, two-pass player stencil (the external review's "non-negotiable safety net" — already built), underground visibility (air-side face rule + cost-priority march + clarity radius), water rendering, mesh disk cache with full-input keying.

**Open (designed in §11, unbuilt):** lighting bake, shader-side palette composition, outline pass, enclosure-factor fog, material variants, attribute-swap debug views, fringe/dim clarity states, cave-mouth blending. All **0.5.0**.

**Open and previously unacknowledged:** a **particle/VFX system** and **entity visual representation beyond the player capsule** (animation, model or sprite pipeline). Both are hard MVT requirements absent from the design doc. **0.5.0** and **0.9.0** respectively.

**Architectural prep that must precede the lighting bake:** the design doc's `light_level_index` is a *per-vertex byte*, which makes every light change a remesh event. The external review's strongest technical contribution is that light should be **sampled from a per-chunk 3D texture** instead. This is a decision that must be made before 0.5.0 builds the bake, not after (§8, D2).

**Open decision:** greedy meshing (drift 1.6) — decide at **0.4.0** exit against measured triangle budgets.

### 3.6 ECS & runtime — settled for the player, empty above it

`bevy_ecs` embedded, `FrameStage` contract, fixed-timestep player sim as a pure function, semantic action layer, Authoring⇄Play toggle — settled.

**Drift (new):** three independent ad-hoc clocks exist (`PlayerClock`, `FluidClock`, `ClimateClock`) with no shared tick framework, and no concept of block ticks, random ticks, or scheduled updates — the substrate every survival simulation needs. Unified tick framework in **0.5.0** (P14).

**Open:** headless schedule variant (**0.6.0**); the entire entity layer above the player (**0.9.0**).

### 3.7 Networking — designed, nothing built

The proposal is complete. Prerequisite chain: mutation-door completeness (**0.3.0**) → fluid tombstones + per-layer versioning (**0.6.0**) → server-core extraction (**0.6.0**) → observer-set streaming (**0.6.0**) → loopback (**0.7.0**) → LAN, hardening, internet (**0.8.0**).

### 3.8 Tooling — the largest gap relative to ambition

Detailed inventory in **§4**. The graph editor, hierarchy editor, field probe, noise previewer, hot reload, and debug toggles exist and are good. Missing: authoring UIs for non-graph content, previews that de-risk a change before it hits the world, diagnostics that explain engine behavior, and validation that catches bad content before runtime.

### 3.9 Audio — entirely unestablished

Nothing exists. The material registry reserves sound profiles; undergroundness is designed to drive reverb. Foundation **0.5.0**; game-facing API, soundscape system, and music direction **0.9.0**.

### 3.10 Asset standards — partially established by practice

Prefabs today are **procedural silhouettes** (Rock/Bush/GrassTuft) — not authored geometry, and nothing like what structures need. Real blueprint format, asset conventions, palette standard, registry extension rules, and graph asset versioning land in **0.4.0**.

### 3.11 The engine/game boundary — defined here

The engine ships ready when a title needs **content and rules**, not engine surgery. Engine-side by 1.0: everything in §6. Game-side, permanently: progression design, crafting recipes, combat balance, specific creature behaviors, narrative, and the art direction of a particular title.

### 3.12 Known unknowns carried forward

- **~28 ms at-rest frame hitch**, cause unidentified. Highest-priority open defect; 0.3.0 gates on diagnosing it.
- **Cloud hydrology** ("pretty sure it doesn't work right"): quarantined. Fix-or-remove decision at 0.5.0, zero engineering before then.

---

## 4. The tooling track

Tooling is a first-class deliverable (P11), tracked as its own cross-cutting arc so it cannot be quietly deprioritized when feature work runs long.

### 4.1 Maturity levels

| Level | Meaning |
|---|---|
| **T0 — Raw** | Editable only by hand-editing files outside the engine; no preview; failures surface as runtime misbehavior. |
| **T1 — In-engine** | Editable in an engine panel; hot-reloads; errors surface as messages, not crashes. |
| **T2 — Previewable** | The author sees the result *before* committing it to the world, and can inspect why it looks the way it does. |
| **T3 — Diagnosable** | The subsystem explains itself at runtime: state visualizers, timings, validation, and a path from symptom to cause without adding code. |

**Rule:** a content type at T0 blocks the version that ships content of that type. Every subsystem reaches **T2 by 1.0**; subsystems on the critical path for performance, determinism, or multiplayer reach **T3**.

### 4.2 Category A — Content authoring tools

| Tool | Now | Target | Version |
|---|---|---|---|
| **Node graph editor** (canvas, pins, params) | T1 | T2 | ongoing |
| ↳ Ergonomics: undo/redo, multi-select, cross-graph copy/paste, node search palette, comments and groups, collapse-to-library, inline node docs | T0 | T2 | **0.4.0** |
| ↳ Graph diff & change review | T0 | T2 | **0.4.0** |
| **Hierarchy / manifest editor** | T1 | T2 | **0.4.0** |
| **Material authoring** (palette, light response, sound profile, gameplay flags) | T0 | T2 | **0.5.0** |
| **Palette authoring** (per-axis ramps, biome tint sets, time-of-day bands) | T0 | T2 | **0.5.0** |
| **Blueprint authoring** — in-world blockout → capture, anchor/footprint, destruction policy, sway, variants | T0 | T2 | **0.4.0** |
| **Structure authoring** — multi-chunk templates, placement rules, priority, protected-volume flag | — | T2 | **0.4.0** |
| **Foliage/detail authoring** | T1 | T2 | **0.4.0** |
| **Entity/actor definition authoring** | — | T2 | **0.9.0** |
| **Audio authoring** — surface sound sets, ambience beds, reverb zones, music-director states | — | T1 | **0.9.0** |
| **Input binding editor** | T0 | T1 | **0.9.0** |
| **World creation UI** (seed, manifest, naming, templates) | T0 | T2 | **0.9.0** |
| **Simulation authoring** — material reaction tables (flammability, heat, phase change) | — | T2 | **0.10.0** |
| **Region-transformation authoring** — parameter deltas, front shape, progression rate, edit-survival policy | — | T2 | **0.10.0** |

### 4.3 Category B — Preview & visualization tools

The highest-leverage defense against the 0.2.0 failure mode.

| Tool | What it answers | Version |
|---|---|---|
| **Noise previewer** (shipped) | What does this noise field look like? | ✅ |
| **Field probe** (shipped) | What is the selected node actually outputting? | ✅ |
| ↳ **Extensions**: volumetric slice view, cross-section scrubbing, A/B against the previous value | Where in the volume does this go wrong? | **0.4.0** |
| **Column inspector** | For this (x,z): climate, zone, biome, border distances, per-stage density, materials, fluid, detail, scatter — the whole pipeline, stage by stage | **0.4.0** |
| **Biome/zone map preview** | Top-down world map at arbitrary scale without generating chunks — what makes multi-biome authoring tractable | **0.4.0** |
| **Live parameter scrubbing** | What happens as I drag this value? | **0.4.0** |
| **Isolated preview scene** | How does this biome/structure/blueprint look alone? | **0.4.0** |
| **Regeneration diff view** | Which chunks did this edit invalidate, and what changed? | **0.4.0** |
| **Palette / time-of-day scrubber** | How does the world read at every hour and season? | **0.5.0** |
| **Mesh attribute views** | Why is this surface shaded this way? | **0.5.0** |
| **Lighting debug views** (skylight, block light, quantization bands, enclosure, gameplay-vs-render light) | Why is this area dark — and does the server agree? | **0.5.0** |
| **Region graph visualizer** | Which connected air region is this, where are its boundaries, what is it connected to? | **0.5.0** |
| **Replication inspector** | What does the *other* client see right now? | **0.7.0** |
| **Prefab/entity preview viewport** | How does this asset look and animate in isolation? | **0.9.0** |
| **Region-transformation preview & scrub** | What will this transformation do, and how does the front advance over time? | **0.10.0** |

### 4.4 Category C — Diagnostics & observability

| Tool | Version |
|---|---|
| **`FrameTimings`** — per-`FrameStage` timing + live breakdown panel | **0.3.0** |
| **Job system inspector** — queue depths, priorities, dependency stalls, per-job-kind time | **0.3.0** |
| **Streaming visualizer** — resident set, in-flight generation, mesh queue, evictions, load-margin vs. throughput | **0.3.0** |
| **Cache instrumentation** — mesh cache hit/miss with key attribution | **0.3.0** |
| **Memory/residency panel** — per-layer chunk memory, GPU buffers, session growth | **0.3.0** |
| **Determinism checker** — regenerate a resident chunk in place, diff, surface the first divergent voxel | **0.3.0** |
| **Mutation & event log** — recent commands through the door with origin, mode, resulting invalidation, emitted events | **0.3.0** |
| **In-engine console** — teleport, time set, chunk reload, force regen, spawn, toggle systems | **0.4.0** |
| **Profiling capture export** + frame graph | **0.5.0** |
| **Tick scheduler inspector** — scheduled/random/block tick load per region | **0.5.0** |
| **Network panel** — bandwidth, delta queue depth, prediction corrections, interpolation health, per-chunk sequences | **0.7.0** |
| **Entity inspector** — live components; realized/abstract tier state | **0.9.0** |
| **Simulation heatmaps** — active-cell density, wake propagation, dirty-region churn | **0.10.0** |

### 4.5 Category D — Validation, CI, and pipeline

| Tool | Version |
|---|---|
| **Headless generation CLI** — generate N reference chunks, hash, compare | **0.3.0** |
| **Graph validation CLI** — manifest resolution, cycles, boundary mismatches, orphan outputs, type errors | **0.4.0** |
| **Asset lint** — registry ID collisions, dangling references, missing files, naming violations, hardcoded user-facing strings | **0.4.0** |
| **Golden-image render regression** | **0.5.0** |
| **Perf regression job** | **0.5.0** |
| **Desync test suite** — one test per taxonomy class | **0.7.0** |
| **Save migration test matrix** | **0.11.0** |
| **Asset bundling & release packaging** | **0.11.0** |
| **Project scaffolding** — new-title template | **0.11.0** |

---

## 5. The 1.0 readiness rubric

1.0 is reached when every row passes. The proof title (§19) is how several rows are demonstrated, not a substitute for them.

| # | Domain | 1.0 criterion |
|---|---|---|
| **R1** | **World authoring** | An author can create a world — zones, biomes, caves, structures, rivers, foliage, materials, palettes — entirely in-engine, without hand-editing a file, previewing each before committing it. |
| **R2** | **Tooling maturity** | Every content type at **T2** or better; performance, determinism, and networking subsystems at **T3**. No T0 content types remain. |
| **R3** | **Runtime capability** | Infinite streamed world with biome variety, caves, structures, water, weather, lighting, audio, VFX, and at least one deep interactive simulation — stable at budget for a multi-hour session. |
| **R4** | **Game-facing API** | Every §6 capability has a documented, stable surface callable from game code with no reaching into engine internals. Proven by §6.30's conformance set. |
| **R5** | **Multiplayer** | Co-op over LAN and internet: join-in-progress, host live-editing with clients connected, every desync class covered by a passing test, graceful disconnect and rejoin. |
| **R6** | **Performance** | §7.3 budgets green on declared min-spec, verified by automated regression, zero unattributed hitches. |
| **R7** | **Data durability** | Saves migrate forward across versions; registry identity is stable across content changes; atomic writes with backup recovery; no known data-loss defect. |
| **R8** | **Extensibility** | Registries extensible from data without recompiling; asset formats versioned and documented; the mod-API boundary defined even if unshipped. |
| **R9** | **Coherence** | *(added v1.2)* Visibility, lighting, and interaction agree: what the renderer shows, what the interaction system targets, and what the server simulates are the same world state (P13). No system's "truth" contradicts another's. |
| **R10** | **Documentation** | Design doc current; asset standards published; a getting-started guide takes a newcomer from clone to a running custom world; every §6 API documented. |
| **R11** | **Distribution** | Installable artifact, first-run experience, crash reporting, settings persistence, accessibility options — usable by someone who has never opened the repo. |

---

## 6. Core engine capabilities for building game features

The concrete surface a game feature is implemented against — the detail behind R4, and the checklist that says whether "the engine can support a game" is true.

### 6.1 World query API
Point and interval solidity (`WorldView::solid_interval`, slab-aware), material/shape lookup, surface height per column, fluid level and type, ray/DDA cast with face and adjacent-empty results, region-volume queries, chunk residency, biome/zone/climate lookup, line-of-sight, and **region-graph membership** (§6.22). Must be safe under streaming (P3): every query distinguishes "not resident" from "empty."
**State:** core queries shipped and used by collision, targeting, and room detection. Formalization as a documented, streaming-safe trait — **0.9.0**. If the grid-handle decision (§8, D3) lands, queries are addressed against a grid rather than the world singleton.

### 6.2 World mutation API (the door)
**The only write path** (P5). Command kinds for voxels, detail paint, scatter, fluid, blueprint stamping, and region transformation; each carries origin (dev-authoring vs. play), actor identity, and validation context. Returns the resulting delta: touched chunks, invalidated meshes, persistence marks, **emitted events** (§6.7), and — from 0.7.0 — the network broadcast. Supports batching for brush and structure operations, and transactional apply with rollback on validation failure.
**State:** shipped for player and editor edits. **0.3.0** audits every call site onto it and asserts origin/mode. Batching, transactions, blueprint stamping, event emission — **0.4.0**. Network broadcast — **0.7.0**.

### 6.3 Entity & actor framework
**Everything dynamic that isn't voxels.** Three concerns kept separate — the external review's clearest structural point, and one the engine has not yet gotten wrong only because it has no entities:
- **Storage:** a global archetype ECS store. Entities are *not* owned by chunks. Chunk ownership conflates storage with locality and forecloses out-of-range simulation.
- **Spatial queries:** a separate loose grid at roughly chunk resolution. Entities move constantly, so cheap insert/remove beats query speed; a BVH is the wrong shape. This same index is the **network interest-management structure** (§6.18).
- **Persistence:** serialized *by region*, without being *owned* by one.

Plus: stable entity IDs across save/load and network, component conventions (position, velocity, collider, renderable, replicated), lifetime and ownership rules, and data-driven entity type definitions so a game adds a type by authoring rather than writing an ECS bundle.
**State:** the player is the only entity; no framework above it. The single largest gap for game implementation. **0.9.0**.

### 6.4 Character controller & physics
Fixed-timestep pure-function stepping, slab-aware **swept** AABB resolution (not discrete — tunneling is a swept-collision problem), auto-step, gravity and terminal velocity, fluid buoyancy and swimming, sub-tick ordering when moving geometry displaces entities, and an exposed parameter block so a game tunes a controller rather than rewriting one. Reusable by non-player actors.
**Isometric-specific requirement:** players misjudge depth constantly under this projection. Generous step-up, forgiving edge collision, and ledge-snap assist are not polish — **physics tuning is readability work here**, and belongs in the same version as the controller generalization.
**State:** shipped and working for the player. Generalization, fluid interaction, forgiveness tuning, and the parameter block — **0.9.0**.

### 6.5 Interaction & targeting
Reach-limited cursor and forward raycast; target resolution across voxels, scatter, fluids, and entities with cycling when several share an anchor; placement validation against footprints; highlight rendering hooks; registerable interaction kinds.
**Two requirements from the external review, both real for this camera:**
- **Targeting must consult visibility state** (P13). Under an orthographic camera the mouse ray passes diagonally through an entire column, and first-hit DDA cannot reach anything under an overhang. The fix is a **hybrid**: first-hit by default, with scroll stepping backward through candidate cells along the ray. Hidden geometry is skipped using the same visibility classification the renderer uses — not a separate heuristic.
- **The cursor cell must render as a visible 3D box drawn through geometry.** A first-person crosshair is self-evident; an isometric cursor is not.
**State:** reach-limited break/place shipped for the player, voxels only, first-hit only. Candidate stepping, visibility-aware skipping, multi-store resolution, entity targeting, registerable kinds — **0.9.0**; the visibility-agreement gate is verified at **0.5.0** when the region graph is promoted to shared state.

### 6.6 Blueprint & prefab system
A real format: voxel template (shapes and materials, slabs included), anchor offset, footprint, destruction policy, sway weights, variant sets, placement rules, and a **protected-volume flag** that region transformation (§6.26) must respect — the mechanism that lets authored vaults survive a self-modifying world. Runtime: capture a region to a blueprint, stamp through the mutation door with priority resolution, reference blueprints from graphs for worldgen structures.
**State:** `PrefabDef` is a procedural silhouette — nothing like this. Format, authoring, capture, stamping — **0.4.0**; game-side runtime use (build copy/paste) — **0.9.0**.

### 6.7 Event & messaging layer (world mutation event bus)
**How systems talk without coupling.** Typed engine events with subscription: voxel changed, chunk loaded/unloaded, entity spawned/despawned, fluid entered cell, region graph changed, **region parameters changed** (§6.26), player mode changed, world time crossed threshold, interaction performed. Ordering guarantees relative to `FrameStage`, and a declared local-vs-replicated policy per event type.
The external review's argument for building this early is correct and specific: a region-scale change must invalidate biome assignment, spawn and loot tables, ambient audio and weather, structure placement, ecosystem populations, the pathfinding region graph, lighting fields, and meshes across a large area. Hard-wiring those calls into the transformation code means every future system edits that code. **Subscribers, not call sites.**
**State:** ad-hoc; systems observe each other's resources directly. Bus foundation + mutation-door emission — **0.4.0** (small now, structural later); full game-facing event surface with replication policy — **0.9.0**.

### 6.8 Data, registries & configuration
Registries for materials, prefabs, palettes, sounds, entity types, interaction kinds, and material reaction tables — each with name-keyed identity, RON authoring against a validated schema, hot reload, and extension by additional files (the mechanism mods later use).
**Identity discipline (new requirement, and a latent defect today):** the authoring and *persistence* identity is a **stable namespaced string** (`voxulacrum:oak_slab`); numeric IDs are a runtime interning detail, and **every save embeds its own name↔ID mapping**. Numeric IDs baked into region files without a mapping are why modded Minecraft worlds corrupted for a decade — and the engine currently does exactly that. Fix lands with the format redesign at **0.6.0**.
Plus engine configuration: graphics, audio, input, and accessibility settings with persistence and defaults.
**State:** materials and prefabs on the registry pattern; palettes partial; sounds, entity types, interaction kinds, reaction tables absent. Schema validation and a uniform registry trait — **0.4.0**; string identity + save mapping — **0.6.0**; remaining registries with their subsystems — **0.9.0**/**0.10.0**.

### 6.9 Behavior definition & scripting
For 1.0: game rules are **Rust against the §6 APIs**, plus **data-driven definitions** (entity types, interaction kinds, spawn tables, weighted selection tables, reaction tables) in RON. A scripting runtime is explicitly *not* in 1.0 — the API surface must stabilize first. What 1.0 guarantees is that adding scripting later is a binding exercise, not a redesign. The **mod-API boundary is defined at 0.11.0 even though modding does not ship**, because defining it is what forces clean engine/content separation.
**State:** data-driven definition surface — **0.9.0**; boundary definition — **0.11.0**; scripting — post-1.0.

### 6.10 AI & navigation primitives
Geometric walkability queries; **hierarchical pathfinding** — coarse routing over the region graph (§6.22), fine path within a region (flat voxel A* over long distances is brutal at this world scale); streaming-aware path invalidation; spatial queries (nearest entity, entities in radius) served by §6.3's loose grid; a steering/locomotion helper.
**Architectural note:** ECS is the right tool for entity *storage* and data-parallel work (movement integration, index updates) and the wrong tool for the behavior logic itself. Behavior runs as trees or utility scoring over ECS data. Deep conditional logic inside ECS systems becomes unmaintainable quickly.
**State:** the resident walkability mask was removed in Phase 9 in favor of direct geometric queries; nothing consumes them for AI. **0.9.0**.

### 6.11 Camera framework
Camera modes (follow, free, cinematic, fixed), zoom bands with pixel snapping preserved, target switching and blending, shake/impulse hooks, screen↔world projection helpers.
**Follow behavior requirement:** tight vertical tracking of a player whose Y changes constantly makes the world bob nauseatingly. Camera Y must track a **quantized floor level** — last grounded Y, moved only past a threshold, eased over ~200 ms — and ideally anchor to the current **enclosure region's floor**, which holds perfectly still while the player moves around inside a building. Horizontal follow uses an asymmetric screen-space deadzone extended in the movement direction, defined in camera space.
**State:** interpolated pixel-snapped follow shipped; free camera in authoring mode. Quantized-floor follow and region anchoring — **0.5.0** (it depends on the region graph and is a felt-quality issue worth fixing early); mode framework and game-facing control — **0.9.0**.

### 6.12 UI framework
**The single most common way projects of this size stall.** The external review puts it at 30–40% of total development time and identifies ad-hoc UI as the month-eight killer. Two rules that must be structural rather than retrofitted:
- **UI is a view, never an owner.** Under authoritative multiplayer, screens render *replicas* of server state and every interaction is a request. The widget layer must be architecturally incapable of holding authority.
- **Screens must be data-definable.** New content adds new screens; if every screen is hand-written Rust, content work becomes engine work.

Needed: an in-game UI layer at pixel-art scale (egui remains dev tooling permanently, per P9 — it is not the game UI stack), layout with resolution scaling, controller navigation, data binding, HUD element registration, menu/screen stack with input focus and pause semantics, and world-space anchors (nameplates, prompts).
**State:** a minimal play HUD. **0.9.0**, and it should be scoped as one of that version's two largest items rather than a line on a list.

### 6.13 Input & bindings
`GameAction` enum, `input.ron` maps, edge/held/continuous triggers, context stacks (gameplay vs. menu vs. editor), gamepad support, runtime rebinding.
**State:** semantic layer and map file shipped. Context stacks, gamepad, rebinding UI — **0.9.0**.

### 6.14 Audio API
Positional one-shots and loops; surface-material-driven footsteps and impacts via the registry; ambience beds by biome, depth, weather, and time; **occlusion-aware attenuation reusing the region graph**; mix buses with settings-backed volumes; voice limits; streaming versus cached decisions; and network-appropriate behavior (sounds triggered by replicated events, not simulated twice).
**Listener placement — a real decision for this camera:** at the camera, audio is spatially correct but emotionally detached; at the player, it is gameplay-correct but panning will not match the screen. Settled answer: **listener at the player, panning computed in camera space.**
**Soundscape and music direction:** the ambience this engine is aimed at comes substantially from *silence and rare arrival*, not from a playlist — so the music layer is a **state-driven director** (biome, depth, weather, time, tension) rather than a track queue. This belongs in the engine because its inputs are engine state.
**State:** nothing. Foundation — **0.5.0**; game-facing API, soundscape, director, mixing — **0.9.0**.

### 6.15 VFX & particles
A particle system respecting the pixel-art constraint (quantized, palette-driven, no smooth gradients or sub-pixel drift): block-break debris, fluid splashes, dust, footstep puffs, ambient motes, weather precipitation, fire and smoke. Emitters attachable to entities, voxel positions, and events; pooled and budgeted; replicated as events rather than per-particle state. Budget generously — under this camera, particles carry much of the atmosphere that perspective and camera motion would otherwise provide.
**State:** nothing. Design and build — **0.5.0**; game-facing emitter API — **0.9.0**.

### 6.16 World time, weather & ambience
Day-night cycle as replicated simulation state driving palette bands and lighting; calendar and season hooks; a weather state machine with per-biome eligibility; wind as a shared value driving foliage sway, particles, and audio; and an **interior/exterior determination** sourced from the region graph so weather and temperature stop at a roof.
**State:** time-of-day exists as a rendering concern; cloud hydrology exists and is suspect. Clock as simulation state, weather, wind — **0.5.0**; game-facing surface — **0.9.0**.

### 6.17 Game-state persistence
Entity state (with sleep-in-unloaded-chunk semantics), player profiles and inventory, world metadata (time, weather, seed, manifest hash, **per-region generation parameters** per §6.26), session data, and a versioned game-data section a title extends without touching the chunk format.
**Durability requirement:** **atomic saves** — write to temp, fsync, rename; rotating backups; partial-write detection with fallback on load. Roughly a day of work, and it is the difference between a bug report and a player who never launches the game again.
**State:** chunk persistence only; no backup or recovery discipline. Atomic-write and backup — **0.6.0**; game-state layer — **0.9.0**.

### 6.18 Replication patterns for game features
Documented patterns: server-authoritative component replication, event replication, opt-in client prediction for player-controlled state, interest scoping via §6.3's spatial index, and the rule that any game state outside the chunk model declares its replication policy at definition time.
**State:** architecture supports it; no patterns documented because no game features exist. **0.9.0**, validated by the proof title.

### 6.19 Inventory & container framework
**Generalize "holds item stacks" exactly once.** Player inventory, chests, machine slots, and entity inventories are one interface. The base abstraction must include **programmatic insert/extract with per-slot filtering** from the start — automation-style content needs it, and retrofitting it means editing thirty call sites. Plus slots, stacks, capacity, transfer operations, hotbar binding to actions, container entities, and item definitions as registry data.
Recipes, progression, and balance stay game-side.
**State:** a material-cycling swatch. **0.9.0**.

### 6.20 Debug & development surface for game code
Registerable console commands, cheat/toggle registry, entity inspector, log capture with filtering, assertion and error conventions, hot-reloadable game data, and a **photo mode** — which for a game selling on ambience is marketing infrastructure, not a toy.
**State:** engine-side toggles exist; nothing registerable from game code. Console — **0.4.0**; registerable surface — **0.9.0**; photo mode — **0.11.0**.

### 6.21 Accessibility & localization
**Localization:** all user-facing text lives in string tables from the first UI work — extraction is nearly free at the start and expensive after a thousand hardcoded strings. Asset lint enforces it (§4.5). Translations ship whenever.
**Accessibility:** colorblind-safe palette variants (load-bearing given P8's palette-driven identity), text scaling, motion and screen-shake reduction, remappable input including gamepad, and adjustable interaction assist (the ledge-snap and step-up forgiveness in §6.4 are accessibility features as much as feel features).
**State:** nothing. String tables enforced from **0.9.0** (first game UI); accessibility options — **0.11.0**.

### 6.22 Region graph (enclosure & connectivity as shared world state)
**Adopted from the external review, and the highest-value structural addition it offered.** A parallel field over air voxels assigning each to a connected region, with per-chunk connectivity summaries joined across chunk borders and updated incrementally — never globally refilled. Regions carry derived state: enclosed or sky-connected, revealed, volume, boundary faces.

It is worth building properly rather than as a rendering byproduct because it has **at least seven consumers**:

1. Occlusion — hide ceilings of enclosed interiors (P13: shared state, not a shader decision)
2. Creature spawn shelter checks
3. Interior/exterior flag for weather and temperature (§6.16)
4. Audio occlusion and reverb (§6.14)
5. Hierarchical pathfinding — coarse routing over regions (§6.10)
6. Abstract creature movement between regions (§6.23)
7. Interior fill light for revealed regions (§6.25)

Plus, for this engine specifically: camera Y anchoring to a region floor (§6.11).

Runs at **voxel resolution using face coverage masks**, never at a finer subdivision — a system with seven consumers cannot afford an 8× cost multiplier. Known to degrade on huge irregular caves, overhangs, and half-built structures, which is why occlusion remains a hybrid rather than relying on regions alone.
**State:** the engine computes a bounded visibility flood and room detection per frame for rendering, discarding the result. Promoting that computation to persistent, incrementally-maintained, multi-consumer world state — **0.5.0**.

### 6.23 Entity simulation tiers
**How a world feels alive beyond the streaming radius.** Three tiers, with the transitions defined before either side is built:

| Tier | Contents | Tick rate | Scale |
|---|---|---|---|
| **Realized** | Full ECS, physics, animation, behavior | Full | Dozens |
| **Abstract** | Position on the region graph, needs, health, current goal; moves between regions | ~1 Hz | Thousands |
| **Statistical** | Per-region populations, prey density, territory pressure; spawns abstracts to maintain equilibrium | Rare | All regions |

**The transitions are where the bugs live.** Abstract→realized must produce state consistent with abstract history; realized→abstract must write state back faithfully. Getting this wrong makes creatures appear to "reset" when the player walks away, which destroys the illusion more thoroughly than never having built it.
**State:** nothing. Realized and abstract tiers plus the transition contract — **0.9.0**; statistical tier — **0.10.0** (it is also the quietest form of world-scale change).

### 6.24 Tick & scheduling framework
**The substrate every survival simulation needs, and currently absent.** One tick framework replacing the three ad-hoc clocks (`PlayerClock`, `FluidClock`, `ClimateClock`): fixed simulation tick with render interpolation, scheduled block updates, random ticks over resident regions with a defined distribution, sub-rate clocks derived from the primary tick rather than free-running, and a coarse region tick for distant simulation. All of it replicated-clock-aware so server and client agree on tick identity (P14).
**State:** three independent clocks, no block/random/scheduled tick concept. **0.5.0**.

### 6.25 Lighting as a queryable field
**Two lights, named separately** — the external review's reframe, and correct for authoritative multiplayer: `GameplayLight` and `RenderLight`. The server must answer "can a creature spawn here," "does this crop grow," "is snow melting," "is the player in darkness" — questions no GPU shadow map can answer. Conflating them is how a server ends up disagreeing with what players see (R9).

Gameplay light can therefore be cheap: quantized sky and block channels at voxel resolution, updated over several ticks, exposed as a world query.

**Render light samples a per-chunk 3D texture, not baked vertex attributes.** This is the architectural prep that must land *before* the bake is built: with vertex-baked light, placing a torch is a remesh event, and remeshing is comparatively expensive here. With a 3D texture (one-voxel border for correct interpolation, roughly 4–6 KB per chunk) light changes stop touching meshes at all, interpolation is smooth rather than banded, and any later real-time sun-shadow term is additive rather than a pipeline rewrite. It also preserves the baked path as a low-spec quality setting for free.
**Interior fill light** for revealed regions resolves the conflict between cutaway occlusion and directional lighting: a hidden ceiling that still casts leaves the room a black hole; one excluded from casting floods the interior with sunlight. Neither is acceptable; a per-region `revealed` flag driving an ambient boost reads as "the camera is showing you inside."
**State:** the design doc specifies `light_level_index` as a per-vertex byte. Decision at **0.5.0** (§8, D2), implemented in the same version. Colored-light data layout is a separate forcing-date decision (§8, D6).

### 6.26 World-scale change (region transformation)
**The capability that separates a building sandbox from a survival sandbox with progression** — a world that visibly reshapes in response to player progress, in the Terraria-hardmode sense. It requires one storage property the engine does not currently have:

> Generation cannot be a pure function of `(seed, coords)`. It must be a function of `(seed, coords, region_parameters)`, where region parameters are **persistent, mutable world state**.

With per-region parameters stored alongside the existing edit delta, a transformation becomes *change the parameters, regenerate the base, reapply the delta* — rather than loading and rewriting every affected chunk's absolute contents. That buys: affordable retroactive change, saves that stay small, replication of "region parameters changed" instead of millions of voxels, and transformations that can be re-run or reverted because parameters are data rather than baked results.

The engine's generated/authored split already provides the delta half (P4). **The missing half is persistent per-region generation parameters, and adding it after the format freezes is a redesign.** Hence the forcing date (§8, D4) at 0.6.0 — the storage lands then even though the feature ships later.

**Transformation is a process, not a snap.** Applying change progressively — spreading, rising, creeping across the landscape over minutes or in-game days — makes the performance problem trivial (a background job on the existing scheduler) *and* is the better design: a player watching a transformation advance is memorable; a pause followed by a different world is not. It also gives players agency: something advancing can be resisted, diverted, or raced.

Propagation is via the event bus (§6.7), not hardcoded calls. **Protected volumes** (§6.6) and any fixed sub-dimensions (§6.27) are exempt. Whether player edits survive a given transformation is a per-transformation, possibly per-material policy — a delta-preserved house standing intact inside radically altered terrain is sometimes a fortress holding out and sometimes absurd.
**State:** nothing; generation is a pure function today. Storage — **0.6.0**; the feature, authoring, preview, and propagation — **0.10.0**.

### 6.27 World instances & protected volumes
Multiple world instances (sub-dimensions) with per-instance seed and generation configuration, shared registries, independent streaming and persistence, and transit between them. The engine's manifest currently assumes exactly one world.

This exists for a specific reason beyond "dimensions are a genre staple": **procedural generation structurally cannot deliver authored, shared secrets.** A fixed-seed sub-dimension identical for every player restores that, and combined with **protected volumes** (authored vaults placed procedurally but exempt from region transformation) it gives hand-authored content a home inside a self-modifying procedural world.
**State:** nothing; single-world assumption throughout. Design — **0.6.0** (it interacts with persistence keying and must not be retrofitted into the format); build — **0.10.0**.

### 6.28 Interactive simulation framework
The substrate for material simulation beyond fluids, and the engine-side half of what makes a survival sandbox feel reactive.

**Scope decision, aligned with the external review:** the goal is *interaction density among a small number of materials*, not falling-sand granularity. A dozen materials with rich cross-reactions beats a hundred with none, and per-voxel material simulation across a 3D world would consume the entire performance budget.

**The transferable technique — a parallel regioned update model:** simulation chunks update in parallel, coloured by parity in x, y, and z so no two simultaneously-updating chunks are face-, edge-, or vertex-adjacent. In 3D that is **8 passes**, and it eliminates locks and races outright. Per-chunk **dirty regions** track a bounding box of active cells so sleeping chunks are skipped entirely — the trick that makes large-scale flow affordable, since most chunks are asleep most of the time. **Wake propagation** rouses a chunk when a neighbour's boundary activity touches it.

The engine's fluid simulation already uses two-phase updates and active sets — closely related, and the natural first consumer of the generalized model. **Fire and heat** is the strongest pairing: cheap, high gameplay value, the best interaction partner for fluids, and a natural driver of §6.26-scale change (floods, burns, regrowth are exactly the visible progressive transformations that section calls for). Material reaction tables (flammability, ignition, heat capacity, phase change) are registry data (§6.8).
**State:** fluid simulation shipped with a related but bespoke update model. Generalized framework + fire/heat — **0.10.0**.

### 6.29 Grid abstraction *(pending decision D3)*
Systems addressed against a grid handle rather than a world singleton, so that finite detached voxel grids — static ruins, moving platforms, buildable vehicles — become a policy difference rather than a different kind of thing. Consumers: raycasting, collision, lighting, meshing, region detection, pathfinding, targeting, block updates.
This entry is **conditional**: it is in the capability list because the retrofit cost is severe and the decision is dated (§8, D3), not because the feature is committed. If D3 declines, this entry is struck and the reasons recorded.

### 6.30 API conformance set (how R4 is proven)
A fixed set of small game features, each exercising a different slice of the above, implemented **without modifying engine crates** and kept building in CI from 0.9.0 onward:

1. A placeable, breakable light source (registry + mutation door + lighting field + VFX + audio).
2. A pickup item that persists, replicates, and stacks in the hotbar (entity + persistence + replication + inventory + UI).
3. A wandering creature that paths around terrain, shelters indoors, and reacts to the player (entity + region graph + hierarchical pathfinding + controller reuse + events).
4. A container entity two clients use simultaneously (interaction + inventory + replication + conflict resolution).
5. A weather-triggered ambient effect that stops at a roofline (world time + region graph interior flag + VFX + audio).
6. *(from 0.10.0)* A material that burns, spreads to neighbours, and leaves a changed surface (simulation framework + reaction tables + events + VFX).

If any requires an engine change, the engine is not at R4 — and the required change becomes a gate on the version owning that capability.

---

## 7. Versioning policy

### 7.1 Scheme

Pre-1.0: `0.MINOR.PATCH`. A minor version is a completed arc, shipped when its exit gates pass; patches are fixes only. Post-1.0: semver proper — a major version is a breaking change to save compatibility, asset formats, or protocol that migrations cannot bridge.

### 7.2 Compatibility promises by version

| Boundary | Promise |
|---|---|
| ≤ 0.5.x | Saves may be wiped on upgrade. The cheap-wipe era ends at 0.6.0. |
| 0.6.0 | Per-layer save versioning, string-keyed registry identity with embedded mapping, and per-region generation parameters land together at the **final planned** format redesign. Changes are additive or migrated from here. |
| ≥ 0.7.0 | Protocol version = build version; mismatched clients refused, never partially accommodated. |
| ≥ 0.9.0 | The §6 game-facing APIs enter a deprecation policy: breaking changes require a documented migration note. |
| ≥ 0.11.0 | Saves migrate forward across versions. Wipes are no longer an option. |
| 1.0.0 | Save compatibility, asset-format stability, API stability, and co-op protocol stability are user-facing promises. |

### 7.3 Performance budgets (tracked from 0.3.0, gating from 0.5.0)

- **Frame:** 16.6 ms sustained at default zoom and streaming radius; no recurring hitch > 4 ms without an attributed cause; worst case < 33 ms at an active generation frontier.
- **Streaming:** visible area fully populated within 2 s of camera rest at any supported zoom.
- **Memory:** documented per-chunk resident budget; session growth flat after warm-up.
- **Save:** an edited chunk's persisted size proportional to genuine deviations (post-tombstones, 0.6.0).
- **Network** (from 0.7.0): steady-state per-client bandwidth bounded by active-set deltas; join payload = manifest + graphs + registries + deviations only.
- **Entities** (from 0.9.0): budget for realized entities per observer and abstract entities per world, with a documented degradation path.
- **Simulation** (from 0.10.0): active-cell budget per tick with graceful throttling; a fully burning forest degrades rate, never frame time.
- **Min-spec** (declared at 0.11.0): budgets verified on that hardware. The target audience is explicitly people on modest hardware — the baked-lighting path is preserved as a quality setting rather than deleted, which the texture-sampling architecture (§6.25) provides for free.

### 7.4 Release checklist (every minor)

Workspace version bumped; CHANGELOG entry written; close-of-version audit committed; design-doc revision for newly-settled architecture; **any forcing-date decisions due this version recorded with reasons** (§8); determinism suite green; perf baselines recorded and compared; tooling-maturity table updated; open-defect list triaged with nothing silently dropped.

---

## 8. Forcing-date decisions

Architectural choices whose cost rises sharply at a named version. Each is decided in writing on or before its date, with reasons, recorded in that version's audit. **Silence is not a decision** — but neither is adoption automatic; several of these will rightly be declined.

| # | Decision | Due | Framing |
|---|---|---|---|
| **D1** | **Shape vocabulary: keep 4 shapes, or move to an octant-occupancy mask?** | **0.6.0** (format freeze) | An octant mask (`u8`, 2×2×2) is a strict superset of the current vocabulary — `Cube` = `0xFF`, `SlabBottom` = `0x0F` — and yields corners, steps, stairs, and ~240 more shapes at the same struct size, with combination as bitwise OR and orientation implicit. All faces remain cardinal-axis-aligned, so **P8 is not violated**. Against: it multiplies mesh candidate cells 8×, complicates the mesher and the slab-smoothing pass, and the current vocabulary is a deliberate, load-bearing simplification whose legibility at this camera distance is proven. The honest question is whether a survival sandbox's *building* wants stairs and corners badly enough to pay for it. Decide consciously; it is effectively unrevisitable after the format freezes. |
| **D2** | **Light sampling: per-vertex attribute, or per-chunk 3D texture?** | **0.5.0** (before the bake is built) | Recommended: **texture**. Vertex-baked light makes every light change a remesh, and remeshing is not cheap here. The texture path decouples light from meshing, gives smooth interpolation, keeps the baked field as a low-spec setting, and makes any later real-time term additive. Cost is a shader path and per-chunk texture management. Deciding after the bake exists means building it twice. |
| **D3** | **Grid abstraction: address systems against a grid handle, or keep the world singleton?** | **0.6.0** (server-core extraction touches this code anyway) | The discipline is cheap *if* adopted while the world types are being moved regardless; it is a year of untangling later. It unlocks detached static grids (nearly free), moving platforms, and buildable vehicles (a real exploration payoff), at the cost of an abstraction layer over everything and, if grids ever move, hierarchical networking, per-grid region detection, moving occluders, and local-vs-world lighting. Reasonable outcomes: adopt the handle discipline without committing to moving grids; or decline explicitly and record that vehicles are out of scope. |
| **D4** | **Per-region generation parameters in the save format?** | **0.6.0** (format freeze) | Recommended: **yes**, independent of whether region transformation (§6.26) ever ships. Storing generation inputs per region alongside the delta costs little and is the only thing that keeps world-scale change, seed-independent regional variation, and parameter-versioned regeneration possible at all. Declining forecloses the category permanently. |
| **D5** | **String-keyed registry identity with per-save mapping?** | **0.6.0** (format freeze) | Recommended: **yes**, and this one is closer to a defect fix than a design choice. Saves currently store bare numeric IDs; any registry reorder or added material silently reinterprets existing worlds. Cheap now, data-corrupting later. |
| **D6** | **Colored block light (RGB) or single channel?** | **0.5.0** (with the bake) | A data-layout decision, not a later feature: 3–4× the light field cost, and not retrofittable once the format and shader path exist. Weigh against P8 — a quantized, palette-driven look may get more from a single channel plus per-material palette response than from true RGB. |
| **D7** | **4-way camera rotation?** | **0.5.0** (before palette/lighting authoring hardens) | Not currently in the engine. It removes the dominant isometric frustration (building behind a wall you cannot see around) but the consequences are large and specific here: tri-tonal `face_axis` ramps currently authored for three visible axes would need all six to read well; the streaming region becomes a union of four sheared boxes; block art must read from four angles; and a world-fixed sun cannot be well-offset from all four azimuths, so lighting must be tuned to be acceptable at every rotation rather than optimal at one. Defensible outcomes: adopt now (before palette authoring bakes in three-axis assumptions), or decline and solve occlusion entirely through visibility state (P13) — which the engine has already invested in and which works. |
| **D8** | **Greedy meshing for side and bottom faces?** | **0.4.0** exit | Decide against measured triangle budgets on the 3-biome reference world. Interacts with D1: an octant vocabulary changes the arithmetic substantially. |

---

## 9. Version ladder overview

| Version | Theme | Headline |
|---|---|---|
| 0.3.0 | Consolidation & observability | The foundation becomes true: drift paid down, one scheduler, every millisecond attributable |
| 0.4.0 | Worldgen completeness & authoring | The 0.2.0 biome promise honored — and authorable in-engine, with previews |
| 0.5.0 | Presentation & world state | Light, atmosphere, audio, VFX — on a region graph and a tick framework that many systems share |
| 0.6.0 | Server-core split & format endgame | Compiler-enforced boundary; the last format redesign, carrying five forcing-date decisions |
| 0.7.0 | Loopback multiplayer | Two processes, one machine; 90% of protocol issues surfaced |
| 0.8.0 | Real transport | LAN, loss/latency hardening, internet, host live-editing with clients |
| 0.9.0 | Game-layer capability | The §6 surface built and proven by the conformance set |
| 0.10.0 | World simulation & progression | Fire/heat, entity tiers, region transformation, sub-dimensions — what makes it a *survival* sandbox |
| 0.11.0 | Hardening & distribution | Migrations, min-spec, accessibility, packaging, docs, scaffolding |
| 1.0.0 | Readiness | The §5 rubric passes; the proof title demonstrates it |

**Where scope can be cut.** If 1.0 must arrive sooner, **0.10.0 is the version to trim** — its capabilities are the ones whose *hooks* (per-region parameters, event bus, tick framework, simulation update model, entity tiers' storage shape) land earlier by design, so cutting them is deferral rather than redesign. Nothing else on this ladder has that property.

---

## 10. 0.3.0 — Consolidation & observability

**Theme:** No new player-visible features. Close every gap between what the docs claim and what the code does; make the engine explain itself; fix the release process.

### Deliverables

**Observability (P10):** `FrameTimings` per-`FrameStage` panel; job system inspector; streaming visualizer; cache hit/miss with key attribution; memory/residency panel; mutation and event log; in-place determinism checker. Diagnose the ~28 ms hitch with them — fix it, or characterize it precisely if external. "Unknown" does not pass the gate. Record the first perf baseline.

**Unified job system (P14):** one scheduler with declared dependencies and priorities, replacing the split rayon-pool / raw-`std::thread` model and its hand-duplicated core-budget arithmetic (drift 2.2). Consumers at this version: generation, meshing, chunk I/O. Priority derives from distance to the observed region. Later versions add lighting, region detection, pathfinding, structures, and transformation as consumers without a second scheduler ever existing.

**Correctness & determinism (P1):** re-seed `PoissonDisk` world-absolute (drift 2.1) — free now, catastrophic after 0.11.0; reorder the pipeline so per-column fluid precedes foliage and submersion reaches `SurfaceFilter`/scatter (drift 1.4); extend determinism tests to a slab-bearing chunk and to Poisson continuity across borders.

**Unification (P5, P7):** mutation-door audit — enumerate every world-write call site, route stragglers through the door, runtime-assert origin and mode. Fluid eval→storage moves behind `StorageBoundary` (drift 2.3).

**Throughput:** generation and meshing throughput derive from the load-margin radius rather than fixed constants.

**Process:** CHANGELOG rebuilt with real 0.1.x and 0.2.0 entries; workspace version corrected; §7.4 checklist adopted; headless generation CLI in CI.

### Non-goals
New biomes, new nodes, rendering features, audio, entity work, networking beyond the mutation-door audit, cloud hydrology.

### Exit gates
- [ ] Hitch diagnosed: fixed or root-caused with evidence
- [ ] Drift findings 1.4, 2.1, 2.2, 2.3 closed and verified
- [ ] One job system; no second threading model anywhere
- [ ] Mutation-door audit: zero write paths outside the door
- [ ] Streamed area fills within budget at all supported zooms
- [ ] Determinism CI job green, including slab and Poisson coverage
- [ ] All seven category-C diagnostics shipped and usable
- [ ] Perf baseline committed
- [ ] §7.4 checklist passes

---

## 11. 0.4.0 — Worldgen completeness & authoring

**Theme:** Honor the 0.2.0 promise, and make it authorable. Content breadth, the last missing generation infrastructure, and — equally weighted — the tools that make authoring a loop instead of a guess (P11).

### Deliverables

**Cross-chunk generation infrastructure** — blocking structures and rivers. Designed first as a design-doc extension: deterministic cross-chunk placement with margin-band ownership generalized from the scatter model, priority-resolved stamping across boundaries, generation-order independence from world-absolute derivation (P1).

**World mutation event bus (§6.7)** — typed events emitted by the mutation door and by generation, with subscription and `FrameStage` ordering guarantees. Small now; the thing that keeps region transformation, spawn tables, audio, and pathfinding from being hardcoded into each other later.

**Content systems:** blueprint/prefab format (voxel templates, anchor, footprint, destruction policy, sway, variants, **protected-volume flag**) with in-world capture and mutation-door stamping; ZoneGraph structures with priority resolution; rivers with carved beds feeding fluid initialization; authored library graph bodies closing the `LibraryRef` density gap; foliage tiers 2/3 matured with trees as hero blueprints and destruction-policy enforcement.

**Authoring tools:** graph editor ergonomics (undo/redo, multi-select, cross-graph copy/paste, search palette, comments and groups, collapse-to-library, inline docs); graph diff review; hierarchy and manifest editor at T2; blueprint and structure authoring; foliage tuning.

**Preview tools:** column inspector; biome/zone map preview; field-probe volumetric and cross-section views with A/B; live parameter scrubbing; isolated preview scene; regeneration diff view.

**Validation:** graph validation CLI with in-editor error surfacing; asset lint; in-engine console.

**Asset standards** written into the design doc, including registry schema and graph asset versioning.

**Content proof:** the reference world ships **3+ biomes across 2 zones**, including one below-sea-level biome and one cave-bearing biome via `StandardCaveNoise`, with structures and a river crossing chunk and biome boundaries — and **every piece of it authored through the tools, not by hand-editing JSON.** That last clause is the real gate.

**Decision due:** D8 (greedy meshing), with measurements.

### Non-goals
Rendering features, networking, entity work, simulation, multi-distance smoothing.

### Exit gates
- [ ] Reference world: 3+ biomes, 2 zones, structures, river — deterministic, seam-free, hot-editable
- [ ] Every reference-world asset authored in-engine; zero hand-edited JSON in shipped content
- [ ] Cross-chunk placement passes generation-order-independence tests
- [ ] Blueprint format shipped; a structure captured in-world places correctly through worldgen
- [ ] Event bus carrying mutation and generation events, with at least two independent subscribers
- [ ] Column inspector and biome map preview answer their questions on the reference world
- [ ] Graph validation CLI green in CI; asset lint clean
- [ ] Asset standards written into the design doc
- [ ] D8 recorded with measurements
- [ ] §7.4 checklist passes

---

## 12. 0.5.0 — Presentation & shared world state

**Theme:** Build the §11 rendering surface, the two systems the design doc never accounted for (audio, VFX) — and, underneath both, the two pieces of shared world state that many later systems depend on: the region graph and the tick framework. Three forcing-date decisions land here.

### Deliverables

**Region graph (§6.22)** — promoted from a per-frame rendering computation to persistent, incrementally-maintained world state with per-chunk connectivity summaries joined across borders. Immediate consumers this version: occlusion (now reading shared state per P13), audio occlusion and reverb, interior/exterior determination for weather, interior fill light, and camera Y anchoring. Pathfinding and abstract creature movement consume it at 0.9.0.

**Tick & scheduling framework (§6.24)** — one framework replacing `PlayerClock`/`FluidClock`/`ClimateClock`, adding scheduled block updates, random ticks, and a coarse region tick. Replicated-clock-aware from the start.

**Lighting (D2, D6 due):** skylight flood + block-light BFS bake; `GameplayLight`/`RenderLight` separation with gameplay light exposed as a world query; render light delivered per the D2 decision; per-material light response; incremental relight on edits within budget; per-region interior fill light resolving the cutaway/lighting conflict.

**Palette & surface:** shader composes material, biome tint, light, and face axis at draw time; per-biome palettes; time-of-day ramps **without re-meshing**; material variants by world-position hash.

**Outlines & atmosphere:** screen-space Sobel + mesh edge-flag composite outlines; enclosure-driven cave fog; per-Y atmospheric tinting and distance desaturation; underground-visibility polish.

**World clock & weather:** day-night as simulation state on the new tick framework; season hooks; weather state machine with per-biome eligibility and region-graph-sourced interior exemption; wind as a shared value driving sway, particles, and audio.

**VFX/particles (§6.15):** quantized palette-driven particle system; emitters on entities, voxels, and events; break debris, splashes, dust, footsteps, motes, precipitation; pooled and budgeted.

**Audio foundation (§6.14):** device plumbing, spatial attenuation, listener-at-player with camera-space panning, surface sounds through material profiles, region-graph occlusion and reverb, ambience and fluid loops.

**Camera:** quantized-floor Y follow with region anchoring (§6.11) — a felt-quality fix worth making before the world gets more vertical.

**Interaction coherence gate (R9, P13):** verify that targeting and collision agree with the visibility classification the renderer uses. Any disagreement is fixed here, where the shared state is being built.

**Tools:** material and palette authoring at T2; palette/time-of-day scrubber; mesh attribute views; lighting debug views including gameplay-vs-render comparison; region graph visualizer; tick inspector; profiling capture export; golden-image and perf regression jobs in CI.

**Decisions due:** D2 (light sampling), D6 (colored light), D7 (4-way rotation).

**Greedy meshing** if D8 said yes. **Cloud hydrology:** fixed to the standard of the rest of the atmosphere stack, or removed.

### Non-goals
Networking, new worldgen features, entity or gameplay systems, region transformation.

### Exit gates
- [ ] Region graph persistent, incrementally updated, and consumed by at least four systems
- [ ] One tick framework; the three ad-hoc clocks are gone
- [ ] Lighting bake shipped; time-of-day shifts without re-meshing; edit relight within budget
- [ ] `GameplayLight` queryable and demonstrably consistent with what the renderer shows
- [ ] Targeting and collision agree with rendered visibility in every occlusion mode
- [ ] Outlines, fog, variants, debug views on by default and stable
- [ ] Particles running under a budget cap with no frame-time regression
- [ ] Audio: positional surface sounds and region-driven reverb working in caves and rooms
- [ ] Perf budgets (§7.3) hold with the full stack on — **budgets become gating from this version**
- [ ] D2, D6, D7 recorded with reasons
- [ ] Cloud hydrology fixed or removed
- [ ] §7.4 checklist passes

---

## 13. 0.6.0 — Server-core split & format endgame

**Theme:** The last single-process version, and the **last cheap format change**. Five forcing-date decisions become format facts here; then the codebase splits along the client/server boundary. Pure-refactor discipline outside the format work.

### Deliverables

**Format endgame — one coupled event, the final cheap wipe:**
- **Per-layer save versioning:** layer version fields, absent-layer defaults for forward compatibility, migration scaffolding — and explicitly designed so a *new layer* can be added later without rewriting storage. More layers are certain.
- **String-keyed registry identity (D5):** namespaced string identity as the authoring and persistence key, with every save embedding its own name↔ID mapping. Closes the latent corruption defect.
- **Per-region generation parameters (D4):** generation becomes a function of recorded, mutable per-region inputs, stored alongside the delta. The storage half of §6.26 lands even though the feature ships at 0.10.0.
- **Fluid diff-with-tombstones:** drained water stays drained; straddle chunks stop serializing untouched generated cells; disk and wire fixed together.
- **Atomic saves and backups (§6.17):** temp-write, fsync, rename; rotating backups; partial-write detection with fallback.
- **World instance keying (§6.27):** persistence keyed so multiple world instances are addressable, even though sub-dimensions ship at 0.10.0.
- **Shape vocabulary (D1)** decided and, if changed, landed here.

**Server-core crate extraction:** world state, generation, fluids, overrides, persistence, event bus, tick framework, and the mutation door move to a crate with no wgpu/winit/egui types; renderer-side chunk views split out. Compiler enforces the boundary thereafter. Headless `FrameStage` schedule. Binaries: dev/host, dedicated server, and the client shape 0.7.0 connects.
**Grid abstraction (D3)** decided here, because this refactor touches exactly the code the decision affects — adopting the handle discipline during a move that is happening anyway is the cheap moment.

**Observer-set streaming:** `ChunkStreamingManager` generalizes to per-observer subscription sets; local player = observer 0; server residency = union of subscriptions + active-simulation chunks. Single-player behavior identical by construction.

**Tools:** persistence inspector (what is stored for this chunk, and why); streaming visualizer extended to per-observer sets.

### Non-goals
Actual networking, new features, rendering changes.

### Exit gates
- [ ] All five format decisions (D1, D3, D4, D5, plus per-layer versioning) recorded and, where adopted, implemented
- [ ] A save survives a registry reorder and a material insertion with no data reinterpretation
- [ ] Adding a new chunk layer requires no storage rewrite (demonstrated with a throwaway layer)
- [ ] Drained water stays drained; save-size budget met on straddle chunks
- [ ] Atomic save verified by kill-during-save testing; backup restore works
- [ ] Server-core compiles with no renderer dependencies (CI-enforced)
- [ ] Headless server generates, simulates, and persists with no window
- [ ] Single-player runs client + embedded core with zero user-visible change; perf within tolerance of 0.5.0
- [ ] §7.4 checklist passes

---

## 14. 0.7.0 — Loopback multiplayer

**Theme:** Two processes on one machine over an in-memory channel. No transport polish — pure architecture: subscription lifecycle, prediction, delta ordering. This surfaces ~90% of protocol issues cheaply.

### Deliverables

- **Protocol v1:** versioned envelope, version-lock handshake, join payload (seed, manifest, graphs, registries with identity mapping, content hash).
- **Chunk replication:** client-local generation + per-chunk base-content hash verification + overrides, fluid deviations, and region parameters applied; authoritative-fetch fallback on mismatch.
- **Mutation flow:** commands up, server validation through the door, authoritative deltas down with per-chunk sequence numbers; sender-side optimistic apply with confirm-or-correct.
- **Player movement:** input frames up, server sim, client rewind-reconcile-replay from the shared pure step function; other players by snapshot interpolation ~100–150 ms behind, no extrapolation.
- **Simulation:** server-only; changed-cell deltas per tick bounded by the active set; cosmetic client interpolation.
- **Event replication:** the §6.7 bus gains its network policy; replicated events drive client-side audio and VFX rather than being simulated twice.
- **Join-in-progress:** subscribe payloads snapshot at tick boundaries.
- **Tools:** network panel; replication inspector; desync test suite covering every taxonomy class.

### Non-goals
Real sockets, latency handling, sessions and permissions, host live-editing with clients, gameplay features.

### Exit gates
- [ ] Two-process loopback: a second client joins in progress, sees a hash-identical world, builds alongside the first
- [ ] Concurrent same-cell edits resolve identically on all clients
- [ ] Prediction corrections near zero in a still world; correct under edit conflict
- [ ] Fluid poured by one client renders correctly on the other, within bandwidth budget
- [ ] Replicated events produce identical audio and VFX on both clients
- [ ] Every desync class has a passing test
- [ ] §7.4 checklist passes

---

## 15. 0.8.0 — Real transport

**Theme:** From loopback to hostile networks.

### Deliverables

- **Transport:** reliable-ordered streams + unreliable-sequenced datagrams (`renet` or QUIC via `quinn`; plain TCP acceptable if measurements say head-of-line blocking is tolerable — measure, don't assume). Swappable behind the message layer.
- **Loss/latency harness:** delay, loss, and reorder injection; soak tests at 50/150/300 ms and 1–5% loss before internet exposure.
- **Sessions:** join/leave lifecycle, graceful disconnect and rejoin with subscription state rebuilt, server-side player identity, permission hooks in the mutation door.
- **Host live-editing with clients connected:** the invalidation table as a network event — server re-overlays overrides on regenerated chunks, bumps graph hashes, re-sends affected subscribed chunks.
- **LAN discovery** and direct-IP internet connect.

### Non-goals
Accounts, auth, encryption; matchmaking; dedicated-server ops tooling.

### Exit gates
- [ ] LAN session: 3+ clients, join-in-progress, stable 30+ minute build session
- [ ] Soak profiles pass; no desync class regresses; movement acceptable at 150 ms
- [ ] Rejoin after disconnect restores a consistent world
- [ ] Host edits a graph mid-session; clients converge without rejoining
- [ ] Internet session between real remote machines succeeds
- [ ] §7.4 checklist passes

---

## 16. 0.9.0 — Game-layer capability

**Theme:** The version that decides whether this is an engine or a tech demo. Everything in §6 that doesn't exist gets built, exposed, documented, and proven.

**Scoping note:** this is the largest version on the ladder, and its two heaviest items — the **UI framework** and the **entity framework** — each deserve treatment as an arc rather than a bullet. Expect to re-plan this version when 0.8.0 closes, with real information. If it splits, it splits along that seam.

### Deliverables

**Entity & actor framework (§6.3):** global archetype store (not chunk-owned), separate loose spatial grid doubling as network interest management, region-serialized persistence, stable IDs, component conventions, data-driven type definitions, sleep-in-unloaded-chunk semantics.

**Entity simulation tiers (§6.23):** realized and abstract tiers with the transition contract defined and tested — abstract→realized consistent with history, realized→abstract faithful. Statistical tier deferred to 0.10.0 but its seam is designed here.

**Controller generalization (§6.4):** reusable actor controller with swept collision, exposed parameter block, fluid buoyancy and swimming, moving-geometry displacement ordering, and the isometric forgiveness tuning (generous step-up, forgiving edges, ledge-snap assist).

**Interaction & targeting (§6.5):** hybrid first-hit-plus-candidate-stepping, visibility-aware skipping, visible 3D cursor box, multi-store resolution with cycling, entity targeting, registerable interaction kinds.

**AI & navigation (§6.10):** hierarchical pathfinding over the region graph, streaming-aware invalidation, spatial queries, steering helper, behavior-tree/utility layer over ECS data.

**Inventory & container framework (§6.19):** one abstraction with programmatic insert/extract and per-slot filtering from the start; slots, stacks, transfers, hotbar binding, container entities, item registry.

**UI framework (§6.12):** in-game UI layer at pixel-art scale; layout with resolution scaling; controller navigation; data binding; view-never-owner enforced structurally; data-definable screens; HUD registration; menu stack with focus and pause; world-space anchors; world create/load/select; settings screens. **String tables from the first screen** (§6.21).

**Input completion (§6.13):** context stacks, gamepad, rebinding UI.

**Camera framework (§6.11):** modes, blending, shake hooks, projection helpers.

**Event layer completion & replication patterns (§6.7, §6.18):** full game-facing event surface with per-type replication policy documented.

**Game-state persistence (§6.17):** entity state, player profiles, world metadata, versioned game-data section.

**Audio and VFX game-facing APIs:** emitter and sound APIs from game code; soundscape system; state-driven music director; mix buses wired to settings.

**Entity visuals:** animation and model/sprite pipeline for non-player entities, obeying the same tri-tonal face-axis discipline as terrain.

**Debug surface for game code (§6.20):** registerable console commands, cheat registry, entity inspector, log filtering.

**The conformance set (§6.30), items 1–5:** implemented against the APIs with no engine-crate modifications, kept building in CI.

### Non-goals
Actual game content, region transformation, deep simulation, scripting runtime, modding API surface, console ports.

### Exit gates
- [ ] Conformance features 1–5 build and run without engine-crate changes
- [ ] Feature 4 (shared container) behaves correctly for two clients
- [ ] Feature 3 demonstrates hierarchical pathfinding and shelter behavior via the region graph
- [ ] Entities persist, sleep in unloaded chunks, replicate, and survive the abstract↔realized transition without state loss
- [ ] A player can create a world, play, save, quit, resume, host, and configure settings entirely through game UI — no dev panels
- [ ] No hardcoded user-facing strings (asset lint enforced)
- [ ] Every §6 capability has documented API surface
- [ ] Entity, audio, and VFX budgets defined and green
- [ ] §7.4 checklist passes

---

## 17. 0.10.0 — World simulation & progression

**Theme:** What makes it a *survival* sandbox rather than a building sandbox. Every capability here sits on infrastructure that landed earlier by design — which is also why this is the version to cut if 1.0 must arrive sooner.

### Deliverables

**Interactive simulation framework (§6.28):** the generalized parallel regioned update model — 8-pass 3D parity colouring, per-chunk dirty regions, wake propagation — with the existing fluid simulation migrated onto it as the first consumer.

**Fire & heat:** propagation along flammable material, consumption, heat and smoke production, and meaningful interaction with fluids and with enclosure (a coarse heat grid pairs naturally with the region graph for interiors). Material reaction tables as registry data, authorable at T2.

**Region transformation (§6.26):** parameter-delta transformations applied as a **visibly progressive process** — a front advancing across the landscape over minutes or in-game days, running as a background job on the existing scheduler. Propagation via the event bus to biome assignment, spawn and loot tables, ambience and weather, structure placement, populations, the region graph, lighting, and meshing. Protected volumes respected. Per-transformation edit-survival policy. Server-authoritative, replicated as a parameter change plus a progress front.

**Statistical entity tier (§6.23):** per-region populations, prey density, territory pressure, spawning abstracts to maintain equilibrium — the quietest form of world-scale change, and the thing that makes an ecosystem feel like it exists when the player is not watching.

**World instances & sub-dimensions (§6.27):** multiple instances with per-instance seed and generation config, shared registries, independent streaming and persistence, transit between them, and exemption from region transformation for fixed-seed authored spaces.

**Tools:** simulation authoring (reaction tables); region-transformation authoring and preview with scrubbing; simulation heatmaps.

**Conformance set item 6:** a material that burns, spreads, and leaves a changed surface — no engine changes.

### Non-goals
Structural integrity simulation, per-voxel granular materials, additional dimensions beyond the mechanism, scripting.

### Exit gates
- [ ] Fluid simulation migrated onto the generalized update model with no behavior regression
- [ ] Fire propagates, interacts with water and enclosure, and stays within the active-cell budget under a burning-forest stress test
- [ ] A region transformation runs progressively to completion on a live world with a client connected, and all listed systems update via events with no hardcoded call sites
- [ ] Protected volumes and a sub-dimension survive a transformation intact
- [ ] Abstract population state persists and re-realizes consistently after an hour away
- [ ] Conformance item 6 builds without engine-crate changes
- [ ] §7.4 checklist passes

---

## 18. 0.11.0 — Hardening & distribution

**Theme:** Turn a capable engine into shippable software.

### Deliverables

- **Save migrations live:** the first real layer-version migration ships; migration test matrix in CI; wipes retired permanently.
- **Min-spec declared and verified:** target hardware named; budgets measured there; degradation paths (reduced streaming radius, particle and entity caps, the baked-lighting quality setting, simulation throttling) implemented and exposed in settings.
- **Accessibility (§6.21):** colorblind-safe palette variants, text scaling, motion and shake reduction, full input remapping including gamepad, adjustable interaction assist.
- **Packaging:** release profile, asset bundling, installer or portable artifact, crash reporting with log capture and opt-in reporting, first-run experience.
- **Stability soak:** 4+ hour sessions, single-player and 4-player, with leak monitoring.
- **Documentation:** design doc final pre-1.0 revision; asset standards published; getting-started guide from clone to running custom world; §6 API reference.
- **Mod-API boundary defined (§6.9)** — not shipped, but drawn, because drawing it is what proves the engine/content separation is real.
- **Project scaffolding:** new-title template that builds and runs.
- **Photo mode.**

### Non-goals
New engine capability, new content systems, console ports, shipped modding.

### Exit gates
- [ ] Saves from 0.6.0, 0.9.0, 0.10.0, and 0.11.0 all load via migration
- [ ] Budgets green on declared min-spec via the automated regression job
- [ ] 4-hour 4-player soak with no leak or degradation
- [ ] Accessibility options present and effective
- [ ] A newcomer following the getting-started guide reaches a running custom world
- [ ] Scaffolded template project builds and runs
- [ ] §7.4 checklist passes

---

## 19. 1.0.0 — Readiness

**Theme:** Prove the rubric. No new systems.

### Deliverables

- **Rubric verification:** every row of §5 checked with evidence in the release audit. A failing row sends work back to the version that owns it — 1.0 does not ship around a failed row.
- **The proof title:** a small complete co-op survival sandbox built on the reference world and the conformance features — one biome cluster, one cave stratum, one deep simulation system, one progression gate, one small-scale transformation event, and a full ambience pass. Content and rules only; **if it requires engine surgery, a 0.x gate failed** and the work returns there.
- **Promise activation:** save, asset-format, API, and protocol stability documented and tested; deprecation policy written; a planned post-1.0 format change dry-run migrated.
- **Full-pass QA:** desync suite, migration matrix, min-spec budgets, extended multiplayer soak, fresh-machine install-to-play.

### Exit gates
- [ ] All eleven rubric rows pass with recorded evidence
- [ ] The proof title is playable co-op for an evening without engineer intervention
- [ ] Zero known data-loss defects; zero unattributed hitches
- [ ] Compatibility promises tested and survivable
- [ ] §7.4 checklist passes

---

## 20. Post-1.0 outlook (sketch)

- **Modding API:** the boundary is drawn at 0.11.0; the work is a stable surface, load order, sandboxing, and validation.
- **Scripting runtime:** evaluated once the §6 APIs have lived through a real title.
- **Real-time sun shadows:** orthographic with fixed elevation is near best-case — a translating fixed-size shadow box, no cascade splits, trivial texel snapping, precisely sizeable coverage. A discrete project with a large visual return *if* D2 chose the texture path, since the shader already receives light as a sample and the shadow term is additive. Two cautions stand: the target aesthetic is stylized and hand-lit rather than physically plausible, and the baked path must remain as a low-spec setting.
- **Moving grids:** if D3 adopted the handle discipline — platforms, then buildable vehicles at one level of hierarchy (the realistic exploration payoff), with multi-part animated grids as a stretch beyond that.
- **Zoom-band LOD:** the scale-driven strategy §11 reserves.
- **Multi-distance smoothing:** revisit with cross-chunk infrastructure and scatter reordering in hand — two of the four recorded blockers retired by then.
- **Decal layer:** the reserved chunk layer becomes real.
- **Structural integrity simulation:** the remaining major candidate from §6.28's list.
- **Dedicated-server ops:** admin tooling, headless config, hosting docs; accounts, auth, encryption at the transport layer.
- **Voxel raymarched shadows / cone-traced GI:** exists, works in production, requires GPU-resident voxel data via a camera clipmap, and raises minimum spec substantially. Worth knowing about; not worth doing before there is a game.

---

## 21. Appendix — external architecture review disposition

Every item from `engine-architecture-reference.md`, with what was done about it. The review was written without direct knowledge of this engine, so a substantial fraction describes systems that already exist here in a different form; recording that explicitly is as useful as recording the adoptions.

### 21.1 Already implemented — no action

| Review item | Engine status |
|---|---|
| Multiplayer designed in from the start | Settled since v1.5; full proposal exists |
| Orthographic projection, fixed elevation | Shipped |
| Player silhouette depth-fail pass ("build it first") | Shipped in Phase 9 as a two-pass stencil |
| Enclosure detection for occlusion | Shipped as the air-side face rule + cost-priority march — arguably a stronger formulation, since "no surface leakage" is a property of the classification rather than a rule to enforce |
| Layered storage: edits as a delta over generated terrain | Shipped (`ChunkOverrides`, §9 of the design doc) — though only half of what §6.2 of the review means; see 21.3, D4 |
| Chunked store with palette compression and parallel layers | Shipped |
| Fixed timestep + render interpolation | Shipped for the player |
| Deterministic, versioned generation | Shipped and CI-tested |
| Data-driven registry | Shipped for materials and prefabs |
| Hot-reloading definitions | Shipped |
| In-engine world editor, worldgen preview | Shipped in part; extended substantially at 0.4.0 |
| Threaded meshing with dirty-region invalidation | Shipped |
| Not pursuing granular falling-sand fidelity | Agreed; §6.28 takes the behavior and update model only |

### 21.2 Adopted

| Review item | Where it landed |
|---|---|
| Visibility as queryable world state, not a rendering effect | **P13**, R9, verified at 0.5.0 |
| Region graph with multiple consumers | **§6.22**, built at 0.5.0 |
| `GameplayLight` / `RenderLight` separation | **§6.25**, 0.5.0 |
| Light sampled from a 3D texture rather than vertex attributes | **§8 D2**, decided and built at 0.5.0 |
| Per-region interior fill light for revealed regions | **§6.25**, 0.5.0 |
| Entity storage: global store + separate spatial index, not chunk-owned | **§6.3**, 0.9.0 |
| Spatial index doubles as network interest management | **§6.3 / §6.18** |
| Three-tier entity simulation (realized / abstract / statistical) | **§6.23**, 0.9.0 + 0.10.0 |
| ECS for storage, behavior trees for AI | **§6.10** |
| Hierarchical pathfinding over the region graph | **§6.10**, 0.9.0 |
| World mutation event bus, built early | **§6.7**, foundation at 0.4.0 |
| Per-region generation parameters enabling world-scale change | **§6.26 / §8 D4**, storage at 0.6.0, feature at 0.10.0 |
| Transformation as a visibly progressive process | **§6.26**, 0.10.0 |
| Protected volumes and fixed sub-dimensions | **§6.6 / §6.27** |
| Stable string IDs with mapping stored in the save | **§8 D5** — found to be a live latent defect; 0.6.0 |
| Storage designed so new layers don't require a rewrite | **0.6.0 exit gate** |
| One job system with dependencies and priorities | **P14**, 0.3.0 |
| Tick scheduler: block, random, scheduled, region ticks | **§6.24**, 0.5.0 |
| Atomic saves, rotating backups, partial-write detection | **§6.17**, 0.6.0 |
| UI is a view, never an owner; data-definable screens | **§6.12**, 0.9.0 |
| UI framework as a project-scale risk | **§16 scoping note** |
| One container abstraction with programmatic insert/extract and filtering | **§6.19**, 0.9.0 |
| Swept AABB, sub-tick ordering for moving geometry | **§6.4**, 0.9.0 |
| Physics forgiveness as readability work under this camera | **§6.4**, 0.9.0 |
| Hybrid targeting: first-hit plus candidate stepping | **§6.5**, 0.9.0 |
| Visible 3D cursor box drawn through geometry | **§6.5** |
| Camera Y quantization; anchoring to region floor | **§6.11**, 0.5.0 |
| Audio listener at player, panning in camera space | **§6.14** |
| Soundscape driven by biome/depth/weather/time with region occlusion | **§6.14**, 0.9.0 |
| State-driven music director rather than a playlist | **§6.14**, 0.9.0 |
| Generous particle budget | **§6.15 / §7.3** |
| 8-pass 3D parity colouring, dirty regions, wake propagation | **§6.28**, 0.10.0 |
| Fire/heat as the pairing for fluids | **§6.28**, 0.10.0 |
| Localization: extract strings from the start | **§6.21**, enforced from 0.9.0 by asset lint |
| Accessibility options | **§6.21**, 0.11.0 |
| In-engine profiler / frame graph | **§4.4**, 0.3.0 and 0.5.0 |
| Photo mode as marketing infrastructure | **§6.20**, 0.11.0 |
| Mod-API boundary defined even if modding never ships | **§6.9**, 0.11.0 |
| Region-transformation preview in-editor | **§4.3**, 0.10.0 |
| Real-time sun shadows as a discrete later project | **§20** |
| Keep the baked path as a low-spec setting | **§7.3**, min-spec degradation |
| Vertical-slice scoping over architectural completeness | **§19**, the proof title's shape |

### 21.3 Raised as forcing-date decisions rather than adopted

Adopting these silently would overwrite settled architecture; declining them silently would foreclose real capability. Both outcomes are recorded either way.

- **Octant occupancy mask replacing the shape vocabulary** → **D1**. Compatible with P8 (all faces stay cardinal-aligned) and a strict superset of the current vocabulary, but a large change to a deliberate simplification.
- **Grid tree / `GridHandle` discipline** → **D3**. The review calls it nearly free; here it is cheap only during the 0.6.0 extraction, and it commits to an abstraction over every world query.
- **4-way camera rotation** → **D7**. Settled for the review's design, absent from this one, and interacting specifically with tri-tonal `face_axis` lighting, streaming volume, and block art.
- **Colored (RGB) block light** → **D6**. A data-layout decision that must precede the bake.

### 21.4 Declined, with reasons

- **Cone/cylinder cutaway occlusion** — the review rates it poorly, and Phase 9 independently tried and rejected a clip-box formulation for the same reason: reveal-by-deletion exposes whatever is behind the deletion.
- **Per-voxel fade occlusion as a primary strategy** — costs the opaque pass, sorting, and greedy-mesh batching. The air-side face rule already solves the problem structurally. Reserved only for the residual single-occluder case, and only if it ever proves necessary.
- **Free-angle or 6DOF grid rotation** — conflicts with P8, and the review reaches the same conclusion for the same reason.
- **4×4×4 octant resolution** — not legible at this camera distance; the review also recommends against it. Moot unless D1 adopts octants at all.
- **Noita-level granular material simulation** — out of scope by agreement.
- **Voxel raymarched shadows / cone-traced GI** — noted in §20; requires GPU-resident voxel clipmaps and raises min-spec against a stated modest-hardware audience.
- **Design-layer content (spatial gating, progression structure, secret design, extrinsic-motivator tensions)** — outside the engine/game boundary (§3.11). The *engine requirements* those sections imply — protected volumes, sub-dimensions, discovery-flag storage, withholding map information — are captured in §6.27 and §6.17.

---

## 22. Document maintenance

This roadmap is authoritative for sequencing and for the 1.0 readiness definition; `engine-design.md` is authoritative for architecture. When a version closes, its audit reconciles both: gates checked here, newly-settled shapes revised there, forcing-date decisions recorded in both. When reality forces re-sequencing, the ladder is edited deliberately — moved items keep their content, and the change is recorded below.

**Revision history:**
- v1.0 — Initial roadmap. MVT defined as co-op multiplayer sandbox; ladder 0.3.0→1.0.0; 0.2.0 retrospective encoded as process rules; drift findings and post-phase-9 open items assigned to versions.
- v1.1 — Redefined 1.0 by the readiness rubric rather than by shipping an MVT. Added the tooling track with a T0–T3 maturity model. Added the core engine capability surface for game implementation, with an API conformance set as the rubric's proof. Added pillars P11 and P12. Split hardening out of the game-layer version. Identified VFX/particles and non-player entity visuals as unacknowledged gaps. Removed resolved items (player collision, cross-chunk fluid persistence).
- v1.2 — Integrated the external architecture review (`engine-architecture-reference.md`), with full disposition recorded in §21. Added pillars **P13** (visibility as queryable world state) and **P14** (one scheduler, one clock family), and rubric row **R9** (coherence). Added §8, the forcing-date decision list (D1–D8), and made recording them a release-checklist item. Added capabilities §6.22–§6.29: region graph, entity simulation tiers, tick framework, lighting as a queryable field, world-scale change, world instances and protected volumes, interactive simulation framework, and the conditional grid abstraction; renumbered the conformance set to §6.30 and added a sixth conformance feature. Amended §6.1, 6.3–6.5, 6.7, 6.8, 6.10–6.12, 6.14, 6.17, 6.19–6.21 with adopted corrections. Added **0.10.0 (World simulation & progression)** and moved hardening to 0.11.0, with an explicit note that 0.10.0 is the designated scope-cut point. Recorded a newly-found latent defect: saves store numeric registry IDs with no embedded name mapping.
