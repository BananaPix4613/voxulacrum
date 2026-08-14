# Phase 11 handoff

Written at a deliberate pause in the foliage rework, to move to a fresh agent
session without losing context. Read this, then `docs/pre-phase-11-audit.md`,
which is the running decision record and carries the reasoning behind everything
below.

**The next session should design before it builds.** The work paused precisely
because several coupled problems were being solved one substep at a time when
they need a shape agreed first. Do not open with a transcription package.

---

## 1. How this collaboration works

**Plan-and-present.** The agent may read anything, run analysis, run commands
(tests, benches, verification binaries), and edit `docs/`. The agent must **not**
apply code edits. Every code change is presented as a transcription-ready
BEFORE/AFTER package that the human types by hand.

Consequences that matter more than they look:

- **Transcription is expensive, so precision beats volume.** A named file, a
  unique anchor, exact before/after text. Never prose like "find where X is
  built and add Y" - that is where every miss has happened.
- **Transcription is serial, so one substep at a time.** Present, wait for the
  build/test report, then proceed. Each substep must leave the suite green on
  its own.
- Prefer new modules presented whole over sprawling edits.
- Include execution and verification steps at the end of every package.

**Before presenting any change to a type or signature**, run:

```
grep -rn "TypeName\|fn_name" --include=*.rs crates/
```

and give an exact BEFORE/AFTER for **every** hit, including test fixtures and
other crates. Also **read the entire file** before editing it - grep finds call
sites, but only a full read shows derives, stale module docs, and whether a
helper exists. Adding one struct field means visiting every construction site in
the workspace.

This was the single largest source of friction in the previous session. Misses
that actually happened, all the same shape: `WorldGenerator` gained a field and
both constructors used `Self { .. }` (invisible to grep); `StreamingParams` lost
two fields and `world/streaming.rs` was missed; `FeatureSource` gained a field
twice and `structure_sources` was missed **both times**; a test assumed
`FeatureSource: Clone` when it derives nothing.

**Scratch verification is available but should be rare.** There is a scratch
cargo workspace and a naga WGSL validator under the session scratchpad; rebuild
them if needed. Reserve them for genuinely uncertain changes - borrow-checker
risk, performance claims, asserting a bug exists. Skip them for changes that
mirror an existing pattern. The app crate cannot be built in scratch
(`fastnoise2-sys` needs a native source tree), so app-side changes are verified
by the human regardless.

**Style.** In `.rs` files including doc comments: no em dashes, use `-`.
American spelling (neighbor, color, behavior). `§` is fine and preferred for
design references. Em dashes are fine in `log::`, `panic!`, `assert!`, `format!`
and UI text. The human normalizes as they transcribe, so quote BEFORE anchors
**from the file on disk**, not from what was originally written.

