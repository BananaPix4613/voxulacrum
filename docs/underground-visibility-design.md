# Underground Visibility — Design Note v2 (Air-Side Face Rule)

**Status:** Revised design note, superseding v1 of this document after review of
the first implementation attempt. v1's §4.2 (camera-direction sweep + section
caps) was the flawed part: over-specified, direction/cap semantics easy to get
wrong, and — decisively — unnecessary. v2 replaces it with a strictly simpler
rule that satisfies the visibility invariant *by construction*. §7 is a
post-mortem of the specific defects found in the current tree, with file/line
references, so the implementing session can fix rather than re-derive.

**The invariant (owner-stated, this is the spec):** culling always prioritizes
maximum visibility of the space the player occupies. Every face between the
camera and the player's known space is removed; geometry closer to the camera
is never preserved at the expense of geometry spatially closer to the player.

Requirements R1–R5 from v1 are unchanged:
R1 immediate clarity · R2 faint periphery · R3 no surface leakage ·
R4 arbitrary geometry · R5 player stencil.

---

## 1. What v1 got right (retained)

- **The flood-fill visibility volume** (§4.1 in v1) — implemented in
  `world/room.rs::flood_visibility` and correct as built: bounded 6-connected
  flood through air from the head cell; undergroundness `u` = the share of the
  flood frontier that is not sky-lit; solid walls always count as enclosure.
  The tests in room.rs are good. **Keep all of it.**
- **The three-state clarity model** (visible / fringe / unknown), the windowed
  3D mask texture with toroidal update, Bayer-dithered transitions, the
  recompute-on-head-cell throttle, and the u-blend at cave mouths.
- Player-anchored/transient/single-writer reasoning for sharing the flood with
  future fog and audio consumers.

## 2. What v1 got wrong (corrected here)

v1 derived the cutaway as a *cell set*: sweep the visible volume toward the
camera, collect occluding solid cells, treat them specially, and cap the rest.
Two failure modes fell out of that formulation in practice:

1. **Cell-classification of fragments is ill-posed on faces.** A face fragment
   lies exactly on the boundary between its solid owner and the air in front of
   it; `floor(world_position)` resolves to one or the other depending on face
   orientation. A cut cell's camera-facing top face classifies into the air
   cell *above* it — state "unknown" — and renders as an opaque cap. Result:
   occluders stay, recolored. This is the "very little gets culled" symptom.
2. **The cut set itself is fragile**: ray direction sign conventions, float
   marching that skips corner-crossed cells, rotation-dependent recompute, and
   |visible| × depth work per recompute — all for a set the correct rule never
   needs.

## 3. The corrected core: the air-side face rule

The mesher emits faces only where solid meets air. Every terrain fragment
therefore belongs to a face with a well-defined **front air cell**:

```
air_cell(fragment) = floor(world_position + 0.5 * face_normal)
```

The half-normal offset lands robustly inside the air cell the face looks into,
independent of face orientation — this replaces raw `floor(world_position)`
everywhere in the mask path.

**The whole visibility rule, in enclosed mode:**

| mask state of `air_cell` | rendering |
|---|---|
| visible | normal shading |
| fringe (later step) | dimmed fill; outline pass still draws edges |
| unknown / outside window | **discard** |

That is the entire mechanism. No cut set, no sweep, no cap state:

- Any occluder between the camera and the room fronts air the player has not
  reached → its faces discard → the camera sees through to the room.
  **Maximum visibility is a property of the rule, not a goal it approximates.**
- The room's own floor/walls/ceiling front visible air → they render lit.
- Everything else in the world fronts unknown air → discarded → background
  void. R3 (no surface visibility) is structural: the surface is never drawn
  at all while enclosed.
- Depth ordering can never prioritize near geometry: near geometry that
  obstructs is simply absent from the frame.

Section caps (the Dungeon-Keeper cross-section look at the reveal border) are
**deleted from the core design** and noted as optional future polish; if ever
wanted, they are a boundary-face detail, not a visibility mechanism.

### Consequences for the current code

- `world/room.rs::camera_cut_set` — **delete** (and its tests/callers).
- `RoomState.cut`, `last_rotation`, and the rotation-triggered recompute —
  **delete** (the rule is view-independent; only the flood is positional).
- Mask states reduce to: 0 unknown, 1 visible (2 reserved for fringe).
- `terrain.wgsl` mask branch becomes: sample at `wp + 0.5*n`; state 1 → shade
  normally; else discard (dithered edge later). The dark-cap return path goes.
- The camera-ray math, `to_camera` derivation, and CUT_DEPTH constant go.

## 4. Water and foliage: same rule, not blanket hiding

The current `hide_water: … || room_enclosed` / `hide_foliage: … ||
room_enclosed` shortcut is replaced by mask sampling in each pass — underground
water and cave foliage are *content*, and the player must see them inside the
visible volume:

- **Water** (`water.wgsl`): a water surface quad's front air cell is the cell
  above it — same `wp + 0.5*n` sample; discard when unknown.
- **Scatter** (`scatter.wgsl`): sample at the instance's anchor air cell
  (anchor + up), in the vertex stage; collapse the instance (zero-size) or flag
  fragments to discard when unknown.
