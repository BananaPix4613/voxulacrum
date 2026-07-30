# Post-Phase-10 Audit — Consolidation & Observability (0.3.0)

Close-of-phase snapshot for **Phase 10**, which serves version **0.3.0**.
Companion to `pre-phase-10-audit.md` (the planning reference and the record of
the investigations), `post-phase-9-audit.md` (prior state), `perf-baseline.md`
(the figures), `CHANGELOG.md`, `engine-design.md` v1.9, and `roadmap.md` §10.
Branch `mc-revision`.

**Status: complete, with one exit gate passing partially by deliberate
decision** (§2, gate 3) and a short list of items carried forward open (§4). The
phase closes on that basis, not on "everything green."

The phase's prime directive was **no new player-visible features** — close the
gap between what the documents claim and what the code does, make the engine
explain itself, fix the release process. §5 records the one place that
directive was tested.

---

## 1. What Phase 10 delivered

Organized by the six workstreams the phase was planned around.

### A — Observability (7 Category-C tools)

All seven shipped, all reachable from the engine panel without a rebuild.

| Tool | Where | What it reports |
|---|---|---|
| `FrameTimings` | Performance | Per-`FrameStage` CPU mean/max, **present block isolated from work**, inter-frame gap, time outside the schedule, frames over budget |
| Job inspector | Performance | Per-kind queue depth, in-flight vs. limit, mean/max job time, completion count |
| Streaming visualizer | Performance | Resident/visible/meshed counts, radius, evictions, missing and in-flight, and fill time for the last settle that actually opened a deficit |
| Cache instrumentation | Meshing Pipeline → Disk Cache | Hit rate with **miss attribution split cold vs. stale**, error count, on-disk file count and size |
| Memory / residency | Performance | Per-layer CPU bytes (voxel, detail, scatter, fluid, overrides), GPU buffer total, uniform-chunk ratio, session peak |
| Determinism checker | Performance | Button-triggered regenerate-and-diff over resident chunks; reports first divergent voxel with both values, and skips explicitly |
| Mutation & event log | Performance | Rejections split by reason (wrong-mode vs. system-origin), per-intent cell counts, and a scrollable list of recent actor commands with origin |

Two notes on shape rather than delivery:

- The mutation log's specified "emitted events" column has **no referent yet** —
  the world-mutation event bus is §6.7, unbuilt. The log records origin, mode,
  intent and resulting invalidation (mesh / scatter / water). The events column
  arrives with the bus.
- The mutation log was first built as an inline list that pushed the rest of the
  panel down on every expansion. It is now a fixed-height scroll area, on the
  principle that a diagnostic which disturbs the thing you are reading is not
  usable.

### B — Hitch diagnosis

**The hitch did not exist.** The phase was planned around a reported ~28 ms
at-rest stall recurring several times per second. It was two stacked measurement
artifacts:

1. A **debug build** — the workspace had no `[profile.dev]` at all, so every
   dependency compiled at opt-level 0. Roughly 7× slower: 22 ms mean frame with
   99.3 % of frames over 20 ms. That distribution is the tell — a hitch is a
   tail, and this was the whole curve.
2. **Wall-clock frame time under `Fifo`**, which measures blocking rather than
   work and moves *inversely* to engine cost. A `PresentMode::Immediate` A/B
   settled it: night rendering, the supposedly expensive case, measured 22 %
   *faster*.

Release build, at rest: locked 60 fps, 3–5 ms CPU against a 16.6 ms budget.

Delivered as fixes rather than findings: `[profile.dev] opt-level = 1` with
dependencies at 3, and `FrameTimings` reporting CPU as
`schedule span − present block` so the budget is stated against work.

Eight candidate causes were closed by measurement rather than by argument, and
are recorded individually in `pre-phase-10-audit.md` §4.8–4.11 — including
`climate_tick_system`, the top-ranked suspect, which exonerated the cloud-
hydrology quarantine collision.

### C — Unified job system (P14)

`jobs.rs`: one scheduler between the submitters and the shared rayon pool.
`JobKind` (Generate / Mesh / ChunkIo), `Priority { class, distance }` with
interactive work ordered ahead of streaming and ties broken by distance², one
global concurrency budget dispatched by priority, and jobs as inspectable
objects carrying a kind and timings rather than opaque closures.

The previous static split reserved `usable/3` to generation — capping it at 4
threads while 10 sat reserved for meshing that was usually idle.

Two clauses of the gate are deliberately unmet; see §2 gate 3.

### D — Correctness and determinism (P1)

- The pre-existing determinism test was **vacuous** — it asserted on chunk
  `(1,0,2)` with nothing establishing the chunk contained slabs. It now has a
  slab-bearing guard that fails loudly if the shipped world stops producing
  slabs.
