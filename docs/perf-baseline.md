# Performance Baseline

**Status:** Living record. One section per minor version; earlier sections are
never edited, so figures stay comparable across releases (roadmap §7.4: "perf
baselines recorded and compared").
**Companions:** `roadmap.md` §7.3 (the budgets), `pre-phase-10-audit.md` §4.8–4.11
(the investigations these figures came from), `engine-design.md` §12.

---

## How to reproduce a reading

Getting this wrong invalidates the numbers, and it did once during 0.3.0 — see
"Measurement hazards" below.

### With the scripted benchmark (from M3)

Baselines and comparisons from M3 on are taken this way. A scenario in `bench/`
fixes the camera path, the input and the measured window, so a reading repeats.

1. **Conditions.** AC power, Windows Best performance mode, other heavy
   applications closed. Leave the machine alone while runs go: a run ignores the
   keyboard and mouse, but closing its window fails it.
2. **Build once, then start the built binary**, so no rebuild comes right before
   a run. The M3 baseline's one outlying S2 run followed a rebuild.

   ```
   cargo build --release -p voxulacrum-app --bin voxulacrum-app
   .\target\release\voxulacrum-app.exe --bench s1-rest-launch --runs 5
   ```

   `--bench` takes a file name in `bench/` without `.ron`, or a path to a `.ron`
   file. Append `--gpu low-power` for the integrated GPU.
3. **Five runs per scenario**, 30 s apart by default (`--cooldown <seconds>`).
   Each runs in its own process, from a freshly generated world.
4. **Compare medians, not runs.** The console table and
   `target\bench\<name>-runs.json` give each figure per run, its median and its
   spread. M3's gate compares the median of five runs with the baseline.
5. **Copy `target\bench` aside before the next set.** Every set rewrites
   `<name>.json` and `<name>-runs.json` there.

### From the Performance panel

For exploratory readings only: a panel reading is one second of one session,
not a baseline (hazard 5).

1. **Release build only.** `cargo run -p voxulacrum-app --bin voxulacrum-app --release`.
   The dev profile runs roughly 7× slower (0.3.0 measured 22 ms/frame against
   ~3 ms), so a debug reading is not a slow version of the truth, it is a
   different number entirely. Append `-- --gpu low-power` to run on the
   integrated GPU of a machine with two; the Performance panel's first line
   names the GPU every reading came from.
2. **Let the view settle for 15+ seconds.** `FrameTimings` aggregates over a
   one-second window, so a reading taken right after a camera move reports the
   *fill*, not the resting state.
3. **Confirm the engine is actually at rest** before reading: the job inspector
   should show `0/N threads busy` and the streaming panel `missing 0`,
   `in-flight 0`.
4. Read the Performance panel (F1 toggles the UI). It carries the per-stage
   figures, the job and streaming panels, and — from 0.4.0 — the
   generation-frontier sample, which has no other form.

   *Through 0.3.0 the same per-second figures were also written to the console.
   They were removed during 0.4.0: they published every second and drowned the
   log, and everything they carried is in the panel.*

**The metric that matters is `CPU worst`, not FPS or wall-clock frame time.**
Under `PresentMode::Fifo`, wall-clock frame time measures blocking and moves
*inversely* to engine cost — a faster engine waits longer in
`get_current_texture()`. `FrameTimings` reports CPU work as
`schedule span − present block` precisely so the budget is stated against work.

---

## 0.3.0 — Consolidation & Observability

**Measured:** 2026-07-28, branch `mc-revision`.

### Hardware and build

| | |
|---|---|
| Machine | Lenovo ThinkPad P16 Gen 3 |
| CPU | Intel Core Ultra 7 155H @ 1.40 GHz (engine detected 20 usable worker threads) |
| GPU | NVIDIA RTX 500 Ada Generation Laptop + Intel integrated |
| Memory | 32 GB DDR5 |
| Display | 1920×1200, ~60 Hz |
| OS | Windows 11 |
| Build | release: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1` |
| Present mode | `Fifo` (vsync) |

### §7.3 budget status

All figures at **radius 15 — the worst case inside the supported range** (see
below). Cost is monotonic in radius, so this bounds the whole range.

| Budget | Target | Measured | Status |
|---|---|---|---|
| Frame CPU, supported range | 16.6 ms sustained | **5.5 ms** at r=15 | **pass** — 3× margin |
| Recurring hitch > 4 ms | none without attributed cause | none; the reported ~28 ms at-rest hitch **did not exist** — it was a debug build plus a wall-clock metric that inverts under vsync | **pass** |
| Streaming fill after camera rest | < 2 s at any supported zoom | **177 ms for 111 chunks** at r=15 | **pass** — 11× margin |
| Memory: per-chunk resident budget | documented | **~7 KB CPU per resident chunk**, plus ~222 KB GPU per *meshed* chunk | **documented** |
| Memory: session growth | flat after warm-up | repeated round trips return CPU and GPU totals to the same range; session peak stops climbing | **pass** |
| Worst case at generation frontier | < 33 ms | **not separately measured** — frame cost during an active fill was not captured at rest-quality conditions | *open, carried to 0.4.0* |
| Frame CPU beyond the supported range | — | 31.4 ms at r=25 | over budget, and outside the range by design — see "Why the range stops at 15" |

**Reading the r=15 figures.** `render` reports a 14.49 ms stage mean against a
5.5 ms `CPU worst`, because the stage span includes the present block: roughly
11 ms of it is the CPU idling in `get_current_texture()` waiting for vsync with
5.5 ms of real work behind it. This is the clearest single illustration of why
the budget is stated against CPU work rather than frame time.

### Frame breakdown — radius 25, steady state (outside the supported range)

Recorded with `zoom_max` raised past its shipped value, so this is a diagnostic
reading rather than a baseline one. It is kept because it identifies *which*
stage grows with zoom, and therefore what LOD at 0.5.0 has to address.

4,388 resident chunks, 2,210 meshed, 1,465 visible, 4.6M triangles.

```
FPS 32   CPU worst 31.4 ms / 16.6 budget   Present mean 7.4 ms
stages mean/max ms:
  input   0.02 / 0.07
  sim     5.33 / 7.08
  mesh    0.99 / 2.55
  uniform 0.43 / 1.25
  render 24.25 / 29.89     ← 78 % of frame CPU
  post    0.01 / 0.01
```

**`render` dominates, and it is draw-call recording.** Terrain is drawn by three
passes — shadow, main scene, and the reflection pass — each issuing
`set_vertex_buffer` + `set_index_buffer` + `draw_indexed` per chunk, so ~1,465
visible chunks become ~4,400 chunk draws plus foliage and scatter, at roughly
5 µs each. The worker pool is idle (`0/20 busy`) and per-job costs are flat
across every radius measured, so generation, meshing and the scheduler are not
implicated.

**`sim` at 5.33 ms is ~4× its low-resident-set figure** (1.4 ms). Several systems
in that stage scan the whole resident set every frame with no bound —
`fluid_tick_system` sums `active.len()` across every chunk;
`param_change_detection_system` deep-clones and compares all of `EngineParams`.

### Supported zoom range — declared

**Streaming radius 6 to 15.** Enforced by the shipped camera limits: `zoom_min =
1.0`, `zoom_max = 100.0`, `initial_zoom = 40.0` (`params.rs`, `CameraParams`).
100.0 corresponds to just under radius 15, so **the engine as configured cannot
leave the supported range** — reaching wider radii requires raising `zoom_max` in
the params panel.

| | radius | note |
|---|---|---|
| Minimum | 6 | The camera can zoom in until roughly one chunk fills the view, but the streamed radius floors at 6 |
| Default on launch | 9 | `initial_zoom = 40.0` |
| **Maximum supported** | **15** | `zoom_max = 100.0` |

**Why the range stops at 15.** Not a performance limit — a legibility one. At
1920×1200, radius 15 is around the point where a voxel occupies a handful of
pixels; past it no fine detail is discernible, so rendering full voxel geometry
buys nothing. That is LOD territory, and LOD is §11 / 0.5.0. The engine still
*permits* wider zooms, and they are useful for diagnostics, but they are outside
what this version supports and outside what the budgets are stated against.

| radius | resident | visible | CPU worst | render mean | fill | verdict |
|---|---|---|---|---|---|---|
| **15** (max supported) | 1,220 | 342 / 486 meshed | **5.5 ms** | 14.49 ms (incl. ~11 ms present) | **177 ms / 111 chunks** | **pass** |
| 25 (diagnostic only) | 4,388 | 1,465 / 2,210 | 31.4 ms | 24.25 ms (incl. 7.4 ms present) | 8,704 ms / 2,619 chunks | unsupported |

Radius 15 bounds the range because frame and fill cost are both monotonic in
resident and visible chunk count, so no supported zoom is more expensive than the
row measured. The r=25 row is retained deliberately: it is the evidence for where
the range stops, and it is the measured input to decision D8.

### Job system

| kind | mean | notes |
|---|---|---|
| generate | 8.5–9.0 ms | flat across every radius measured |
| mesh | 3.5–4.9 ms | flat |
| chunk-io | — | no consumer; writes remain main-thread (~0.1 ms/chunk) |

Concurrency is one global budget of 20 dispatched by priority. The previous
static split reserved `usable/3` to generation, capping it at 4 threads while 10
sat reserved for meshing that was usually idle.

### Memory

Maximum zoom, 4,388 resident chunks:

```
29.9 MB CPU + 490.5 MB GPU     (2,396 of 4,388 chunks uniform — 55 %)
  voxel 15.6   detail 9.6   scatter 0.4   fluid 2.5   overrides 1.76   [MB]
```

**GPU mesh buffers are 16× the CPU footprint.** At a 32-byte `FaceVertex`, one
quad costs 152 bytes, so 490 MB is ≈ **3.4M quads / 6.8M triangles** across 2,210
meshed chunks — about 1,530 quads per chunk. That is one-quad-per-exposed-face:
design §10 specifies side and bottom faces as greedy-merged and drift 1.6 records
that as unimplemented.

**This is the first measured input to decision D8** (greedy meshing, due at 0.4.0
exit, to be decided "against measured triangle budgets"). It is also the same
root cause as the render finding — draw-call count and vertex volume are both
downstream of naive meshing, so D8 and the render cost are one decision.

Forward note for the 0.11.0 min-spec, whose audience is explicitly modest
hardware: 490 MB of mesh buffers at maximum zoom, or roughly 310 MB extrapolated
to mid-range, is a real constraint on a 2 GB card.

### Mesh disk cache

| | before 0.3.0 | after |
|---|---|---|
| Warm-run hit rate | 66 % | **100 %** |
| Stale (re-key) misses | ~1,400 | **0** |
| Warm-run total misses | 1,611 | **8** |

The key hashed the snapshot's edge and corner border cells, which are
load-order dependent and which the mesher never reads. Keying on interior plus
the six face planes — exactly the data meshing waits for — makes it load-order
independent by construction.

### Determinism

- Workspace suite: 281 tests green, including a slab-bearing generation test that
  fails loudly if the shipped world stops producing slabs, and scatter
  seed-derivation tests pinning world-absolute cell seeding from both directions.
- Headless CLI: 64 chunks generated twice **in parallel** and compared
  bit-for-bit, run in CI on every push. Parallel execution is the point — §12's
  determinism rules include "no thread-order-dependent generation."
- In-engine checker: regenerates resident chunks and diffs, reporting skips
  explicitly (chunks carrying overrides are not comparable).

---

## 0.4.0 — Worldgen Completeness & Authoring

*In progress. Sections are added as measurements are taken, and the comparison
against 0.3.0 — the first this project can make — is assembled at the close.*

### Generation frontier — pre-content baseline

**Measured:** 2026-07-31, branch `v0.4.0`, Phase 11 Substep 2. Same machine and
build profile as the 0.3.0 section above. **Taken deliberately before this
version's content lands**, so the post-content reading at Substep 30 has
something to compare against rather than standing alone.

This is the budget row 0.3.0 recorded as *not separately measured* and carried
forward. It was not measurable before this substep: `CPU worst` is a one-second
aggregate, a fill at the supported maximum radius lasts ~180 ms, so ~80 % of the
frames in that window are at-rest frames and the aggregate's maximum is usually
not a frontier frame at all. The sample below is conditioned on the streamed set
being unsatisfied (`missing > 0 || in_flight > 0`) and is session-scoped with an
explicit reset.

| # | Motion | CPU worst | over 33 ms | frames | CPU mean |
|---|---|---|---|---|---|
| 1 | Pan across unvisited terrain, default zoom (40) | 15.5 ms | 0 / 238 | 238 | 7.3 ms |
| 2 | Pan across unvisited terrain, max supported zoom (100 ≈ r=15) | **59.1 ms** | **17 / 701** | 701 | 13.8 ms |
| 3 | Single zoom-out, minimum → 100 | 15.1 ms | 0 / 74 | 74 | 8.2 ms |
| 4 | Cold-start fill, no reset | 15.7 ms | 0 / 21 | 21 | 10.2 ms |

**Verdict: the budget is breached, at the top of the supported range only.**
Three of four motions sit at less than half the 33 ms budget. Reading 2 breaches
it on 2.4 % of its frames.

**Worst-frame stage split, per motion (ms):**

```
        input   sim    mesh   uniform  render  post
  #1     0.0    1.7    12.5     0.0     1.4    0.0
  #2     0.0    4.7    42.4     0.3    12.0    0.0
  #3     0.0    1.2    12.5     0.0     4.1    0.0
  #4     0.0    4.3     8.6     0.1     2.9    0.0
```

**`mesh` owns the worst frame in every reading**, and in reading 2 it is 42.4 of
the 59.1 ms. Note that `FrameStage::Meshing` is not only meshing: it contains
`streaming_tick_system` (result drain and chunk insertion), `seam_smoothing_system`
(cross-chunk seam finalization, up to 16 chunks per frame), `job_pump_system`, and
`meshing_tick_system` (snapshot extraction, capped at `max_mesh_per_frame = 32`,
plus GPU buffer upload). The 0.3.0 figures show `mesh 0.99 / 2.55` at r=25, but
those were taken **at rest**, where three of those four systems have nothing to do.

**Observed by the author, and load-bearing for this version:** the frames over
budget occurred only in a region with more biome-border blending than usual;
before reaching it the worst frame never exceeded 25 ms. That matters because
this version adds biomes and zones, which increases exactly that condition — so
59.1 ms is a floor for what the post-content reading will show, not a ceiling.

### Attribution attempts, and why the first two failed

Substeps 2b and 2c split the stage, then split its dominant system. Three runs
of nominally the same motion:

| Run | CPU worst | over 33 ms | frames | worst frame's `stream` | worst frame's `upload` |
|---|---|---|---|---|---|
| 2 (initial) | 59.1 | 17 / 701 | 701 | — | — |
| 2b | 54.4 | 7 / 392 | 392 | **42.8** | 0.1 |
| 2c | 31.2 | **0 / 384** | 384 | 0.2 | **18.3** |

**The runs disagree, and the instrument is why.** `frontier_worst_stages` and
`frontier_worst_sub` snapshot the *single frame* that set the maximum. Which
frame that is varies between runs, and the two candidate spans are independently
bursty — `stream` spikes when a band of chunks evicts at once, `upload` spikes
when many meshes complete at once, and the two need not coincide. A one-frame
snapshot is a sample of size one for an attribution question. Run 2c also never
breached the budget, so its "worst frame" is not the same kind of event as run
2b's at all.

A second confounder: the motion is not reproducible. The breach was observed
only in a blend-heavy region, and whether a given pan reaches one is incidental.

**Corrected instrument (Substep 2d):** per-span maximum *and* mean across every
frontier frame, rather than one frame's split. "Which span reaches the highest
peak" and "which span holds the most time" are then separate, answerable
questions, and a single unlucky frame cannot decide either.

### Attributed — Substep 2d, 3-minute run, 2,919 frontier frames

```
CPU worst 118.2 ms / 33.0 budget — 38 of 2919 frames over    CPU mean 13.7 ms
worst frame:  input 0.0  sim 2.6  mesh 111.1  uniform 0.1  render 4.6  post 0.0

span        mean     max      (all frontier frames)
  stream      0.64  104.08
  seam        0.36   12.40
  upload      4.29   35.44
  pump        0.50   14.80
    drain       0.01    4.78
    submit      0.05    8.27
    scan        0.05    2.66
    unload      0.50  103.38
```

**Two separate phenomena, not one.**

**A — `upload` is the sustained cost.** Mean 4.29 ms, the largest of any span and
~31 % of the 13.7 ms frontier CPU mean; peak 35.44 ms. This is
`meshing_tick_system`: 34³ snapshot extraction bounded by
`max_mesh_per_frame = 32`, plus GPU buffer upload for completed meshes. It is
what a frontier frame *normally* costs, and it is the reason frontier mean CPU
(13.7 ms) is roughly double the at-rest figure.

**B — `unload` is the spike.** Mean 0.50 ms but peak **103.38 ms**, and it
accounts for essentially all of `stream`'s 104.08 ms peak, which in turn is
essentially all of the 111.1 ms `mesh` on the worst frame. Rare (mean two orders
of magnitude below peak) and enormous. This is the eviction phase: the unload
scan, `save_chunk_on_unload` (zstd + SQLite, **on the main thread**), and
`EvictChunk` through the door.

**Closed as causes:** `drain` (0.01 mean / 4.78 max), `submit` (0.05 / 8.27),
`scan` (0.05 / 2.66), `seam` (0.36 / 12.40), `pump` (0.50 / 14.80). None is
capable of the observed worst frame.

**One question the split does not answer.** Within `unload`, the per-chunk save
and the eviction itself (hash removal, GPU mesh buffer release) are not
separated. The save is the strongly-favoured mechanism — it is the only part
doing real work per chunk, and it is documented main-thread — but it is not
isolated by measurement.

**Consequence for a recorded deferral.** `jobs.rs:10–15` defers moving chunk I/O
off the main thread to 0.6.0, with the named trigger *"or any measurement showing
writes above ~1 ms."* A 103 ms main-thread eviction phase is past that trigger by
two orders of magnitude. Whether to act on it at 0.4.0 is a scope decision, not
an implementation one.

### Mitigated — Substep 2e, unload bounded by time

`StreamingParams::unload_budget_ms` (default 4.0 ms, §7.3's hitch line) bounds
the unload phase, with at least one eviction per frame guaranteed so a single
expensive chunk cannot stall eviction. A *time* budget rather than a chunk count
because per-chunk cost scales with override-bucket size, which is what differed
between regions. Chunks not evicted are reconsidered next frame.

Same 3-minute motion, 2,989 frontier frames:

```
CPU worst 62.6 ms / 33.0 budget — 15 of 2989 frames over    CPU mean 12.9 ms
worst frame:  input 0.0  sim 3.6  mesh 33.2  uniform 0.2  render 26.1  post 0.0

span        mean     max
  stream      0.18    9.16      (was 0.64 / 104.08)
  seam        0.16   11.28
  upload      3.91   45.92      (was 4.29 / 35.44)
  pump        0.50   13.18
    drain       0.01    2.18
    submit      0.05    8.96
    scan        0.05    0.92
    unload      0.05    7.09      (was 0.50 / 103.38)
```

**The targeted spike is gone.** `unload` peak fell 15×, `stream` peak 11×, worst
frame 118.2 → 62.6 ms, breaching frames 1.3 % → 0.5 %.

**The residency trade is benign.** During a sustained pan: `resident 1336`
against `wanted 1220` — one band of excess, stable rather than growing, returning
to the same figure at rest. Memory 13.3 MB CPU / 142.8 MB GPU against a session
peak of 14.4 / 150.5, i.e. flat. Fill unaffected at 153–289 ms for 116 chunks
against a 2,000 ms budget.

*(Minor: the streaming panel's `evicted` total was quoted as 24.0K mid-run and
23.2K at the end. That counter is `+=` only and cannot decrease, so one reading
is misattributed rather than the counter being wrong. Worth a glance if it
recurs; not pursued.)*

### Post-content — the first 0.3.0 → 0.4.0 comparison

**Measured** after Substep 11 authored the reference world's first wave: three
biomes across two zones, one below sea level, one carving caves through
`StandardCaveNoise`. Same machine and build profile. Radius 14 (max supported).

Two readings: **A** at rest after the startup fill (1.2 K generate samples), **B**
after a three-minute pan across both zones (10.6 K samples). B is the
representative one for per-job costs; A's sample is mostly the startup fill.

| | 0.3.0 | 0.4.0 (B) | change |
|---|---|---|---|
| `generate` mean | 8.5–9.0 ms (flat) | **9.3 ms** (max 73.9) | +4 % |
| `mesh` mean | 3.5–4.9 ms (flat) | **16.3 ms** (max 137.7) | **≈ 3.5×** |
| Fill after camera rest | 177 ms / 111 chunks | **286 ms / 116 chunks** | +54 % per chunk |
| Frame CPU worst, at rest | 5.5 ms | **4.1 ms** | better |
| Fluid layer, resident | 2.5 MB *(at r=25)* | **25.6 MB** *(at r=14)* | ≈10× at half the radius |
| CPU resident total | 29.9 MB *(r=25)* | 37.1 MB, peak 37.9 *(r=14)* | — |
| GPU mesh buffers | 490 MB *(r=25)* | 140.7 MB, peak 189.5 *(r=14)* | not directly comparable |

**The headline is that generation barely moved and meshing tripled.** Three
biomes, a second zone evaluated per chunk, and the first 3D noise in shipped
content — Worley plus a ridged fractal, per voxel — together cost about 4 % on
`generate` mean. Caves cost 3.5× on `mesh`, because a carved chunk has far more
exposed faces than a solid one. Fill absorbed both and still holds a 7× margin
against the 2 s budget.

`generate` max at 73.9 ms is worth noting against a 9.3 ms mean: the expensive
chunks are almost certainly those on a zone border, where `composite_terrain`
evaluates a layer for every present *and fade-neighbouring* biome.

### Two recorded items whose triggers fired

Both were carried out of 0.3.0 with explicit conditions. Both conditions are now
met, by this version's content rather than by any change to the systems involved.

**1. The `sim` stage.** `post-phase-10-audit.md` §4: *"`sim` stage scans the
resident set unbounded each frame … in scope whenever `sim` approaches its share
of budget."* Reading B's worst frontier frame is `sim 23.7 ms` — against 3.6 ms
in the equivalent pre-content reading, and against a 1.24 ms mean at rest. The
mechanism is not in doubt in kind: the ocean biome grew the resident fluid layer
tenfold, and `fluid_tick_system` sums `active.len()` across every chunk.

**2. The `post` stage.** Main stage table, reading B: `post 0.51 mean / 30.21
max`. `post` holds regeneration polling and the persistence autosave, and
regeneration is idle unless triggered. `engine-design.md` §9 records fluid
persistence as a full-field snapshot — *"serializes hundreds of untouched
generated cells per straddle chunk"* — and the ocean has made "straddle chunk"
the common case.

Neither is attributed below stage granularity yet, and neither is being optimised
on suspicion. Substep 12b instruments both.

**Frontier, for comparison with the pre-content reading:**

```
                      pre-content (2e)      post-content (B)
CPU worst             62.6 ms               52.4 ms
frames over 33 ms     15 / 2989  (0.5 %)    15 / 580   (2.6 %)
upload  mean/max      3.91 / 45.92          6.77 / 29.10
stream  mean/max      0.18 /  9.16          0.22 /  8.98
unload  mean/max      0.05 /  7.09          0.08 /  8.53
```

The worst frame fell, the *rate* of breaching frames rose five-fold, and the
composition changed: the pre-content worst frame was `mesh + render`, this one is
`sim 23.7 + mesh 19.0 + render 9.6`. `upload`'s mean rose with the extra geometry
caves produce — which is more evidence for **D8**, on a world that now has the
content D8 is meant to be decided against.

### The residual breach is D8's, and is recorded as an input to it

What remains after 2e is a different frame. The worst frame is now
`mesh 33.2 + render 26.1`, where `render` was 4.6 ms before — and `upload`'s peak
*rose* (35.44 → 45.92) rather than falling. Both spans are quad-volume and
buffer-churn costs: `upload` extracts 34³ snapshots and creates GPU buffers,
`render` records per-chunk draw calls across three terrain passes. The 0.3.0
section already identifies both as one root cause and assigns them to **D8**.

This changes what D8 is deciding against. Previously the evidence was 490 MB of
mesh buffers and 24.25 ms of `render` at **r=25, outside the supported range** —
easy to discount as a diagnostic-only reading. The frontier data is inside the
supported range: at r≈14, 0.5 % of frontier frames exceed the 33 ms budget, with
the cost in exactly the two spans greedy meshing would reduce.

**The frontier investigation is closed here.** Measured (Substep 2), attributed
(2b–2d), mitigated where the cause was independent (2e). The remainder is not a
streaming defect and is not fixed by bounding a phase; it is the meshing
strategy, and it is decided — not implemented — at this version's exit.

---

## M2 — Integrated-GPU baseline

**Measured:** 2026-10-20, branch `perf/integrated-gpu-baseline` (T2, #85),
commit `cd49769`. The first reading on the minimum-spec graphics roadmap §7.3
declares: the development laptop's integrated GPU, selected with
`--gpu low-power`, beside the discrete GPU from the same build.

### Hardware and build

| | |
|---|---|
| Machine | Lenovo ThinkPad P16 Gen 3, as in 0.3.0 |
| CPU | Intel Core Ultra 7 155H (20 usable worker threads) |
| Discrete GPU | NVIDIA RTX 500 Ada Generation Laptop GPU, Vulkan (the default) |
| Integrated GPU | Intel(R) Graphics as wgpu names it, the 155H's own, Vulkan (`--gpu low-power`) |
| Window | 1280×720, the launch size; display 60 Hz |
| Power | Windows Balanced power mode |
| Build | release: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1` |
| Present mode | `Fifo` (vsync) |
| World | the sandbox package from launch; no camera movement but zoom |

### Readings at rest

One reading per column, each taken after 15+ seconds with `0/20 threads busy`,
`missing 0` and `in-flight 0`. r=9 is the launch zoom; r=14 is `zoom_max`, the
widest supported radius. The r=9 fill is the startup fill; the r=14 fill is the
zoom-out from r=9.

| | discrete, r=9 | integrated, r=9 | discrete, r=14 | integrated, r=14 |
|---|---|---|---|---|
| FPS | 62 | 61 | 60 | **31** |
| CPU worst | 8.2 ms | 7.5 ms | 10.7 ms | 15.7 ms |
| Present mean | 10.7 ms | 11.0 ms | 9.3 ms | **20.0 ms** |
| Wall worst | 50.9 ms | 19.0 ms | 33.2 ms | 35.8 ms |
| `render` mean / max | 12.92 / 47.84 | 13.36 / 15.14 | 13.38 / 29.23 | 27.79 / 32.54 |
| `sim` mean / max | 3.19 / 5.90 | 2.89 / 5.00 | 2.90 / 4.26 | 3.63 / 5.36 |
| Resident chunks | 420 | 420 | 1,220 | 1,220 |
| Chunks visible / total | 67 / 124 | 67 / 124 | 283 / 412 | 283 / 412 |
| Triangles | 315.7 K | 315.7 K | 1.3 M | 1.3 M |
| Fill | 1,143 ms / 248 chunks | 1,117 ms / 248 chunks | **3,383 ms / 641 chunks** | **3,372 ms / 585 chunks** |
| Frontier CPU worst | 38.5 ms (1 of 68 over) | 42.9 ms (1 of 65) | 45.4 ms (5 of 322) | **98.3 ms (20 of 238)** |
| Frontier worst frame | `mesh 11.1 + render 25.4` | `mesh 26.7 + render 22.0` | `mesh 23.6 + render 18.9` | `mesh 32.6 + render 62.7` |
| Memory | 5.8 MB CPU + 37.7 MB GPU | same | 27.6 MB CPU + 120.9 MB GPU | same |

**At the launch zoom the integrated GPU keeps up.** Both GPUs hold vsync, CPU
work is under half the budget, and the ~11 ms present mean on both is the CPU
idling for vblank, not waiting on the GPU.

**At the widest supported zoom the integrated GPU is the limit.** It drops to
31 fps with a 20.0 ms present mean in a ~32 ms frame: the CPU finishes its work
and waits for the GPU. Under `Fifo` a frame that misses one vblank waits for
the next, so 31 fps says only that the integrated GPU's frame takes between
16.7 and 33.3 ms. Nothing in this build measures where in that range it falls.

**CPU work also rises on the integrated GPU at r=14, unattributed.** CPU worst
is 15.7 ms against 10.7, and `render` minus the present block is ~7.8 ms against
~4.1. Two plausible causes are the Intel driver's draw-recording cost, and the
CPU and the integrated GPU sharing one package power budget under Balanced
mode. Neither is tested.

**Two breaches are not the integrated GPU's.** Fill at r=14 takes 3.4 s for
~600 chunks on both GPUs, so it is generation-bound. The frontier breaches 33 ms
on both, as recorded in 0.4.0. Its worst frame is the one place the integrated
GPU makes things clearly worse: `render 62.7 ms` against 18.9.

### §7.3 budget status on the integrated GPU

Tracked, not gating until M5.

| Budget | Target | Integrated GPU | Status |
|---|---|---|---|
| Minimum spec frame rate | 30 fps at reduced settings | 61 fps at r=9, 31 fps at r=14, at default settings | **meets it at r=14 with no margin**; reduced settings don't exist yet (V2) |
| Frame CPU | 16.6 ms | 15.7 ms at r=14 | pass, 6 % margin |
| Streaming fill | < 2 s after camera rest | 3.4 s at r=14 | **breach**, on both GPUs |
| Frontier worst | < 33 ms | 98.3 ms at r=14 | **breach**, on both GPUs; 2× worse here |

### Not compared with 0.4.0

The jobs panel read `generate` 60–79 ms mean here, against 0.4.0's 9.3 ms, and
the fills above are far slower per chunk than 0.4.0's 286 ms for 116 chunks.
These readings are startup fills and a single zoom-out, the kind 0.4.0 calls
reading **A** and does not treat as representative. 0.4.0's figures are reading
**B**, taken after a three-minute pan. A reading B on the current sandbox comes
before calling the difference a regression.

### Carried forward

- **GPU time.** Under `Fifo` the panel can only bound the GPU's frame time. GPU
  timestamp queries, shown beside `CPU worst`, would measure it.
- **A reading B on the current sandbox**, to settle whether `generate` and fill
  regressed since 0.4.0, or whether this section only caught startup costs.
- **The integrated GPU's extra CPU work** at r=14, once GPU time can be
  separated from it.

---

## M3 — CPU baseline

**Measured:** 2026-10-21, branch `perf/scripted-benchmark` (T4, #91), commit
`d025a6a`. This is the baseline M3's exit gate compares singleplayer against:
v0.5.0 behavior, measured before any R2 or R4 code merges. Every reading comes
from `--bench`, not from the Performance panel.

### Conditions

| | |
|---|---|
| Machine | Lenovo ThinkPad P16 Gen 3, as in 0.3.0 |
| CPU | Intel Core Ultra 7 155H (20 usable worker threads) |
| Discrete GPU | NVIDIA RTX 500 Ada Generation Laptop GPU, Vulkan (the default) |
| Integrated GPU | Intel(R) Graphics, Vulkan (`--gpu low-power`) |
| Window | 1280×720, not resizable during a run; display 60 Hz |
| Power | AC, Windows Best performance mode (Balanced only where stated) |
| Build | release: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1` |
| Toolchain | Rust 1.92.0, pinned by `rust-toolchain.toml` |
| Present mode | `Fifo` (vsync) |
| World | the sandbox package, regenerated in `saves/bench` for every run; engine default parameters; mesh cache off |
| Runs | 5 per scenario on the discrete GPU, 3 on the integrated; each run its own process, 30 s apart |

### Scenarios

| | File | What it measures |
|---|---|---|
| S1 | `bench/s1-rest-launch.ron` | 60 s at rest at the launch zoom (r=9), after the fill and 15 s of settling |
| S2 | `bench/s2-rest-r14.ron` | The same at r=14, the widest supported zoom |
| S3 | `bench/s3-pan-r14.ron` | Four 300-unit pans east across both zones at r=14, 12 units/s, each followed by a stop that waits for the fill. Serves as reading B |
| S4 | `bench/s4-play.ron` | One fluid pour, then Play: a 119 s walk with a break or place every 0.5 s |

### Baseline: discrete GPU, median of five runs

Spread is `(max - min) / median` across the runs.

| | S1 | S2 | S3 | S4 |
|---|---|---|---|---|
| Frame CPU mean | **2.12 ms** (5.5%) | **3.01 ms** (55.0%) | **5.16 ms** (2.7%) | **2.70 ms** (9.7%) |
| Frame CPU p95 | 2.78 ms (4.7%) | 3.98 ms (95.2%) | 14.92 ms (2.6%) | 6.45 ms (40.6%) |
| Frame CPU max | 27.45 ms (24.4%) | 106.64 ms (12.1%) | 141.95 ms (14.3%) | 46.09 ms (61.8%) |
| Process CPU | 0.14 s/s (42.7%) | 0.25 s/s (52.5%) | 3.11 s/s (3.4%) | 0.49 s/s (98.5%) |
| `generate` mean | | | 84.69 ms (1.5%) | 74.47 ms (14.9%) |
| `mesh` mean | | | 0.51 ms (5.1%) | 0.46 ms (19.4%) |
| Fill mean | | | 640.16 ms (0.9%) | 448.28 ms (3.9%) |

Per run, frame CPU mean, in ms:

```
S1   2.13  2.11  2.12  2.10  2.21
S2   4.60  3.01  3.01  2.98  2.95
S3   5.10  5.24  5.16  5.14  5.21
S4   2.69  2.95  2.69  2.74  2.70
```

**Most of the large spreads come from one run each.** S2's run 1 read
4.60 ms against 2.95–3.01 ms for the other four; it started right after
`cargo run` rebuilt the binary, which is a plausible cause but untested. S1's
run 5 read 2.21 ms against 2.10–2.13 ms. The maxima are single frames and spread
as single frames do. Process CPU at rest is a fraction of one core, so small
absolute changes are large relative ones.

Only the console tables of these runs were kept: their results files were
overwritten by the integrated-GPU set before being copied. Per-run detail (fill
sizes, S4's `walked`) comes from the integrated runs below, where it is the
same path.

### S1's spread is over 2%; a revised comparison method is proposed

The issue asks for S1's run-to-run spread under 2%. On frame CPU mean it is
5.5%, set by run 5 alone; the other four span 1.4%. A spread measured as the
range of five runs is decided by the single worst run, so it will keep
failing on runs that are otherwise steady. **Proposed, for agreement before R5
closes:**

1. The gate compares the **median of five runs** of frame CPU mean against the
   baseline median above, never a single run.
2. Spread is reported as the **median absolute deviation** over the median.
   Frame CPU mean on that measure: S1 0.5%, S2 1.0%, S3 1.0%, S4 0.4%.
3. Runs start from the built binary (`target\release\voxulacrum-app.exe`) so
   that no rebuild precedes a run.

### Integrated GPU: median of three runs

Reported, not gating.

| | S1 | S2 | S3 | S4 |
|---|---|---|---|---|
| Frame CPU mean | 2.86 ms (1.9%) | 11.49 ms (1.6%) | 14.65 ms (2.1%) | 3.88 ms (3.9%) |
| Frame CPU p95 | 3.62 ms (2.1%) | 32.98 ms (1.3%) | 42.56 ms (2.3%) | 8.51 ms (6.0%) |
| Frame CPU max | 34.51 ms (10.4%) | 142.35 ms (10.2%) | 167.75 ms (4.2%) | 89.66 ms (11.7%) |
| Process CPU | 0.21 s/s (1.7%) | 0.22 s/s (15.3%) | 2.85 s/s (0.4%) | 0.52 s/s (3.2%) |
| `generate` mean | | | 114.82 ms (0.7%) | 104.76 ms (2.7%) |
| `mesh` mean | | | 1.22 ms (11.5%) | 0.81 ms (19.5%) |
| Fill mean | | | 956.61 ms (0.8%) | 706.25 ms (9.8%) |

S2 records about 1,870 frames in 60 s, i.e. 31 fps, as M2 found. S3 holds
about 32 fps across its pans.

**CPU-only work is slower while the integrated GPU renders.** `generate` runs
on worker threads and never touches the GPU, yet its mean is 114.8 ms here
against 84.7 ms on the discrete GPU, and fill is 957 ms against 640 ms. This
supports, without proving, M2's untested explanation for the integrated GPU's
extra CPU work: the CPU and the integrated GPU share one package power budget.

### S3 beside 0.4.0's reading B

| | 0.4.0, reading B | M3 baseline, S3 |
|---|---|---|
| `generate` mean | 9.3 ms | **84.7 ms** |
| `mesh` mean | 16.3 ms | **0.51 ms** |
| Fill | 286 ms for 116 chunks, one fill after the camera stopped | 640 ms mean over every fill in the window, about 100 chunks each |

The two fill figures are not like for like. Reading B is one fill after a stop;
S3 records every fill, including the many that complete while the camera is
still moving. The chunks per fill are the integrated runs' median, which take
the same path.

**`generate`: about 9× reading B, and unchanged since 0.4.1.** The job mean
measures execution only, not time in the queue, and that definition has not
changed since 0.4.0. Run on one thread, `--verify-generation 128` takes
77.6 ms per generation on v0.4.1 and 68.9 ms on this build, and both produce
aggregate hash `0xa8ea564c2743e2d8`: the same world at the same cost. (The
per-generation figure includes air chunks and startup, so it compares builds,
not chunks.) The increase therefore happened inside 0.4.0, after reading B was
taken following Substep 11 and before the release. It was not narrowed
further: v0.4.0 depends on `fastnoise2`, which needs a native toolchain to
build. It is not an M2 or M3 regression, and it does not affect M3's gate,
which compares against this baseline.

**`mesh`: about 30× below reading B.** A decrease, so not a regression, and not
attributed.

### 0.4.0's 4.1 ms and M2's 10.7 ms at r=14 at rest

| | 0.4.0, panel | M2, panel, Balanced | S2, Best performance, median of 5 | S2, Balanced, median of 3 |
|---|---|---|---|---|
| Frame CPU | 4.1 ms worst | 10.7 ms worst | mean 3.01, p95 3.98, max 106.64 ms | mean 3.08, p95 4.11, max 110.67 ms |
| `sim` mean | 1.24 ms | 2.90 ms | not kept | 1.22–1.35 ms |

**Shown to come from measurement conditions, not code.** Power mode is not the
cause: Balanced and Best performance differ by 2–3% on every figure, inside
the run-to-run noise. The instrument is. Every S2 run holds frames over 100 ms
at rest, against a p95 near 4 ms. The panel's `CPU worst` is the maximum of a
single second, so it reads near the p95 when no spike falls in that second and
anything up to ~110 ms when one does. 0.4.0's 4.1 ms and M2's 10.7 ms are two
samples of the same distribution.

`sim` on this build matches 0.4.0. M2's 2.90 ms was read on `saves/default`, an
established world, where a benchmark starts from a fresh one; that is the
likely cause, untested.

### Carried forward

- **At-rest frames over 100 ms at r=14.** S2's maximum is 100–113 ms in every
  discrete run and 141–156 ms on the integrated GPU, against a p95 under
  4.2 ms; S1's is 27–33 ms. Rare, present in every run, unattributed. The
  harness keeps no per-frame series or stage split for the frame that set the
  maximum.
- **`generate` at 70–85 ms a chunk**, from 0.4.0's development. Fill holds the
  2 s budget in S3, but M2 measured a 3.4 s startup fill at r=14.
- **S4's walk stalls about 209 voxels in**, against terrain. It stalls at the
  same place each run (the integrated runs read 208.9 three times), so
  readings compare; `walked` is the check.
- **The revised comparison method** above, to agree before R5 closes.

---

## Measurement hazards

Recorded because each one produced a wrong conclusion during 0.3.0 before being
caught.

1. **Debug builds.** ~7× slower. The workspace had no `[profile.dev]` at all, so
   every dependency compiled at opt-level 0. The famous "~28 ms at-rest hitch,
   several times per second" that gated this version turned out to be a debug
   artifact: 22 ms mean frame with 99.3 % of frames over 20 ms. Release holds a
   locked 60 fps.
2. **Wall-clock frame time under vsync.** It measures blocking, not work, and
   moves inversely to engine cost. A metronomic ~24 ms "worst frame" is the CPU
   idling in `get_current_texture()`, not a stall — the tell is its consistency.
3. **One-second aggregates read too soon.** `FPS` and `CPU worst` cover the
   preceding second. Readings taken right after each zoom step suggested frame
   time degraded 10× with resident set; steady-state figures were ~3× better.
4. **Metrics with no deficit behind them.** The first fill-time metric restarted
   its clock on any zoom change, including nudges that required no new chunks,
   mixing near-zero non-measurements into the data and making it non-monotonic.
   It now records only settles that actually opened a deficit, and reports the
   chunk count so a reading is interpretable.
5. **A single second's `CPU worst`.** At r=14 at rest, frame CPU p95 is under
   4.2 ms, yet every 60-second window also holds a frame over 100 ms. A panel
   reading of `CPU worst` reports whichever second it caught: 0.4.0 read 4.1 ms
   and M2 read 10.7 ms from the same distribution (M3 section). Compare
   `--bench` medians, not panel maxima.

---

## Known scaling limits carried out of 0.3.0

| Limit | Evidence | Trigger |
|---|---|---|
| Frame cost scales with **visible** chunk count via draw-call recording | §4.9 | D8 (greedy meshing) at 0.4.0 exit, or sooner if a supported zoom breaches budget. §11's LOD text also needs revising — drift 5.4 notes it presumes a distance gradient this camera does not produce |
| ~490 MB resident GPU mesh buffers at max zoom | §4.10 | Same decision |
| `sim` stage scans the resident set unbounded each frame | §4.9 | Engine-side and in scope whenever `sim` approaches its share of the budget |
| Chunk I/O writes stay on the main thread | §6.3 | Per-layer save versioning at 0.6.0. **Trigger armed twice over as of 0.4.0**: eviction writes (mitigated by a time budget, 2e) and autosave writes (30.21 ms, below) |
| Seam finalization persists ~every visited chunk as an override, so autosave writes 218–354 chunks per cycle and save files grow with chunks *visited* rather than *edited* | 0.4.0 §12; `post` stage max 30.21 ms; 9.7 K seam finalizations in a 3-minute run | **0.6.0**, with the format cluster. The demotions are re-derivable — `world::seam` is idempotent and the `voxel_diffs` guard, not persistence, is what protects player edits — so the fix is to stop persisting them and drop the `seam_finalized` flag, which is a `BLOB_VERSION` bump. Decided at 0.4.0 to record rather than fix: budgets are tracked and not gating until 0.5.0, and saves are wipeable through 0.5.x |
| Fluid is the largest resident CPU layer (25.6 MB at r=14, vs 6.4 MB of voxels) and is persisted as a full-field snapshot | 0.4.0 §12 memory | 0.6.0. Design §9's stated trigger — "when save-file size on ocean-heavy worlds becomes a problem" — is met now that an ocean-bearing biome ships |
| Concurrency partitioned per kind → now one global budget; **submission** limits remain per-kind | §6.3 | Only if a consumer is found starving |

## What to re-measure at 0.4.0

- The full §7.3 table on the 3-biome reference world, not the single meadow.
- Triangle and draw-call counts feeding decision D8, on that same world.
- Fill time at the declared maximum supported zoom, after any change to
  generation cost — new biomes and structures will move `generate mean` from its
  current flat 8.5–9.0 ms.
