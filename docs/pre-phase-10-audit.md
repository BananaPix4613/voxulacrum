# Pre-Phase-10 State Audit — Consolidation & Observability (serves 0.3.0)

**Date:** 2026-07-27
**Branch:** `mc-revision`, HEAD `b258ad7`, **working tree dirty** (see §1.2)
**Companions:** `roadmap.md` §10 (scope contract), `engine-design.md` v1.8 (uncommitted revision in tree), `architecture-drift-review.md` (2026-07-10), `post-phase-8-audit.md`, `post-phase-9-audit.md`.

Substep 0. No code. Every line number below was re-derived against the current
working tree, not carried from the drift review. Where I state something is
closed, the evidence is a citation, not the audit that claimed it.

---

## 0. Headline findings — read these before the ordering

Six things materially change the plan the roadmap sketches.

**0.1 — Three of the four drift findings §10 assigns to this version are already closed.**
Drift 1.4 (pipeline order), 2.1 (PoissonDisk seeding), and 2.2 (mesh worker
threading model + duplicated core budget) were all fixed in Phase 8, commit
`757d5b6` (2026-07-13), three days *after* the drift review was written
(2026-07-10). The roadmap author was reading the drift review, not
`post-phase-8-audit.md`. Verified in code, §5 and §6 below. **Only 2.3 (eval→storage
boundary) is genuinely open.** This removes roughly a third of the nominal
workload and frees it for the parts that are real.

**0.2 — The hitch investigation's ground has shifted underneath it.**
`post-phase-9-audit.md` §3 concluded "every unconditional per-frame system found by
reading the schedule was checked." That was written against `216a449` (2026-07-20).
Two substantial commits landed after it: `3e325ef` (hydrology/climate, 2026-07-23)
and `b258ad7` (water reflection/refraction, 2026-07-24). Both add unconditional
per-frame main-thread and GPU work. The "everything was checked" conclusion does
**not** cover the current tree. §4.2.

**0.3 — Present mode is `PresentMode::Fifo` (`main.rs:188`), and `frame_time_ms` is wall-clock inter-frame time.**
`SimulationManager::tick` measures `dt` from the previous frame's simulation start
(`simulation/manager.rs:76–78`), and `compute_stats_system` reports it verbatim
(`ecs/systems.rs:1086`). Under vsync that number quantizes to refresh intervals.
A reported ~28 ms frame is therefore **one missed vsync deadline**, not 28 ms of
work — the engine needed roughly 10–12 ms more than the 16.6 ms budget on that
frame, not 28. This reframes workstream B from "find the 28 ms thing" to "find
what occasionally adds ~10 ms," which is a much wider and more plausible
candidate set. It also constrains the `FrameTimings` design: if it measures
wall-clock spans that include the present block, the vsync wait will dominate
every reading and hide exactly what we are hunting. §4.1.

**0.4 — The working tree carries ~1,140 lines of uncommitted 0.4.0-scope work.**
Biome authoring in the hierarchy editor, manifest read/write, a `new_biome_graph`
template, an untracked `assets/graphs/biome_test.graph.json`, plus water/outline
shader edits and the v1.6–v1.8 `engine-design.md` revision. This is
worldgen-authoring — explicitly a 0.4.0 deliverable and a 0.3.0 non-goal. It also
introduces the tree's only two compiler warnings. Starting Phase 10 on top of it
means every BEFORE block I write is anchored against unreviewed, uncommitted
code. **This needs a decision before Substep 1.** §1.2, §14.

**0.5 — Two §10 deliverables cannot be met as literally worded.**
- *"Poisson scatter continuity across chunk borders"* (determinism coverage, D):
  `PoissonDisk` uses a per-chunk Bridson walk. Its **seed** is world-absolute
  (that was the 2.1 fix), but the point set is not and cannot be seam-continuous
  without a world-tiled Bridson, which is a redesign, not a test. `scatter.rs:6–9`
  and `context.rs:54–59` both say so in-source. A test asserting continuity would
  fail by design. §5.3.
- *"One job system; no second threading model anywhere in the tree"* (exit gate):
  already true. `grep` finds no `std::thread` chunk work in the app; everything
  runs on one shared `rayon::ThreadPool` (`main.rs:406`). §6.1.

**0.6 — There is no CI, at all.**
No `.github/`, no workflow files, no runner config. The exit gate "Determinism CI
job green" and the deliverable "headless generation CLI in CI" therefore include
*building CI from nothing*, which the roadmap does not acknowledge. §8.3.

---

## 1. Tree state and baseline

### 1.1 Versions and build

| Fact | Value | Where |
|---|---|---|
| Workspace version | `0.1.1` | `Cargo.toml` `[workspace.package]` |
| Announced version | 0.2.0 | roadmap §1 rule 6 |
| `CHANGELOG.md` location | `docs/CHANGELOG.md` (not repo root) | — |
| `CHANGELOG.md` content | `## [Unreleased] — Phase 0`, one entry | 16 lines total |
| `cargo build --workspace` | succeeds, **2 warnings** | both from uncommitted work |
| `cargo test --workspace` | **273 passed, 0 failed, 1 ignored** | — |
| CI | none | no `.github/` |

The two warnings are unused imports introduced by the uncommitted biome-authoring
work (`world_generator.rs:19` `SubtractParams`, `:22` `PinType::Density`). Phases 7
and 8 both held a zero-warning bar; the tree currently does not.

`docs/CHANGELOG.md` also contradicts roadmap §7.4, which speaks of "CHANGELOG entry
written" without saying where the file lives. The rebuild (F2) should decide root
vs. `docs/` explicitly rather than inheriting the accident.

### 1.2 Uncommitted work inventory

```
 assets/graphs/biome_meadow.graph.json              | 470 +++--    graph content
 assets/graphs/world.manifest.json                  |  24 +-
 crates/nodegraph-editor/{params,state,style,viewer} | 237 +-      editor UI
 crates/nodegraph-eval/{border,column_eval,world_eval}|  40 +-     ZoneOutputParams field
 crates/nodegraph-ir/src/node.rs                    |  18 +-
 crates/voxulacrum-app/src/ui/hierarchy_editor.rs   | 221 +++      biome create/delete UI
 crates/voxulacrum-app/src/world/world_generator.rs | 105 ++-      manifest write, new_biome_graph
 crates/voxulacrum-app/src/{main,params,paths,ui/panels} |  62 +-
 crates/voxulacrum-app/src/rendering/{pipelines,reflection_pass,water_pass} | 26 +-
 shaders/{outline,player,water}.wgsl                |  12 +-
 docs/engine-design.md                              | 151 +++--    v1.6/1.7/1.8 revisions
?? assets/graphs/biome_test.graph.json
?? docs/roadmap.md                                  (untracked — the scope contract itself)
?? docs/phase-10-agent-prompt.md
?? .cargo/
```

Two observations. First, `docs/roadmap.md` — the document that is authoritative for
sequencing — is itself untracked. Second, the `engine-design.md` diff is the
v1.6→v1.8 revision that `post-phase-9-audit.md` and the roadmap both cite as
existing; it has never been committed. Roadmap §7.4 requires the design-doc revision
as part of a release; the 0.2.0 one is sitting unstaged.

### 1.3 What the app looks like at runtime, structurally

One `winit` loop, one `wgpu` device, `bevy_ecs` schedule of six `FrameStage`s
(`ecs/schedule.rs:9–22`), 28 systems. One shared `rayon::ThreadPool` named
`chunk-gen-N` sized by `CoreBudget::detect().pool_threads` (`main.rs:406–412`),
consumed by: startup fill (`world/mod.rs:75`), streaming generation
(`streaming.rs:267`), meshing (`meshing/mod.rs:227`), background regen
(`regen.rs:24`), and the field probe (`ui/field_probe.rs:201`).

---

## 2. Workstream A — Observability: what exists today

Roadmap §4.4 lists seven Category-C tools for 0.3.0. Current state:

| Tool | State | Evidence |
|---|---|---|
| `FrameTimings` per-`FrameStage` + panel | **Nothing.** Panel shows FPS, frame ms, triangles, chunks, culled %, streaming counts — six lines total | `ui/panels.rs:791–803` |
| Job system inspector | **Nothing.** No job system to inspect (§6) | — |
| Streaming visualizer | **Fragments.** `ui.streaming_loaded` / `streaming_pending` are two integers set in `streaming_tick_system` (`ecs/systems.rs:755–756`). No in-flight breakdown, no eviction count, no margin-vs-throughput view | `streaming.rs:490` |
| Mesh cache hit/miss | **Half.** `AtomicCacheStats` tracks hits/misses/errors/bytes/evictions and the UI shows them (`ui/panels.rs:~770–788`). **No key attribution** — you cannot see *why* a key missed | `meshing/cache.rs`, `meshing/mod.rs:253–258` |
| Memory / residency panel | **Nothing** in a live panel. `World::print_debug_stats` computes per-storage bytes once at startup and logs it | `world/mod.rs:449+` |
| Determinism checker (in-place) | **Nothing** at runtime. One unit test exists and may be vacuous (§5.3) | `world_generator.rs:665–679` |
| Mutation & event log | **Nothing.** `MutationOutcome` is returned and consumed, never recorded | `world/mutation.rs:78–97` |

So A is close to greenfield, with two partial exceptions (meshing stats, cache
stats) that already have a home in the UI and a shape to extend rather than
replace.

**One design constraint worth surfacing now:** `MeshingStats` is cloned into
`UiState` every frame (`meshing/coordinator.rs:52`). Adding six more stat structs on
that pattern means six struct clones per frame in the hot path. The observability
work should pick a shape (shared `Arc<Atomic*>` snapshots, or a single
`Diagnostics` resource read directly by the panel) rather than replicate the clone.

---

## 3. Workstream A/B interface — what `FrameTimings` must measure

Because of §0.3, a naive per-stage `Instant::now()` bracket will produce a
`Render` stage that swallows the vsync block and looks like the whole problem
every frame. The instrument needs to separate three things:

