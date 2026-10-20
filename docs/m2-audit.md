# M2 "Engine/game boundary" — close-of-milestone audit

**Status:** M2 work complete and merged; released as 0.5.0.
**Date:** 2026-10-20
**Scope:** `docs/roadmap.md` §12 M2: work items B1–B7 (issues #7–#13, with B5's
steps #66–#69 and B6's steps #74–#76) and T2 (#85).
**Source of truth for each item:** its GitHub issue. This document records what
was done, how it was verified, and what was found and deliberately left alone.

---

## 1. Summary

Every M2 issue is closed and merged to `master`. B5 and B6 were each split into
steps before starting, and B7 was rescoped mid-milestone (§3). T2 was added to
M2 when its last exit gate turned out to have no owning issue.

| Item | Title | PR | Merge | Delivered |
|---|---|---|---|---|
| B1 #7 | App library target with a thin binary | #62 | `daeee94` | `voxulacrum-app` is a library; the binary is a few lines over it |
| B2 #8 | Split `ecs/systems.rs` by subsystem | #63 | `4bbaa43` | 16 subsystem modules, same systems in the same order |
| B3 #9 | Facade crate and CI dependency check | #64 | `83cb0d1` | the `voxulacrum` facade, and `boundary-check` in CI |
| B4 #10 | Testbed project on the facade | #65 | `30e2d96` | `crates/testbed`, a flat world on the facade alone |
| B5a #66 | Registry identity | #70 | `dd234df` | `QualifiedName`, one name rule for every registry entry |
| B5b #67 | Material references by name | #71 | `c646586` | graphs name materials; pickers choose from the registry |
| B5c #68 | Library and prefab references by name | #72 | `9bc1016` | libraries and prefabs by name; a content-declared prefab `placeholder` |
| B5d #69 | Detail and scatter tables as registries | #73 | `b713bf1` | `foliage.ron`; species colors reach the shader as data |
| B6a #74 | Package manifest and single-package loader | #77 | `6d7c3a0` | `package.ron`, `RunOptions::package`, the testbed as a package |
| B6b #75 | The reference world as a package | #78 | `8df563a` | `packages/sandbox/`, with `saves: "default"` |
| B6c #76 | Package dependencies and the engine package | #79 | `47937e9` | dependency loading; the built-in `voxulacrum` package |
| B7 #13 | Game repository; move game-specific code out | #84 | `3edd610` | `play.ron`; `boundary-check --manifest-path`; the Voxbox repository |
| T2 #85 | Integrated-GPU baseline | #86 | `a365b6b` | `--gpu low-power`; the baseline in `perf-baseline.md` |

Workspace tests at the release cut: **500**, all passing (474 after B6b).

Invariants held on every branch:

- **No save-format change.** `BLOB_VERSION` and `VOXEL_FORMAT_VERSION` never
  moved, and 0.4.2 saves open: the sandbox keeps `saves/default/` through its
  manifest's `saves` field.
- **Generation unchanged.** `--verify-generation 512` gave `0xed02babdf0f40420`
  after every step, and the testbed's own hash (`0x7e940ec7e1130057` at the cut)
  never moved.
- **No warnings.** Each step built with `cargo build --workspace --all-targets`
  clean.

## 2. Exit gates (roadmap §12)

| Gate | Evidence |
|---|---|
| Games and the testbed depend only on the facade crate (CI-enforced) | `boundary-check` in the engine's CI since B3; `--manifest-path` in Voxbox's CI since B7 |
| The testbed and the reference world load as data-only packages | B6a (testbed) and B6b (sandbox); both start through `RunOptions::package` and the same loader |
| The game repository builds against the facade through a local `[patch]` | Voxbox's gitignored `.cargo/config.toml`; built and checked against this checkout at 0.5.0 |
| Registries are namespaced in memory | B5a–B5d: every registry entry is a `QualifiedName` |
| Integrated-GPU baseline recorded in `perf-baseline.md` | T2's M2 section, four readings on both GPUs |

## 3. Items

### B1–B4 — the library, the split, the facade, the testbed

Four mechanical steps, each landed with no behavior change. The one surprise was
B4's first cut of the testbed's content paths, which hardcoded `../testbed`
relative paths that only worked from one working directory; content roots are
now anchored to `CARGO_MANIFEST_DIR`, as the engine's own asset root is.

### B5 — namespaced registries (split into B5a–B5d)

Inventorying every registry and every place content named something by number
showed four independent pieces, so B5 was split. Each step converted its graphs
with a script that mapped every numeric id through the content's own registry,
so a converted graph names exactly what it numbered before. Two latent defects
surfaced and were fixed in place: the material registry's validation tests were
passing on a RON parse error (B5a), and `WorldGenerator::from_path` skipped name
resolution (B5b).

### B6 — packages (split into B6a–B6c)

B6a made the manifest and the loader, B6b moved the shipped world into
`packages/sandbox/`, and B6c added dependency loading and the engine package.
B6c's inventory found that materials, prefabs and foliage store their ids in
saves, so those tables stay with the world's own package until D3 (M4); a
dependency that ships one is refused. Libraries, blueprints and species merge
across packages. A package sharing another's namespace is refused.

### B7 — the game repository (rescoped)

The issue as first written moved code: the play HUD, the action set, the spawn
rule. The facade can host no game code before B9 (M3), so the issue was
rewritten to move **policy as data**: player tuning, reach, the spawn rule and
the build tool's materials became the package's `play.ron`, and the mechanisms
stayed in the engine. The game repository,
[voxulacrum-sandbox-game](https://github.com/BananaPix4613/voxulacrum-sandbox-game),
holds Voxbox, a copy of the sandbox under its own id. Three copies of player
constants turned up beyond the ones the issue named (room detection's eye height
and the avatar mesh's body size) and moved with them. Each deferred part is
noted on its owning issue: E1 #45, E2 #46, B9 #81 and M8 #50.

### T2 — integrated-GPU baseline (added)

The engine always asked for the high-performance adapter, so the gate could not
be measured. `--gpu low-power` selects the integrated GPU (Intel, through
Vulkan, confirmed by the new adapter line in the log and the Performance panel).
The integrated GPU holds 60 fps at the launch zoom and drops to 31 fps at the
widest supported zoom, where it is the limit.

## 4. What users will notice (release-note material)

- The shipped world lives in `packages/sandbox/`, and starts as a package. Its
  saves stay in `saves/default/`.
- Content names what it references: materials, libraries, prefabs, detail layers,
  species and scatter types. A name that doesn't resolve fails the load with an
  error naming it. Standard libraries are named `voxulacrum:<name>`.
- Play mode is configured by a package's `play.ron`. A package without one has
  no Play mode. The sandbox plays exactly as before.
- `--gpu low-power` runs on the integrated GPU; the Performance panel names the
  GPU.
- `cargo run -p voxulacrum` is now `cargo run -p voxulacrum-app`.

## 5. Method

Each item ran on its own branch, in steps. Most steps were presented as edits
for the maintainer to apply; moves, docs and the Voxbox scaffold were made
directly. Every step was checked before its commit with: a warning-free
workspace build, `cargo test --workspace`, `--validate-graphs` and
`--verify-generation` for both the sandbox and the testbed, `boundary-check`, and
a windowed run where behavior could change. Manual checks covered what tests
cannot reach: a missing dependency refused and then logged, a dependency's
library joining the set, a package without `play.ron` refused Play mode, a bad
build material stopping startup.

## 6. Not verified

- **Release archives on CI.** The B6b layout was dry-run locally, staged with
  `release.yml`'s own copy commands; the workflow itself first runs on this tag.
- **Linux builds.** CI builds on Windows only, as before M2; the release
  workflow's Linux job first runs on this tag. macOS is not built at all.
- **A real dependency package.** None ships; dependency loading is covered by
  temp-directory fixtures and a scratch `packages/demo`.
- **Voxbox's CI.** It installs `boundary-check` from `master` and builds against
  the git dependency, so it runs green only once 0.5.0 is on `master` and Voxbox's
  engine range is `^0.5`.
- **The integrated-GPU baseline on other hardware**, and GPU time at all (T3).

## 7. Open findings (candidate issues; none were fixed)

| Issue | Finding |
|---|---|
| F8 #87 | Four rustdoc link warnings, from before M2 |
| F9 #88 | A package-load error prints its path twice |
| F10 #89 | The engine map's lenses point at functions B2 moved |
| T3 #90 | The Performance panel cannot measure GPU time; the integrated GPU's extra CPU work at radius 14 is unattributed |
| T4 #91 | `generate` and fill read far slower than 0.4.0, but from A-type readings; a B-type reading must come first |
| T5 #92 | The editor's pickers list only the world's own blueprints and species |
| T6 #93 | Hot reload doesn't watch dependency packages |

Also recorded, without an issue:

- **Voxbox's `Cargo.lock` and the local `[patch]`.** A lock written against the
  git dependency pins it, and Cargo then skips the patch with a warning; after
  the 0.5.0 bump, `cargo update -p voxulacrum@0.4.2` re-resolved it. The lock is
  committed without `--locked` in CI for this reason.
- **`THIRD-PARTY-NOTICES.md` was missing `semver`,** added by B6a. Regenerated
  for this release.

Open `type:defect` issues at the cut: none.

## 8. The release cut (roadmap §7.4)

| Checklist item | State |
|---|---|
| Workspace version bumped | 0.4.2 → 0.5.0; both shipped packages and Voxbox require `^0.5` |
| CHANGELOG entry | `[0.5.0] — Engine/game boundary`, written through M2 from each closed issue |
| Close-of-milestone audit | this document |
| Design-doc revision | `engine-design.md` v1.12: packages and content namespaces |
| Forcing-date decisions due for M3 | none is due at M3; D6 (due M5) stays open |
| Determinism suite green | `--verify-generation 512` `0xed02babdf0f40420`; all 500 tests pass |
| Perf baselines recorded and compared | T2's M2 section; the 0.4.0 comparison is T4 (#91) |
| Tooling-maturity tables | `roadmap.md` §4: the facade check and the testbed preview scene shipped; GPU selection added |
| Open-defect list triaged | none open; seven candidates filed (§7) |
| Annotated tag pushed on `master` | `v0.5.0`, by the maintainer |

## 9. Addendum: the first release run

The `v0.5.0` tag's release run failed both builds at **Verify package layout**:
`missing packages/sandbox/libraries`. B6b had added that path to the check, and
B6c then deleted the directory when the engine package took over the library
declarations. Nothing runs the check before a tag, which §6 already recorded as
unverified. The check now lists `packages/sandbox/play.ron` in its place, and the
release was rebuilt from `master` through the workflow's manual run, attached to
the existing `v0.5.0` tag; the engine code is the same at both commits.
