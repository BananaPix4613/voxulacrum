# M1 "Clean slate" — close-of-milestone audit

**Status:** M1 work complete and merged; released as 0.4.1.
**Date:** 2026-10-14
**Scope:** `docs/roadmap.md` §12 M1, work items F1–F6 (issues #1–#6).
**Source of truth for each item:** its GitHub issue. This document records what
was done, how it was verified, and what was found and deliberately left alone.

---

## 1. Summary

All six M1 issues are closed and merged to `master`. F4 was closed before this
work began and was not touched. The workspace version is still `0.4.0`; the
patch release is the only M1 work outstanding.

| Item | Title | PR | Commit | App tests added | Failed before fix | Evidence |
|---|---|---|---|---|---|---|
| F1 #1 | Edits lost after promotion to a full save | #55 | `39e3564` | +8 | yes | `8192 of 32768 voxels differ after reload`, first at index 0 |
| F2 #2 | Zstd dictionary trained mid-session | #56 | `1c462b5` | +2 | yes | `Corrupt: flags=3 but no dictionary` |
| F3 #3 | Fluid capacity underflow and mass conservation | #57 | `9e255c4` | +5 | yes (all five) | panics at `fluid_sim.rs` lines 207, 270, 219; `40000 -> 42767`; `97535 -> 89919`; one full cell (65535) created; `65535` vs `32767` |
| F4 #4 | Tree capsule taper test | — | — | — | — | closed before this work; not touched |
| F5 #5 | One material table | #58 | `38ab17f` | +3 | n/a (refactor) | color table equals the registry for all 256 uploaded slots, before and after |
| F6 #6 | Small cleanups | #59 | `e70c299`, `5a07bf0` | +5 | yes where applicable | `KeyR -> [PrimaryAction, RotateTool]` |

Application test count: 150 (inferred) → 158 (F1) → 160 (F2) → 165 (F3) →
168 (F5) → **173** (F6).

Invariants held on every branch:

- Generation hash unchanged at **`0xed02babdf0f40420`**
  (`--verify-generation 512`).
- **No save-format change.** `BLOB_VERSION` and `VOXEL_FORMAT_VERSION` in
  `world/persistence.rs` were never touched.
- `cargo build --workspace` and `cargo test --workspace` clean, no new warnings.
- `--validate-graphs`: 0 errors, 0 warnings.

## 2. Exit gates (roadmap §12)

| Gate | Status |
|---|---|
| Regression tests for F1, F2 and F3, each failing before its fix | **Met.** The failing output was captured for each before any fix was proposed. |
| CI green, including the tree taper test | **Met locally.** Every branch passed `cargo test --workspace`; F4 is closed. CI itself was not observed. |
| 0.4.0 tagged on `master`; fixes released as a patch | Tag `v0.4.0` exists. **The patch release is pending** (section 8). |

## 3. Items

### F1 — Edits lost after promotion to a full save

**Defect.** Once a batch reached `DELTA_THRESHOLD`, `overrides` was set to
`None`, which is also what an untouched chunk has. Every later touch (the next
edit, water flowing in, a scatter change, a seam demotion) called
`get_or_insert_with` and started a near-empty bucket, which `build_chunk_edits`
saved as a `Delta` over the stored `Full` record. A chunk loaded from a `Full`
record also came back as `None`, so the first edit after a reload lost the chunk
the same way.

**Fix.** `Chunk::overrides: Option<ChunkOverrides>` became
`edit_state: EditState` with variants `Untouched`, `Delta(ChunkOverrides)` and
`Promoted(ChunkOverrides)`. The state, not the bucket's size, picks the save
record. A `Full` record loads as `Promoted`. The threshold is applied in one
place, `EditState::record_voxels`, used by the batch handler and the seam pass.
`Promoted` carries a bucket for the layers a `Full` record has no slot for
(scatter, fluid), with `voxel_diffs` released at promotion.

**Verification.** Four regression tests (control; edit after promotion; fluid
flowing into a promoted chunk; first edit after reloading a `Full` save) and
four `EditState` unit tests. Three of the four regression tests failed on
`master` for the stated reason. Manual check in the running engine passed.

**Behavior changes.**
- The determinism checker skips promoted chunks (it previously reported them as
  diverged).
- Scatter edits survive promotion for the session (promotion used to discard
  them). They are still not persisted.
- An untouched-but-dirty chunk saves nothing instead of a `Full` snapshot (no
  live path reaches this).
- Fluid and scatter on a promoted chunk are still not persisted: a `Full`
  record has no slot for them and adding one is a format change (M4, D6).

### F2 — Zstd dictionary trained mid-session

**Defect.** `tick_autosave` trained the dictionary after every autosave once 200
rows existed, and later rows were written with it. The streaming workers capture
the dictionary once (startup, or after a regeneration), so in the session that
crossed 200 chunks their loads of those rows failed, the chunk loaded without its
edits, and its next save overwrote the good row.

**Fix.** Training only in `WorldPersistence::open`, before the workers exist.
This is the smaller of the issue's two options. `try_train_dictionary` now
documents that it must run before any reader is handed the dictionary.

**Verification.** A control (a row written with the dictionary is readable by a
reader holding it) and a regression test that reads a mid-session row through a
reader holding only the dictionary captured at open. It failed on `master` with
the message quoted above. Manual check passed with the threshold temporarily
lowered on a fresh world.

**Behavior change.** A world that crosses 200 chunks mid-session keeps writing
plain zstd rows until its next open. They are fully readable, only slightly less
compact for the rest of that session. Rows overwritten by the old bug cannot be
recovered.

### F3 — Fluid capacity underflow and mass conservation

**Defects.** Three, in `world/fluid_sim.rs`.
1. Capacity minus mass was computed in `u16` at three sites (falling, spreading
   sideways, arriving from the chunk above). Voxel edits do not touch the fluid
   layer, so a slab placed into a full cell left it over capacity: a panic in
   dev builds, a wrapped value in release.
2. In a `Submerged` chunk an absent cell reads back as full, and `commit_chunk`
   removed any cell that drained to `MIN_MASS`, so draining created water.
3. At chunk seams each side capped its half of a transfer differently (the
   source by its live mass, the destination by its live room), so mass was
   created or lost crossing a border.

**Fix.**
- Every cell is read as `min(mass, capacity)`, subtractions saturate, and every
  active cell is seeded into the plan so one stored above its capacity is
  brought down to it.
- `handle_edit_voxel_batch` clamps a fluid cell to the capacity of the voxel the
  batch left there. The simulation-side clamp stays because saves can already
  hold such cells.
- A drained cell in a `Submerged` chunk is stored as an explicit empty, settled
  cell. The renderer already reads an explicit `0` as not water.
- Seam transfers apply first, with one amount computed only from snapshot values
  both sides read identically:
  `min(raw, source mass / source faces, destination room / destination faces)`.
  Splitting by the number of chunk faces a cell touches means a cell with
  transfers to several chunks cannot give away or take in more than it has,
  without either side seeing the third chunk.

**Verification.** Four `fluid_sim` tests (three subtraction sites in one test,
submerged drain, both seam directions, and a 1,000-tick conservation soak) and
one edit-time clamp test, all failing before the fix. Prototyped and verified in
a scratch copy before the tests were handed over.

**Behavior changes.**
- Edge and corner cells move water across a seam more slowly (never faster).
  Interior face cells are unchanged; conservation is exact.
- Seam transfers apply before in-chunk gravity within a tick.
- Surplus above a cell's capacity is destroyed, not displaced (the issue's
  "clamp" direction).
