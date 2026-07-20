# Post-Phase-9 Audit — Player Character (Networking-Aware, Single-Player)

Close-of-phase snapshot for **Phase 9 (Player Character)**. Companion to
`pre-phase-9-audit.md` (planning reference), `post-phase-8-audit.md` (prior
state), `engine-design.md` §9/§11/§12, and `underground-visibility-design.md`
(the in-phase design revision that superseded the original Substep 9 plan).
Branch `mc-revision`.

**Status: complete**, with two perf items explicitly carried forward open (see
§4) rather than fixed — the phase closes on that basis, not on "everything
resolved."

## 1. What Phase 9 delivered

Substeps 0–8 landed largely as planned in `pre-phase-9-audit.md`; Substep 9
(camera occlusion) grew far beyond its original scope into a full design
revision. Substeps 10–13 follow.

- **0 — Audit.** `pre-phase-9-audit.md`; no code.
- **1 — `FaceVertex` migration.** 64-byte baked-color `TerrainVertex` replaced
  by the 32-byte packed format (i8 normal, `face_axis`, and the other §10
  fields), landed before any player rendering work per the drift-8.5/1.5
  deferral.
- **2 — Walkability residency, then removed.** Shipped first as a resident
  per-voxel bitmask (design's original shape). This caused the chunk-Y seam
  defect ([[project_walkability_chunk_y_seam]], fixed in 2b via
  `world::seam`'s cross-chunk finalization) and a streaming performance
  regression ([[project_chunk_streaming_perf]]). Mid-phase, after the player
  reported movement/collision was badly broken (tunneling at speed, drift,
  the mask "making less and less sense for this application"), a separate
  agent session **removed the walkability mask entirely**
  (`world/walkability.rs` deleted) in favor of direct geometric
  `WorldView::solid_interval` queries consumed straight from collision and
  room detection. The two candidate perf causes tied to the mask
  (`walkability_recompute_system`, the per-chunk 32³ mask build) are moot as
  of this audit — confirmed by grep, not assumed.
- **3 — Semantic action layer.** `GameAction` enum, `InputMap`/`input.ron`,
  edge/held/continuous trigger modes, the explicit `TogglePlayMode` (F5)
  Authoring⇄Play transition.
- **4 — Player sim.** Pure `player_step(state, input, &world) -> state`,
  fixed 30 Hz `PlayerClock`, double-buffered `prev`/`curr` for interpolation.
- **5 — Collision + auto-step.** Slab-aware AABB resolution and geometric
  step-up. Flagged mid-phase as broken alongside the walkability mask; fixed
  in the same separate-agent pass as the mask removal. User-confirmed
  working; not independently re-verified in this session (explicit
  direction — see §4).
- **6 — Camera follow.** Interpolated follow target from the fixed-tick
  buffer, pixel-snapped camera transform.
- **7 — Player render + shadow.** Avatar rebuilt from an initial box-based
  form (rejected on sight) to a capsule + visor + circular drop-shadow
  (`rendering/player_pass.rs`). Shadow bias/slope-scale issues from this
  session's own attempts were **not** resolved by this session (both fixes
  attempted here failed); fixed by the same separate-agent pass as movement.
- **8 — Tri-tonal axis lighting.** `face_axis`-driven `tri_tonal_factor` in
  `terrain.wgsl` (top/side-X/side-Z/bottom ramps), consuming Substep 1's
  vertex attribute.
- **9 — Camera occlusion + room detection.** By far the largest substep;
  superseded its own original plan twice. See §2.
- **10 — Player stencil.** Started as a depth-fail silhouette draw (`depth_compare:
  Greater`), which correctly showed the player through terrain but couldn't
  also stay under foliage correctly (foliage occlusion vs. terrain occlusion
  need opposite priority). Rebuilt as a real two-pass stencil: a pre-foliage
  mark pass (writes stencil where the avatar is behind terrain) and a
  post-foliage composite pass (stencil-masked, depth-test off). Required
  migrating the scene depth format from `Depth32Float` to
  `Depth24PlusStencil8` across every pipeline drawing into the main scene
  pass (terrain, wireframe, detail-paint, water, scatter, cap, debug lines,
  player) plus adding a depth-only sampling view for the outline/post-process
  passes that read depth as a texture.
- **11 — Targeting.** Reach-limited (`PLAYER_REACH` = 4.5 voxels) break/place
  via the existing cursor-ray DDA, extended to also track the face-adjacent
  empty cell for placement, wired through `MutationCommand::play`. Three
  distinct bugs found and fixed here:
  - Edits didn't render until an unrelated camera/streaming event — the
    mutation API's own contract (caller submits `mesh_invalidated` chunks to
    the mesher) was dropped by the new call site; fixed by making
    `MeshingCoordinator::tick` call `submit_all_dirty` unconditionally every
    frame instead of relying on scattered call sites remembering to.
  - `seam_finalized` was never persisted, so the cross-chunk seam pass
    re-derived boundary demotions from scratch on every chunk reload —
    including reloads of chunks the player had already edited near a border —
    silently reverting placed/broken blocks there. Fixed by persisting
    `seam_finalized` (bumping `BLOB_VERSION` 6→7) and making the seam pass
    both override-aware (never overwrite a player-edited index) and
    persist its own demotions through the same override/diff mechanism, so
    skipping the pass on a later load loses nothing.
  - `room_detection_system`'s cheap overhead pre-filter used a fixed
    radius-6 column sample, tuned for caves (where solid rock extends far in
    every direction); a small player-built room's footprint is mostly outside
    that sample, diluting the coverage ratio and wrongly skipping the flood
    entirely. Fixed by shrinking the pre-filter to radius 0 (a direct
    "is there any ceiling above me at all" check), which removes the
    room-size dependency rather than just retuning it for one size.
