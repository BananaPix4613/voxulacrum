# Contributing

Contributions are welcome. Two things to know before you open a pull request.

## Sign your commits (DCO)

Every commit needs a `Signed-off-by` line. Git adds it for you:

```
git commit -s -m "your message"
```

This is the [Developer Certificate of Origin](https://developercertificate.org/) —
a one-line statement that you wrote the change, or otherwise have the right to
submit it under this project's license. It is not a copyright assignment: you
keep the copyright to your contribution, and it is licensed to the project under
MPL-2.0, the same terms as the rest of the engine.

Commits without a sign-off cannot be merged.

## What goes where

The repository is an **engine**. `docs/roadmap.md` §3.11 draws the line: engine
concerns are the capability surface in §6 — world queries, mutation, streaming,
rendering, persistence, entities, tooling. Game concerns are progression,
crafting recipes, combat balance, specific creature behaviours, narrative, and
the art direction of a particular title. Those belong in a game built on the
engine, not here.

`docs/engine-design.md` is the architectural goalpost, and `docs/roadmap.md` is
authoritative for version sequencing and scope. A change that contradicts either
needs the document revised in the same pull request — an undocumented divergence
is a defect regardless of whether the code works.

## Before you open a PR

- `cargo build --workspace` and `cargo test --workspace` both clean, with no new
  warnings.
- `cargo run -p voxulacrum --bin voxulacrum-app -- --verify-generation 64` passes
  if you touched anything in the generation path. Determinism is pillar P1;
  generation must be bit-identical across runs and independent of thread order.
- Performance claims come with measurements from a **release** build. See
  `docs/perf-baseline.md` — the dev profile runs roughly 7× slower, and
  wall-clock frame time under vsync measures blocking rather than work.

CI runs the first two on every push.