- Drained submerged cells persist as explicit empty cells.
- The `MIN_MASS` small-loss threshold is unchanged for non-submerged chunks, so
  "total mass constant" holds for scenarios that never cross it.

### F4 — Tree capsule taper test

Closed before this work began. Not touched.

### F5 — One material table

**Defect.** The material table existed three times: `assets/materials.ron`,
`MaterialRegistry::load_initial()` in `voxel-core`, and
`MaterialParams::default()` in `params.rs`. A lock test held the first two
together; nothing held the third, and the third is what the play HUD, tool
cycling and terrain color table read. Because it was part of the persisted
`EngineParams`, a saved preset could also recolor terrain away from the file.

**Fix.**
- `assets/materials.ron` is the only source. `load_initial` is deleted and
  `from_ron_str` is public.
- The app compiles a copy of the same file into the binary
  (`materials::embedded_registry`) and uses it only if the file is missing or
  invalid at startup, preserving "never fail to start over materials" without a
  second table. Tests in both crates build their registry from that file.
- The color table, play HUD and tool cycling read the registry.
  `MaterialParams`, `MaterialEntry`, `EngineParams.materials`, the change
  detector's materials arm, and the per-frame color upload system are removed.
- The Materials panel became a read-only listing. Of its five editable fields
  only `color` was ever read.

