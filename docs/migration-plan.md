# Migration plan

Close 0.4.0 now and fix the two save-data-loss bugs first. After that, work is organized as workstreams of small items with explicit dependencies, grouped into milestones, and a version number is cut only when a milestone completes or compatibility changes. The engine/game boundary (M2) comes first; the runtime spine (M3) and the format freeze (M4) can then run in parallel; and presentation work (M5) waits for the new data model so the mesher, light layout and region graph are built once.

## Where v0.4.0 stands

The v0.4.0 branch (commit 2b2cf1e, about 50,600 lines across six crates, with 380 tests) is further along the target than most engines at this stage, and its purpose is met, so it closes now (next section). Much of what exists should be carried forward rather than rewritten:

- **The mutation door** (`World::execute` with origin and mode, and a per-intent whitelist for `System`), which becomes the Commands verb for every system.
- **Overrides-only persistence** with deterministic, key-sorted encoders and a self-healing filter on load.
- **Deterministic node-graph generation** with world-absolute seeding, the margin-band feature model, CI determinism checks and in-engine authoring with hot reload.
- **Palette-compressed cubic chunks** with a uniform fast path and copy-on-write `Arc` storage.
- **One job scheduler** with `(class, distance)` priority and introspection.
- **A pure, server-portable player step** at a fixed 30 Hz with interpolation.
- **The world event bus**, the mutation log and the in-engine diagnostics suite.
- **The presentation core**: texel-snapped projection, low-resolution targets, sub-pixel compensation, palette and outline passes, shader hot reload.
- **The process itself**: themes with non-goals, exit gates, forcing-date decisions, audits and one branch per version. The proposed ladder below keeps all of it.

What is missing is mostly what the roadmap already schedules after 0.4.0: the tick framework, light, region graph, the client/server split, entities above the player, audio, game UI and any engine/game boundary. The app is a single binary crate that owns systems, content wiring and editor panels together, which is exactly the mixing this plan addresses.

## Closing 0.4.0

0.4.0 closes with its finished work and an honest gate record, which roadmap rule 2 already allows: a dropped gate is a recorded scope decision.

1. Fix the one failing test, `a_tree_emits_capsules_owned_by_the_chunk_holding_their_thick_end` in `nodegraph-eval`. A node with a single tip child solves to radius 0.059999995 against a 0.06 tip, failing `ra >= rb` by one float step. A tolerance in the test keeps the generation hash anchor; clamping each node to its widest child in `skeleton.rs` `solve()` fixes the cause but may move the anchor.
2. Bump the workspace version (it still reads 0.3.0), write the 0.4.0 CHANGELOG entry and record each gate's verdict. Unmet gates move to the game backlog or to a named work item.
3. Merge `v0.4.0` to `master`, tag `v0.4.0` and delete the branch. It is the last version branch.
4. From then on, every change is a short branch off `master`.

| 0.4.0 gate | What the code shows |
| --- | --- |
| Reference world: 3+ biomes, 2 zones, structures, river | Partly met: three biome graphs and two zone graphs ship, but no shipped graph uses the `River` node |
| Cross-chunk placement is order-independent | Met: `--verify-generation` regenerates in reverse order in CI |
| Blueprint format; a captured structure places through worldgen | Format, capture and stamping exist |
| Event bus with two independent subscribers | The bus exists with two readers, one of which is the log |
| Column inspector and biome map | Both exist |
| Graph validation in CI; asset lint clean | Validation runs in CI; no asset lint exists |
| Every asset authored in-engine; asset standards in the design doc | Not checkable from the code; your call |
| D8 recorded with measurements | Superseded: binary greedy meshing is decided (decision register) |

## Defects to fix first

These were found by reading the code, not by reproducing them. Each is small to fix and is an M1 work item on its own short branch, with a regression test, because two of them lose player work.