1. **CPU time per `FrameStage`** — schedule-side work, excluding GPU waits.
2. **The present/acquire block** — `surface.get_current_texture()` and
   `present()`, isolated so a GPU-bound frame is distinguishable from a
   CPU-bound one.
3. **Frame-to-frame wall clock** — what `dt` already is, kept for comparison.

If CPU stage totals sum to ~6 ms while wall clock reads 28 ms, the hitch is GPU
or compositor and workstream B's answer is "characterize the external cause." If
CPU stage totals spike to ~15 ms on hitch frames, the stage that spiked names the
owner. Either outcome passes the gate; a design that cannot distinguish them does
not.

A rolling per-stage max/p99 over the last N frames matters more than the current
frame's value — a hitch several times per second is invisible in an instantaneous
readout that the eye samples at reading speed.

---

## 4. Workstream B — the at-rest hitch

### 4.1 What is actually being reported

> **Superseded by measurement — see §4.5.** This section's reasoning (and §0.3's
> vsync reframing) was written before any instrument existed. Substep 1's hitch
> counter shows that vsync quantization is *not* operative: the engine runs slower
> than the refresh interval, so Fifo does not clamp it and frame times do not
> quantize. Both the original "~28 ms hitch" framing and my correction of it are
> wrong. §4.5 records what the numbers actually show. §4.2–§4.4 are retained as
> the reasoning that led to the measurement.

~28 ms frames recurring several times per second with the camera at rest, no
identified cause (`post-phase-9-audit.md` §3, §4). Given Fifo and the
inter-frame `dt` measurement (§0.3), the correct restatement is: **the engine
occasionally misses a 16.6 ms vsync deadline, several times per second.** The
overrun is on the order of 10 ms, not 28.

### 4.2 What post-phase-9 established, and what has expired

**Still valid (re-verified this session):**
- Shader and graph hot-reload are `notify`-backed with `try_recv` drains, not
  stat polling (`shader_reload.rs:33–48`, `nodegraph-hotreload/src/watcher.rs:31–46`).
  Near-zero when idle. Confirmed.
- The 30-second autosave sweep is two orders of magnitude too infrequent
  (`persistence.rs:848–864`, `AUTOSAVE_INTERVAL_SECS`).
- The walkability mask and its recompute system no longer exist. Confirmed by
  grep: no `walkability` symbol in the tree.
- The seam re-mesh cascade was ruled out by live A/B experiment. That experiment's
  result stands on its own evidence.

**Expired or never covered:**
- Everything added by `3e325ef` and `b258ad7`. The audit predates both.
- `room_detection_system` was never named. It is in fact **safe**: throttled on
  head-cell change (`ecs/systems.rs:637–640`) and gated to `EngineMode::Play`
  (`:620`). At rest it returns in a few instructions. Ruled out here.
- **The unload save path was never checked.** This is distinct from the autosave
  sweep that *was* checked. `streaming_tick_system` passes an `on_unload` closure
  that calls `persistence.save_chunk_on_unload(chunk)` **synchronously on the main
  thread** (`ecs/systems.rs:733–739`), which does `build_chunk_record` → zstd
  compress → SQLite write (`persistence.rs:867–874`). Any chunk churn produces
  main-thread compression + disk I/O inside the frame.

### 4.3 Ranked candidates for the current tree

Presented as candidates with the evidence I have, not findings. Workstream A is
what settles them; this list is what A should be pointed at first.

**C1 — `climate_tick_system` → `CloudShadowState::tick`. Unconditional, main thread, every frame.**
`ecs/schedule.rs:50–52` runs it every frame with no clock gate (`ClimateClock` only
accumulates `dt`; `ecs/systems.rs:462–465`). Per frame it performs:
- Up to `BIOME_FILL_BUDGET = 64` fresh biome resolves (`cloud_shadow.rs:35, 384–404`),
  each calling `WorldGenerator::biome_id_at_world` → `world_eval.biome_column(ctx)`
  — **a full World+Zone graph column evaluation per call**
  (`world_generator.rs:169–178`).
- A `retain` over the entire `biome_cache`, bounded at `ENV_SIZE * 3` cells in each
  direction (`cloud_shadow.rs:407–410`) — up to ~37,000 entries scanned per frame.
- A 13×13 water re-query loop (`:419–430`).
- Four layers × (seed / simulate / settle / evict) over a 33×33 cell grid, with
  `condensation_at` invoking `climate::regional_condensation_target(world_eval, …)`
  per cell (`:447–466`).
- Four layers × 1,024 `density_at` for `env_typical` (`:478–484`), then four ×
  1,024 again for the texture pack (`:499–508`).

The author's own comment at `cloud_shadow.rs:32–35` says the fill budget exists
because uncapped resolves *hitch*. Warm-cache steady state is cheaper than
first-entry, but the `retain`, the four grid simulations, and the 8,192 `density_at`
calls are unconditional. **This is the single largest per-frame main-thread cost I
found, and it is the newest code in the tree.**

*Complication:* roadmap §3.12 quarantines cloud hydrology — "fix-or-remove decision
at 0.5.0, zero engineering before then." If C1 is the hitch, the quarantine and the
hitch gate collide. Measuring it is not engineering it; deciding what to do if it
*is* the cause is a scope call for you, not me. Flagged in §14.

**C2 — Reflection pass is a full second geometry render every frame.**
`reflection_pass.rs:44–122` re-renders terrain, detail paint, scatter, and the
player into the half-res reflection targets with a mirrored view-projection.

*(Corrected 2026-07-28.* An earlier revision of this audit claimed the terrain loop
was unculled because it lacks the `frustum.is_chunk_visible` test the detail and
scatter loops carry. That is wrong: `chunks: &visible_chunks` is already
frustum-culled at `ecs/systems.rs:1278–1280` before it reaches either the main scene
node or the reflection node. The detail/scatter loops test visibility because they
iterate their own un-prefiltered per-chunk maps. **No culling fix is needed; no code
change is proposed here.**)

It remains a real steady per-frame GPU cost — a second full pass over the visible
set, added in `b258ad7`. Steady rather than bursty, but it consumes the headroom
that turns an occasional 10 ms spike into a missed deadline. Already gated at
runtime by `enabled: !ui.params.debug.hide_water && !water_pass.chunk_meshes.is_empty()`
(`ecs/systems.rs:1347`), so the experiment costs nothing.

**C3 — Chunk churn driving main-thread save/compress.** See §4.2. Requires chunk
unloads at rest; the streaming visualizer (A3) answers directly whether they occur.

**C4 — `param_change_detection_system` clones all of `EngineParams` every frame.**
`ecs/systems.rs:262–269`: `ui.change_detector.detect(&ui.params)` then
`ui.params.clone()` and `snapshot`. `EngineParams` is a deep nested struct
including `Vec`-backed material tables. Two deep comparisons plus a deep clone per
frame, unconditionally. Allocation churn of this shape produces exactly the
"occasional spike from allocator behavior" pattern.

**C5 — egui tessellation + buffer upload.** `EguiRenderer::draw` runs every frame,
tessellates the full panel tree, and calls `update_buffers` (`ui/mod.rs:161–190`).
With the engine panel expanded this is real work and it varies with panel content.
Worth confirming by toggling the UI off (F-key) and watching the hitch rate — a
one-minute experiment that costs nothing and is worth running *before* any code.

**C6 — Unconditional full-resident-set scans, several per frame.** Each is
individually cheap; collectively they scale with the resident set and none is
bounded:
- `MeshingCoordinator::tick` → `submit_all_dirty` iterates every chunk every frame
  (`meshing/coordinator.rs:31`, `meshing/mod.rs:156`).
- `seam_smoothing_system` iterates every chunk every frame looking for
  unfinalized candidates (`ecs/systems.rs:826–837`).
- `fluid_tick_system` sums `active.len()` over every chunk every frame
  (`ecs/systems.rs:353–354`).
- `compute_stats_system` iterates every chunk with a frustum test
  (`ecs/systems.rs:1090–1098`).
- `streaming.tick` builds a `Vec` of unload candidates from every chunk key
  (`streaming.rs:467–472`).

**Explicitly ruled out this session:** `room_detection_system` (throttled + mode-gated),
`visibility_mask_upload_system` (early-returns unless `room.dirty`,
`ecs/systems.rs:1031–1034`), hot-reload polling (re-verified), autosave interval.

### 4.4 The cheap experiments to run first

Both candidates that already have runtime toggles, before any instrumentation:

- **F1** hides the entire egui UI (`input.ron` → `ToggleUI`, `input.rs:586–588`) — tests C5.
- The **Hide Water** debug checkbox sets `params.debug.hide_water`, which disables
  both the water scene pass and the reflection pass (`ecs/systems.rs:1347`) — tests C2.

Neither costs a line of engine code. The one obstacle is that F1 also hides the
Performance panel, so the fps readout disappears exactly when it is needed; Substep 1
adds a once-per-second `log::info!` hitch counter so both experiments are readable
with the UI hidden. Run them before writing any of `FrameTimings`.

---

### 4.5 Measured — Substep 1 results, 2026-07-28

Three conditions, camera at rest, **debug build** (`cargo build`, dev profile,
`unoptimized + debuginfo`; the workspace declares no `[profile.dev]` override, so
every dependency and every workspace crate is at opt-level 0).

| Condition | Samples | Mean fps | Mean frame | Frames > 20 ms | Δ vs. A |
|---|---|---|---|---|---|
| **A** baseline | 23 s | 45.2 | **22.1 ms** | **99.3 %** | — |
| **B** UI hidden (F1) | 16 s | 46.6 | 21.5 ms | 92.5 % | −0.67 ms |
| **C** water + reflection off | 11 s | 47.7 | 21.0 ms | 76.4 % | −1.17 ms |

