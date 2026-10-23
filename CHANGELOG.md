# Changelog

All notable changes to the voxulacrum engine.

Pre-1.0 versioning follows `roadmap.md` §7.1: a version is cut when a milestone
completes or compatibility changes, and patches are fixes only. Saves may be
wiped on upgrade until milestone M4, the format freeze.

> **Entries for 0.1.0 through 0.2.0 were reconstructed at 0.3.0** from the commit
> history and the phase audits in `docs/`, because no CHANGELOG entries or git
> tags existed for them — the gap this version exists partly to close
> (`roadmap.md` §1 rule 6). Version boundaries were placed at the natural seams
> in the history — the foundation reset, and the first engine-integrated phase.
> Retroactive tags at those commits are what make the boundaries verifiable
> rather than asserted; see `roadmap.md` §7.5.

---

## [Unreleased] — Runtime spine

Milestone M3, in progress: R1, T4, B8, T7, R7, F8, F9 and R3a are merged. The
runtime spine and the engine object model are designed in `engine-design.md`,
the CPU baseline M3's exit gate compares against is recorded, CI compares
Tier-1 results across Windows, Linux and macOS, generation no longer calls the
platform's math library, the API docs build without warnings, package-load
errors name each path once, and the world code no longer reads the editor's
settings.

### Added