- Scatter seed-derivation tests pin world-absolute cell seeding from both
  directions.
- `--verify-generation [N] [--seed S]`: headless, generates N chunks twice **in
  parallel** and compares bit-for-bit. Parallel execution is the point —
  §12's determinism rules include "no thread-order-dependent generation."
- CI (`.github/workflows/ci.yml`) runs build, test, and verify-generation on
  every push.

### E — Unification (mutation door + drift 2.3)

- Drift 2.3 closed: `StorageBoundary` became stateful, owning `sea_level` and
  materializing voxels, fluid **and** foliage.
- The door gained `EvictChunk`, `FinalizeSeam`, and `CommitFluidPlans`, and
  `MarkFluidDirty` was removed — plan/commit replaced dirty-marking.
- `execute` now gates on origin with a System-intent whitelist, so a system
  cannot issue an authoring mutation.
- Drift 1.4, 2.1 and 2.2 were verified closed against the tree (they had landed
  in Phase 8) rather than rebuilt, with evidence recorded in `roadmap.md` §3.2.

### F — Throughput and process

- **Mesh disk cache**: the key hashed the snapshot's edge and corner border
  cells, which are load-order dependent and which the mesher never reads. Warm
  runs were re-keying ~1,400 chunks. Keying on interior plus the six face
  planes — exactly the data meshing waits for — makes it load-order independent
  by construction. Warm-run hit rate 66 % → **100 %**.
- **Startup data loss**: `World::generate` never applied persisted overrides,
  and `WorldPersistence::open` ran 121 lines *after* world generation. Player
  edits were silently discarded on every launch.
- **Graph hot-reload**, two defects (see §3).
- Process: `CHANGELOG.md` reconstructed with real 0.1.0 / 0.1.1 / 0.2.0 entries
  and a full 0.3.0 entry; workspace version corrected 0.1.1 → 0.3.0;
  `perf-baseline.md` established as a living per-version record; `roadmap.md`
  §7.5 branch-and-tag discipline adopted; `engine-design.md` revised to v1.9.

---

## 2. Exit gates — verdict

| # | Gate | Verdict | Evidence |
|---|---|---|---|
| 1 | Hitch diagnosed: fixed or root-caused with evidence | **pass** | Root-caused by disproving the premise. `pre-phase-10-audit.md` §4.1/§4.8–4.11; `perf-baseline.md` "Measurement hazards" |
| 2 | Drift 2.3 closed (fluid **and** foliage behind `StorageBoundary`); 1.4, 2.1, 2.2 verified closed | **pass** | `storage_boundary.rs` materializes all three; `roadmap.md` §3.2 records the verification of the other three against the tree |
| 3 | One scheduler: declared dependencies, distance-derived priority, gen + mesh + chunk I/O as consumers, jobs introspectable | **partial — by decision** | See below |
| 4 | Mutation-door audit: zero write paths outside the door | **pass** | Audited this session; see below |
| 5 | Streamed area fills within budget at all supported zooms | **pass** | 177 ms / 111 chunks at r=15 against a 2 s budget — 11× margin. Cost is monotonic in radius, so r=15 bounds the range |
| 6 | Determinism CI job green, including slab and Poisson coverage | **pass, Poisson clause by substitution** | See below |
| 7 | All seven category-C diagnostics shipped and usable | **pass** | §1A |
| 8 | Perf baseline committed | **pass** | `perf-baseline.md` |
| 9 | §7.4 checklist passes | **pass on completion of the merge** | §6 |

### Gate 3 — what is not built, and why

Three of five clauses pass: distance-derived global priority, jobs
introspectable, no second threading model. Two do not:

**Declared job-to-job dependencies — not built.** There is no consumer at this
version. The dependency that would justify the machinery — a chunk cannot mesh
until its six neighbours are generated — is currently expressed by the mesher
simply not submitting until the snapshot is complete, which is correct and
costs nothing. Building a dependency graph with one implicit edge and no second
edge to generalize from would be designing against an imagined consumer.
**Trigger:** the staged cross-chunk generation pass at 0.4.0, where structures
and rivers make genuine multi-stage ordering real. Recorded at `jobs.rs:10–15`.

**Chunk I/O as a consumer — not built.** `JobKind::ChunkIo` exists but has zero
submitters; unload-time zstd + SQLite writes remain on the main thread. They
measure ~0.1 ms/chunk, so the move buys nothing now, and moving writes off the
main thread introduces a save/load ordering hazard — a chunk can be re-requested
while its write is still queued — that wants the per-layer save versioning work
rather than a bolted-on guard. **Trigger:** per-layer save versioning at 0.6.0,
or any measurement above ~1 ms.