**This is not a hitch. It is a uniformly slow frame.** In the baseline, 1,033 of
1,040 frames exceeded 20 ms — the threshold is not detecting excursions, it is
sitting below the mean. The engine runs at a steady ~22 ms/frame, every frame.
The original report ("~28 ms hitch, several times per second") appears to have
been a reading of ordinary variance around a 22–28 ms baseline, not a distinct
event class.

**Vsync is not the limiter, and §0.3 was wrong.** Frame times cluster at 21–26 ms
with no quantization to 16.7/33.3 ms multiples. Under Fifo, the swapchain clamps
throughput only when the producer is *faster* than the refresh interval; a
producer slower than refresh runs at its own rate. The engine is that producer.
So ~22 ms is genuine CPU+GPU cost per frame, not a scheduling artifact — which is
better news, because genuine cost is attributable and vsync interaction is not.

**Two distinct phenomena, not one:**

1. **An elevated baseline of ~22 ms on every frame.** This is the dominant
   problem and it is a *throughput* issue, not a hitch. It is what makes the
   16.6 ms budget (roadmap §7.3) unmet.
2. **Intermittent 30–53 ms spikes, roughly every 5–8 seconds** (A: 43.0, 53.1,
   35.1, 33.2; B: 51.6, 35.9; C: 33.3). These *are* excursions, they persist
   unchanged across all three conditions, and they are far rarer than "several
   times per second." Closer to the original description in kind, wrong in
   frequency by an order of magnitude.

**What the toggles proved.** Both candidates are real and both are small:
egui costs ~0.67 ms/frame (C5), water + reflection ~1.17 ms/frame (C2). Together
~1.8 ms of a 22 ms frame — under 9 %. **Neither is the story, and neither is
worth fixing on these numbers.** C5 and C2 are hereby closed as hitch candidates;
they remain minor optimization targets with no current claim on this phase.

**The unexamined variable: this is a debug build.** For an engine whose hot path
is per-frame graph evaluation, HashMap iteration over the resident set, and ~8,200
`density_at` calls in `climate_tick_system` (C1), opt-level 0 is not a small
factor. Until the same three readings exist from a release build, no conclusion
about the ~22 ms baseline is safe, and neither `FrameTimings` nor any fix should
be built against debug numbers — the per-stage *proportions* differ between
profiles, so a debug profile would point the instrument at the wrong stage.
**Release measurement is the immediate next step** (§14 Q7).

### 4.6 Measured — release build, same day

Same three conditions, `cargo run --release` (opt-level 3, thin LTO,
`codegen-units = 1`).

| Condition | Samples | fps | Hitches/s | Worst |
|---|---|---|---|---|
| **A** baseline (steady state) | 15 s | **60–61** | 20–21 | **23.7–24.3 ms** |
| **B** UI hidden | 26 s | 60–61 | 20–21 | 23.8–25.6 ms |
| **C** water + reflection off, first 11 s | 11 s | 60–62 | 20–25 | 23.9–27.1 ms |
| **C** water + reflection off, remaining 62 s | 62 s | 54–62 | 13–28 | **26.7–208.9 ms** |

**The originally-reported phenomenon is a debug-build artifact.** Debug: 22.1 ms
mean, 99.3 % of frames over 20 ms. Release: a locked 60 fps. The "~28 ms hitch
several times per second" that `post-phase-9-audit.md` §3–§4 carried forward as the
highest-priority open defect, and that roadmap §3.12 and §10 gate on, **does not
occur in an optimized build.**

**The steady ~24 ms "worst" in A and B is swapchain pacing, not a stall.** The
evidence is its metronomic consistency: across ~40 seconds of samples it never
leaves 23.7–24.3 ms. A stall from compression, I/O, allocation, or eviction varies;
this does not. With `PresentMode::Fifo` and `desired_maximum_frame_latency: 2`
(`main.rs:188–191`), the CPU queues several frames quickly and then blocks in
`get_current_texture()`, producing a bimodal CPU-side inter-frame distribution that
sums to the refresh rate. 60 fps sustained *is* the deadline being met. **The 20
hitches/second in A and B are the instrument mislabelling normal pacing**, because
the 20 ms threshold sits below the pacing peak. Retune to ~35 ms.

**Condition C's second half is the first genuine defect this investigation has
found.** Roughly one frame per second at 45–60 ms, superimposed on otherwise-locked
60 fps, with six excursions past 100 ms (111.4, 123.0, 195.2, 208.9, 107.8, 102.4).
It began ~11 s into the run and persisted and worsened for the remaining 62 s.
Throughput held (fps stays ~60) — these are single-frame excursions, not a rate drop.

Hiding water is not a plausible *cause* (it removes work, and C's own first 11 s
were clean). Condition C was, however, **~3× longer than A or B** (73 s vs. 21 s and
26 s), so the likeliest reading is that C was simply the first window long enough to
contain the phenomenon. Two candidate mechanisms, not yet distinguished:

1. **Main-thread persistence.** Both write paths run on the main thread:
   `save_chunk_on_unload` inside `streaming_tick_system` (`ecs/systems.rs:733–739`)
   and the 30-second `save_dirty_chunks` sweep (`persistence.rs:848–864`,
   `AUTOSAVE_INTERVAL_SECS = 30.0`). Each does dirty-chunk scan → zstd → SQLite
   write. Supporting evidence: `saves/world.vxdb-wal` stood at **4.3 MB** immediately
   after this run. A WAL that large implies sustained write traffic at rest, and
   SQLite's default auto-checkpoint (~1000 pages ≈ 4 MB) blocks the *writing* thread
   — which is the main thread. That is a textbook source of cumulative,
   worsening 100–200 ms stalls. Note the fluid sim marks chunks persist-dirty
   whenever cells change (`MarkFluidDirty`), so ocean cells still settling would
   feed this continuously.
2. **Streaming churn from camera movement**, driving generation, meshing, per-chunk
   GPU buffer creation, and unload-time saves.

**Ruled out here:** mesh-cache LRU eviction — the cache holds 3.2 MB across 164
files against a 256 MB budget (`params.rs:618`), so `lru.evict()` has never run.

The unfiltered console log distinguishes (1) from (2) at zero cost: autosave emits
`"Saved N modified chunks"` (`persistence.rs:843`) and meshing emits
`"Meshing batch complete in Xms"`, so correlating either against the spike seconds
settles it without writing a line of code.

### 4.7 Root cause localized — time-of-day sweep, 2026-07-28

Two further zero-code experiments, release build, camera untouched, water visible,
using the existing `Paused` / `manual_time` controls (`ui/panels.rs:274–289`).

**Test 1 — break the time-of-day / session-length confound.** Time paused at
`manual_time = 0.30` (daylight) for ~2 minutes: after an initial settling burst
(including one 704.8 ms frame at the moment of pausing), the engine ran **107
seconds at a flat 23.8–25.9 ms worst**, spanning an autosave (`Saved 24 modified
chunks`, 14:09:56) whose second reported **24.0 ms** — no cost at all.

Two conclusions. **Frame cost is a function of time-of-day state, not elapsed
session time**; every prior run's "degrades after ~130 s" was the sun setting on a
300-second day (`params.rs:405`) from a start time of 0.30. And **main-thread
persistence is refuted twice over** — the earlier 3 ms reading was not a fluke.

**Test 2 — sweep `manual_time`, ~10 s held at each value.** Sun elevation is
`sin((t − 0.25)·2π)`.

| `manual_time` | Sun elevation | Hitches/s | Worst | Regime |
|---|---|---|---|---|
| 0.30 | +0.309 | 20–21 | 24.0–24.7 ms | clean |
| 0.50 (noon) | +1.000 | 20–21 | 24.0–24.5 ms | clean |
| 0.65 | +0.588 | 20–21 | 24.3–26.4 ms | clean |
| 0.72 | +0.187 | 20–21 | 23.9–26.2 ms | clean |
| **0.76** | **−0.063** | 21–25 | **43.9–49.9 ms** | **elevated** |
| 0.85 | −0.588 | 18–24 | 43.8–50.6 ms | elevated |
| 0.95 | −0.951 | 30–31 | 29.9–33.2 ms | intermediate |
| 0.10 | −0.809 | 30–31 | 30.5–33.1 ms | intermediate |

**The transition sits exactly at the sun crossing the horizon.** `light_space_matrix`
early-returns `Mat4::IDENTITY` when `sun_dir.y <= 0.01`
(`simulation/time_of_day.rs:91`), which for this arc is `t ≥ 0.7484`. The sweep
brackets that boundary within 0.04 of a cycle: 0.72 clean, 0.76 elevated. Nothing
else in the tree is known to be discontinuous in that interval.

**But the obvious mechanism is refuted by reading the code.** I expected the
identity matrix to inflate the shadow frustum and multiply `shadow_chunks`
(`ecs/systems.rs:1281–1283`). It does the opposite. Extracting Gribb/Hartmann planes
from the identity matrix (`rendering/frustum.rs:from_view_projection`) yields the box
`x,y ∈ [−1,1]`, `z ∈ [0,1]` — every chunk except the one at the origin fails the
p-vertex test, so the shadow pass draws *almost nothing* at night. It should get
cheaper. **The mechanism is therefore not shadow-chunk count, and is not yet
identified.**

**Three regimes, not two.** Deep night (0.95, 0.10) is distinct from early night
(0.76, 0.85): 30 hitches at ~31 ms versus 24 hitches at ~47 ms. Both are past the
horizon and both take the identity branch, so a second variable is also in play. A
single boolean switch does not explain the data.

**Throughput is unaffected in every regime — 59–62 fps throughout.** Only the
*distribution* of CPU-side inter-frame gaps changes; the mean stays at ~16.6 ms.
This is consistent with GPU frame cost rising at dusk while remaining under the vsync
budget, which redistributes how the CPU blocks in `get_current_texture()` without
dropping a frame. **It is possible there is no user-visible defect here at all** —
only a metric artifact. Confirming or refuting that is precisely what a per-stage
instrument separating CPU work from the present block is for (§3).