- **Scripted benchmarks** (T4, #91) — `--bench <scenario>` runs a release build
  through a RON scenario in `bench/`: camera placements and pans, waits for the
  engine to settle, input actions and held movement, around one measured
  window. It records each frame's CPU work and writes the mean, p95 and maximum,
  process CPU per second, job means and every fill to
  `target/bench/<name>.json`, then exits. A run starts from a fresh world in
  `saves/bench` with the engine's default parameters and no mesh cache, keeps
  its window size, and ignores the keyboard and mouse. Debug builds are
  refused. `--runs <n>` repeats it, each run in its own process and
  `--cooldown <seconds>` apart (30 by default), and prints and writes each
  figure's median and spread. Four scenarios ship: S1 and S2 at rest at the
  launch and widest zooms, S3 a pan across both zones, S4 play with edits and a
  fluid pour.
- **The M3 CPU baseline** (T4, #91) in `perf-baseline.md`: five runs of each
  scenario on the discrete GPU and three on the integrated. Median frame CPU
  mean is 2.12, 3.01, 5.16 and 2.70 ms for S1 to S4.
- **The engine object model** (B8, #80) — `engine-design.md` §12 now names
  every primitive and asset type the engine has or plans, with each type's
  owner, side, override rule and milestone, and lists every registry as an
  extension point. For a game or package author it sets the rules ahead of the
  code: assets are named, never addressed by path; a package replaces only
  names from packages it depends on; a texture pack may restyle but never change
  behavior; and two package kinds, `Library` and `ResourcePack`, join `Game` and
  `Mod`. Design only: nothing loads differently yet.
- **Cross-platform determinism job** (T7, #94) — CI now checks that Tier-1
  results are bit-identical across machines, not only within one run. Windows,
  Linux and macOS runners each write `--determinism-manifest <file>`: a hash of
  every layer of the 512 chunks `--verify-generation` samples, taken over the
  persistence encoding, plus a fluid pour stepped 200 times and a run of edits
  ending in a blueprint stamp. The new `determinism-diff` tool compares two
  manifests and names the first differing chunk and layer. Windows against
  Linux fails the build; macOS reports. A run also fails if its sample holds no
  chunk with fluid, trees, structures, two biomes or caves, or if a scenario
  did not exercise what it is for. The first run was identical on all three.

### Changed

- **The toolchain is pinned** (T4, #91). `rust-toolchain.toml` names Rust
  1.92.0, and CI and release builds install it instead of the latest stable, so
  a baseline reading never moves with a compiler update.
- **Generation math is the same on every machine** (R7, #95) — tree skeletons
  and Poisson-disk placement take their transcendental functions from the
  `libm` crate instead of the platform's math library, and skeletons rotate
  with a scalar quaternion instead of glam's SIMD-backed `Quat`, whose SSE2 and
  NEON backends sum a dot product in different orders. A new CI step denies
  platform math, fused multiply-add and glam's SIMD types in `nodegraph-eval`
  (`crates/nodegraph-eval/clippy.toml`). The determinism manifest gains a
  `kernels` section that hashes skeletons and Poisson-disk point sets directly,
  because a difference smaller than a voxel changes no chunk hash. glam's `libm`
  feature is now on, which applies to every crate in a build that uses glam,
  games included. **The generation hash anchor is unchanged** at
  `0xed02babdf0f40420`, against the issue's expectation that it would move:
  skeleton nodes now differ in their last bits, far below a voxel, and no layer
  of the 512 sampled chunks changed, 131 of them tree-bearing. Saves need
  nothing.
- **The world code is decoupled from the editor and the renderer** (R3a, #17)
  — the first step toward the server-core crate. Chunk streaming budgets, the
  fluid limits and the autosave interval are now a `ServerConfig` the world
  code owns, with the same defaults, so a saved parameter preset's `streaming`
  block is ignored and its meshing budget, `max_mesh_per_frame`, is read from
  `meshing`. The panel's Terrain Generation sliders are gone: no generation code
  had read them since graphs took over, and a preset's `terrain_gen` block is
  ignored. Its Regenerate World button remains, under World Regeneration, and
  now says that regenerating clears saved edits. Regeneration no longer reads
  the editor's state, persistence no longer calls into the mesh cache (a call
  that deleted nothing), and no module under `world/` imports a client module
  apart from the streaming and regeneration couplings R4 and R6 remove. The
  generation hash is unchanged at `0xed02babdf0f40420`; saves need nothing.

### Fixed

- **The API docs build cleanly** (F8, #87) — `cargo doc --workspace --no-deps`
  no longer warns. `EffectivePin`'s doc linked `PinSpec` from a module where it
  isn't in scope, and three public docs in `nodegraph-eval`'s skeleton module
  linked the private `solve`. CI now builds the docs with `-D warnings`, so a
  broken or private doc link fails the build.
- **Package-load errors name each path once** (F9, #88) — a package that fails
  to load at startup printed its path twice, once from `run_with` and once from
  the loader. Each error now starts with the manifest it concerns, such as
  `error: …\packages\sandbox\package.ron: The system cannot find the file
  specified.`, and names any other path once: a missing world as the manifest
  writes it, a missing dependency by its directory. Manifest parse errors,
  which named only the package directory, now name `package.ron`.

---

## [0.5.0] — Engine/game boundary

Milestone M2. The engine has separated from the game that grew inside it. It is
now a library behind a facade crate, the world it ships and a testbed project
both load as packages, content names everything it references rather than
numbering it, and the prototype game's policy is package data in a repository of
its own.
**There is no save-format change**, so 0.4.2 saves open as they are, and the
generation hash is unchanged at `0xed02babdf0f40420`.

### Added

- **The `voxulacrum` facade crate** (B3, #9) — the only supported engine surface
  for games and the testbed. It exports `run`, `run_with` and `RunOptions`, and
  its crate docs set the rules every export follows.
- **`boundary-check`** (B3, #9) — a CI step that fails if any crate outside the
  engine depends on an engine crate other than the facade. The engine crates are
  listed under `[workspace.metadata.voxulacrum]` in the root `Cargo.toml`, and a
  crate left off the list is treated as a consumer. `--manifest-path` (B7, #13)
  checks a workspace outside the engine, such as a game's, against the list in
  the engine workspace its `voxulacrum` dependency belongs to.
- **The testbed** (B4, #10) — `crates/testbed`, a flat world with test materials
  that depends only on the facade. It is the boundary check's first consumer.
- **`--gpu low-power|high-performance`** (T2, #85) — chooses the GPU power
  preference the engine asks for; the default stays high performance. The
  chosen adapter is named in the startup log and on the Performance panel's
  first line. `perf-baseline.md` records the first integrated-GPU baseline with
  it: 61 fps at the launch zoom and 31 fps at the widest supported zoom, where
  the integrated GPU is the limit.
- **Packages** (B4, #10; B6a, #74; B6b, #75; B6c, #76) —
  `RunOptions::package(root)` starts the world of a data-only package: a
  directory with a `package.ron` manifest beside its content (graphs, materials,
  prefabs, libraries, blueprints, species, foliage). The manifest declares the
  package's id, its save directory under `saves/` (the id unless `saves` names
  another, so a second world never opens another's save), plus its content
  namespace, version, kind, the engine versions it supports (a Cargo-style
  requirement such as `^0.4`), its world and its dependencies. A package that
  needs a different engine, declares code modules or names no world stops
  startup with a message saying why. Dependencies load from `packages/<id>/`
  before the packages that need them, in dependency order and then by id. A
  dependency that is missing, the wrong version, part of a cycle, shares another
  package's namespace, or ships `materials.ron`, `prefabs.ron` or `foliage.ron`
  stops startup too. A qualified name reaches into any loaded package for
  libraries, blueprints and species; materials, prefabs and foliage come from
  the world's own package until saves carry a registry mapping (D3, M4).
  Shaders, palettes, input bindings and presets stay with the engine.
- **The sandbox package** (B6b, #75) — the world that shipped in `assets/` is
  now `packages/sandbox/`, a package like any other, with id and namespace
  `sandbox`. `run()` starts it through the same loader as any other package, and
  its manifest sets `saves: "default"` so existing saves keep opening.
- **The engine package** (B6c, #76) — a built-in package, `voxulacrum`, that
  declares one library per engine kernel, beside the kernels themselves. Every
  package loads after it, so content names `voxulacrum:standard_cave_noise`
  instead of shipping its own copy of the declaration. Its id and namespace are
  reserved, and a package may depend on it to state the engine versions it
  supports.
- **Namespaced registry identity** (B5a, #66) — every registry entry is
  `namespace:name`. A package's namespace is the one its manifest declares, or
  its id: the sandbox world's is `sandbox` (B6b), and `voxulacrum` is the engine
  package's (B6c). A bare name in content resolves in its own namespace.
- **`foliage.ron`** (B5d, #69) — detail layers with the species in each, and
  scatter types. Species colors reach the shader as data instead of a hardcoded
  switch in `detail_paint.wgsl`.
- **A content-declared prefab `placeholder`** (B5c, #68) — the prefab drawn for
  an instance whose id the table doesn't define. It replaces prefab 0 being
  chosen in engine code.
- **`play.ron`** (B7, #13) — a package's Play mode: the player's body, movement
  and reach (`player`), where entering Play puts the player (`spawn`, today only
  `SurfaceUnderCamera`), the break and place tool with the materials it cycles
  through (`build`), and whether the selected material shows on screen (`hud`).
  Build materials are names, resolved when the package loads. A package without
  `play.ron` has no Play mode; a malformed one, or a build material that doesn't
  resolve, stops startup.
- **The game repository** (B7, #13) —
  [voxulacrum-sandbox-game](https://github.com/BananaPix4613/voxulacrum-sandbox-game)
  holds Voxbox, a prototype game that starts as a copy of the sandbox world. It
  builds on the facade alone, through a git dependency or a local `[patch]`, and
  its CI runs `boundary-check --manifest-path`.

### Changed

- **`voxulacrum-app` is a library with a thin binary** (B1, #7). The package is
  renamed from `voxulacrum` to `voxulacrum-app`; the binary is still
  `voxulacrum-app`, so release artifacts keep their names.
- **`ecs/systems.rs` is split into 16 subsystem modules** (B2, #8). The schedule
  registers the same systems in the same order.
- **Graphs name what they reference** (B5b–B5d, #67–#69). Materials (`Layer`,
  `ConstantMaterial`, `FindFlat`, `SurfaceFilter`), libraries (`LibraryRef`),
  prefabs (`ScatterPlace`, canopies), detail layers and species (`PaintDensity`)
  and scatter types are names, resolved to ids when content loads. A name that
  doesn't resolve fails the load with an error naming it, and
  `--validate-graphs` reports it. The graph editor picks each of these from its
  registry instead of editing raw numbers. Numeric ids still come from the
  authored `id` fields; moving them into the save is D3 (M4).
- **No engine code chooses content.** New `ScatterPlace`, `PaintDensity` and
  canopy nodes name nothing until the author picks. A new detail graph takes the
  content's first layer and species. `PlaceScatter` carries the prefab its
  caller chose.
- **Content names follow one rule**, `a-z`, `0-9`, `_` and `-` (B5a, #66). It
  applies to every registry entry, and also to blueprint capture names, which
  previously accepted uppercase letters that no graph could reference.
- **Debug and fallback colors.** The material debug view derives a distinct
  false color from each id instead of a table naming the sandbox world's
  materials (B5b). A detail species the content doesn't define draws magenta
  instead of green (B5d).
- **`packages/sandbox/graphs/example.graph.json` is regenerated from its builder**
  (B5b). It had fallen far enough behind that it no longer parsed. Tests now pin 
  it to the builder and to the shipped materials.
- **Content moved out of `assets/`** (B6b, #75). Graphs, materials, prefabs,
  libraries, blueprints, species and foliage now live in `packages/sandbox/`.
  `assets/` keeps the engine's input bindings and the content license. Release
  archives ship `packages/` beside the executable, and one started without
  `packages/sandbox/package.ron` stops with an error instead of running on the
  tables compiled into the binary.
- **Libraries come from the engine package** (B6c, #76). The sandbox's five
  `.library.json` files, the testbed's copy of `poisson_placement` and the
  hand-written fallback set are gone. A package's `libraries/` is optional; its
  entries load after the engine's, with ids assigned in load order.
- **No game constants in the engine** (B7, #13). The player's size, gravity,
  speed, fall speed, step height, eye height and reach, the spawn rule, and the
  build tool's materials come from `play.ron`. The sandbox and the testbed ship
  the values the engine used to hardcode, so both play as before. The avatar is
  drawn at the tuned body size, and the enclosed-space check measures from the
  tuned eye height. The build tool cycles through the package's list instead of
  every material after air.

### Fixed

- **The material registry's validation tests were passing on a RON parse
  error** (B5a, #66). Their fixtures wrote colors as `[..]` where RON needs
  `(..)`, so none of them reached the check it was named for. They now assert
  the validation error itself.

### Migration

- **Commands:** `cargo run -p voxulacrum …` becomes `cargo run -p voxulacrum-app …`
  (B1).
- **Content outside this repository** (none ships with the engine) loads as a
  package with a `package.ron`, and must name what it references:
- Graph files write names where they wrote ids, and three keys change:
  `prefab_id` → `prefab`, `layer_id` → `layer` and `type_id` → `scatter_type`.
  A number in any of these places fails to load.
- `prefabs.ron` requires a top-level `placeholder: "<name>"`.
- A package needs a `foliage.ron`. Without one, the engine logs an error and
  falls back to the sandbox package's table.
- **Paths** (B6b): content that lived under `assets/` (`graphs/`, `libraries/`,
  `blueprints/`, `species/`, `materials.ron`, `prefabs.ron`, `foliage.ron`) is
  now under `packages/sandbox/`, so uncommitted local edits to it need moving
  across. `regen_example` and `watch_demo` default to `packages/sandbox/graphs`.
  A qualified name that reached the shipped world as `voxulacrum:<name>` is now
  `sandbox:<name>`.
- **Libraries** (B6c): a package no longer declares the engine's libraries.
  Delete copies of the standard `.library.json` files, and write `LibraryRef`
  names as `voxulacrum:<name>`. A bare name resolves in the package's own
  namespace, so it fails unless the package declares that library itself. A
  package can't use `voxulacrum` as its id or namespace. A dependency's
  blueprint resolves its bare material names in its own namespace, so until
  dependencies can ship materials (D3), it qualifies them.
- **Play mode** (B7): a package needs a `play.ron` for Play mode. Copy the
  sandbox's as a starting point, and list the materials the build tool should
  place under `build`.

---

## [0.4.2] — Clean slate follow-up

One more way a world could lose saved edits, found while closing M1. No
save-format change, and the generation hash is unchanged.

### Fixed

- **A chunk whose saved record failed to load no longer overwrites it** (F7,
  #60). When a record could not be decoded (a corrupt blob, an unknown
  compression flag, a SQLite error), the chunk came back looking untouched, and
  its next save, or water flowing into it, replaced the unreadable row with a
  small delta and lost every edit the row held. This is what turned F2 from a
  failed load into lost edits. Such a chunk is now blocked from saving while it
  stays loaded: it remains playable, the record is left exactly as it was, a
  warning names its coordinates, and the Performance panel counts blocked
  chunks. Edits made in a blocked chunk are not saved. Unloading and reloading
  the chunk tries the load again. No save-format change.

---

## [0.4.1] — Clean slate

Milestone M1. Fixes only: three ways a world could lose data or water, one
material table instead of three, and four small cleanups. Every fix to saving or
fluids came with a regression test that failed before the fix. **There is no
save-format change**, so 0.4.0 saves open as they are, and the generation hash
is unchanged at `0xed02babdf0f40420`. The close-of-milestone audit is
`docs/m1-audit.md`.

### Fixed

- **Edits were lost after a chunk was promoted to a full save** (F1, #1). Past
  8,192 voxel edits a chunk saves whole, but its edit state was an `Option` whose
  `None` meant both "never edited" and "promoted", so the next edit, or water
  flowing in, started a near-empty delta that replaced the full record on disk. A
  chunk reloaded from a full record came back the same way, so the first edit
  after a reload did the same. The edit state is now an explicit `EditState`
  (untouched, delta, promoted) and a promoted chunk keeps saving full. As before,
  fluid and scatter edits on a promoted chunk are not persisted: a full record
  has no slot for them, and adding one is a format change (M4). No save-format
  change; 0.4.0 saves open as they are.
- The determinism checker no longer reports promoted chunks as diverged. They are
  skipped with the other edited chunks.
- Scatter removed or placed on a chunk now stays that way for the session after
  the chunk is promoted. Promotion used to discard it.
- **Streaming loads failed in the session where a world first passed 200 saved
  chunks** (F2, #2). Autosave trained the compression dictionary mid-session and
  later rows were written with it, but the streaming workers capture the
  dictionary once, when they are built, so loads of those rows failed with
  `flags=3 but no dictionary`. The chunk then loaded without its edits and its
  next save overwrote the row. The dictionary is now trained only when a world
  opens, before the workers exist. A world that crosses 200 chunks mid-session
  keeps writing plain zstd rows, fully readable, until its next open. No
  save-format change.
- **Fluid mass was created or lost three ways** (F3, #3). A slab placed into a
  full cell left it holding more than the slab can, and the simulation
  subtracted that from the capacity as `u16`: a panic in dev builds, a wrapped
  value in release. A cell that drained in a chunk wholly under the sea was
  removed, and an absent cell there reads back as full, so draining created
  water. At chunk seams each side capped its half of a transfer differently, so
  water appeared or vanished crossing a border. Cells are now clamped to their
  capacity when an edit lands and whenever they are simulated, a drained
  submerged cell is stored explicitly empty, and both sides of a seam apply the
  same amount, computed from the same snapshot. Cells on a chunk edge or corner
  move water across a seam more slowly as a result. No save-format change.
- **`R` was bound to two actions** (F6, #6). `PrimaryAction` (break a voxel in
  Play mode) and `RotateTool` (rotate an armed blueprint stamp) were both on
  `R`, so one press did both. `R` keeps rotating, as the stamp HUD says;
  `PrimaryAction` moves to `V`. An `assets/input.ron` of your own keeps whatever
  it says.
- **The presets directory followed the working directory.** `params/` was a
  relative path, so one install saw different presets depending on how it was
  launched. It now resolves through the asset root, like shaders and input
  bindings. Presets saved from another working directory need moving.
- **`MeshCacheParams.enabled` was never read**, so a preset asking for no mesh
  cache still got one. It is now honored: when false, meshing neither reads nor
  writes the cache. It is read once, at startup.

### Changed

- **One material table** (F5, #5). The table existed three times:
  `assets/materials.ron`, a built-in copy in `voxel-core`, and a third in the
  engine's UI parameters, which the play HUD, tool cycling and the terrain color
  table read. `assets/materials.ron` is now the only source. The built-in copy is
  gone; a copy of the file itself is compiled into the binary and used only if
  the file is missing or invalid at startup. The HUD, tool cycling and the color
  table read the registry, and rendered colors are unchanged. Two things go with
  the old copy: the Materials panel is a read-only listing (its color,
  sharpness, permeable and flora edits are gone; only color was ever read), and a
  saved preset's `materials` block is ignored on load. No save-format change.
- **Dropped the unused `fastnoise2` dependency** (F6, #6). Nothing referenced
  it, but it pulled in `bindgen`, `cmake` and libclang to build. The workspace
  and the release workflow no longer need them. The pure-Rust `fastnoise-lite`
  is unaffected.
- `THIRD-PARTY-NOTICES.md` regenerated: the crates `fastnoise2` pulled in are
  gone, and the workspace crates are listed at their current version under
  MPL-2.0.

### Known limits carried forward

- **Performance baselines were not recorded for this release.** Frame timing is
  currently too inconsistent for a comparison to mean anything, and F3 changed
  how much work the fluid simulation does per tick. Both are measured together
  later.
- **A chunk whose saved record fails to load comes back looking untouched**, and
  its next save overwrites the unreadable record. This is what made F2 lose
  edits rather than just fail a load. Tracked for a follow-up patch.
- **Voxel edits do not wake nearby water.** Digging beside a pond does not make
  it flow, and a drained cell in a chunk under the sea is never refilled.

---

## [0.4.0] — Worldgen completeness & authoring

Phase 11. The theme was making worldgen content authorable in-engine and making
generation complete across chunk borders: structures that span chunks, worlds
with more than one zone, and blueprints captured from the world rather than
written by hand. Every new piece of content in this version was authored through
the tools.

0.4.0 also carried the first tree system, which grew past the theme. That drift
is why this is the last version cut from a long-lived version branch; from here
versions are cut on `master` when a milestone completes (`roadmap.md` v2.0).

**Saves from 0.3.0 are wiped on first open** (voxel format 3 → 4, chunk blob
v7 → v8). The generation hash anchor is now `0xed02babdf0f40420`, recorded in
`docs/pre-phase-11-audit.md`.

### Added

- **Blueprints** — a format in `voxel-core` that names materials rather than
  numbering them, with a blueprint-local shape vocabulary and a version field
  from the first byte. Replaces `PrefabTemplate`, which is deleted.
- **In-engine capture and stamping** through the mutation door as one command,
  with an armed-tool model (a button arms, the next world click acts, Esc
  cancels, Shift repeats), per-axis trim, quarter-turn rotation and an isometric
  preview.
- **Cross-chunk structures** — features are derived world-absolutely over a
  margin band and each chunk stamps only its own window, so placement does not
  depend on load order. Order independence and the seam property are asserted
  in CI.
- **Multi-zone worlds** — manifest schema v1 with a zones list and the seed on
  the manifest; per-column zone selection.
- **Trees** — skeletons as a pure function of seed, species and age; species
  authored as `assets/species/*.species.json` and hot-reloaded; `PlaceTree` as a
  zone declaration that rides the cross-chunk feature model. Wood is voxels in
  the grid, drawn as analytic capsule impostors with true depth, shadows and
  water reflections. Skeleton smoothing, and a skeleton debug view with live
  species tuning, a species picker and a solid preview.
- **Canopy cores** — canopy lobes carried out of generation and drawn as
  ellipsoid cores, behind a debug toggle that is off by default.
- **`WorldParam`** and a world-level parameter sidecar; the manifest declares
  the world's `min_y` and `max_y`.
- **`SurfaceToDensity`**, the one bridge from the column domain into a density
  chain; arithmetic and curve nodes now evaluate per column.
- **Authoring and inspection tools** — the hierarchy editor creates, renames and
  reseeds zones and detail graphs; `LibraryRef` and `GraphRef` are menu-listed
  and bound at insert; column inspector; biome and zone map; a world event bus
  with eight typed events.
- **`--validate-graphs`** — manifest resolution, graph kind against manifest
  position, reference and material resolution, and assignment coverage. Runs
  in CI.
- **Release workflow** packaging Windows and Linux builds.
- **Licensing** — the engine under MPL-2.0 and sample content under CC BY 4.0,
  with a README, contributing guide and third-party notices.
- Pointwise evaluation benchmark (`benches/pointwise.rs`).

### Changed

- **Graph inputs resolve through an edge index** instead of a linear scan:
  −58 % at 129 edges, with a small regression below about 25 edges that is
  negligible against a generate job.
- `--verify-generation` samples 512 chunks rather than 64 and also checks order
  independence against a fresh generator.
- Every per-column evaluation goes through `WorldEvaluator::column_eval`, and
  the upstream evaluator for cross-graph reads is built once at insert rather
  than per sampled column.
- `Graph::validate` rejects a non-column node kind in a World or Zone graph.
- The frontier budget is instrumented to twelve spans; chunk unload is
  time-budgeted rather than count-limited.
- Materials can be render-delegated; the outline pass treats wood as its own
  surface class.
- CI's stale branch filter removed. rust-analyzer uses its own target directory,
  so it no longer blocks `cargo build`.

### Fixed

- **Pointwise and bulk cross-graph reads disagreed** whenever a graph exposed
  more than one boundary output (418 of 1,024 columns in the test case): a seam
  defect waiting for content to trigger it.
- **A `GraphRef` in a biome or detail graph** rendered, wired and saved in the
  editor, then failed its chunk at generation.
- Column-domain arithmetic (`SurfaceNoise → Add`, `Constant → WorldOutput`) was
  rejected as a type mismatch.
- Two hard-coded vertical extents that agreed only by coincidence are replaced
  by the manifest's.
- Capsule impostors discarded branches pointing at the camera, and returned the
  back surface whenever a branch pointed toward or away from it.
- The tree stamping test failed on joint spheres by one float rounding step.

### Exit gates

Theme met. Unmet gates are recorded rather than holding the version open (the
closing record is in `roadmap.md` v2.0 §11):

- **Rivers** — the network design and `River` node exist, but the first content
  pass reads as ravines and no shipped graph uses it. River content moves to the
  game backlog.
- **Asset lint** does not exist yet; it moves to milestone M5.
- **D8** (greedy meshing) is superseded by the decision to adopt binary greedy
  meshing with 0.5 m voxels.

### Known defects carried forward

To be fixed in 0.4.1 (milestone M1):

- Edits made after a chunk is promoted to a full save are lost on reload (#1).
- In the session where a world first passes 200 saved chunks, the compression
  dictionary is trained mid-session and streaming loads of edited chunks fail
  (#2).
- Fluid capacity can underflow when a block is placed into water, and fluid
  mass is not conserved across chunk seams (#3).

---

## [0.3.0] — Consolidation & Observability

Phase 10. **No new player-visible features.** The theme was closing the gap
between what the documents claimed and what the code did, making the engine
explain itself, and fixing the release process.

The headline: the version was gated on diagnosing a "~28 ms at-rest frame hitch
recurring several times per second" that had been the highest-priority open
defect since 0.2.0. **It did not exist.** It was a debug build (7× slower; the
workspace had no `[profile.dev]` at all) measured with a wall-clock metric that
*inverts* under vsync — a faster engine waits longer in `get_current_texture()`.
Release holds a locked 60 fps, and at the worst supported zoom the engine uses
5.5 ms of its 16.6 ms CPU budget.

### Added

- **`FrameTimings`** — per-`FrameStage` CPU timing with the swapchain present
  block isolated, so the frame budget is stated against work rather than
  blocking. Surfaced live and logged once per second.
- **Unified job system** (pillar P14) — one scheduler owning dispatch and
  concurrency for all continuous background work. Priority is
  `(class, distance²-to-camera)`; an interactive class lets an edit-driven
  re-mesh preempt bulk streaming.
- **Six further diagnostics** completing roadmap §4.4's Category C: job system
  inspector, streaming visualizer with a fill-time metric against the §7.3
  budget, memory/residency panel with session-growth tracking, mesh cache
  hit/miss with cold-vs-stale key attribution, mutation and event log, and an
  in-place determinism checker that regenerates resident chunks and surfaces the
  first divergent voxel.
- **Headless generation CLI** — `--verify-generation [N] [--seed S]` generates
  chunks twice in parallel and requires bit-identical output, wired into CI.
- **CI** — build, test and determinism verification on every push. The repository
  had none.
- **`[profile.dev]`** optimization levels, so development builds are usable for
  observation. Performance measurement remains release-only.
- Determinism tests for a **slab-bearing** chunk (guarding against a vacuous
  test) and for scatter seed derivation, pinned from both directions.
- Perf baseline document (`docs/perf-baseline.md`) and branch/tag discipline
  (`roadmap.md` §7.5).

### Changed

- **Concurrency is one global budget** dispatched by priority, replacing a static
  per-kind split that capped generation at a quarter of the pool while the rest
  sat reserved for meshing that was usually idle. `CoreBudget` reduces to a
  single `pool_threads`.
- **`StorageBoundary` is the single eval→storage crossing** for all three
  generated layers — terrain, fluid and foliage — closing drift 2.3. It carries
  the world's sea level.
- **Mutation door** gained `FinalizeSeam`, `EvictChunk` and `CommitFluidPlans`
  (which subsumes and removes `MarkFluidDirty`). `MutationOrigin::System` is now
  a per-intent whitelist rather than a universal exemption from the mode gate.
- **Mesh cache key** excludes the snapshot's edge and corner border cells, which
  are load-order dependent and which the mesher never reads.
- **Streaming view test is altitude-aware.** Under an isometric projection an
  altitude `Δy` displaces geometry up-screen by `Δy·√2` of ground-equivalent
  distance, so the test compares a column's projected span rather than its
  footprint.
- `half_extents` derives the away-axis from `zoom / sin(pitch)` rather than
  `zoom * aspect`, agreeing with `Camera::visible_radius`.
- `max_mesh_per_frame` is wired to the meshing snapshot budget; it had been
  declared and never read.
- **Worldgen hot-reload is save-triggered.** A graph file changing on disk is now
  the *only* trigger, so an editor Save and an external text-editor change take
  the identical path. Regeneration always reloads the whole hierarchy from disk,
  which makes a mix of edited and on-disk state unrepresentable rather than
  merely avoided. Editing no longer regenerates on every keystroke — which also
  removes a pathology where dragging a slider regenerated the world continuously.
  The authoring loop becomes edit → Save → see result; P11's bar is that an
  author needn't restart the engine, hand-edit a file, or guess at a result, and
  a save keystroke is none of those.
- CHANGELOG moved to the repository root.

### Fixed

- **Startup-fill chunks silently discarded persisted player edits.** Chunks
  within the boot radius regenerated without their override overlay, and the next
  edit in one wrote a record that had forgotten every earlier edit — data loss on
  disk, not just on screen. Both loaders now share one override-application path,
  and persistence opens before world generation.
- **Chunks meshed in scattered order**, with some filling in only after
  everything else. Meshing had no priority at all: its queue was filled in
  `HashMap` order, `swap_remove` scrambled it further, and chunks failing the
  face-neighbour check accumulated while later arrivals jumped ahead.
- **Tall terrain at the screen edge never loaded**, because the view test ignored
  altitude entirely.
- **Mesh cache re-key churn** — 1,400 of 4,758 lookups on a warm second run were
  rebuilds of meshes already on disk, each also deleting and rewriting a file.
  Warm-run hit rate went from 66 % to 100 %.
- **Graph hot-reload mixed in-editor and on-disk state.** Regeneration read the
  edited graph from memory and *every other graph from disk*, so editing one
  graph while another had unsaved changes produced a world matching neither.
  Higher-tier graphs suffered worst: a Zone graph's `GraphRef(World)` resolved
  against the on-disk World, so unsaved World edits were silently dropped. A
  second path — a filesystem watcher hardcoded to biome 0 — could regenerate the
  same content twice and refresh a canvas the author wasn't looking at. This is
  the defect 0.2.0 shipped with.
- **Zone-graph edits regenerated too few chunks.** Invalidation matches against
  `ChunkTags`, which describe the *previous* generation — sound for a Biome edit
  (which changes how an assigned biome looks) but not for a Zone edit (which
  changes which biome a column *is*). Moving a biome band boundary left every
  reassigned chunk untouched, since each was still tagged with the biome it was
  leaving. Zone edits now invalidate broadly; the constraint is recorded against
  design §4's invalidation table.
- **The editor's `● modified` marker did not survive switching graphs**, so
  returning to an edited graph showed the edits with Save greyed out and no way
  to commit them. Unsaved state is now tracked per slot and marked in the
  selector.
- Redundant face-neighbour marking on every chunk insert (the handler already did
  it); a doc-comment/behaviour mismatch in the streaming margin formula.

### Verified closed, not changed

Drift findings 1.4 (pipeline order), 2.1 (PoissonDisk seeding) and 2.2 (mesh
worker threading) were assigned to this version by the roadmap but had already
been fixed in 0.2.0's Phase 8 — the roadmap had been written against the drift
review rather than the audit that superseded it. Re-derived against the tree and
recorded.

### Known limits carried forward

Frame cost scales with *visible* chunk count through draw-call recording — three
terrain passes at ~5 µs per chunk draw. Outside the supported zoom range this
exceeds budget (31.4 ms at radius 25). ~490 MB of resident GPU mesh buffers at
that radius, ≈3.4M quads, is the first measured input to decision D8 (greedy
meshing, due at 0.4.0 exit). Both are downstream of naive one-quad-per-face
meshing, so they are one decision.

---

## [0.2.0] — Node-graph worldgen, foliage, fluid, player

Phases 1–9, 2026-06-10 through 2026-07-24. Announced but never version-stamped;
the workspace remained at `0.1.1`.

> **The stated core shipped complete** — infinite streamed world, the five-graph
> hierarchy, and multi-biome terrain rendering visibly distinct biomes.
>
> What went wrong was late and narrow: an eleventh-hour attempt to let an author
> switch the biomes used for world generation **from inside the engine**, by
> editing the Zone graph, rather than editing files externally and restarting.
> It was implemented too hastily to be tested properly, and shipped with an
> undiagnosed defect in terrain hot-reload and graph saving — see 0.3.0, where it
> is fixed.

### Added

- **Node-graph world generation.** A five-graph hierarchy — World (climate, zone
  assignment) → Zone (biome assignment) → Biome (density, material) → Detail
  (foliage), plus Library — assembled from a manifest and composited per chunk.
  Typed boundary ports, cross-graph dataflow (`GraphRef`/`GraphOutput`), per-biome
  parameter sidecars, density-blended biome borders, and tag-driven invalidation
  so a graph edit regenerates only the chunks it affects. The editor became a
  first-class panel inside the engine with a graph selector.
- **Chunk data model.** `ChunkCoord`/`LocalPos`/`FaceAxis` primitives; the runtime
  chunk split into a pure data `Chunk` and a `LoadedChunk` wrapper; typed sidecar
  layers (detail, scatter, fluid, decal, lighting); `ChunkTags`; the canonical
  `ChunkOverrides` diff set replacing the former edit list; a multi-layer save
  blob.
- **Slab smoothing** with a cross-chunk seam finalization pass, persisted so
  reloads neither re-derive nor revert it.
- **Foliage**, three tiers: GPU-generated grass blades from a per-chunk density
  buffer with wind sway and tint; instanced prefab scatter grouped per prefab; a
  data-driven prefab registry; anchor-based placement and removal.
- **Fluid** — ocean fill from a global sea level, cellular-automata flow with
  two-phase plan/commit, cross-chunk mass conservation, and water rendering with
  reflection, refraction and depth colouring.
- **Mutation command API** — a single door for every world write, declaring
  origin (authoring / play) and validated against the engine mode, with handlers
  owning persistence marking, mesh invalidation and override bookkeeping.
- **Player character** — semantic action layer with rebindable input, pure
  fixed-timestep simulation, slab-aware collision with auto-step, interpolated
  camera follow, capsule avatar with drop shadow and a two-pass stencil
  silhouette, reach-limited break/place, and a material HUD.
- **Underground visibility** — an air-side face rule with a cost-priority march
  and clarity radius, replacing an earlier clip-box cutaway that leaked the
  surface world under an angled camera.
- Tri-tonal axis lighting; the 32-byte packed `FaceVertex` format; a mesh disk
  cache; RON-driven material and prefab registries; SQLite + zstd persistence.
- Simulated climate driving cloud shadows.

### Removed

- The `voxulacrum-preview` standalone crate, its unique visualization (colormap
  and field probe) ported into the engine first.
- The resident walkability mask, after it shipped and produced a chunk-Y seam
  defect, a streaming regression and a poor fit for continuous collision.
  Movement, collision and room detection query geometry directly.
- The param-based `TerrainGenerator`, once the meadow graph reproduced its output.

### Fixed

- Water crossing a chunk boundary into an unedited chunk vanished on unload.
- Foliage generated before fluid and underwater.
- `PoissonDisk` seeded from chunk coordinates rather than world-absolute cells.
- Mesh workers ran on raw `std::thread` outside the shared pool, with
  hand-duplicated core-budget arithmetic.
- Per-voxel edits cloned an entire chunk's storage.

---

## [0.1.1] — Foundation reset

2026-05-26 through 2026-06-02. The prototype was peeled back to a cube-only voxel
world and rebuilt on a crate boundary.

### Added

- `voxel-core` — voxel value type, material identity, shared errors. No I/O, no
  rendering.
- `nodegraph-ir`, `nodegraph-eval`, `nodegraph-hotreload` — graph data model,
  per-chunk evaluator, filesystem watcher.
- A node vocabulary and a visual graph editor with 2D heatmap and 3D preview.
- Scatter point, scanner and prop nodes; a shape-aware mesher.

### Changed

- The voxel model unified on `voxel_core::Voxel` — a small shape vocabulary with
  no rotation field, packed into a `u32`. Chunk storage holds packed voxels
  behind a palette rather than bare material ids.
- Save blob format `BLOB_VERSION` 3 → 4; world disk-cache format 4 → 5.

### Migration

- **One-shot save wipe, intentional and pre-release.** Saved chunks under
  `saves/<world>/` and the mesh cache under `cache/meshes/` are cleared when the
  stored `voxel_format_version` is older than the current. Terrain regenerates
  from seed; no forward migration.

---

## [0.1.0] — Prototype

2026-02-09 through 2026-03-27. The original single-crate engine, superseded by
the 0.1.1 reset.

Isometric camera and window; voxel chunks and terrain generation; a dual
contouring mesher later replaced by marching cubes; lighting, shadows and a
day-night cycle; wind-driven instanced vegetation; water and lakes with
transparency; post-processing, a render-target upscale pass keyed to world pixel
density, Oklab colour quantization, pixel-perfect edge highlighting and camera
snapping; shader hot-reloading; asynchronous multithreaded meshing; frustum
culling; mesh and world caching; camera rotation and cross-section controls;
`bevy_ecs` and a render graph; an input abstraction layer; world streaming with a
priority queue; sparse storage and persistence.