Both omissions are deliberate, both are recorded in the code with their
triggers, and neither is silent. The gate is marked partial rather than passed
because the roadmap wrote it as five clauses and only three hold —
`roadmap.md` §10's gate text should be amended to match what shipped rather than
the audit claiming a pass it did not earn.

### Gate 4 — mutation-door audit, method and result

Every mutating call site was enumerated and classified. Voxel writes
(`set_voxel` / `voxels_mut` / `set_material` / `overrides_mut`) resolve to:

- **2 door handlers** — `handle_edit_voxel_batch`, `handle_finalize_seam`.
- **1 pure reconstruction function** — `apply_overrides_to_storage`, which
  clones a base and replays an already-committed diff during load. It produces a
  new `ChunkStorage` rather than writing resident state.
- **1 generation-time function** — `slab_smoothing`, operating on storage before
  the chunk is resident. Generation is not mutation.
- **5 test fixtures**, all inside `#[cfg(test)]`.

From outside `world/`, every world write goes through
`execute(MutationCommand::…)`: `PourFluidColumn`, `CommitFluidPlans`,
`FinalizeSeam`, `RemoveScatter`, `PlaceScatter`, `EditVoxel`. The single
mutable-chunk access outside the door — `meshing/coordinator.rs:71`,
`mark_mesh_dirty()` on a Remesh button press — sets render state, not content.

**Result: zero content-write paths outside the door.**

### Gate 6 — the Poisson clause

Poisson *continuity across chunk borders* is not tested, and is not scheduled.
`PoissonDisk`'s point set is per-chunk by construction: Bridson is a single
sequential walk, so margin-band points differ across a border even though the
seed is world-absolute. True seam continuity needs a world-tiled Bridson.

Coverage went instead to **the scatter path the shipped content actually
uses** — `PoissonDistribution`, a world-cell jittered grid that is
seam-continuous by construction — plus determinism tests on `PoissonDisk` and
`PoissonPlacement` themselves. This was a conscious substitution, made in
planning (`roadmap.md` §10) rather than discovered at the gate: a test of the
path players see is worth more than a test of an unwired node.

281 tests pass, 0 fail, across 14 test binaries.

---

## 3. What the phase found that it did not set out to find

Three defects surfaced from investigation rather than from the plan. All three
are the same species: a mechanism that was correct for the case it was written
for and silently wrong one case over.

**Tag-driven invalidation is backward-looking.** `InvalidationTarget::matches`
tests the tags of the *previous* generation to decide what the *next* one
affects. That is sound for Biome and Detail edits, where a chunk's biome
assignment is unchanged and its old tags still identify it. It is unsound for
**Zone** edits, which change the assignment itself: a chunk that is about to
become Rocky is still tagged Meadow, so it does not match and is never
regenerated. Fixed by routing `GraphSlot::Zone` to `InvalidationTarget::AllChunks`.
The constraint is now recorded in `engine-design.md` §4 as a bound on the
invalidation table, because it will re-appear for any future graph tier that
changes assignment rather than content.

**Hot reload mixed in-memory and on-disk state.**
`from_manifest_with_override` loaded every graph from disk and then swapped one
in, producing a hierarchy that was part editor and part file — which is what
users hit when editing World or Zone graphs at 0.2.0. Compounding it, the
watcher was hardcoded to biome 0, so an edit to any biome regenerated twice.
Resolved by a rule rather than a patch: **hot reload is save-triggered, disk is
authoritative for the world, and the editor is authoritative for its canvas.**
That rule made `HierarchyEditor::refresh` and `consume_dirty` obsolete — the
watcher pushing a disk graph onto the canvas *was* the defect — and both were
deleted rather than left dead.

**The mesh cache key was input-addressed where it should have been
content-addressed.** Covered in §1F. Worth restating as a principle: the key
must hash exactly the data the consumer reads, no more. Hashing the surrounding
border cells made an otherwise-pure function load-order dependent.

A fourth, smaller: the first fill-time metric restarted its clock on any zoom
change, including nudges requiring no new chunks, mixing near-zero
non-measurements into the data and making it non-monotonic. **A metric with no
deficit behind it is not a measurement.** It now records only settles that
actually opened a deficit, and reports the chunk count so a reading is
interpretable.

---

## 4. Deferred and open — triaged, nothing dropped

