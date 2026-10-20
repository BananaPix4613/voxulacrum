# Testbed content

The world the testbed loads: the package `testbed`, described by `package.ron`
and started through `RunOptions::package`. Saves go to `saves/testbed/`, never
to the sandbox package's `saves/default/`.

| Path | What |
|---|---|
| `graphs/world.manifest.json` | One zone, one biome, 64 voxels tall, seed 1 |
| `graphs/biome_flat.graph.json` | Flat surface at y = 32: one light voxel, three mid, dark below |
| `materials.ron` | Air, three greys, and red, green and blue for placed blocks |
| `prefabs.ron` | One entry, because the registry may not be empty |
| `foliage.ron` | No layers or scatter types: the flat world paints and scatters nothing |
| `play.ron` | The sandbox's player tuning, and a build tool over the six test materials |

The testbed declares no libraries. Every library it could name comes from the
engine package, as `voxulacrum:<name>`.