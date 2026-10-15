# Testbed content

The world the testbed loads, through `RunOptions::content("testbed", ...)`.
Saves go to `saves/testbed/`, never to the reference world's `saves/default/`.

| Path | What |
|---|---|
| `graphs/world.manifest.json` | One zone, one biome, 64 voxels tall, seed 1 |
| `graphs/biome_flat.graph.json` | Flat surface at y = 32: one light voxel, three mid, dark below |
| `materials.ron` | Air, three greys, and red, green and blue for placed blocks |
| `prefabs.ron`, `libraries/` | One entry each, because neither registry may be empty |

`libraries/` repeats an engine kernel declaration because library assets
currently live with content. Packages (B6) decide where they belong.