**What this buys the phase:** a **frozen, 100 %-reproducible bad state**. Park
`manual_time` at 0.76 and the engine sits at ~47 ms indefinitely; move it to 0.72 and
it returns to 24 ms. Substep 2's `FrameTimings` is no longer a fishing expedition —
it is an A/B read against two stable configurations.

---

### 4.8 Closed — the hitch gate, with evidence

Substeps 1–3 (2026-07-28). **There was no hitch.** The defect that
`post-phase-9-audit.md` §3–§4 carried as the highest-priority open item, that
roadmap §3.12 lists as a known unknown, and that §10 gates 0.3.0 on, was two
stacked measurement artifacts.

**Artifact 1 — debug build.** The workspace declares `[profile.release]` but no
`[profile.dev]`, so `cargo run` builds every crate *and every dependency* at
opt-level 0. Debug: 22.1 ms mean frame, 99.3 % of frames over 20 ms. Release: a
locked 60 fps (§4.5, §4.6).

**Artifact 2 — wall-clock inter-frame time under `PresentMode::Fifo`.** It
measures blocking, not work, and it moves *inversely* to engine cost: a faster
engine fills the swapchain queue sooner and blocks longer in
`get_current_texture()`. The "worst frame" statistic was reporting idleness.

**The measurement chain.** `FrameTimings` (Substep 2/2b) separates CPU work per
`FrameStage` from the present block and from time outside the schedule. Paused at
`manual_time` 0.72 (day) vs 0.76 (night), release:

| | Fifo, 0.72 | Fifo, 0.76 | Uncapped, 0.72 | Uncapped, 0.76 |
|---|---|---|---|---|
| fps | 60–61 | 59–62 | 303–320 | **366–388** |
| cpu max | 3.2–5.3 ms | 2.8–4.1 ms | 4.3–5.8 ms | 3.6–5.3 ms |
| frames over 16.6 ms budget | **0** | **0** | **0** | **0** |
| present max | 21.2–22.4 ms | **41.2–44.7 ms** | ≤0.9 ms | ≤1.3 ms |
| outside max | 0.3–0.8 ms | 0.3–0.5 ms | ≤1.2 ms | ≤1.0 ms |

Removing the vsync cap (`PresentMode::Immediate`, a temporary diagnostic since
reverted) shows **night is 22 % faster**, not slower — 376 fps against 309 fps.
The apparent night-time "hitch" was the engine idling more because it had less to
do.

**And the mechanism is consistent with §4.7's code reading after all.** Below the
horizon `light_space_matrix` returns `Mat4::IDENTITY`; the frustum extracted from
it is the unit box, so nearly every chunk fails the p-vertex test and the shadow
pass draws almost nothing. `render` stage mean drops from ~0.95 ms to ~0.83 ms
accordingly. §4.7 called that reading "refuted" because it predicted the opposite
of the observed symptom — it was correct, and it was the *symptom* that was
inverted.

**Engine position at 0.3.0.** 3–5 ms of CPU work per frame against a 16.6 ms
budget, zero over-budget frames in any configuration tested, ~309 fps uncapped at
the worst time of day. Largest CPU consumer is `FrameStage::Simulation` at
~1.4–2.2 ms mean / 6.5 ms worst observed. These numbers are the perf baseline.

**Candidates closed by measurement, not reasoning** — none should be
re-investigated without new symptoms: main-thread persistence (autosave measured
at ~3 ms for 24 chunks, then 0 ms), streaming churn, session-length accumulation,
mesh-cache LRU eviction, egui, the water + reflection pass, the winit event loop,
and `climate_tick_system` — the audit's top-ranked C1, bounded at ≤2.2 ms
*including* the fluid tick, player sim, room detection, picking, and param-change
detection. **Roadmap §10's R7 collision between the hitch gate and §3.12's
cloud-hydrology quarantine never materialized; the quarantine holds untouched.**

**Consequences for the phase:**

- **Q7 is answered by force:** all perf measurement is release-only. §7.3's
  16.6 ms budget is meaningless against an unoptimized build, and a perf baseline
  taken from one would be fiction.
- **§7.3's "no recurring hitch > 4 ms without an attributed cause" needs
  restating.** As written it is unmeasurable on a vsync-limited app, where
  wall-clock frame time quantizes to the refresh interval and inverts with load.
  The budget belongs on **CPU work** (`schedule span − present block`), which is
  what `FrameTimings.cpu_max_ms` reports. Proposed as a roadmap revision.

---

### 4.9 Frame cost at high zoom is draw-call recording — root-caused, not fixed

Substep 12a (2026-07-28), release, steady state at maximum zoom: camera still for
15+ seconds, `missing 0`, `in-flight 0`, **`0/20` worker threads busy**.

```
FPS 32   CPU worst 31.4ms / 16.6 budget   Present mean 7.4ms
stages mean/max ms:
  input 0.02/0.07   sim 5.33/7.08   mesh 0.99/2.55
  uniform 0.43/1.25   render 24.25/29.89   post 0.01/0.01
Jobs: generate 9.0ms mean (4.2K done), mesh 4.4ms mean (4.0K done), 0 queued
Chunks 1465 visible / 2210 meshed / 4388 resident, 4.6M triangles
```

**`render` is 78% of frame CPU.** The worker pool is idle and per-job costs are
flat across every radius measured (`generate` 8.5–9.0 ms throughout), so
generation, meshing, and the scheduler are not implicated. The cost is
**draw-call recording, scaling with *visible* chunk count**: terrain is drawn by
three separate passes — shadow, main scene, and the reflection pass added in
`b258ad7` — each issuing `set_vertex_buffer` + `set_index_buffer` +
`draw_indexed` per chunk, so ~1,465 visible chunks become ~4,400 chunk draws plus
foliage and scatter, at roughly 5 µs each.

**Not fixed, and deliberately so.** Batching, pass consolidation, or LOD is
rendering work, which roadmap §10 lists as a non-goal. It already has a home:
§11's LOD strategy and decision D8 (greedy meshing, due at 0.4.0 exit) both
target this, and drift observation 5.4 notes §11's LOD text presumes a
distance gradient the orthographic isometric camera does not produce — so the
strategy needs revisiting before it is built. **Trigger:** D8 at 0.4.0 exit, or
sooner if a supported zoom level is found to breach the frame budget.

**Two consequences the phase should carry:**

1. **`sim` at 5.33 ms is ~4× its §4.8 baseline of 1.4 ms**, and it is engine
   logic rather than rendering, so it *is* in scope. Several systems in that
   stage scan the entire resident set every frame with no bound —
   `fluid_tick_system` sums `active.len()` across all 4,388 chunks,
   `param_change_detection_system` deep-clones and compares all of
   `EngineParams`. Audit §4.3's C6 list enumerates the rest.
2. **"Supported zoom" is undeclared.** The gate reads "streamed area fills within
   budget at all *supported* zooms", but nothing states that range and `zoom_max`
   permits zoom levels the engine demonstrably cannot hold budget at (fill of
   2,619 chunks took 8,704 ms at maximum zoom, against a 2,000 ms budget).
   Incremental steps within the moderate range all measured inside budget. The
   range must be **declared by measurement** in the perf baseline; closing the
   gate without that would be reinterpreting it rather than meeting it.

**Measurement caveat worth recording.** An earlier pass at this suggested frame
time degraded 10× with resident set (down to 10 fps). Those readings were taken
in the second immediately following each zoom step, so `FPS` and `CPU worst` —
both one-second aggregates — covered the *fill* rather than steady state. The
steady-state figures above are ~3× better. When reading `FrameTimings`, let the
view settle for several seconds first; the instrument aggregates over a window
and will otherwise attribute transient work to the resting state.

---

### 4.10 First measured input to decision D8 (greedy meshing)

Substep 12b, release, maximum zoom, 4,388 resident chunks:

```
Memory — 29.9 MB CPU + 490.5 MB GPU   (4388 chunks, 2396 uniform)
  voxel 15.6   detail 9.6   scatter 0.4   fluid 2.5   overrides 1.76  [MB]
  session peak 29.9 MB CPU / 490.5 MB GPU
```

**GPU mesh buffers are 16× the CPU footprint.** At a 32-byte `FaceVertex`, one
quad costs 4 vertices + 6 indices = 152 bytes, so 490 MB is roughly **3.4M quads
/ 6.8M triangles resident** across 2,210 meshed chunks — about 1,530 quads per
chunk. That is what one-quad-per-exposed-face produces: design §10 specifies side
and bottom faces as greedy-merged, and drift 1.6 records that as unimplemented.

**Roadmap §8 D8 is due at 0.4.0 exit and says to decide "against measured
triangle budgets on the 3-biome reference world."** Nothing could measure them
until the residency panel existed; this is the first figure. It is also the same
root cause as §4.9 — draw-call count and vertex volume are both downstream of
naive meshing, so D8 and the render-cost finding are one decision, not two.

Forward-looking note for §7.3's min-spec (declared at 0.11.0, audience explicitly
on modest hardware): 490 MB of mesh buffers at maximum zoom, or roughly 310 MB
extrapolated to a mid-range supported zoom, is a real constraint on a 2 GB card.

**No leak.** Repeated round trips between areas return CPU and GPU totals to the
same range and the session peak stops climbing — §7.3's "session growth flat
after warm-up", verified for the first time. 2,396 of 4,388 chunks (55%) hold
uniform voxel storage, confirming palette collapse works on the air chunks above
and below terrain.

---

### 4.11 Drift observation 5.1 — measured, then closed

Substeps 13a / 13a-follow. The observation predicted "systematic
miss/rebuild/delete churn during load-order-dependent streaming" from the mesh
cache key including the neighbour border. Cache attribution (splitting misses into
*cold* — no cached mesh for this position — and *stale* — a cached mesh exists
under a different key) priced it for the first time.

**Measured, warm second run over an identical route:**

| | before | after |
|---|---|---|
| Hit rate | 66 % | **100 %** |
| Stale misses | ~1,400 | **0** |
| Total misses | 1,611 | **8** |