**Verification of "rendered colors must not change".** There is no `--capture`
flag, so the table was proved instead. Terrain color is composed only from
`material_colors[id]`. Before the change, a test showed the old copy equals a
registry-derived reference for every field the readers use and for all 256
uploaded slots (bit-exact, including the magenta padding). After the change, a
test shows the new function equals the same reference. The one case not covered
is a developer's local preset with edited materials. The one value deliberately
pinned is the shipped ids (`grass_soil` is 6), because saves store raw ids.

**Behavior changes.**
- The Materials panel is read-only; live recoloring and the sharpness,
  permeable and flora toggles are gone.
- A saved preset's `materials` block is ignored on load (tested).
- `ParamChangeKind::MeshInvalidating` has no producer; its variant is kept with
  an explanatory `allow(dead_code)`.

### F6 — Small cleanups

1. **`KeyR` double binding.** `PrimaryAction` (break a voxel in Play mode) and
   `RotateTool` (rotate an armed blueprint stamp) were both on `R`, so one press
   did both, and an armed stamp survives the F5 mode toggle. `R` stays
   `RotateTool` (the stamp HUD documents it); `PrimaryAction` moves to `V`, which
   is unused, left-hand reachable, and leaves `T` (`SecondaryAction`) meaning what
   it meant. Two tests assert that no trigger maps to two distinct actions, in
   the defaults and in the shipped `assets/input.ron`.
2. **Presets directory.** `PathBuf::from("params")` was relative to the working
   directory, while every other asset path resolves through `asset_root()`. It
   is now `paths::presets_dir()`, with a test that it is absolute and under the
   asset root. `/params/` is gitignored beside `/saves/` and `/cache/`.
3. **`MeshCacheParams.enabled`** was never read. It is now honored in
   `mesh_chunk_task`: disabled, the task meshes without hashing, looking up or
   writing. The cache key is content-addressed and omits the mesher version, so a
   bypass that does not destroy the cache is a useful comparison tool for the
   coming mesher work. Read once at startup. Two tests (a control and the
   bypass).
4. **`fastnoise2`** was used by no source file and dragged in `bindgen`,
   `cmake` and libclang to build. Removing it prunes ten packages from
   `Cargo.lock` (`bindgen`, `cexpr`, `clang-sys`, `cmake`, `fastnoise2`,
   `fastnoise2-sys`, `glob`, `minimal-lexical`, `nom 7.1.3`, `prettyplease`) and
   drops `cmake` from the Linux release job.

A second commit regenerated `THIRD-PARTY-NOTICES.md` with `cargo about`. It
dropped the ten crates and corrected the workspace crates from `0.3.0` /
Apache-2.0 to `0.4.0` / MPL-2.0 (the file had last been generated at 0.3.0).

**Behavior changes.**
- In Play mode, breaking a voxel is `V`, not `R`. A hand-edited
  `assets/input.ron` keeps its own bindings.
- Presets saved while launching from another working directory need moving to
  `<root>/params`.
- A preset with `"enabled": false` now really runs without the mesh cache.

## 4. What users will notice (release-note material)

1. The determinism checker skips promoted chunks (F1).
2. Scatter edits persist through promotion for the session (F1).
3. A world crossing 200 chunks mid-session no longer corrupts reloads (F2).
4. Water crossing chunk seams is conserved, and edge and corner cells pass it
   more slowly (F3).
5. Placing a slab or cube into water clamps the water instead of panicking (F3).
6. The Materials panel is a read-only list; preset `materials` blocks are
   ignored (F5).
7. Breaking a voxel in Play mode is `V`, not `R` (F6).
8. Presets live under the asset root, so some need moving (F6).
9. `"enabled": false` in a preset really bypasses the mesh cache (F6).

## 5. Method

- **Plan-and-present.** No repository file was edited by the agent. Every change
  was presented as a transcription-ready block and applied by hand, one substep
  at a time, with build and test output reported back before continuing.
- **Test-first for F1–F3.** The first substep of each was a regression test that
  fails on `master` for the reason the issue describes; the failing output was
  shown before any fix was proposed.
- **Scratch verification where a test depended on something unreadable by
  inspection**: the zstd trainer on synthetic rows (F2), integer seam
  arithmetic and a prototype fix (F3), and the lockfile pruning and notices
  regeneration (F6). Scratch copies lived outside the repository.

Things that went wrong, for the record:
- Three of the agent's own tests were wrong before transcription and were caught
  in scratch: the F3 sideways scenario passed on unfixed code because the slab
  drained by gravity first, the F2 rows needed proving trainable, and the F3
  seam numbers needed the real integer arithmetic.
