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

1. **Release build only.** `cargo run -p voxulacrum --bin voxulacrum-app --release`.
   The dev profile runs roughly 7× slower (0.3.0 measured 22 ms/frame against
   ~3 ms), so a debug reading is not a slow version of the truth, it is a
   different number entirely.
2. **Let the view settle for 15+ seconds.** `FrameTimings` aggregates over a
   one-second window, so a reading taken right after a camera move reports the
   *fill*, not the resting state.
3. **Confirm the engine is actually at rest** before reading: the job inspector
   should show `0/N threads busy` and the streaming panel `missing 0`,
   `in-flight 0`.
4. Read the Performance panel, or the two `frame:` / `stages` lines the engine
   logs once per second.

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

---

## Known scaling limits carried out of 0.3.0

| Limit | Evidence | Trigger |
|---|---|---|
| Frame cost scales with **visible** chunk count via draw-call recording | §4.9 | D8 (greedy meshing) at 0.4.0 exit, or sooner if a supported zoom breaches budget. §11's LOD text also needs revising — drift 5.4 notes it presumes a distance gradient this camera does not produce |
| ~490 MB resident GPU mesh buffers at max zoom | §4.10 | Same decision |
| `sim` stage scans the resident set unbounded each frame | §4.9 | Engine-side and in scope whenever `sim` approaches its share of the budget |
| Chunk I/O writes stay on the main thread | §6.3 | Per-layer save versioning at 0.6.0, or any measurement above ~1 ms |
| Concurrency partitioned per kind → now one global budget; **submission** limits remain per-kind | §6.3 | Only if a consumer is found starving |

## What to re-measure at 0.4.0

- The full §7.3 table on the 3-biome reference world, not the single meadow.
- Triangle and draw-call counts feeding decision D8, on that same world.
- Fill time at the declared maximum supported zoom, after any change to
  generation cost — new biomes and structures will move `generate mean` from its
  current flat 8.5–9.0 ms.