**Commits.** Regular per-substep commits. The agent drafts the message; the
human stages and pushes. End with
`Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

---

## 2. Where the project is

**Version 0.4.0 - "Worldgen Completeness & Authoring"**, on branch `v0.4.0`.
The organizing rule: *every piece of the reference world is authored through the
tools, not by hand-editing JSON.*

Last commit `518c528`. Generation hash anchor **`0x12e6a0644184099f`**
(`--verify-generation 512`).

### Landed this phase

- Blueprint format, in-engine capture and stamping, per-axis trim, rotation,
  isometric preview - all through the one mutation door.
- Cross-chunk feature model (design § 5 v1.10): world-absolute derivation over a
  margin band, order independence asserted in CI.
- Multi-zone worlds, manifest schema v1, `--validate-graphs`, column inspector,
  biome/zone map, world event bus.
- **Phase A of the macro-terrain research plan**, complete: the `(node, pin)`
  edge index with a pointwise bench; `GraphRef` resolution in the density
  evaluator (B2); `SurfaceToDensity` and `Abs` (B3); arithmetic and curves in the
  column domain via domain-polymorphic pins (B1); the `WorldParams` sidecar and
  `WorldParam` node; the manifest owning `min_y`/`max_y`.
- Foliage visibility: the detail-paint, scatter and water passes now sample the
  visibility mask instead of being blanket-hidden when enclosed, and run the
  cost-priority march (water deliberately does not - see § 4).

### Uncommitted right now

The foliage rework in progress, plus `assets/blueprints/tree_trunk.blueprint.json`
(untracked). Roughly: the `biomes` filter on features, `FeatureCanopy` and
`canopy_instances`, `PlaceStructure`'s canopy declaration, wood and leaves
materials, the canopy prefab, and the shader visibility work. **The last
presented package (foliage respecting `supports_flora`) was not confirmed as
applied.** Check `git diff` before assuming.

---

## 3. The foliage rework: agreed scope

From the phase prompt: *"Foliage tiers 2/3 matured - trees as hero blueprints
with sway, `AnchorDestructionPolicy` enforcement on terrain edits, multi-species
detail layers."* Widened by the human after looking at reference art.

**Trees split across two systems.** The trunk occupies the voxel grid; the canopy
is foliage. This is the only cheap route to the requested trunk collision:
`player/sim.rs` resolves movement purely through `solid_interval(voxel)`, so
nothing outside the voxel grid can collide. It sits against § 6's "foliage does
not occupy voxel cells", which still holds for what it was written about - a
trunk is not foliage under this split.

**Trunks are authored blueprints for now.** Procedural curved growth is wanted
later and is the natural future home of the otherwise-redundant `PlaceTree`
node.

**Grass and leaf sprites are procedurally generated placeholders**, exercising a
real sprite pipeline without building an art pipeline first.

| Part | Work | State |
|---|---|---|
| A | Trunk into the voxel grid as a blueprint; canopy into the foliage store, anchored to it | mechanism landed, content in progress |
| B | `AnchorDestructionPolicy` on terrain edits; paint reanchoring (F1) is the trivial case | not started |
| C | Sprite pipeline: atlas, procedural generation, animation frames, texel-to-pixel snapping | not started |
| D | Grass and canopy re-authored as sprites, replacing the 3D blades | not started |

C and D are **not separable** - removing the 3D blades without a sprite
replacement ships a version with no visible foliage.

---

## 4. Open problems that need design, not code

This is why the session paused. Each of these was found by running the engine
with real trees, and each is bigger than a substep.

**4.1 Slab smoothing runs after structures.** `world_generator.rs` labels its
smoothing pass "Worldgen stage 7", but it executes at the storage boundary -
after `evaluate_chunk` has already run stage 8 (structures). Design § 5 orders
smoothing at 7 and structures at 8. A trunk is a column of single-cube steps,
which is exactly what the smoother halves, so trunks get chewed. Candidate
fixes: a per-material `smoothable` flag (cheap, data-driven), or restructuring
the eval->storage boundary so smoothing precedes structures (correct, invasive).
**Not yet decided.**

**4.2 Foliage grows on trunks.** `supports_flora` has been on every material
since the registry existed, is authored and UI-editable, and **nothing in
generation reads it**. `eval_paint` checks biome and surface presence, never the
surface material. A package for this was drafted (thread `MaterialRegistry` into
`DetailEvaluator`, test in `eval_paint` and `eval_scatter`) but not confirmed
applied. This one is well-understood and small.

**4.3 Trees generate below sea level.** Nothing constrains a feature by height.
`SurfaceFilter` has `min_height`/`max_height`; `PlaceStructure` has no
equivalent. Simplest fix is min/max world Y on the source. Consider whether the
right rule is height or "not submerged", given fluid is computed after
structures.

**4.4 The canopy does not appear.** Trees generate whole, on the right biomes,
with correct trunks - but no canopy instance is visible. Undiagnosed.
`canopy_instances` clips to the chunk window; a canopy above the trunk may be
landing in the chunk above, which should still emit it since features are
derived in every vertical chunk. Needs a real diagnostic (column inspector on a
tree column, or instrument `scatter_to_store`) rather than a guess.

**4.5 Blueprint anchors are uncontrollable.** Capture puts the anchor at a
corner, and there is no way to move it, so a tree's anchor is at the corner of
its bounding box with no voxel above it. § 6's `PrefabMeta.anchor_offset` was
specified and never built. This is a blueprint-tooling substep and it blocks
good tree content.

**4.6 The material table exists in three places.** `assets/materials.ron`,
`MaterialRegistry::load_initial()`, and `voxulacrum-app/src/params.rs`'s
`MaterialEntry` list. A drift test covers only the first two.

**4.7 `debug_material_color` in `terrain.wgsl` is a hardcoded switch** ending at
id 8, so new materials render white in the material-id debug view.

---

## 5. Also outstanding

- **F1 (unrecorded before this phase):** paint has no anchor. `PaintTexel` is
  `{species, density, tint, flags}` with no Y; `build_chunk_columns` re-derives
  the surface every time it builds a chunk's paint buffer, so grass climbs onto
  whatever you place on it. Folded into part B - "the anchor changed" for paint
  simply means "disappear".
- **Water occlusion** needs its own rule; it currently skips the cost-priority
  march deliberately and draws a void shell outside the volume. Deferred by
  agreement.
- **Terrain's screen-space ring carve** (`in2 && !in1` in `terrain.wgsl`) is not
  replicated by the foliage passes. Watch for props floating in that ring.
- Remaining 0.4.0 substeps, by name: preview and probe tools; editor ergonomics
  (multi-select, copy/paste, node search, comments/groups, collapse-to-library,
  inline docs), graph diff, in-engine console; content wave 2; asset standards +
  graph asset versioning; **D8 greedy-meshing decision with measurements, due at
  0.4.0 exit**; close-out.
- Roadmap amendments owed at close: rivers off 0.4.0, macro terrain as the 0.5.0
  headline, plus the six deferred from Q6.
- Deferred lints: no drift test for `InputMap::default()` against
  `assets/input.ron`; `PlaceStructure` outside a ZoneGraph is a runtime rather
  than validation error; no unknown-field lint for graph assets.
- **D2 (light as a per-vertex byte vs a per-chunk 3D texture)** must be decided
  before 0.5.0 builds the lighting bake. Worth deciding during 0.4.0's close.

---

## 6. Standing lessons worth keeping

- **When a substep ships content that should move the generation hash and it
  does not, that is the finding - chase it.** A 64-chunk sample silently stopped
  being evidence about structures; the fix was raising the sample to 512 *and*
  asserting the property directly in tests.
- **A metric with no deficit behind it is not a measurement.** The edge index
  was landed on measured numbers that contradicted the research's estimate.
- **Land an instrument in its own commit.** A bench that ships with the thing it
  measures cannot produce a baseline afterwards - this happened.
- **Two copies of a lookup will diverge.** The `GraphRef` output-name lookup had
  two and they disagreed about which pin index to use; 418 of 1024 columns
  differed between the bulk and pointwise paths. Bulk-versus-pointwise
  divergence is the recurring seam-defect shape in this engine.