| Defect | How it happens | Effect | Fix |
| --- | --- | --- | --- |
| **Edits lost after a chunk is promoted to a full save** | Once a batch pushes a chunk past `DELTA_THRESHOLD` (8,192 edits), `world/mod.rs:374` sets `overrides = None`. The next edit, or water flowing in (`mod.rs:365`, `:470`, `:524`), calls `get_or_insert_with` and creates a nearly empty bucket. `build_chunk_edits` (`persistence.rs:716`) then saves a `Delta` that replaces the stored `Full` record | Every edit before the promotion is lost on the next load. Heavy building in one chunk is exactly the case | Track promotion explicitly (an enum rather than `Option`), and once promoted keep saving `Full` |
| **Dictionary trained mid-session breaks streaming loads** | Autosave calls `try_train_dictionary` once 200 or more rows exist (`persistence.rs:872`) and later rows are written with flag 3. Streaming workers got the dictionary only at startup (`main.rs:563`) or on regeneration (`regen.rs:121`) | In the session where a world crosses 200 saved chunks, reloaded chunks fail with "flags=3 but no dictionary", load without their edits (a logged warning), and the next edit there overwrites the saved row | Train only when opening the world, or bump the reader generation and hand the new dictionary to the streaming manager after training |
| **Fluid capacity underflow** | `below_capacity - mass` and `capacity - m` in `fluid_sim.rs` are u16 subtractions. Voxel edits do not touch the fluid layer, so a cell can hold more mass than its new capacity, for example after a slab is placed into full water | Panic in dev builds (overflow checks are on), a wrapped value in release | Clamp mass when capacity changes and use `saturating_sub` |

Two related fluid issues come from the same reading: in a `Submerged` chunk, deleting a cell at `MIN_MASS` makes it read back as full, which creates mass; and at chunk seams each side caps its half of a transfer differently (`fluid_sim.rs:276-284`), so mass is not conserved across borders.

**Smaller fixes**, each a one-line or one-file change:

- `KeyR` is bound to both `PrimaryAction` and `RotateTool` (`input.rs:252`, `:262`).
- The material table exists three times: `assets/materials.ron`, `load_initial()` in `material_registry.rs`, and `params.rs`, and the tool and play HUD read the `params.rs` copy. Keep only the RON file.
- `MeshCacheParams.enabled` is never read.
- The presets directory is `PathBuf::from("params")` relative to the working directory (`main.rs:212`), unlike every other asset path.
- `fastnoise2` is a dependency but imported nowhere; removing it also lets the app crate build in scratch workspaces.

## Gap map

Most areas are reshaped rather than replaced; only the mesher, draw submission and the shape vocabulary are replaced outright. The Target architecture tab has the detail for each row.

| Area | Today | Action | Milestone |
| --- | --- | --- | --- |
| Engine/game boundary | One binary crate; content wired through `main.rs` and `ecs/systems.rs`; no facade | New: facade, packages, separate game repo, testbed | M2 |
| Client/server | Single process, single schedule, no network code | New: shared, client and server crates with an integrated server | M3 |
| Clocks | Three ad-hoc clocks and wall-clock frame `dt` | Replace with one 30 Hz tick framework | M3 |
| Registries | Numeric IDs on disk; material table in three places | Reshape: namespaced keys now, per-save mapping with the format | M2, M4 |
| Voxel and chunk format | 1 m voxels, 4 shapes, 32 m chunks, fixed vertical range, monolithic blob version | Replace shapes with 0.5 m voxels and two-level storage; add occupancy masks, per-layer versions, region parameters, cubic streaming | M4 |
| Grids | World singleton | Reshape: grid handles, static grids first | M4 |
| Blueprints | JSON v1, yaw only | Reshape into the voxel clip format | M4 |
| Persistence safety | SQLite transactions only | Add checksums, atomic saves, backups, placeholders | M4 |
| Worldgen | Mature; fixed 32-cell resolution; no macro layer | Keep; add resolution parameter, column cache, probe memoization, macro tiles, `libm` | M4, M5 |
| Meshing and draws | Naive faces, 32-byte vertices, per-chunk draws | Replace: binary greedy quads, buffer arena, indirect draws | M5 |
| Light and region graph | None; per-frame visibility flood discarded | New, on the new data model | M5 |
| Render extensibility | Fixed pass list, no shader includes | Reshape: render graph insertion points, shader library, surface functions, lighting models, presentation presets | M5 |
| Audio, particles | None | New | M6 |
| Networking transport | None; proposal complete | New: QUIC, prediction, sessions, content sync | M7 |
| Entities, physics, input, UI | Player only; custom collision; keys only; egui tools | New: entity store, Rapier scenes, action layer with gamepad, game UI | M5 (input, controller), M8 |
| Moving grids, assemblies, AI, logic | None | New | M9 |
| World simulation | Fluid automaton only | Keep fluids; add generalized update model, fire and heat, transformation | M10 |
| Mods and hardening | None | New: WebAssembly runtime, stability tooling, migrations, distribution | M11 |