- **12 — HUD + tool selection.** `PlayerToolState` cycles the build material
  on the previously-unwired `CycleTool` action, replacing Substep 11's
  hardcoded placeholder material. Reach-aware highlight coloring (yellow
  in-reach / grey out-of-reach) surfaced a real gating bug in the process:
  `picking_system` (Authoring's dev-tool pick) was never mode-gated, so it
  ran in Play mode too and unconditionally overwrote the shared highlight
  with its own fixed yellow — fixed by gating it to `EngineMode::Authoring`.
  A cursor-following crosshair was prototyped and then removed at the user's
  request ("too awkward"); the minimal bottom-left material-swatch readout
  shipped.
- **13 — Verification sweep.** This document, plus the perf investigation in
  §4.

## 2. Substep 9's actual shape (superseded its own plan twice)

The original plan was a clip-box cutaway (`clip_enabled == 2`, discard a
column above the player's head) plus a binary overhead-coverage room
detector. Both were replaced before the phase closed:

1. **Room detection went from binary to threshold-based** after the binary
   flood-escape formulation rejected any room with so much as a doorway to
   daylight. Replaced with `overhead_coverage` (a leniency pre-filter) and
   later a full `flood_visibility` (bounded 6-connected through-air flood
   yielding an undergroundness `u ∈ [0,1]`).
2. **The clip-box cutaway was replaced entirely** by `underground-visibility-design.md`
   (v1 then v2), after the clip box was shown to violate its own requirement
   under an angled isometric camera — "reveal by deletion" exposes whatever
   is *behind* the deleted rock, including the surface world. v1's cut-set
   sweep (mark occluders, cap them) was itself replaced in v2 by the
   **air-side face rule**: every terrain face fronts a well-defined air cell
   (`floor(wp + 0.5·normal)`); a face discards unless the player has reached
   that air. This is structural, not heuristic — R3 (no surface leakage)
   becomes a property of the classification rather than a rule the
   implementation must remember to enforce.
3. **The air-side rule alone under-culled nearer geometry.** A face fronting
   *reached* air still passed even when a nearer chamber or the ground around
   a cave mouth stood between the camera and the player's actual position.
   Fixed with a **cost-priority march**: the mask stores flood-cost+1 per
   cell (not a flag), and a face discards if any reached air *closer to the
   player* sits behind it along the camera's view direction — this reinstates
   a camera-relative term the v2 note had argued away, because the owner's
   stated invariant ("maximum visibility of the space the player occupies")
   is inherently camera-relative.
4. **A clarity radius** bounds what actually renders in color (vs. what the
   flood merely "knows about") — otherwise a cave mouth's wide-open exterior
   floods too and renders in full color alongside the room itself.
5. **The "rest of the world" presentation** went through several redesigns
   per direct feedback: dark-gray void → per-voxel wireframe (rejected, "not
   what we had before") → opaque gray "clay" material (rejected, "should
   blend with the void, not look like clay") → near-black material color
   matched to the background. The exclusion geometry went from a world-space
   radius (rejected — "no application ever uses world-space for this,
   vertical displacement always breaks it") to a pair of **screen-space
   circles**, sized from the flood's actual world-space radius and
   re-projected every frame (so it tracks both room size and camera zoom),
   with a second, larger circle offset downward (not scaled-in-place, to
   avoid a tangent tear at the top) carving a deliberate "nothing renders
   here" margin below the volume — encoding the specific rule "void never
   subtracts from the volume; the volume's own occlusion sweep decides what's
   in front of it."
6. **Slab-specific culling defects**, found only once real cave geometry with
   slopes existed to test against: the air-side sample's 0.5-cell offset put
   a slab-top's sample on the wrong side of its own cell boundary (fixed:
   0.25-cell offset); and the cost-priority march treated a slab's own
   half-solid backing as "free space it occludes," culling slab side faces on
   slopes (fixed: a mask high-bit flags slab cells so the march skips them as
   neither solid nor free).
7. **Cave content**: the shipped meadow test caves were **not** produced by
   this session's code — after the library-kernel route
   (`StandardCaveNoise`, blocked on a `sample_density` gap for `LibraryRef`,
   see §4) was explicitly deferred ("needs more time to cook"), the user
   authored a workable cave structure directly in the graph JSON from
   suggested noise/transform node shapes, off-session.

## 3. Perf investigation (Substep 13)

Ran per the memory note deferring streaming-perf profiling to this audit.
Method: direct code reading and structural reasoning (no wall-clock profiler
available through this session's tools), plus one live A/B experiment the
user ran in-engine.

**Corrected from a 6-day-stale memory note** — two of its four candidates no
longer exist:
- `walkability_recompute_system` and the per-chunk 32³ walkability mask in
  `LoadedChunk::new` are gone (§1, Substep 2's mask removal). Confirmed by
  grep, not assumed.

**Ruled out by live experiment:**
- The seam re-mesh cascade (the memory's most-suspect candidate —
  `seam_smoothing_system` marking six neighbors dirty per demotion,
  potentially waving across a generation frontier) is **not** the dominant
  cause of the fresh-chunk-generation frame-rate drop. Disabling the
  neighbor-remesh-marking block and re-testing did not resolve the reported
  55→40 fps / 30→20 fps drops when moving into newly-generating terrain.
  (The experimental disable was reverted after the test.)

**Ruled out by code reading:**
- Shader/graph hot-reload polling: both drain a `notify`-based filesystem
  event channel (`rx.try_recv()`), not stat-based polling — near-zero cost
  when nothing changed.
- The 30-second autosave sweep: real work (dirty-chunk scan, per-chunk zstd
  compression, SQLite write) but far too infrequent — the reported at-rest
  hitch recurs multiple times per second, two orders of magnitude faster
  than the autosave timer.
- `seam_finalized` persistence (Substep 11) incidentally reduces
  `seam_smoothing_system`'s per-frame scan cost for *reloaded, previously-
  visited* terrain (most candidates now short-circuit immediately), but does
  nothing for fresh generation, where the flag is unavoidably `false`.

**A real, confirmed, actionable finding — distinct from either of the above:**
- `ChunkStreamingManager::half_extents` correctly scales the streaming
  *radius* with camera zoom. But the generation/meshing *throughput*
  (`max_in_flight`, `max_gen_per_frame`) are fixed constants, independent of
  how many chunks that radius now covers. At high zoom-out, the system
  correctly identifies a much larger area to keep loaded but can only push a
  fixed number of chunks through generation/meshing per frame regardless —
  this is the mechanism behind the reported "edges of the screen don't stay
  filled at some zoom levels," and it reproduced specifically on a
  single-biome (non-blending) area, ruling out per-chunk biome-blend cost as
  the explanation.

**Open — not resolved this phase:**
- The frequent (multiple-times-per-second) ~28 ms at-rest frame hitch has
  **no identified cause**. Every unconditional per-frame system found by
  reading the schedule was checked and ruled out (above); nothing else
  running every frame regardless of state has been identified as a candidate.
  Coarse per-`FrameStage` timing instrumentation (a `FrameTimings` resource
  plus six marker systems surfaced as a live breakdown in the Performance
  panel) was designed to localize it but was **not implemented** — deferred
  at the user's request to close out the phase. This is carried forward, not
  fixed.
- The zoom-vs-throughput scaling gap above has a clear mechanism but no
  fix implemented yet.

## 4. Deferred / open items

- **At-rest frame hitch** (~28 ms, several times/sec) — cause unknown; next
  step is the `FrameTimings` instrumentation described in §3 (fully designed,
  not built).
- **Generation/meshing throughput doesn't scale with zoom** — needs
  `max_in_flight`/`max_gen_per_frame` (or an equivalent) to scale with the
  load-margin radius rather than stay fixed; currently causes edge chunks to
  lag at high zoom-out.
- **Seam re-mesh cascade mechanism** — ruled out as the dominant cause of the
  specific symptom tested, but otherwise unchanged; `seam_smoothing_system`
  still scans all resident chunks every frame (now mostly short-circuited for
  revisited terrain via persisted `seam_finalized`, not for fresh generation).
  Not further optimized.
- **`StandardCaveNoise` library wiring** — the kernel exists and is tested,
  but `Evaluator::sample_density` still errors on `LibraryRef`
  (`eval.rs:599–601`), and the chunk-Y-seam material-continuation fallback
  silently reintroduces grass-strata banding if a `LibraryRef` enters a
  density chain without that gap closed first (per
  `underground-visibility-design.md` §6). Explicitly deferred, "needs more
  time to cook" — the shipped caves are hand-authored plain-noise graphs
  instead.
- **Player movement/collision rework** — flagged mid-phase as broken
  (tunneling, drift, mask fit); fixed by a separate agent session. User-
  confirmed working; not independently re-verified in this session, per
  explicit direction ("don't bother... I already had it fixed earlier").
- **Fringe/dim clarity state** (`underground-visibility-design.md` §4.3) and
  **cave-mouth `u`-blending** (§8 step 4–5) — the visibility system ships
  with only the binary visible/unknown states; the dimmed "remembered
  periphery" and smooth mouth-blend were part of the note's build order but
  not reached.
- **Enclosure-factor fog / audio occlusion** (design §11/§12) — the flood's
  `undergroundness` was explicitly designed to be shareable with a future fog
  pass and audio occlusion; neither consumer exists yet.

## 5. State for Phase 10

The player is a real ECS entity — pure fixed-tick sim, geometric collision,
capsule render with a working shadow, full occlusion/visibility handling
underground, a working stencil for above-ground occlusion, and reach-limited
break/place through the same mutation API every other actor edit uses. No
renderer types touch the player entity (the boundary Phase 9 was built to
respect for Phase 10). The two open perf items above (§4) are the concrete
starting point for whichever phase or interstitial pass picks up
profiling next — both have a clear mechanism identified, neither has a fix
written yet.