1,400 of 4,758 lookups were re-keys — each one a mesh rebuild plus a file delete
plus a file write for a mesh already on disk. The file count proved the mechanism
both times: before, `3,092 files + 184 cold = 3,276`, because stale misses
*replace* a file 1:1; after, `2,452 + 8 = 2,460`, because nothing is rewritten.

**Cause.** `ChunkSnapshot::extract` fills every non-interior cell of the 34³
snapshot — six face planes (6,144 cells), twelve edges (384) and eight corners
(8) — from the full 26-neighbour set, missing neighbours becoming `Voxel::EMPTY`.
But meshing is gated only on the **six face** neighbours (`has_face_neighbors`)
and `cube_mesher` culls against the single face-adjacent voxel; its own doc
comment confirms no AO and no diagonal reads. So the 392 edge/corner cells were
load-order dependent *and never read by the mesher*. One differing corner voxel
re-keyed the whole chunk.

This refines the observation's suggested fix. It proposed keying on "interior
content only," but the face-adjacent border genuinely affects the mesh through
culling. The correct key is **interior + the six face planes**, which is exactly
the data meshing waits for — making the key load-order independent by
construction.

**Still recorded as divergent:** the key remains *content*-addressed rather than
the *input*-addressed form design §10 specifies (graph hash + mesher version +
registry hash + seed). Content addressing is strictly stronger for correctness —
§10's stated failure mode cannot occur — and that divergence stays on the books
rather than being closed here.

---

## 5. Workstream D — correctness and determinism

### 5.1 Drift 2.1 (PoissonDisk seeding) — **CLOSED**

`EvalContext::chunk_world_seed` (`nodegraph-eval/src/context.rs:60–68`) derives from
the chunk's world-space base voxel `chunk * CHUNK_DIM`, folding X, Y, and Z. The old
`scatter_seed` is gone. `poisson_disk` consumes it (`scatter.rs:165`). The stale
"acceptable for Phase 10" comment is gone. Observation 5.3 (`chunk.y` omitted) is
closed by the same change and covered by `poisson_disk_varies_with_chunk_y`
(`scatter.rs:251–261`).

**Recommendation: no work. Verify by reading and record it closed.** The roadmap's
"do it now while save wipes are cheap" urgency was already honored, one version early.

### 5.2 Drift 1.4 (fluid before foliage) — **CLOSED, but by a different mechanism than §10 words**

Both halves of the ordering are correct:
- `WorldEvaluator::evaluate_chunk` runs `composite_fluid` (line 297) before
  `evaluate_foliage` (line 298) — `nodegraph-eval/src/world_eval.rs:293–300`.
- `WorldGenerator::generate_chunk` runs `ocean_fill` + `apply_biome_ponds` (stage 9,
  lines 242–247) before `paint_to_detail_layers` + `scatter_to_store` (stage 10,
  lines 252–253) — `world/world_generator.rs:239–253`.

Submersion is enforced by a shared predicate `fluid_gen::foliage_submerged`
(`world/fluid_gen.rs:184–198`) applied **at the storage-boundary translators**
(`world_generator.rs:300`, `:351`), which drop submerged paint texels and scatter
instances. Covered by three tests (`fluid_gen.rs:278`, `world_generator.rs:835`, `:861`).

The roadmap says "expose submersion to `SurfaceFilter` and scatter placement." That
is **not** what shipped. `SurfaceFilter` (`nodegraph-eval/src/detail_eval.rs:95`,
`:151`) has no fluid input; the eval layer stays fluid-blind and the filter happens
downstream. The in-source comment at `world_eval.rs:293–296` documents this
deliberately.

**Recommendation: record the shipped shape as settled, do not rebuild it.** The
drift's *impact* (grass underwater) is closed and tested. Name the migration
trigger explicitly: the downstream filter can only *suppress* foliage, never *select
different* foliage. The first DetailGraph that wants to place aquatic species where
a column is submerged — rather than place nothing — forces submersion into
`SurfaceFilter` as a real graph input. That is 0.4.0 content territory. Write the
trigger into the audit and `engine-design.md` §5 rather than paying for it now.

### 5.3 Determinism test coverage — **genuinely open, with one caveat**

`generate_chunk_is_deterministic` (`world/world_generator.rs:665–679`) generates
chunk `(1,0,2)` twice and compares all `CHUNK_VOLUME` voxels. It does **not** assert
that the chunk contains any slabs. The vacuity risk named in `cowork-handoff.md`
Part 1 item 1 is still live and unaddressed. Real work.

The guard is easy and should follow the established convention (`post-phase-8`'s
"guard against vacuous tests"): count `SlabBottom`/`SlabTop` voxels and assert
`> 0`, on a chunk chosen because it has them. If no chunk in the shipped meadow
reliably contains slabs, that is itself a finding — but `traversal_smoothing_distance`
defaults to 1 (`world_generator.rs:38`), so surface-step chunks should produce them.

**Poisson continuity is the caveat.** As stated in §0.5, the point set is
per-chunk by construction; `poisson_placement` runs one Bridson walk over the
chunk's margin band (`scatter.rs:97–160`). What *can* be tested, and is worth
testing:
- `poisson_disk` is a pure function of `(ctx, params)` — already covered
  (`scatter.rs:239–249`).
- The seed is world-absolute: two `EvalContext`s for the same chunk under
  different evaluation order produce identical seeds. Trivially true; low value.