## How the work is organized

- **Workstreams** are long-lived areas: Fixes (F), Boundary (B), Runtime (R), Data model (D), Rendering (V), Simulation (S), Entities and input (E), Tools (T), and later Networking and Mods.
- **Work items** are the unit of work: one item, one branch. Sizes are S (under 500 changed lines), M (500 to 2,000) and L. An L item is split before it starts, usually by landing the new path beside the old one behind a flag.
- **Dependencies** are recorded only where they are real ("the binary mesher needs occupancy masks"). Anything unblocked can be picked in any order, by interest or energy, which suits part-time solo work.
- **Milestones** are named capability sets with done-when checks. They can overlap, and they are planned in detail only when they are next.
- **Versions** are cut from `master`: a minor when a milestone completes or compatibility changes, a patch for fixes. Numbers are assigned at the cut, never planned. Saves may be wiped until M4; from M4 on they migrate.
- **Branches:** `master` is always green. Branches are named for their item (`fix/promotion-data-loss`, `feat/tick-framework`, `refactor/split-systems`), merged and deleted. Incomplete work lands behind a Cargo feature or runtime setting rather than waiting on a branch. A branch past about 2,000 changed lines, or open for a month, gets split or has its finished part merged. There are no version branches; releases are tags on `master`.

## Milestones

&#91;embedded content: milestone dependencies · main edges only; the table lists every need\]

M1 is independent and goes first. Only grid handles in M4 wait on the runtime spine, so M3 and M4 can be worked side by side once the boundary exists.

| ID | Milestone | Contains | Needs | Done when |
| --- | --- | --- | --- | --- |
| M1 | Clean slate | 0.4.0 close-out, F1 to F6 | Nothing | Defect regression tests and CI green; released as a patch |
| M2 | Engine/game boundary | B1 to B7 | Nothing | Testbed and game build against the facade only; the game repo exists |
| M3 | Runtime spine | R1 to R6 | M2 (B1, B2) | Server-core builds without wgpu, winit or egui; one 30 Hz clock; singleplayer through the server boundary within 5% of 0.4.0's CPU budget |
| M4 | Format freeze | D1 to D11 | B1, B5; R3 for grid handles | Saves survive a registry reorder; a new layer needs no storage rewrite; the converted reference world matches the 1 m original voxel for voxel; atomic saves survive kill-during-save. The last save wipe |
| M5 | Playable on the new data | V1 to V6, S1, S2, E1, E2, T1 | D1, D2; R4 for V6 | 30 fps at minimum settings on an integrated GPU; one testbed scene under all three presentation presets; targeting agrees with visibility |
| M6 | Atmosphere | Audio foundation, particles, sprites and depth sprites, decals, LOD rings, world clock and weather | M5 for the rendered parts; audio needs only events | The roadmap's audio, VFX and weather gates |
| M7 | Multiplayer | Serialized loopback, QUIC, replication, prediction, sessions, package handshake | M3, M4 | The roadmap's loopback and transport gates; four players hosted on the minimum spec |
| M8 | Game layer | Entity store and tiers, Rapier scenes, UI framework with string tables, inventory, audio and VFX APIs, game-state persistence | M3, M5 | Conformance features 1, 2, 4 and 5 with no engine changes |
| M9 | Living world | Moving grids and grid operations, debris, assemblies and IK, AI navigation, logic ports and networks | M8, D8, S1 | Conformance feature 3; a block-built boat floats and sinks when breached; an island is cut free and baked back; a power network crosses a docked ship |
| M10 | World simulation and progression | Generalized update model, fire and heat, region transformation, statistical tier, world instances | M8, D5 | The roadmap's 0.10.0 gates; the trim point if 1.0 must come sooner |
| M11 | Mods and hardening | WebAssembly runtime, capabilities, content download, fault isolation, stability tooling, live migrations, accessibility, packaging | M2, M7 | The roadmap's 0.11.0 gates; the mod runtime may follow 1.0 |
| 1.0 | Readiness | The §5 rubric and the proof title | Everything above | Every rubric row passes |

## Work items for M1 to M5

Items with nothing in Needs can start now, before their milestone: B1, B5, R1, D1, D4, V1, V2 and V3 are all open alongside M1.