- Several scratch trees shared a compiled-unit hash, so Cargo once reused another
  tree's binary. This was caught from impossible results and re-run after
  forcing a rebuild; nothing reported rests on that run.
- A two-filter `cargo test` command that Cargo rejects (F6), and an IDE
  auto-import that slipped into a test module and surfaced as a warning.
- One changelog entry (F2) missed its own commit and rode with F3.

## 6. Not verified

- GUI manual checks for F3, F5 and F6. F1 and F2 were checked by hand.
- `release.yml`, which runs only on tags and manual dispatch. The CMake removal
  is safe by lockfile analysis (nothing else depends on the `cmake` crate), but a
  `workflow_dispatch` run would prove it.
- CI status itself.
- Perf baselines.

## 7. Open findings (candidate issues; none were fixed)

Per roadmap §1 rule 5, each needs an issue before the next release is cut.
Workstream tags are suggestions.

### Persistence and data model (D)

1. A record that fails to load yields a pristine-looking chunk, and its next
   save overwrites the unreadable row. Quarantine on a failed record.
2. `overlay_record` drops `voxel_removed`, `detail_diffs` and `decal_diffs`
   despite the "round-trip" comment. All three are empty today (latent).
3. A promoted chunk loaded with `seam_finalized == false` has no `voxel_diffs`,
   so the seam guard cannot protect player voxels from demotions (rare).
4. `remove_scatter_at` creates a bucket even when nothing is removed, so a no-op
   scatter click turns an untouched chunk into `Delta(empty)` and the determinism
   checker skips it.
5. `open` trains the dictionary before the format-version wipe, so a stale-format
   save trains on rows it then deletes (harmless to correctness).

### Simulation (S)

6. Voxel edits do not wake adjacent fluid: digging beside a pond does not flow,
   and removing a slab does not refill the space.
7. An explicit empty cell in a `Submerged` chunk is never refilled, because the
   implicit cells above it are not in the active set.
8. The edge and corner seam share is conservative. True global resolution of
   seam flows belongs with the R-series tick framework.
9. `fluid_sim.rs`'s module doc ("intra-chunk in this substep… not yet wired") is
   stale.

### Content, tools and boundary (T / B / V)

10. The `mesh_params_pending` machinery (set in `systems.rs`, read in three
    panels, cleared in `coordinator.rs`) has no producer since F5.
11. Hot reload of `materials.ron` would restore live tuning properly; the color
    buffer is left in place for it.
12. `debug_material_color` in `terrain.wgsl` is still a hand-written table
    (handoff 4.7), and numeric material ids remain baked into `nodegraph-ir`
    defaults, examples and `library_kernel`.
13. Both `include_str!("../../../assets/materials.ron")` calls reach outside
    their crate; the M2 data-only-packages work should give shipped assets one
    owner.
14. `InputMap::default()` and `assets/input.ron` are two copies of one table with
    nothing asserting they agree.
15. `max_size_bytes` and `eviction_batch_size` have no UI and are read only at
    startup.
16. `.claude/worktrees/bold-nobel-a54071/` is a stale worktree with its own
    `target/` (gitignored, disk space only).

## 8. Remaining for the release cut (roadmap §7.4)

Release steps are complete.

1. Bump the workspace version `0.4.0` → `0.4.1`.
2. Give the `[0.4.1]` changelog heading its date.
3. Regenerate `THIRD-PARTY-NOTICES.md` after the bump:
   `cargo about generate about.hbs -o THIRD-PARTY-NOTICES.md`.
4. Record perf baselines and update the tooling-maturity tables.
5. Turn section 7 into issues, so no open finding is silently dropped.
6. Tick the M1 gate boxes in `docs/roadmap.md` §12.
7. Optionally run `release.yml` via `workflow_dispatch`, then push the annotated
   tag on `master`.

## 9. Addendum: F7 (0.4.2)

**F7 #60, a saved record that failed to load was overwritten** (PR #61, commit
`375a8e4`). Found as open finding 1 in section 7. A chunk whose record could not
be read came back looking untouched, and its next save replaced the record. The
chunk is now marked `persist_blocked`: it stays playable, nothing is saved over
its record, a warning names it, and the Performance panel counts blocked chunks.
Two regression tests cover four ways a record can fail to load and both save
paths, and both failed before the fix; two controls and one mechanism test pass.
No save-format change; generation hash unchanged. Released as 0.4.2.

Noted for D7 (#30): regenerating terrain and the format-version wipe delete all
saved rows, unreadable ones included.