| Item | Status | Trigger / owner |
|---|---|---|
| `render` is 78 % of frame CPU at r=25 (24.25 ms), from draw-call recording across three terrain passes | Open, **outside the supported range** | Decision **D8** (greedy meshing) at 0.4.0 exit. Same root cause as the row below — one decision, not two |
| ~490 MB GPU mesh buffers at max zoom (≈3.4 M quads, one quad per exposed face) | Open, same cause | D8. Also flagged forward to the 0.11.0 min-spec, whose audience is modest hardware |
| `sim` stage scans the resident set unbounded each frame (5.33 ms at r=25 vs 1.4 ms small) — `fluid_tick_system` sums `active.len()` over every chunk; `param_change_detection_system` deep-clones all of `EngineParams` | Open | Engine-side, in scope whenever `sim` approaches its share of budget |
| "Worst case at generation frontier < 33 ms" | **Not measured** | Carried to 0.4.0. Frame cost during an active fill was never captured under rest-quality conditions |
| Declared job dependencies | Deliberately omitted | 0.4.0 staged cross-chunk generation |
| Chunk I/O writes on the main thread | Deliberately omitted | Per-layer save versioning at 0.6.0, or >1 ms measured |
| Poisson seam continuity | Not scheduled | Needs a world-tiled Bridson. Unwired in shipped content |
| Mutation log's "emitted events" column | No referent | The event bus, §6.7 |
| §11 LOD text presumes a distance gradient this camera does not produce (drift 5.4) | Open | Revise alongside D8 |
| Hierarchy / manifest editor at T1 | On schedule | T2 at 0.4.0 |

Nothing on this list is new debt created by Phase 10. Every row is either a
pre-existing item now *measured* for the first time, or a deliberate omission
with a named trigger.

---

## 5. The prime directive, tested once

"No new player-visible features, except fixing an existing defect a player would
notice."

The phase touched rendering exactly once. The octave ladder in
`compute_render_dimensions` never allowed minification — as the camera zoomed
out past the point where a world pixel fell below screen resolution, the ladder
avoided downsampling and the image began upscaling in the wrong direction. That
is a defect a player sees, so it qualified under the exception. The threshold
(`s < 0.85`) was chosen by the author against the shipped zoom range, and
`perf-baseline.md`'s r=15 figures were re-checked against it: both the old and
new thresholds land on `k=4, s=1.5` at zoom 100, so the baseline stands. Default
zoom 40 now stops at `k=16, s=0.9375`, roughly 4× the fragment work at a
resolution where the budget has 3× margin.

No other rendering, audio, entity, VFX, biome, node, region-graph or tick-
framework work was done. The cloud-hydrology quarantine held — and was
vindicated when `climate_tick_system`, the top-ranked hitch suspect, measured
≤2.2 ms and was cleared.

---

## 6. §7.4 release checklist

| Item | Status |
|---|---|
| Workspace version bumped | ✔ 0.1.1 → 0.3.0 |
| CHANGELOG entry written | ✔ plus reconstructed 0.1.0 / 0.1.1 / 0.2.0 |
| Close-of-version audit committed | ✔ this document |
| Design-doc revision for newly-settled architecture | ✔ `engine-design.md` v1.9 |
| Forcing-date decisions due this version recorded | ✔ **none due** — D1–D8 all fall at 0.4.0 or later |
| Determinism suite green | ✔ 281 pass / 0 fail, plus headless verify in CI |
| Perf baselines recorded and compared | ✔ recorded. **Compared: not possible** — 0.3.0 is the first baseline. Comparison begins at 0.4.0 |
| Tooling-maturity table updated | ✔ Category C rows marked shipped |
| Open-defect list triaged, nothing silently dropped | ✔ §4 |
| Version branch merged to `master`, annotated tag pushed | **pending** — the merge is the release (§7.5) |

The merge and the retroactive tags (`v0.1.0` at `f71aa07`, `v0.1.1` at
`6234839`, `v0.2.0` at `b258ad7`, `v0.3.0` at the merge commit) are the last
action of the phase, taken after this audit is committed.

---

## 7. State entering 0.4.0

**What is solid now that was not before.** The engine explains itself: seven
diagnostics, all reachable without a rebuild, all reporting against stated
budgets rather than raw numbers. Performance is *measured* rather than
believed — and the phase's central lesson is that the previous belief was
wrong in both direction and magnitude. There is one scheduler, one clock, one
mutation door with an audit behind the claim, and a release process where "done"
has a checklist that blocks something.

**What 0.4.0 inherits.** A generation architecture that is settled and a content
surface that is thin: three biomes, one with a detail graph. The cross-chunk
infrastructure that structures and rivers need does not exist yet, and it is the
same infrastructure that makes declared job dependencies worth building — so
workstream C's omission and 0.4.0's first deliverable are the same piece of
work approached from two sides.

**The measurement to take first.** D8 is due at 0.4.0 exit and wants triangle
and draw-call counts on the 3-biome reference world, not the single meadow the
0.3.0 figures were taken over. New biomes and structures will move
`generate mean` off its current flat 8.5–9.0 ms, so fill time at the declared
maximum supported zoom needs re-measuring after content lands rather than
before.