| ID | Item | Needs | Size | Milestone |
| --- | --- | --- | --- | --- |
| F1 | Fix edits lost after promotion to a full save |  | S | M1 |
| F2 | Fix the dictionary trained mid-session |  | S | M1 |
| F3 | Fluid capacity underflow and mass conservation |  | S | M1 |
| F4 | Tree taper test (CI) |  | S | M1, before the tag |
| F5 | One material table |  | S | M1 |
| F6 | Small cleanups: `KeyR` binding, presets path, mesh-cache flag, drop `fastnoise2` |  | S | M1 |
| B1 | App library target with a thin binary |  | M | M2 |
| B2 | Split `ecs/systems.rs` by subsystem | B1 | M | M2 |
| B3 | Facade crate and CI dependency check | B1 | M | M2 |
| B4 | Testbed project on the facade | B3 | M | M2 |
| B5 | Namespaced registries in memory |  | M | M2 |
| B6 | Package manifest and data-only loader | B3, B5 | M | M2 |
| B7 | Game repo; move game-specific code out | B3 | M | M2 |
| R1 | Tick framework design in `engine-design.md` |  | S | M3 |
| R2 | One tick framework replaces three clocks | R1, B2 | M | M3 |
| R3 | Server-core crate, in three steps: world and persistence; generation and simulation; mutation door and events | B1, B2 | L | M3 |
| R4 | Render world and extract step | B2 | M | M3 |
| R5 | In-process client/server boundary | R3, R4 | M | M3 |
| R6 | Observer-set streaming | R3 | M | M3 |
| D1 | Occupancy masks |  | M | M4 |
| D2 | 0.5 m two-level storage, in three steps: storage behind a flag; generation resolution parameter; content conversion and switch-over | D1 | L | M4 |
| D3 | Registry mapping in saves | B5 | S | M4 |
| D4 | Per-layer versions and registrable layers |  | M | M4 |
| D5 | Per-region parameters and world-instance keys | D4 | M | M4 |
| D6 | Fluid tombstones | D4 | S | M4 |
| D7 | Atomic saves, checksums, backups, placeholders | D4 | M | M4 |
| D8 | Grid handles with static grids | D2, R3 | L | M4 |
| D9 | Voxel clip format | D2 | M | M4 |
| D10 | Cubic vertical streaming | R6 | M | M4 |
| D11 | Fixtures and sub-voxel models in storage | D2 | M | M4 |
| V1 | Buffer arena and multi-draw indirect |  | M | M5 |
| V2 | Presentation settings as toggles (snapping, render scale, upscale filter) |  | M | M5 |
| V3 | Shader library with includes |  | M | M5 |
| V4 | Binary greedy mesher with packed quads | D1, D2 | M | M5 |
| V5 | Lighting models, surface functions, materials | V3 | M | M5 |
| V6 | Render graph insertion points | R4 | M | M5 |
| S1 | Region graph | D2 | L | M5 |
| S2 | Gameplay light and render light texture | D2 | L | M5 |
| E1 | Input action layer, gamepad, rebinding | B3 | M | M5 |
| E2 | Kit character controller with 0.5 m steps | B3, D2 | M | M5 |
| T1 | Console with registered commands | B3 | S | M5 |

## Module disposition

The five library crates already sit on the engine side; the work is splitting `voxulacrum-app` (30,400 lines). "Server" means the server-core crate, shared with the client for prediction.