- **Detail paint** (`detail_paint.wgsl`): sample at the blade's column surface
  air cell, same treatment.
- The player pass and pick highlight never sample the mask (R5).

All three passes already bind the global uniforms; they additionally need the
mask texture binding. Until each pass samples the mask, it should render
*normally* (visible everywhere) rather than be blanket-hidden — wrong in the
dark-cap world, but correct-by-default in the discard world, since anything
floating in the void sits behind discarded terrain only when it genuinely is
outside the volume… (verify visually; if foliage outside the volume reads as
floating sprites against the void, gate that pass before shipping the step).

## 5. Performance corrections

Observed stutter has three plausible contributors in the current
implementation; address in this order:

1. **Delete the sweep** (§3) — removes |visible| × 32 `solid_interval` chunk
   lookups per head-cell move and the rotation-change recompute entirely.
2. **Flood into a dense window-local grid, not HashMap/HashSet.** The flood is
   bounded by the 64³ mask window anyway; flood directly into (or alongside)
   the `[u8; 64³]` upload buffer indexed by window-local coords, with a small
   ring buffer for the BFS queue. Eliminates per-cell hashing and doubles as
   the upload staging — the mask build step disappears.
3. **Amortize `sky_lit`**: cap the upward scan (it only needs to answer within
   the loaded Y range) and consider consulting the column-stack surface data
   when that lands. Frontier size is typically hundreds of cells; this is
   secondary.

If hitches persist after 1–2, move the flood+upload off the crossing frame
(compute into a back buffer, swap next frame). Do not lower BUDGET (22) first —
shrinking the known volume trades correctness-feel for perf before the real
costs are gone. Note the engine has pre-existing frame hitches (remesh churn,
O(world) per-frame scans — see the drift review); attribute before tuning.

## 6. Cave generation (separate but coupled to testing this feature)

The current meadow "caves" are thin 3D-noise fractures — near-unusable for
testing visibility. `StandardCaveNoise` already exists as an authored library
asset (`assets/libraries/standard_cave_noise.library.json`) backed by
`LibraryKernel::StandardCaveNoise` (Worley + ridged composite — produces
player-scale tubes and chambers; design doc §4's standard library list).

- Wire a `LibraryRef(StandardCaveNoise)` into the biome density chains via
  `DensitySubtract`, tuned so bores are ≥2–3 voxels wide (player is 0.6×1.5).
  Zone-level cave subtraction (design §5 stage 4) remains the eventual home;
  biome-level wiring is the pragmatic first step.
- **Companion change, required:** `Evaluator::sample_density` errors on
  `LibraryRef` (`eval.rs:599–601`). The chunk-Y-seam material continuation
  (grass-strata fix) samples the density chain pointwise and *falls back
  silently* to window-relative banding when sampling fails — so adding a
  LibraryRef to a density chain would quietly reintroduce grass strata for
  that biome. Kernel-backed libraries are pure functions; add a
  `sample_density` arm that resolves a kernel-backed `LibraryRef` and calls
  its kernel pointwise before (or alongside) wiring caves in.

## 7. Post-mortem of the current tree (fix list, with locations)

| # | Defect | Where | Fix |
|---|---|---|---|
| 1 | Fragment classified by `floor(world_position)` — boundary-ambiguous, orientation-dependent; occluders render as opaque caps instead of discarding | `shaders/terrain.wgsl` fragment mask branch (~208–222) | Air-side rule: sample at `wp + 0.5*n`; visible → shade, else discard (§3) |
| 2 | Water + foliage blanket-hidden when enclosed | `ecs/systems.rs:1098–1099` | Per-pass mask sampling (§4) |
| 3 | Sweep marches by unit float steps along a diagonal — skips corner-crossed cells (thin occluders survive) | `world/room.rs::camera_cut_set` (151–169) | Moot — delete the sweep (§3) |
| 4 | Rotation-triggered full recompute | `ecs/systems.rs` room system (~524–530) | Moot — rule is view-independent |
| 5 | HashMap/HashSet flood + separate mask build + 256 KB upload on the crossing frame | `room.rs::flood_visibility`, mask upload system (~772–840) | Dense window-local flood doubling as staging (§5.2) |

`flood_visibility` itself, its tests, the throttle, `u`/threshold constants,
and the mask window/uniform plumbing are sound — keep them.

## 8. Build order (revised)

1. Shader fix (#1) + delete sweep (#3/#4): terrain-only, two states. This step
   alone should produce the correct "see the whole cave, nothing occludes,
   void beyond" picture — verify in the awkward narrow caves before improving
   generation, precisely because they stress the rule.
2. Water/foliage mask sampling (#2), removing the blanket hides.
3. Dense-grid flood + staging unification (#5); re-evaluate stutter.
4. Fringe state + outline integration; then mouth blending by `u` (dither).
5. Cave noise (§6) with its `sample_density` companion change — after the rule
   is verified, so generation changes don't confound rendering verification.

---

*End of note. v1's flood/mask/clarity architecture stands; its cut-set
mechanism is withdrawn. The air-side face rule is the load-bearing correction:
visibility maximization becomes a property of the classification, not an
outcome the implementation must engineer toward.*