- **The genuinely useful one:** `jittered_grid` seam continuity is already tested
  (`scatter.rs:192–214`), and `PoissonDistribution` — not `PoissonDisk` — is what
  the live meadow detail graph actually uses (`post-phase-8-audit.md`, "Accepted
  stable-ID break"). Extending seam coverage to the *shipped* scatter path is worth
  more than testing an unused node.

**Recommendation:** ship the slab-bearing guard; replace "Poisson continuity" with
"seam continuity of the scatter path the shipped content uses," and record in the
audit *why* the roadmap's wording cannot be met. This is a roadmap correction, not
a scope cut — say so in writing per §1 rule 2.

---

## 6. Workstream C — the unified job system

### 6.1 Drift 2.2 — **CLOSED**

Both halves. There is one pool:

```
main.rs:406–412     rayon::ThreadPoolBuilder, num_threads = CoreBudget.pool_threads
world/mod.rs:75     startup fill      pool.install(par_iter)
streaming.rs:267    generation        pool.spawn
meshing/mod.rs:227  meshing           pool.spawn      (was std::thread mesh workers)
regen.rs:24         background regen  same Arc
ui/field_probe.rs:201  probe eval     same Arc
```

`grep std::thread` across `crates/` finds only test sleeps in
`nodegraph-hotreload`. No chunk work on a second threading model.

The core-budget arithmetic is centralized in `core_budget.rs:20–29` — `detect()`
computes `pool_threads`, `gen_in_flight`, `mesh_in_flight` in one place, consumed by
`main.rs:408`, `streaming.rs:214`, `meshing/mod.rs:106`. The duplicated `usable/3`
in two files is gone; `num_cpus::get()` appears only in `core_budget.rs`.

**The exit gate "One job system; no second threading model anywhere" passes today.**

### 6.2 What P14 still actually requires

The gap is not threading — it is **scheduling semantics**. What exists is a shared
thread pool with two fire-and-forget submitters, each self-limiting via a fixed
in-flight cap. What P14 and design §1 specify is "a single job system with declared
dependencies and priorities." Concretely missing:

| P14 property | Current state |
|---|---|
| Declared dependencies between jobs | None. Ordering is implicit in when systems call `spawn` |
| Priorities | Partial and local. Streaming sorts its *own* spawn queue by distance (`streaming.rs:439–458`, `priority = dx² + dz²`) before handing to rayon. Once spawned, rayon's work-stealing decides order; a distant chunk already in flight outranks a near one just queued. Meshing has no priority at all — `pending_submissions` is a `Vec` drained in insertion order with `swap_remove` (`meshing/mod.rs:187–233`), which actively *scrambles* order |
| Priority from distance to observed region | Only for generation spawn, and only at queue-build time |
| Cross-consumer budget | Two hardcoded caps that sum to `pool_threads` by construction (`core_budget.rs:25–27`) — correct, but static and blind to actual demand |
| Introspection (queue depth, stalls, per-kind time) | None. This *is* deliverable A2, and it cannot be built against fire-and-forget `pool.spawn` |
| Chunk I/O as a consumer | Not one. Loads happen inside generation tasks (`streaming.rs:276–325`); saves happen **on the main thread** (`ecs/systems.rs:733–739`) |

**This reframes C.** It is not "replace a threading model" — it is "put a scheduler
between the submitters and the pool." That is a smaller, better-defined change than
the roadmap implies, and it is a *prerequisite for A2*, not a peer of it. A job
inspector requires jobs to be objects with a kind, a priority, a submit time, and a
completion time. Today they are closures.

**The interface decision to surface before any code** (§14): whether the scheduler
owns its own queues and feeds rayon a bounded stream, or wraps rayon's scope with a
priority-ordered pending set. And whether chunk *save* becomes a job kind now
(which would move zstd+SQLite off the main thread and possibly resolve C3 as a side
effect) or stays main-thread this version. I have a recommendation; it belongs in
the substep, not here.

**Rollback note applies most sharply here.** Every background consumer changes at
once. The substep must be structured so the scheduler can be introduced with
generation as its only consumer first, meshing second, I/O third — three
independently revertible steps, not one.

---

### 6.3 Shipped — what the scheduler is, and the three things it deliberately is not

Substeps 7–8a (2026-07-28). `crates/voxulacrum-app/src/jobs.rs`.

**Delivered.** One scheduler owning dispatch and concurrency for every continuous
background consumer — generation and meshing both submit through it, neither calls
`pool.spawn`. Priority is `(class, distance²-to-camera)`, so nearest work runs
first and an interactive class lets an edit-driven re-mesh preempt bulk streaming
regardless of distance. Per-kind in-flight, completed, mean and max timings are
recorded by the jobs themselves via shared atomics. The concurrency cap has one
owner (`JobSystem::cap`), which consumers read rather than re-deriving from
`CoreBudget`.

**It fixed a live defect.** Before 8a, meshing had no priority at all:
`submit_all_dirty` filled its queue in `HashMap` iteration order, `swap_remove`
scrambled it further, and chunks failing the `has_face_neighbors` check accumulated
while later-queued chunks jumped ahead. The visible symptom was a scattering of
chunks that meshed only after everything else had. Sorting the pending set by
camera distance removed it.

**Three things it is not, each with a trigger rather than a drop:**

1. **No declared job-to-job dependencies.** The edge everyone reaches for —
   "a chunk may not mesh until its six face neighbors exist" — is a *world-state
   readiness predicate*, not a job edge: a neighbor can be resident without any
   generation job having run this session. A job-completion proxy would need
   per-coordinate completion tracked indefinitely, eviction invalidating it, and
   would still be a worse test than `has_face_neighbors`. **Trigger:** the first
   genuine job-to-job edge arrives with 0.4.0's staged cross-chunk generation pass
   (drift 3.3) — structures and rivers need a cross-chunk stage between per-chunk
   eval and finalize, which is a real ordering constraint between jobs.
2. **Chunk I/O writes stay on the main thread.** Reads already run as jobs
   (`apply_persisted_record` executes inside `JobKind::Generate` tasks and inside
   `World::generate`'s pool `par_iter`). Writes were *measured* at ~0.1 ms/chunk
   (§4.8: 3 ms for a 24-chunk autosave batch; an unload save is one chunk on the
   same path) against 3–5 ms of CPU in a 16.6 ms budget. Moving them would require
   `WorldDatabase` behind an `Arc` — which needs interior mutability for
   `dict_bytes` across five sites including the compression path, because
   `try_train_dictionary` takes `&mut self` — plus a per-chunk-key exclusion
   mechanism to stop `load(C)` reading a stale record before `save(C)` lands.
   **That hazard would be newly introduced by the move.** **Trigger:** per-layer
   save versioning at 0.6.0, or any measurement showing writes above ~1 ms.
3. **Concurrency is still statically partitioned per kind.** One global budget
   arbitrated purely by priority is the real P14 win, but consumers bound their own
   submissions at `cap(kind)`, so a global dispatch budget cannot be exercised
   without also raising those bounds — a live perf and memory change.
   **Trigger:** Substep 11, where it lands with the load-margin throughput work,
   because that gate ("streamed area fills within budget at all zooms") is the only
   metric that can tell whether the change helped.

**Roadmap consequence.** §10's exit gate needs re-wording a second time — I
rewrote it in the R3 correction to say "declared dependencies, distance-derived
priority, generation + meshing + chunk I/O as consumers, jobs introspectable,"
and items 1 and 2 of that list are now recorded as deliberately unbuilt with
triggers. The gate should read: *one scheduler; every continuous background
consumer on it; priority derived from distance to the observed region; jobs
introspectable; no second threading model.* P14's "declared dependencies" clause
is satisfied at 0.4.0, not here, and §3.4 should say so.

---

## 7. Workstream E — unification

### 7.1 The mutation-door enumeration

This *is* the deliverable, so here it is in full. Every site that mutates resident
world state, whether or not it goes through `World::execute`.

**Through the door (correct):**

| Site | Intent | Origin |
|---|---|---|
| `interaction.rs:230` | `RemoveScatter` | `Authoring` |
| `interaction.rs:240` | `PlaceScatter` | `Authoring` |
| `interaction.rs:410` | `EditVoxel` | `Play` |
| `ecs/systems.rs:347` | `PourFluidColumn` | `Authoring` |
| `ecs/systems.rs:421` | `MarkFluidDirty` | `System` |
| `streaming.rs:398` | `InsertLoadedChunk` | `System` |
| `regen.rs:57` | `SwapRegeneratedChunks` | `Authoring` |
| `world/mod.rs:421` | `apply_edit` wrapper → `EditVoxel` | `Authoring` |

**Outside the door (the audit's actual findings):**

| # | Site | What it writes | Assessment |
|---|---|---|---|
| **E-1** | `ecs/systems.rs:851–958` `seam_smoothing_system` | `chunk.data.voxels` (line 869), `fluids.cells` (905), detail layer texels + scatter instances via `remove_submerged_column_foliage` (801–811), `overrides.set_voxel` (937), `persist_dirty` (939), `seam_finalized` (941), neighbor `mark_mesh_dirty` (955) | **The largest bypass.** Documented in-source as deliberate ("generation finalization, not an actor edit", `:819–820`). Correct reasoning, wrong conclusion: `MutationOrigin::System` exists precisely for non-actor writes. Should become a `FinalizeSeam` intent |
| **E-2** | `ecs/systems.rs:402–413` `fluid_tick_system` | `commit_chunk(&mut chunk.data.fluids, plan)` and `fluids.activate(mirror)` | The *state change* bypasses the door; only the persistence bookkeeping goes through it (`MarkFluidDirty`). Deliberate ("physics, not a discrete command", `world/mod.rs:359–360`). Defensible — but it means the mutation log (A7) will show fluid persistence marks and never the fluid changes themselves. Needs a decision, not a reflex fix |
| **E-3** | `streaming.rs:483` `world.remove_chunk(pos)` | Removes a chunk from the resident set | **The door owns insert but not evict.** No `EvictChunk` intent exists. Symmetry gap; also where the streaming visualizer's eviction counter wants to hook |
| **E-4** | `streaming.rs:405–413` | Marks six face neighbors `mesh_dirty` | **Redundant** — `handle_insert_loaded_chunk` already does exactly this (`world/mod.rs:381–394`) and returns them in `outcome.mesh_invalidated`, which this caller discards. Duplicated work every inserted chunk, plus a bypass. Delete, consume the outcome |
| **E-5** | `meshing/coordinator.rs:68–70` | `chunk.mark_mesh_dirty()` over every chunk on the Remesh button | Mesh-dirty is derived render state, but the door owns `mesh_invalidated` everywhere else. Inconsistent |
| **E-6** | `persistence.rs:834–838` | `chunk.persist_dirty = false` over `values_mut` | Clears a door-owned flag from outside. Low risk; note it |
| **E-7** | `streaming.rs:327–335` | Builds `LoadedChunk`, sets `seam_finalized`, `overrides`, `persist_dirty` | Pre-insertion construction on a pool thread; the object is not yet world state. **Not a bypass** — record as such so the enumeration is complete rather than silently skipping it |

**The `System` origin is itself an audit item.** `World::execute` skips the mode gate
entirely for `MutationOrigin::System` (`world/mod.rs:182–189`). It was added in
Phase 9 as an escape hatch for streaming and sim bookkeeping. The deliverable says
"add runtime assertions on declared origin and mode" — the honest version of that
assertion has to say what `System` is *allowed* to do, or `System` becomes the
universal bypass and the gate protects nothing. Recommend: `System` remains
mode-exempt but is restricted by *intent* — a compile-time or debug-assert mapping
of which `WorldMutation` variants may carry `System`. `EditVoxel` from `System`
should be impossible.

Also note `world/mod.rs:175` still carries `#[allow(dead_code)]` on `execute` with
the comment "reachable once live call sites migrate (Substeps 1b/1c)" — stale since
Phase 8. Trivial, but it is exactly the kind of doc-vs-code untruth this version
exists to remove.

### 7.2 Drift 2.3 (eval→storage boundary) — **OPEN, confirmed**

Three conventions, unchanged since the drift review:

| Layer | Crossing | Location |
|---|---|---|
| Terrain | `StorageBoundary::materialize` — a named, documented, permanent seam | `world/storage_boundary.rs:50–59` |
| Foliage | Free functions `paint_to_detail_layers`, `scatter_to_store` | `world/world_generator.rs:300`, `:351` |
| Fluid | **No crossing point at all** — `ocean_fill` and `apply_biome_ponds` build storage-domain `FluidLayer` directly from eval outputs | `world/fluid_gen.rs:66`, `:122` |

The roadmap scopes this as "move the fluid eval→storage crossing behind
`StorageBoundary`." Note that fixing fluid alone leaves foliage as a *second*
inconsistent precedent — which is the exact complaint the drift review made ("the
next layers to generate (decals, lighting) have three inconsistent precedents to
copy from"). Folding both fluid and foliage behind the boundary costs little more
than fluid alone and actually closes the finding. I recommend doing both and saying
so.

`StorageBoundary` is currently a stateless unit struct (`storage_boundary.rs:36–37`)
modeled as a value "so any future boundary configuration has a home." This is that
future. It will need to carry the sea level and the fluid/foliage translation
context, which is a real (small) shape change, not just moving functions.

---

## 8. Workstream F — throughput and process

### 8.1 Throughput does not scale with the streamed area — **OPEN, confirmed, with two extra findings**

The mechanism `post-phase-9-audit.md` §3 identified is exactly right and still
present:

- `ChunkStreamingManager::max_in_flight` = `CoreBudget::detect().gen_in_flight`
  (`streaming.rs:214`) — derived from **CPU count**, not from radius.
- `max_gen_per_frame` = `StreamingParams` constant, default 64
  (`params.rs:587`, `:598`), used as the insertion cap at `streaming.rs:374`.
- `MeshingPipeline::max_in_flight` = `budget.mesh_in_flight` (`meshing/mod.rs:140`),
  and `MAX_SNAPSHOTS_PER_FRAME` is a hardcoded `32` (`meshing/mod.rs:174`).
- The load radius *does* scale: `bounding_radius(load_margin)` derives from
  `zoom` (`streaming.rs:138–147, 163–173`).

Two additional findings while reading it:

**F-a — `max_mesh_per_frame` is dead.** Declared at `params.rs:588`, defaulted at
`:599`, and **referenced nowhere else in the workspace** (grep returns only those
two lines). It is presumably surfaced in the params UI, so the operator has a dial
that does nothing. Either wire it as meshing's per-frame cap (replacing the
hardcoded `32`) as part of this workstream, or delete it. Do not leave it.

**F-b — `half_extents` uses `aspect` for both axes.**
```rust
// streaming.rs:138–147
let _sin_theta = (1.0_f32 / 3.0).sqrt(); // computed, then unused
let half_right_vis = self.zoom * self.aspect / CHUNK_WORLD_SIZE;
let half_up_vis    = self.zoom * self.aspect / CHUNK_WORLD_SIZE;  // aspect again
```
The doc comment directly above (`:109–110`) specifies `half_up = zoom * sin(θ) /
CHUNK_WORLD_SIZE * …`. `_sin_theta` is computed and discarded — an underscore-prefixed
leftover. The loaded region is therefore square in chunk space rather than matching
the foreshortened isometric footprint: over-loading along one axis, and the margin
derivation at `:145` (`max` of two now-identical values) is a no-op. This is a
doc-vs-code divergence sitting in the middle of the exact function the throughput fix
must touch. In scope; fix it with the throughput work rather than separately.

### 8.1b Three streaming defects found and fixed, 2026-07-28

Substeps 11a–11c. Two were on the audit's list; the third was not written down
anywhere and had been live since streaming was built.

**The throughput mechanism was misdiagnosed.** §8.1 and roadmap §10 both frame it
as "throughput constants don't scale with the radius." They don't scale — but they
also never bind: `max_gen_per_frame = 64` results/frame against ~24 chunks/sec
actually completing, and the same for meshing. The binding constraint was
**statically partitioned concurrency**: `CoreBudget` reserved `usable/3` for
generation, so on a 16-core machine generation ran 4 threads while 10 sat reserved
for meshing that was usually idle. One global budget dispatched by priority
(Substep 11a) is the fix; `CoreBudget` collapses to a single `pool_threads`.

**`half_extents` used `aspect` on both axes** (F-b). The away-axis needs
`zoom / sin(pitch)` because the isometric tilt foreshortens it —
`Camera::visible_radius` computes exactly this and documents it, so two functions
in the tree disagreed. `√3 ≈ 1.732` versus `16/9 ≈ 1.778` is why nobody noticed:
correct to within 3% at 16:9, under-loading by 30% at 4:3 (pop-in when moving
forward) and over-loading by 26% at 21:9.

**The view test ignored altitude entirely — new finding, not previously recorded.**
`is_in_view_rect` compared a chunk's *footprint* against the view band, but under
an isometric projection altitude moves geometry up-screen: a ground displacement
`d` shifts by `d·sin(pitch)` while an altitude `Δy` shifts by `Δy·cos(pitch)`, so
altitude is screen-equivalent to `Δy·√2` of ground. Columns whose footprint sat
just past the near edge could therefore have raised geometry plainly on screen and
never load — reported as "one or two chunks at the very edge that vary in height,
usually up." Fixed by testing the column's projected *span* against the band.

Column granularity is deliberate: a per-chunk Y test would be tighter, but
skipping a middle layer leaves the layer above it permanently unmeshable because
`has_face_neighbors` requires both Y neighbours resident. The cost is bounding by
the world's Y range rather than the terrain's — about 10–20% more columns.
**Operator note:** `max_chunk_y = 4` covers world Y 0–128 while meadow terrain
tops out near Y=56; lowering it to 2 roughly halves the resident set *and* shrinks
this expansion proportionally.

### 8.2 Process — versions and changelog

`Cargo.toml` says `0.1.1`. 0.2.0 was announced. `docs/CHANGELOG.md` stops at
"Phase 0". Roadmap §1 rule 6 names this as the failure that must not recur. Real,
mechanical work: reconstruct 0.1.x and 0.2.0 entries from the phase audits (which
are detailed enough to do it honestly), bump to 0.3.0 at phase close, and decide the
file's canonical location.

### 8.3 Headless generation CLI — a placement problem the roadmap does not mention

The CLI must reproduce **full** chunk generation to be a meaningful determinism
check: graph eval (`nodegraph-eval`) *plus* `StorageBoundary::materialize`,
`smooth_slabs`, `ocean_fill`/`apply_biome_ponds`, and the foliage translators — all
of which live in `voxulacrum-app`, the crate that also owns `wgpu` and `winit`.
`world/mod.rs:21` imports `wgpu::util::DeviceExt` at module level.

Three options:

1. **`[[bin]]` in `voxulacrum-app`** that touches only `world::world_generator` and
   never creates a wgpu device. Compiles the graphics stack (slow in CI, ~26 s
   incremental here) but *runs* headless fine. Zero new crates, zero new
   dependencies. **Recommended.**
2. Extract generation into a renderer-free crate. That is the 0.6.0 server-core
   split (roadmap §13), explicitly out of scope, and doing it here half-way is worse
   than either endpoint.
3. Test-only: a `#[test]` that hashes N reference chunks. Loses the "CLI" and the
   ability to dump a divergent chunk, but needs no CI binary target.

Option 1 with a recorded interim note ("lives in the app crate; migrates to
server-core at 0.6.0") is the shape that fits P7. And per §0.6, CI itself has to be
created — a `.github/workflows/ci.yml` running `cargo build`, `cargo test`, and the
determinism binary. That is a deliverable, not a footnote.

---

## 9. Re-derived drift findings — status against the current tree

| Finding | Roadmap says | Actual | Evidence |
|---|---|---|---|
| **1.4** foliage before fluid | fix in 0.3.0 | **Closed** (Phase 8), by a different mechanism than §10 words | `world_eval.rs:297–298`; `world_generator.rs:239–253`; `fluid_gen.rs:184` |
| **2.1** PoissonDisk chunk-coord seed | fix in 0.3.0 | **Closed** (Phase 8) | `context.rs:60–68`; `scatter.rs:165` |
| **2.2** mesh workers off the pool + duplicated budget | fix in 0.3.0 | **Closed** (Phase 8), both halves | `meshing/mod.rs:227`; `core_budget.rs:20–29` |
| **2.3** three eval→storage conventions | fix in 0.3.0 | **Open** | `storage_boundary.rs:50`; `world_generator.rs:300, 351`; `fluid_gen.rs:66, 122` |
| 1.1 regen destroys player work | — | Reclassified as intended §9 authoring behavior (post-8); `clear_all_chunks` still at `regen.rs:76` by design | — |
| 1.2 fluid empty-bucket save | — | Closed (Phase 8) | — |
| 1.3 fluid full-snapshot / no tombstones | 0.6.0 | Open, correctly deferred | design §9 interim note |
| 1.5 vertex format | — | Closed (Phase 9 Substep 1, `FaceVertex`) | `rendering/pipelines.rs` |
| 1.6 greedy meshing | D8 at 0.4.0 exit | Open, correctly deferred | `meshing/cube_mesher.rs` |
| 1.7 per-layer save versioning | 0.6.0 | Open, correctly deferred | — |
| 2.4 `Positions` vs `ScatterPoints` | "whichever phase next touches `nodegraph-ir`" | **Open, and this phase does not need to touch `nodegraph-ir`.** Leave it; do not manufacture a reason | `nodegraph-ir/src/pin.rs` |
| 3.1 per-voxel chunk clone | — | Closed (Phase 8, `EditVoxelBatch`) | `world/mod.rs:227–277` |
| 3.2 walkability mask | — | Moot (mask removed, Phase 9) | — |
| 5.1 mesh cache key churn | — | Open. Directly relevant to deliverable A4 ("hit/miss with key attribution") — the attribution work will surface it | `meshing/cache.rs:146`, `:219` |
| 5.6 eviction is margin-based, not LRU | 0.6.0 | Open; design §12 still says LRU+hysteresis | `streaming.rs:467–472` |

---

## 10. Where the roadmap's assumptions and the code disagree

Per the standing instruction to say so plainly rather than diverge silently. Each
needs either a roadmap correction or an explicit acceptance.

| # | Roadmap statement | Reality | Recommendation |
|---|---|---|---|
| R1 | §10 lists drift 1.4, 2.1, 2.2 as 0.3.0 work | All three closed in Phase 8 | Amend §10 and §3.2/§3.4 to cite `post-phase-8-audit.md`; the exit gate becomes "verified closed," not "closed" |
| R2 | §3.4: "mesh workers are raw `std::thread`… hand-duplicated core-budget arithmetic" | Both false since `757d5b6` | Amend §3.4 |
| R3 | Exit gate: "One job system; no second threading model anywhere" | Already true | Restate the gate as what P14 actually still lacks: declared dependencies, global priority, and introspectable jobs |
| R4 | D: "Poisson scatter continuity across chunk borders" | Architecturally false for `PoissonDisk`; the shipped content uses `PoissonDistribution` instead | Replace with seam coverage of the shipped scatter path; record why |
| R5 | D: "expose submersion to `SurfaceFilter` and scatter placement" | Shipped as a downstream filter at the storage boundary | Record the shipped shape + the migration trigger (aquatic species selection). Do not rebuild |
| R6 | §10 Process: "headless generation CLI in CI" | No CI exists | Add CI creation to the deliverable explicitly |
| R7 | §3.12 quarantines cloud hydrology, "zero engineering before 0.5.0"; §10 gates on diagnosing the hitch | If the hitch is in `climate_tick_system` (C1), these collide | Decide in advance (§14) |
| R8 | §7.3 budget: "no recurring hitch > 4 ms without an attributed cause" | Under Fifo, sub-vsync jitter is unobservable in `frame_time_ms` | The perf baseline (A + deliverable) must record CPU stage time, not just frame time, or the budget is unmeasurable |

None of these change the phase's theme. All of them change what "done" means for a
specific gate, which is exactly what §1 rule 2 says must be recorded rather than
quietly dropped.

---

## 11. Proposed substep ordering

Rationale first: **A2 (job inspector) cannot be built before C (the scheduler)**,
because there are no jobs to inspect — only closures on a pool. And **B (hitch)
cannot be settled before A1 (`FrameTimings`)**, which the post-phase-9 audit already
designed. Meanwhile D and E are independent of both and are the cheapest real wins,
which makes them good early substeps that de-risk the tree before the invasive
scheduler change.

I have therefore front-loaded verification and the small closures, put the
scheduler in the middle where a rollback costs least, and made the hitch a
two-part substep bracketing the instrumentation.

| # | Substep | Depends on | Why here |
|---|---|---|---|
| **0** | This audit | — | Done |
| **1** | **Tree hygiene + free experiments.** Resolve the uncommitted work (§14 Q1); clear the two warnings; run the two zero-cost hitch experiments from §4.4 and record results | 0 | Every later BEFORE block is anchored against a known tree. The experiments may cheaply narrow B before any instrumentation is written |
| **2** | **`FrameTimings` + Performance panel breakdown** (A1) | 1 | The post-phase-9 design, built. Must separate CPU stage time from the present block (§3). First real diagnostic and the input to every later measurement |
| **3** | **Hitch localization pass** (B, part 1) | 2 | Point A1 at the C1–C6 candidates. Output is a named owner or "external," with evidence. If the owner is C1, stop and take the §14 Q4 decision |
| **4** | **Drift 2.3 — fluid + foliage behind `StorageBoundary`** (E2) | 1 | Self-contained, no runtime behavior change, byte-identical output provable by the existing determinism test. Good confidence-builder and it closes a real finding |
| **5** | **Determinism coverage** (D3): slab-bearing guard + shipped-scatter seam coverage | 1 | Cheap, and it must precede any substep that could perturb generation output — which #4 and #7 both could |
| **6** | **Mutation-door closure** (E1): `FinalizeSeam` and `EvictChunk` intents, delete the redundant neighbor marking (E-4), origin/intent assertions, `System`-origin restriction | 5 | The enumeration is in §7.1. Determinism coverage first so seam changes are guarded. Networking prerequisite #1 |
| **7** | **Job system, part A: the scheduler with generation as sole consumer** (C) | 3, 5 | After the hitch owner is known — if the hitch is in a background consumer, the scheduler design should account for it. Independently revertible |
| **8** | **Job system, part B: meshing as a consumer** | 7 | Separate revert boundary |
| **9** | **Job system, part C: chunk I/O as a consumer** | 8 | Moves the main-thread zstd+SQLite unload save (§4.2 C3) onto the scheduler. May close a hitch candidate as a side effect |
| **10** | **Job inspector** (A2) | 9 | Now there are jobs with kinds, priorities, and timings to inspect |
| **11** | **Throughput from load-margin radius** (F1) + `max_mesh_per_frame` (F-a) + `half_extents` (F-b) | 9 | Needs the scheduler's priority model to be meaningful; scaling fixed caps without it just oversubscribes |
| **12** | **Streaming visualizer** (A3) + **memory/residency panel** (A5) | 11 | Both read the same resident-set data; one substep, two panels. A3 is also how F1's gate ("fills within budget at all zooms") gets verified |
| **13** | **Mesh cache attribution** (A4) + **mutation & event log** (A7) | 6, 10 | A7 needs the door closed (#6) to be complete; A4 is independent but small |
| **14** | **In-place determinism checker** (A6) | 5 | Regenerate a resident chunk, diff, surface the first divergent voxel. Reuses #5's comparison logic |
| **15** | **Headless generation CLI + CI** (F3) | 5, 14 | Shares the reference-chunk hashing with #14 |
| **16** | **Hitch resolution** (B, part 2) | 3, 9, 11 | Fix if fixable; characterize if external. Some candidates (C3) may already be closed by #9 |
| **17** | **Perf baseline document** | 2, 16 | Needs stable instrumentation and a settled hitch story |
| **18** | **Process:** CHANGELOG rebuild, version bump to 0.3.0, §7.4 checklist adoption, `engine-design.md` revisions | all | Roadmap §7.4 |
| **19** | **Verification sweep** → `docs/post-phase-10-audit.md` | all | Per the phase prompt |

Nineteen substeps is more than prior phases, but several (#4, #5, #13, #14) are
small and the split exists to keep each transcription package unambiguous and each
revert cheap — which matters most at #7–#9.

**If the phase must be trimmed**, #10 and #12 are the deferrable ones: they are
observability polish whose absence does not block a gate that the others do not
already cover. #13's A7 is the one I would fight to keep, because the mutation log
is the enforcement evidence for E, not just a panel.

---

## 12. Risks

**R-1 — The uncommitted tree (high, immediate).** ~1,140 lines of unreviewed 0.4.0
work under every file I will cite. If it changes mid-phase, BEFORE anchors rot.
Mitigated only by §14 Q1.

**R-2 — The hitch may be external (medium).** Fifo + a compositor + a Windows
laptop GPU is a plausible source of periodic missed deadlines that no engine change
fixes. The gate allows "root-caused with evidence," but proving *external* is harder
than proving internal: it needs CPU-stage totals well under budget on hitch frames
plus a control (windowed vs. fullscreen, different present mode, GPU vendor tools).
Budget for that possibility rather than discovering it at #16.

**R-3 — The scheduler substep touches every background consumer (medium).**
Mitigated by the three-part split (#7–#9) and by keeping `CoreBudget` as the budget
source so the revert target is well-defined.

**R-4 — Cloud-hydrology collision (medium).** §10 R7. Decide before #3 concludes,
not after.

**R-5 — Observability changes the thing it measures (low–medium).** Six per-frame
stat structures cloned into `UiState` on the existing pattern would add measurable
per-frame cost to a phase whose purpose is finding per-frame cost. Pick the shape at
#2 (§2, last paragraph).

**R-6 — `StorageBoundary` growing state (low).** It is currently a unit struct that
`WorldGenerator` holds by value (`world_generator.rs:51`). Giving it sea level and
translation context is fine, but if it ends up holding an `Arc<WorldEvaluator>` the
"deliberate, permanent seam" becomes a god object. Keep it a translator.

**R-7 — Nothing in this phase is player-visible, so regressions hide (medium).**
The determinism test, the byte-identical-output argument at #4, and the runtime
determinism checker at #14 are the only things standing between a refactor and a
silent generation change. #5 before #4 and #7 is not negotiable for that reason.

---

## 13. Exit-gate status at phase start

| Gate | Status now | Notes |
|---|---|---|
| Hitch diagnosed: fixed or root-caused with evidence | ✗ | Candidates in §4.3; framing corrected in §0.3 |
| Drift 1.4, 2.1, 2.2, 2.3 closed and verified | **3 of 4 already closed** | Only 2.3 is work. §9 |
| One job system; no second threading model | ✓ **already passes** | §6.1. Gate should be restated per §10 R3 |
| Mutation-door audit: zero write paths outside the door | ✗ | Enumeration complete (§7.1): 6 live bypasses, 2 of them deliberate and needing a decision |
| Streamed area fills within budget at all supported zooms | ✗ | Mechanism confirmed (§8.1) |
| Determinism CI job green, incl. slab and Poisson coverage | ✗ | No CI exists; slab guard genuinely missing; Poisson clause needs restating (§5.3) |
| All seven observability tools shipped and usable | ✗ | Two are partial, five are greenfield (§2) |
| Perf baseline committed | ✗ | — |
| §7.4 release checklist passes | ✗ | Version 0.1.1, changelog at Phase 0, design-doc revision uncommitted |

---

## 14. Decisions needed before Substep 1

These change what I do next; the rest I will decide myself and state.

**Q1 — The uncommitted 0.4.0 work.** Commit it as-is on `mc-revision`? Move it to a
branch and start Phase 10 from a clean `b258ad7`? Or accept the dirty tree and
proceed? I recommend **committing it** (it builds and tests green, and the
`engine-design.md` v1.8 revision in it is a genuine 0.2.0 release artifact that
should be in history regardless) with the two unused imports removed first — but
this is your call, because it commits editor work you may not consider finished.
Also: `docs/roadmap.md` is untracked and should be committed either way.

**Q2 — Substep ordering.** §11 is the proposal. The load-bearing choices are: D and
E before C; the scheduler split into three; and B bracketing the instrumentation
rather than following it.

**Q3 — Roadmap corrections.** Do you want §10's amendments (§10 R1–R8) proposed as a
transcription package now, or folded into the close-of-phase doc work at #18? I
recommend now for R1–R4, since they change what the gates mean during the phase.

**Q4 — The cloud-hydrology collision.** If instrumentation at #3 names
`climate_tick_system` as the hitch owner, what happens? Options: (a) throttle it to
a fixed tick rate — a scheduling fix, arguably not "hydrology engineering," and
cheap; (b) treat it as breaking quarantine and defer the hitch gate; (c) pull the
0.5.0 fix-or-remove decision forward. I would take (a) if it suffices, but the
quarantine is yours to lift.

**Q5 — Scope of 2.3.** Fluid only (roadmap wording), or fluid *and* foliage (closes
the finding properly, §7.2)? I recommend both.

**Q6 — `MutationOrigin::System` policy.** Does `System` stay mode-exempt with an
intent whitelist (my recommendation), or does the fluid sim (E-2) and seam pass
(E-1) get modeled some other way? This determines what the runtime assertions in
#6 actually assert.

**Q7 — Build profile for all performance work** *(added after Substep 1's measurement, §4.5)*.
Every number in §4.5 came from a debug build at opt-level 0. Options: (a) do all
perf measurement in `--release` and add a `[profile.dev]` opt-level so day-to-day
runs are usable too; (b) release-only measurement, leave dev as-is; (c) accept debug
numbers as the baseline. (c) is untenable — roadmap §7.3's 16.6 ms budget cannot
mean an unoptimized build, and the perf baseline document would be worthless. I
recommend (a).

---

*End of audit. Substep 0 proposed no code. §4.5 records Substep 1's measurement,
which superseded §0.3 and §4.1 and closed hitch candidates C2 and C5.*