| Today | Destination | Action |
| --- | --- | --- |
| `voxel-core` (voxel, shape, palette, buffer, coords, material registry, blueprint) | Engine core: world data | Reshape in M4 (two-level storage, string keys, clips). `load_initial()` moves to test fixtures |
| `nodegraph-ir`, `nodegraph-eval` | Engine core: worldgen | Keep; resolution parameter, kernel registry, column cache |
| `nodegraph-hotreload` | Engine asset system | Watcher becomes the general hot-reload service; species and blueprint loaders become importers; the example builder moves to the testbed |
| `nodegraph-editor` | Editor | Keep; hardcoded default `MaterialId`s come from the registry |
| `app/world/*` (chunk, storage, layers, mutation, events, overrides, persistence, seam, regen, room, fluid\_gen, fluid\_sim, blueprint) | Server | Move largely intact in M3, then reshape in M4 |
| `app/world/streaming.rs` | Server (observer sets) and client (chunk views) | Split; isometric pitch constants become camera-profile inputs |
| `app/world/climate.rs` | Client environment visuals (it drives cloud shadows) | Fix or remove with the cloud hydrology decision |
| `app/jobs.rs`, `core_budget.rs` | Engine core runtime | Keep; add declared dependencies when the region graph needs them |
| `app/meshing/*` | Client render | Replaced by binary meshing in M5 |
| `app/rendering/*`, `cloud_shadow.rs`, `shaders/*` | Client render | Reshape; water constants, grass colors and prefab shapes become data |
| `app/camera.rs` | Engine projection plus kit camera controller | Split |
| `app/input.rs` | Engine input (device and action layers) | `GameAction` becomes registered actions; bindings move to the game |
| `app/player/*` | Kit character controller | Constants move to the game as parameters |
| `app/interaction.rs` | Kit interaction | `PLAYER_REACH` and the default tool move to the game |
| `app/simulation/*` (time of day, wind, manager) | Engine environment | Camera and frustum work moves to the client |
| `app/ecs/schedule.rs`, `ecs/systems.rs` | Split across server, client and render | Break up by subsystem; `render_present_system` becomes the render crate's entry point |
| `app/ui/*` | Editor (all panels) and game (play HUD) | Split |
| `app/params.rs` | Engine settings, editor tunables, game tuning | Split three ways |
| `app/prefabs.rs`, `materials.rs`, `libraries.rs`, `palette.rs` | Engine registries and importers | Reshape |
| `app/diagnostics.rs`, `verify.rs`, `validate.rs`, `graph_reload.rs`, `shader_reload.rs`, `paths.rs` | Engine tools and CLI | Keep |
| `app/main.rs` | Thin binaries | Editor host, client, dedicated server, testbed |
| `assets/*`, `palettes/*` | Testbed and reference-world packages | Move behind package manifests |

## Risks

| Risk | Containment |
| --- | --- |
| More planned work than time, for one part-time developer | Plan only the next milestone in detail; later ones stay at the level of the milestone table. M10 remains the trim point, and the M11 mod runtime can follow 1.0 |
| Branches growing large again | Size and age limits, flags for incomplete work, and L items split before they start |
| 0.5 m voxels touch nearly every system at once | D2 lands in three steps behind a flag; generation stays at 1 m; content converts by exact doubling with a voxel-for-voxel test |
| 8× the cells per volume costs memory and meshing time | Two-level storage keeps unsplit blocks at today's cost; occupancy masks and binary meshing keep per-chunk work small; resident memory per chunk is measured against the §7.3 budget at M4 |
| The server boundary adds overhead to singleplayer | A direct in-process call in M3, serialization only in M7; M3's check holds CPU within 5% of 0.4.0 |
| A presentation style chosen too early hurts how readable the space is in 3D | Styles are runtime presets from M5; compare all three on one testbed scene (terrain, a rotating grid, a character) before committing the first game |
| Integrated-GPU performance is assumed, not measured | Measure from M2 on the current laptop's integrated GPU by forcing the low-power adapter, and track it in `perf-baseline.md` |
| Client-side generation drifts across machines | `libm` in generation, the hash-verify fallback already designed, and a cross-platform determinism job on Windows and Linux runners |
| The game repo splits off before there is much game | Create it in M2 with the few game-specific pieces that exist; it grows through M8 and keeps the boundary honest from the start |

## Documentation drift

These are places where a document or comment no longer matches the code, in the spirit of P7. None changes behavior.

| Where | Says | Code does |
| --- | --- | --- |
| `roadmap.md` §8 D7 | 4-way camera rotation is "not currently in the engine" | Q/E rotates the camera in 90° steps with easing (`camera.rs:122-127`) |
| `world/seam.rs` header | The seam pass bypasses the mutation command API | It goes through the door as `FinalizeSeam` since 0.3.0 (its slabs are still regenerated rather than persisted, as the header says) |
| `shaders/terrain.wgsl:145` | Shadow map is 4096 texels | `SHADOW_MAP_SIZE` is 8192 (`main.rs:70`) |
| `rendering/render_targets.rs` | Reflection targets are half resolution | `REFLECTION_DOWNSCALE` is 1 |
| `world/climate.rs` | Not yet wired to the frame schedule (module-level allow(dead\_code)); biome identity blends over about 320 m | `FADE_RADIUS` is 10 columns, and the automaton runs every frame via `climate_tick_system` and `cloud_shadow.tick` (`world_eval.rs:32`) |
