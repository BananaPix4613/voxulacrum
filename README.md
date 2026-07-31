# voxulacrum

A stylized isometric voxel engine in Rust, with node-graph world generation.

Terrain is authored as a hierarchy of node graphs — World, Zone, Biome, Detail —
edited inside the running engine and hot-reloaded on save. Generation is
deterministic: the same seed and graphs produce bit-identical chunks regardless
of thread order, which is what makes streaming, caching and eventual multiplayer
tractable.

**Status: 0.3.0.** The engine generates, streams, meshes, renders and persists an
infinite world with fluid, foliage and a player character, and it is authorable
in-engine. It is not yet a game, and the content surface is deliberately thin —
see [`docs/roadmap.md`](docs/roadmap.md) for what each version is for.

## Running it

Prebuilt Windows and Linux packages are on the
[releases page](https://github.com/BananaPix4613/voxulacrum/releases). Unzip
somewhere writable — saves and the mesh cache are written next to the executable,
so `Program Files` or `/opt` will not work. Linux needs a Vulkan driver
(`mesa-vulkan-drivers` on Debian and Ubuntu).

From source:

```
cargo run -p voxulacrum --bin voxulacrum-app --release
```

Use `--release` for anything you intend to measure. The dev profile runs roughly
7× slower.

Headless determinism check:

```
cargo run -p voxulacrum --bin voxulacrum-app -- --verify-generation 64
```

## Documentation

| | |
|---|---|
| [`docs/engine-design.md`](docs/engine-design.md) | The architectural goalpost. Data model, generation pipeline, rendering, the settled decisions and the interim shapes |
| [`docs/roadmap.md`](docs/roadmap.md) | Version sequencing, technical pillars, exit gates, open decisions |
| [`docs/perf-baseline.md`](docs/perf-baseline.md) | Measured figures per version, how to reproduce a reading, and the ways measurement goes wrong |
| [`CHANGELOG.md`](CHANGELOG.md) | Release history |

## Licensing

The engine is **free to use, modify and build on — including commercially.**

**Source code: [MPL-2.0](LICENSE).** Use it in your own project, open or closed.
Build and sell a game with it. The one obligation is reciprocal: if you modify
the engine's own source files, those modified files stay under MPL-2.0 and stay
available. Code you write in separate files — your game, your mods — is yours,
under whatever terms you choose. MPL §3.3 covers this explicitly.

**Sample content: [CC BY 4.0](assets/LICENSE).** The graphs, palettes, shaders
and registries in `assets/`, `palettes/` and `shaders/` are the worked examples
of how the engine is meant to be used. Take them apart, adapt them, ship them —
just credit the source.

**The name is not licensed.** Neither license grants rights to the *voxulacrum*
name or any associated branding. Forks and derivative works are welcome and must
be distributed under a different name.

Contributions are accepted under the DCO — see [CONTRIBUTING.md](CONTRIBUTING.md).

Third-party dependency licenses are listed in `THIRD-PARTY-NOTICES.md` and ship
with every release package.
