# Complete terrain generation system for Voxulacrum

**Voxulacrum's terrain should use a "Parameterized Beta" architecture**: the elegant 3D density-field approach from Minecraft Beta 1.7.3 — where a height gradient modulated by 3D noise inherently produces overhangs, arches, and floating outcrops — combined with Hytale V2's philosophy that biomes drive terrain shape through per-biome parameter structs. This synthesis yields **7 FastNoise2 node graphs** (5 cacheable 2D maps, 2 fused 3D graphs), a density function that quantizes cleanly to i8 with full-range utilization, and integrated cave generation — all within a **~1.4ms per-chunk budget** on 4 cores with AVX2. The system is deterministic, data-driven, and compatible with range analysis for an 80–90% chunk skip rate.

---

## Architecture overview: the parameterized Beta approach

The core insight from Beta 1.7.3's decompiled terrain code is remarkably simple. Three 3D Perlin noise fields (called "low limit," "high limit," and "selector") are interpolated together, then an asymmetric height gradient is subtracted. Positive values become stone; negative values become air. The gradient pushes terrain toward solid below a base height and air above, but 3D noise can overpower it — producing overhangs, arches, and floating islands as *emergent properties* of the math rather than special-cased features.

Beta's weakness was that its parameters were globally uniform. Every biome got the same terrain shape; mountains appeared in deserts and forests alike. Hytale V2 solves this by inverting the relationship: **biomes generate terrain, not the other way around**. Each biome defines its own density function parameters — base height, squash factor, 3D noise amplitude, cave density — and transitions blend these parameters smoothly at borders.

Voxulacrum's architecture fuses both approaches into a three-phase pipeline:

**Phase 0 — Column evaluation (2D, cached).** Five 2D FN2 graphs produce continental, temperature, moisture, erosion, and elevation-detail noise maps. These are evaluated once per 32×32 XZ column and cached. From these maps, the system derives biome weights per column and interpolates biome parameters. Cost: **~0.05ms**, amortized across all vertical chunks in a column.

**Phase 1 — Range analysis (skip test).** For each 32³ chunk, conservative range bounds from the 2D heightmap cache determine whether the chunk can possibly contain the surface. Chunks entirely above the maximum possible terrain height or entirely below the minimum are classified as all-air or all-solid respectively and skip 3D noise evaluation entirely. This eliminates **80–90%** of chunks from expensive processing.

**Phase 2 — Density evaluation (3D, surface chunks only).** Two fused FN2 3D graphs are evaluated at full 32³ resolution: the terrain-shape graph (low/high/selector blend with domain warp) and the cave-system graph (cheese caverns + spaghetti tunnels). A Rust-side density function combines these with the biome-interpolated height gradient and quantizes to i8. Cost: **~1.2ms** per surface chunk.

The total pipeline produces a deterministic, seed-derived density field where positive values represent solid terrain and negative values represent air, with the zero-crossing defining the isosurface that marching cubes meshes.

---

## The density function: from noise to voxels

The density function is the mathematical heart of the system. It takes a world-space position (x, y, z), biome-blended parameters, and noise values, and produces an i8 density. The design follows Beta 1.7.3's structure with biome parameterization added.

### Core formulation

```
density(x, y, z) =
    terrain_noise_3d(x, y, z) × amplitude_3d       // 3D shape from fused FN2 graph
  − height_gradient(y, base_height, squash_factor)  // vertical bias: solid below, air above
  + cave_contribution(x, y, z, cave_params)         // integrated cave carving
  + floor_ceiling_clamp(y)                           // bedrock floor + sky ceiling
```

### Height gradient (Beta's key mechanism)

The height gradient is the force that turns an amorphous 3D noise cloud into recognizable terrain with a ground plane. Beta 1.7.3's decompiled code reveals an asymmetric gradient that is **4× steeper below base height** than above:

```rust
fn height_gradient(y: f32, base_height: f32, squash: f32) -> f32 {
    let offset = (y - base_height) * squash;
    if offset < 0.0 {
        offset * 4.0   // strongly biased toward solid below base height
    } else {
        offset          // moderate bias toward air above base height
    }
}
```

The `squash` parameter is the single most powerful terrain-shaping control. **Low squash (0.3–0.5)** produces weak gradients that 3D noise easily overpowers — yielding dramatic overhangs, floating outcrops, and wild vertical features (ideal for Shattered Peaks biome). **High squash (1.5–2.0)** produces strong gradients that suppress 3D noise — yielding conventional, walkable terrain (ideal for Plains). This mirrors Beta's `cn` continentalness parameter, which ranged from 0.5 to 1.5 and controlled terrain "drama."

### The 3D terrain noise: low/high/selector blend

Following Beta 1.7.3's proven three-noise architecture, the terrain shape comes from interpolating two independent noise fields using a third as a selector:

```rust
fn terrain_shape(low: f32, high: f32, selector: f32) -> f32 {
    let t = (selector * 0.5 + 0.5).clamp(0.0, 1.0);  // normalize selector to [0,1]
    low + (high - low) * t                              // linear interpolation
}
```

The low and high noises use the same frequency and octave count but different seeds, producing different terrain shapes. The selector noise operates at roughly **half the frequency** (twice the feature size), creating large regions that smoothly transition between two distinct terrain characters. This is what gave Beta its unpredictable variety — the same biome could have two completely different terrain textures depending on the selector value.

### Cave contribution

Caves are subtracted from the density field, never carved as a post-process. The cave noise graph outputs a single value that represents "how much to remove from terrain density":

```rust
fn cave_contribution(cave_noise: f32, y: f32, cave_density: f32, 
                     cheese_hollowness: f32) -> f32 {
    // Depth factor: caves increase below surface, taper near bedrock
    let depth_factor = depth_cave_mask(y);
    
    // cave_noise is pre-combined cheese+spaghetti from FN2 graph
    // Negative cave_noise = carve; positive = leave solid
    let contribution = cave_noise.min(0.0) * cave_density * depth_factor;
    contribution
}

fn depth_cave_mask(y: f32) -> f32 {
    // No caves in top 40 voxels, full caves below Y=80, taper near bedrock
    let surface_fade = ((160.0 - y) / 40.0).clamp(0.0, 1.0);
    let bedrock_fade = ((y + 160.0) / 32.0).clamp(0.0, 1.0);
    surface_fade * bedrock_fade
}
```

### Quantization to i8

The density function produces floating-point values that must be quantized to i8 (-128 to 127) for storage. The quantization strategy must **use the full range** while preserving precision near the zero-crossing where marching cubes interpolates vertices:

```rust
fn quantize_density(d: f32) -> i8 {
    // The raw density is designed to range roughly [-10, +10]
    // near the surface and larger magnitudes deep inside/outside.
    // Scale so ±1.0 maps to ±127, clamp to i8 range.
    let scaled = (d * 12.7).clamp(-128.0, 127.0);
    scaled as i8
}
```

The **12.7 scale factor** means 1 unit of raw density maps to about 13 i8 steps. Since the height gradient changes by roughly `squash` per voxel vertically, and squash ranges from 0.3 to 2.0, the surface transition spans 6–40 i8 levels per voxel — more than enough precision for quarter-voxel snapping. Deep inside solid terrain, values saturate to -128; high in the air, they saturate to +127. Both extremes encode "maximally far from surface," which is correct for range analysis.

### Complete pseudocode

```rust
fn evaluate_chunk(chunk_pos: IVec3, column_cache: &ColumnCache,
                  terrain_graph: &SafeNode, cave_graph: &SafeNode, 
                  seed: i32) -> [i8; 32768] {
    let mut densities = [0i8; 32768];
    
    // --- Phase 2a: Evaluate 3D noise graphs ---
    let world_offset = chunk_pos.as_vec3() * 32.0;
    
    let mut terrain_noise = vec![0.0f32; 32768];
    terrain_graph.gen_uniform_grid_3d(
        &mut terrain_noise,
        world_offset.x, world_offset.y, world_offset.z,
        32, 32, 32,
        1.0, 1.0,  // 1 voxel per step
        seed,
    );
    
    let mut cave_noise = vec![0.0f32; 32768];
    cave_graph.gen_uniform_grid_3d(
        &mut cave_noise,
        world_offset.x, world_offset.y, world_offset.z,
        32, 32, 32,
        1.0, 1.0,
        seed,
    );
    
    // --- Phase 2b: Combine with biome parameters ---
    for z in 0..32 {
        for x in 0..32 {
            let col = &column_cache.get(chunk_pos.x, chunk_pos.z, x, z);
            // Biome-blended parameters for this column
            let base_height = col.base_height;
            let squash = col.squash_factor;
            let amp = col.amplitude_3d;
            let cave_dens = col.cave_density;
            let cheese_hollow = col.cheese_hollowness;
            
            for y in 0..32 {
                let world_y = world_offset.y + y as f32;
                let idx = z * 32 * 32 + y * 32 + x; // FN2 output order: z→y→x
                
                // Height gradient (asymmetric, Beta-style)
                let mut gradient = (world_y - base_height) * squash;
                if gradient < 0.0 { gradient *= 4.0; }
                
                // Terrain density
                let mut d = terrain_noise[idx] * amp - gradient;
                
                // Cave contribution
                let depth_mask = depth_cave_mask(world_y);
                d += cave_noise[idx].min(0.0) * cave_dens * depth_mask;
                
                // Hard floor (bedrock) and ceiling fade
                if world_y < -176.0 {
                    d += (-176.0 - world_y) * 2.0;
                }
                if world_y > 280.0 {
                    d -= (world_y - 280.0) * 0.5;
                }
                
                densities[idx] = quantize_density(d);
            }
        }
    }
    densities
}
```

---

## Noise graph specification

Seven FN2 node graphs compose the complete noise system. Five are 2D (evaluated once per 32×32 column, cached), and two are 3D (evaluated per surface chunk). All graphs use `Node::from_name()` with `node.set()` for parameter configuration.

### 2D graphs (Phase 0, column-cached)

**Graph 1 — Continental noise.** Determines land versus ocean at the largest scale. Features span **2000–5000 voxels**. Values below -0.2 indicate ocean basins; above 0.1 indicate land masses; the transition zone creates coastlines.

| Node | Type | Key Parameters |
|------|------|---------------|
| source | `Simplex` | SeedOffset: 0 |
| scale | `DomainScale` | Source: source, Scale: 0.0004 |
| fractal | `FractalFBm` | Source: scale, Octaves: 3, Gain: 0.45, Lacunarity: 2.2 |

**Graph 2 — Temperature noise.** Ranges from -1 (frozen) to +1 (scorching). Features span **1000–2000 voxels** for continent-scale climate bands, with a subtle latitude gradient achievable by adding a PositionOutput Z component.

| Node | Type | Key Parameters |
|------|------|---------------|
| source | `Simplex` | SeedOffset: 100 |
| scale | `DomainScale` | Source: source, Scale: 0.0006 |
| fractal | `FractalFBm` | Source: scale, Octaves: 2, Gain: 0.5, Lacunarity: 2.0 |

**Graph 3 — Moisture noise.** Ranges from -1 (arid) to +1 (saturated). Same topology as temperature with a different seed offset. Features span ~1000–2000 voxels.

| Node | Type | Key Parameters |
|------|------|---------------|
| source | `Simplex` | SeedOffset: 200 |
| scale | `DomainScale` | Source: source, Scale: 0.0006 |
| fractal | `FractalFBm` | Source: scale, Octaves: 2, Gain: 0.5, Lacunarity: 2.0 |

**Graph 4 — Erosion noise.** Controls terrain roughness independent of biome. High erosion values flatten terrain; low values allow dramatic features. Features span **300–800 voxels** for regional terrain character variation within biomes.

| Node | Type | Key Parameters |
|------|------|---------------|
| source | `Simplex` | SeedOffset: 300 |
| scale | `DomainScale` | Source: source, Scale: 0.002 |
| fractal | `FractalFBm` | Source: scale, Octaves: 3, Gain: 0.5, Lacunarity: 2.0 |

**Graph 5 — Elevation detail noise.** Adds medium-scale height variation within biomes. Features span **100–300 voxels** for individual hills, valleys, and ridgelines.

| Node | Type | Key Parameters |
|------|------|---------------|
| source | `Simplex` | SeedOffset: 400 |
| scale | `DomainScale` | Source: source, Scale: 0.005 |
| fractal | `FractalFBm` | Source: scale, Octaves: 4, Gain: 0.5, Lacunarity: 2.0 |
| warp_src | `Simplex` | SeedOffset: 450 |
| warp_scale | `DomainScale` | Source: warp_src, Scale: 0.003 |
| warp | `DomainWarpGradient` | Source: fractal, WarpAmplitude: 40.0, WarpSource: warp_scale |

All five 2D graphs are evaluated via `gen_uniform_grid_2d` with x_count=32, y_count=32 (the XZ plane of a chunk column), at step_size 1.0. Total cost: **~0.05ms** for all five on AVX2 hardware (5 × 1024 samples at ~10ns each).

### 3D graphs (Phase 2, per surface chunk)

**Graph 6 — Terrain shape (fused low/high/selector + domain warp).** This is the primary terrain driver, implementing Beta 1.7.3's three-noise interpolation as a single fused FN2 graph. The Fade node performs the low/high interpolation controlled by the selector. DomainAxisScale compresses Y coordinates to bias toward horizontal features (overhangs rather than pillars). DomainWarpGradient with fractal progression adds organic distortion.

```
Node graph topology:

simplex_low ─→ DomainScale(0.015) ─→ FractalFBm(4oct, gain:0.5) ──────────┐
                                                                             │
simplex_high ─→ DomainScale(0.015) ─→ FractalFBm(4oct, gain:0.5) ─────────┤
                                                                             ├─→ Fade ─→ DomainWarpGradient ─→ Output
simplex_sel ─→ DomainScale(0.008) ─→ FractalFBm(2oct, gain:0.5) ──────────┘     ↑
                                                                        warp_simplex ─→ DomainScale(0.004)
                                                                        WarpAmplitude: 25.0
```

Detailed node specification:

| Node Name | Type | Parameters |
|-----------|------|-----------|
| low_src | `Simplex` | SeedOffset: 500 |
| low_scale | `DomainScale` | Source: low_src, Scale: 0.015 |
| low_axis | `DomainAxisScale` | Source: low_scale, ScaleY: 0.7 |
| low_frac | `FractalFBm` | Source: low_axis, Octaves: 4, Gain: 0.5, Lacunarity: 2.0 |
| high_src | `Simplex` | SeedOffset: 600 |
| high_scale | `DomainScale` | Source: high_src, Scale: 0.015 |
| high_axis | `DomainAxisScale` | Source: high_scale, ScaleY: 0.7 |
| high_frac | `FractalFBm` | Source: high_axis, Octaves: 4, Gain: 0.5, Lacunarity: 2.0 |
| sel_src | `Simplex` | SeedOffset: 700 |
| sel_scale | `DomainScale` | Source: sel_src, Scale: 0.008 |
| sel_frac | `FractalFBm` | Source: sel_scale, Octaves: 2, Gain: 0.5, Lacunarity: 2.0 |
| sel_remap | `Remap` | Source: sel_frac, FromMin: -1, FromMax: 1, ToMin: 0, ToMax: 1 |
| blend | `Fade` | A: low_frac, B: high_frac, Fade: sel_remap |
| warp_src | `Simplex` | SeedOffset: 750 |
| warp_scale | `DomainScale` | Source: warp_src, Scale: 0.004 |
| final | `DomainWarpGradient` | Source: blend, WarpAmplitude: 25.0 |

This graph has an effective **12 octave-equivalents** of noise computation (4+4+2 plus warp overhead). At ~10ns per octave-equivalent per sample on AVX2: **12 × 10ns × 32,768 samples / 8 lanes ≈ 0.49ms**.

The **DomainAxisScale with ScaleY: 0.7** is critical — it compresses vertical coordinates so noise features are wider horizontally than vertically, producing natural-looking overhangs and shelves rather than vertical spikes. This is equivalent to Beta's `heightScale` being the same as `coordinateScale` while the noise's natural vertical correlation created similar compression.

The **DomainScale of 0.015** means base noise features span ~67 voxels (1/0.015), and with 4 fractal octaves the finest detail reaches ~8-voxel features. This maps well to the 32-voxel chunk size — each chunk contains 2–4 major terrain features with fine detail.

**Graph 7 — Cave system (fused cheese + spaghetti).** Implements Minecraft 1.18's cave architecture within a single FN2 graph. Cheese caves use raw 3D noise with a hollowness bias for large caverns. Spaghetti caves use the product of two absolute-value noise fields to create tunnel networks along the near-zero crossings.

```
Node graph topology:

Cheese branch:
  simplex_cheese ─→ DomainScale(0.012) ─→ FractalFBm(3oct) ─→ Add(hollowness constant) ──┐
                                                                                             │
Spaghetti branch:                                                                            ├─→ Min ─→ Output
  simplex_spag_A ─→ DomainScale(0.025) ─→ FractalFBm(2oct) ─→ Abs ──┐                      │
                                                                       ├─→ Multiply ─→ Remap │
  simplex_spag_B ─→ DomainScale(0.025) ─→ FractalFBm(2oct) ─→ Abs ──┘             (negate) ┘
```

| Node Name | Type | Parameters |
|-----------|------|-----------|
| cheese_src | `Simplex` | SeedOffset: 800 |
| cheese_scale | `DomainScale` | Source: cheese_src, Scale: 0.012 |
| cheese_frac | `FractalFBm` | Source: cheese_scale, Octaves: 3, Gain: 0.45, Lacunarity: 2.0 |
| hollow_const | `Constant` | Value: 0.15 |
| cheese_biased | `Add` | A: cheese_frac, B: hollow_const |
| spag_a_src | `Simplex` | SeedOffset: 900 |
| spag_a_scale | `DomainScale` | Source: spag_a_src, Scale: 0.025 |
| spag_a_frac | `FractalFBm` | Source: spag_a_scale, Octaves: 2, Gain: 0.5, Lacunarity: 2.0 |
| spag_b_src | `Simplex` | SeedOffset: 1000 |
| spag_b_scale | `DomainScale` | Source: spag_b_src, Scale: 0.025 |
| spag_b_frac | `FractalFBm` | Source: spag_b_scale, Octaves: 2, Gain: 0.5, Lacunarity: 2.0 |
| spag_product | `Multiply` | A: spag_a_frac, B: spag_b_frac |
| spag_negate | `Remap` | Source: spag_product, FromMin: 0, FromMax: 1, ToMin: 0, ToMax: -2.0 |
| thickness | `Constant` | Value: 0.06 |
| spag_tunnel | `Add` | A: spag_negate, B: thickness |
| cave_out | `Min` | A: cheese_biased, B: spag_tunnel |

The spaghetti technique works because `|noise_A| × |noise_B|` is near zero only when *both* noise fields are near their zero-crossings — which form 2D surfaces in 3D space. The intersection of two surfaces is a 1D curve: a tunnel. The `thickness` constant (0.06) controls tunnel width (~2–4 voxels at this frequency). The `Remap` node negates and scales the product so that near-zero values become strongly negative (carving), while far-from-zero values remain near zero (no carving).

The cheese branch produces caverns where `noise + hollowness > 0` → positive → no carving. Where `noise + hollowness < 0` → negative → carving. The `hollowness` bias of 0.15 means roughly **35%** of underground space tends toward hollow (since Simplex output is roughly uniform over [-1,1]).

The `Min` operator ensures that **either** cheese or spaghetti caves carve terrain — whichever produces the stronger carving effect at each point. The cave graph has **7 octave-equivalents**. Estimated cost: **~0.29ms** per 32³ chunk.

---

## Biome system: climate-driven terrain control

### Biome placement

Biomes are determined from a 2D climate space defined by **temperature** (T), **moisture** (M), and **continentalness** (C) noise maps. The continental noise gates the primary land/ocean distinction; temperature and moisture together select from a Whittaker-style biome grid on land. This produces naturalistic climate bands where tropical biomes cluster near warm/wet regions and tundra occupies cold/dry zones.

```rust
fn select_biome(continental: f32, temp: f32, moisture: f32, erosion: f32) -> BiomeId {
    if continental < -0.2 {
        return BiomeId::Ocean;
    }
    if continental < 0.05 {
        return BiomeId::Coast; // beaches, shallow water
    }
    // Land biomes from temperature × moisture grid
    match (temp_band(temp), moisture_band(moisture)) {
        (Cold, Dry)   => BiomeId::FrozenExpanse,
        (Cold, Wet)   => BiomeId::AncientForest,
        (Warm, Dry)   => BiomeId::SunkenDesert,
        (Warm, Wet)   => BiomeId::VerdantHighlands,
        (Hot, Dry)    => BiomeId::VolcanicWastes,
        (Hot, Wet)    => BiomeId::WhisperingMarsh,
        _ => BiomeId::VerdantHighlands, // default fallback
    }
}
```

An additional check: where erosion noise is very low (< -0.5) AND temperature is warm-to-hot, the biome overrides to **Shattered Peaks** — representing areas of extreme geological activity where terrain drama is maximized regardless of moisture.

### Biome parameter struct

Each biome is defined entirely by a data struct. No biome-specific code paths exist in the terrain generator — only parameter differences:

```rust
struct BiomeParams {
    // Terrain shape
    base_height: f32,        // center of terrain band in world Y
    squash_factor: f32,      // height gradient strength (lower = more dramatic)
    amplitude_3d: f32,       // 3D noise contribution strength
    elevation_detail: f32,   // how much 2D elevation detail noise contributes
    
    // Caves
    cave_density: f32,       // multiplier for cave noise contribution
    cheese_hollowness: f32,  // bias for large cavern formation
    spaghetti_thickness: f32,// tunnel width multiplier
    
    // Water
    local_water_level: f32,  // biome-specific water level (sea level default: 128.0)
    
    // Materials (indices into material palette)
    top_material: u16,
    sub_material: u16,
    sub_depth: u8,
    stone_material: u16,
    deep_stone_material: u16,
    
    // Blend
    blend_radius: f32,       // transition width in voxels (16–48)
}
```

### The eight biomes

| Biome | base_height | squash | amp_3d | cave_dens | Character |
|-------|-------------|--------|--------|-----------|-----------|
| **Ocean** | 60 | 2.0 | 0.3 | 0.2 | Deep basins, coral shelves, minimal overhangs |
| **Verdant Highlands** | 140 | 0.8 | 1.2 | 0.8 | Rolling hills, frequent overhangs, lush caves |
| **Shattered Peaks** | 190 | 0.3 | 2.5 | 0.6 | Extreme vertical drama, floating outcrops, arches |
| **Sunken Desert** | 135 | 1.2 | 0.8 | 0.5 | Mesas and canyons, dry cave networks, sandstone |
| **Whispering Marsh** | 125 | 1.8 | 0.4 | 0.9 | Flat, waterlogged, extensive flooded caves |
| **Ancient Forest** | 150 | 0.6 | 1.5 | 1.0 | Tall canopy terrain, root-like cave formations |
| **Volcanic Wastes** | 145 | 0.7 | 1.8 | 1.2 | Lava lakes, basalt pillars, magma tube caves |
| **Frozen Expanse** | 130 | 1.5 | 0.6 | 0.4 | Smooth glacial terrain, ice crevasses, sparse caves |

The `squash_factor` is the primary drama control. **Shattered Peaks at 0.3** means the height gradient is weak enough that 3D noise (amplitude 2.5) routinely overpowers it, creating floating islands and massive arches. **Whispering Marsh at 1.8** means terrain is strongly biased flat, with only gentle undulations.

### Biome blending

Biome transitions use **scattered data interpolation** rather than naive grid-aligned blending, following the technique documented by KdotJPG. For each terrain column, nearby biome evaluation points (on a jittered triangular grid with ~64-voxel spacing) contribute weights based on smooth circular falloff:

```rust
fn biome_weights(world_x: f32, world_z: f32, biome_grid: &BiomeGrid) -> SmallVec<[(BiomeId, f32); 4]> {
    let nearby_points = biome_grid.query_radius(world_x, world_z, BLEND_RADIUS);
    let mut weights = SmallVec::new();
    let mut total = 0.0;
    
    for point in nearby_points {
        let dx = world_x - point.x;
        let dz = world_z - point.z;
        let dist_sq = dx * dx + dz * dz;
        let r_sq = BLEND_RADIUS * BLEND_RADIUS;
        if dist_sq < r_sq {
            let w = (r_sq - dist_sq).powi(2);  // smooth quartic falloff
            weights.push((point.biome, w));
            total += w;
        }
    }
    // Normalize
    for (_, w) in &mut weights { *w /= total; }
    weights
}
```

This produces smooth, organically shaped biome borders without grid artifacts. Each biome parameter is then computed as a weighted average: `base_height = Σ weight_i × biome_i.base_height`. Since these calculations are per-column (not per-voxel), the cost is negligible.

**Biome influence fades with depth.** Below 60 voxels from the surface, biome parameters gradually converge toward global defaults. This prevents biome borders from creating visible vertical seams deep underground and ensures the deep underground feels unified:

```rust
let depth_from_surface = (base_height - world_y).max(0.0);
let biome_influence = 1.0 - ((depth_from_surface - 60.0) / 40.0).clamp(0.0, 1.0);
let effective_params = lerp(global_underground_params, biome_params, biome_influence);
```

---

## Cave architecture: integrated underground variety

Caves emerge from two complementary noise techniques fused into Graph 7, modulated by biome parameters and depth.

### Cheese caves produce grand caverns

The cheese noise (Simplex FBm, 3 octaves, scale 0.012) creates large-scale positive/negative regions throughout the underground. Where `cheese + hollowness_bias < 0`, terrain density is reduced — creating vast open caverns with natural pillars where the noise happens to be positive. At scale 0.012, major cavern features span **~80 voxels** (roughly 2.5 chunk-widths), producing cathedral-scale underground spaces.

The **hollowness bias** is the key per-biome control. Volcanic Wastes uses high hollowness (0.25), creating frequent massive magma chambers. Frozen Expanse uses low hollowness (0.05), producing only rare, tight cavities. Verdant Highlands at 0.15 provides the balanced cavern experience.

### Spaghetti caves create tunnel networks

The spaghetti technique (two independent Simplex FBm fields at scale 0.025, their absolute values multiplied) produces winding tunnels ~3–6 voxels in diameter that snake through solid rock. The tunnel paths follow the intersection curves of two noise zero-crossings — mathematically, these are 1D manifolds embedded in 3D space, producing genuinely winding paths that branch and reconnect naturally.

The `spaghetti_thickness` biome parameter controls tunnel width. At 0.08 (Ancient Forest), tunnels are narrow root-like passages. At 0.03 (Frozen Expanse), they are thin ice crevasses. The thickness parameter modulates the threshold constant in the FN2 graph; since this is a graph constant, different biome regions would need either runtime parameter adjustment or separate graph evaluations. The practical solution is to **evaluate the cave graph once and apply the biome-specific thickness scaling in the Rust density function**:

```rust
let raw_spaghetti = cave_noise_output;
let biome_threshold = col.spaghetti_thickness;
let spaghetti_contribution = (raw_spaghetti + biome_threshold).min(0.0);
```

### Underground layers create vertical variety

Three depth zones give the underground distinct character, primarily through material assignment and cave parameter scaling:

**Shallow underground (0–60 below surface).** Cave density is low; spaghetti tunnels dominate. Surface caves connect directly to the first tunnel networks. Materials: soil→stone transition, with biome-specific stone variants. This layer feels like "just below the surface" — connected to daylight, familiar.

**Mid underground (60–140 below surface).** Full cave density. Cheese caverns appear, creating massive open spaces. Spaghetti tunnels become wider. Materials: standard stone with ore veins. Underground crystal formations appear where `|cheese_noise|` is very small (near cavern walls). This is the primary mining and exploration layer.

**Deep underground (140+ below surface).** Cave density increases further, but cheese hollowness shifts — caverns become more extreme (fewer but larger). Materials transition to deep stone and magma-adjacent materials. Lava replaces water below Y=-80. A 3D Voronoi-cellular noise (evaluated as a separate lightweight FN2 graph only in deep chunks, ~0.15ms) creates **geode formations** — rare spherical cavities lined with crystal materials. The deep layer should feel alien and dangerous.

```rust
fn underground_layer(depth_from_surface: f32) -> UndergroundLayer {
    if depth_from_surface < 60.0 { UndergroundLayer::Shallow }
    else if depth_from_surface < 140.0 { UndergroundLayer::Mid }
    else { UndergroundLayer::Deep }
}
```

### Emergent rare formations

The system produces rare formations as **natural statistical events**, not special-cased features:

- **Arches and bridges** emerge where spaghetti tunnels intersect the surface at two points with solid terrain between. Frequency increases in Shattered Peaks (low squash + high amplitude) and Volcanic Wastes.
- **Floating outcrops** appear where strong positive 3D noise above the base height overpowers the height gradient. In Shattered Peaks (squash 0.3, amplitude 2.5), any noise blob above ~0.9 strength creates a floating island.
- **Cliff faces** form where the selector noise transitions between the low and high terrain shapes, creating a sharp boundary between two different height regimes.
- **Natural pillars** in cheese caverns are regions where positive noise survives within a larger negative (hollow) space, creating columns from floor to ceiling.

These formations require no special code — they are statistical certainties given the parameter ranges, especially in high-drama biomes.

---

## Water body generation

Water uses a hybrid global/local level strategy:

**Global sea level at Y=128** fills any air voxel below this threshold with water, creating coherent oceans wherever continental noise drives terrain below sea level. The continental noise (Graph 1) at scale 0.0004 creates ocean basins spanning **2000+ voxels**, producing realistic continent shapes with coastlines that follow the noise contour.

**Rivers** are carved using a dedicated 2D noise technique during the column evaluation phase. A Simplex FBm at scale 0.003 (Graph 5's warp source, repurposed) produces a scalar field; where `|river_noise| < 0.02`, a river channel exists. The river carving reduces `base_height` by 8–15 voxels along the narrow band, creating valleys that naturally fill with water at sea level or slightly above. River width varies with the noise gradient magnitude — where the noise changes slowly, rivers are wide; where it changes quickly, they narrow.

```rust
let river_value = river_noise_2d.abs();
let river_factor = (0.02 - river_value).max(0.0) / 0.02;  // 1.0 at center, 0.0 at edge
let river_carving = river_factor * 12.0;  // carve up to 12 voxels deep
let effective_base_height = base_height - river_carving;
```

**Lakes** form naturally in terrain depressions below the local water level. Since biome parameters define per-biome water levels, the Whispering Marsh biome uses `local_water_level: 132.0` — slightly above global sea level — ensuring widespread shallow flooding across the biome. The Volcanic Wastes biome uses the global sea level but replaces water material below Y=40 with lava.

**Underground water** fills cave voids below the biome's local water level, creating flooded cave sections. In the deep underground (below Y=-80), lava replaces water universally.

---

## Material assignment strategy

Material assignment runs during the density evaluation pass, using the computed density, biome parameters, and positional data to select from up to 65,536 material IDs (u16). The strategy layers multiple rules by priority:

### Surface detection

A voxel is "surface" if its density is positive (solid) and at least one face-adjacent neighbor is negative (air or water). Surface detection uses the density field directly — no separate heightmap needed. For marching cubes, the surface is the zero-crossing, so the "surface material" applies to all solid voxels within ~2 voxels of an air boundary.

### Assignment rules (evaluated in priority order)

```rust
fn assign_material(world_y: f32, density: f32, gradient_normal: Vec3,
                   biome: &BiomeParams, depth_from_surface: f32,
                   ore_noise: f32, underground_layer: UndergroundLayer) -> u16 {
    
    // 1. Bedrock (absolute floor)
    if world_y < -184.0 { return MAT_BEDROCK; }
    
    // 2. Surface block (within 1 voxel of air)
    if depth_from_surface < 1.5 {
        let steepness = 1.0 - gradient_normal.y;  // 0 = flat, 1 = vertical
        if steepness > 0.6 {
            return biome.cliff_material;  // steep = exposed rock
        }
        return biome.top_material;  // grass, sand, snow, etc.
    }
    
    // 3. Subsurface (2-5 voxels below surface)
    if depth_from_surface < biome.sub_depth as f32 + 0.5 {
        return biome.sub_material;  // dirt, sandstone, packed ice, etc.
    }
    
    // 4. Ore veins (checked before generic stone)
    if let Some(ore) = check_ore_placement(world_y, ore_noise, underground_layer) {
        return ore;
    }
    
    // 5. Underground stone (depth-dependent)
    match underground_layer {
        Shallow => biome.stone_material,
        Mid => {
            // Gradual transition from stone to deep stone
            let t = ((depth_from_surface - 60.0) / 80.0).clamp(0.0, 1.0);
            if hash(world_x, world_y, world_z) < t { biome.deep_stone_material }
            else { biome.stone_material }
        }
        Deep => biome.deep_stone_material,
    }
}
```

### Ore and mineral placement

Ores use a single 3D Simplex noise (Graph 8 — lightweight, 1 octave, no fractal, scale 0.08) evaluated only in solid voxels of surface chunks. Different ore types are selected by **Y-range and threshold**:

| Ore | Y Range | Peak Y | Threshold | Material |
|-----|---------|--------|-----------|----------|
| Coal | 20–200 | 120 | 0.72 | `MAT_COAL` |
| Iron | -40–100 | 30 | 0.78 | `MAT_IRON` |
| Copper | -20–140 | 60 | 0.76 | `MAT_COPPER` |
| Gold | -120–20 | -50 | 0.84 | `MAT_GOLD` |
| Crystal | -160–-40 | -100 | 0.88 | `MAT_CRYSTAL` |
| Magma Gem | -192–-80 | -140 | 0.90 | `MAT_MAGMA_GEM` |

The triangular distribution is implemented by adjusting the threshold based on distance from the peak Y:

```rust
fn ore_threshold(y: f32, peak_y: f32, range: f32, base_threshold: f32) -> f32 {
    let distance = (y - peak_y).abs() / range;
    base_threshold + distance * 0.15  // harder to reach threshold far from peak
}
```

Biomes can multiply ore thresholds: Volcanic Wastes halves the Magma Gem threshold (2× more common), while Frozen Expanse halves the Crystal threshold.

---

## Performance budget

### Per-chunk cost breakdown (target: <2ms on 4-core CPU with AVX2)

| Operation | Time | Notes |
|-----------|------|-------|
| **Range analysis** | 0.02ms | Conservative bounds from 2D heightmap cache |
| **Skipped chunks (80–90%)** | 0.02ms total | Range analysis only |
| **2D column noise (amortized)** | 0.005ms | 5 graphs × 1024 samples, shared across ~3 surface chunks per column |
| **Biome weight computation** | 0.01ms | 1024 columns × ~4 nearby points each |
| **Graph 6: Terrain shape 3D** | 0.49ms | 12 octave-equivalents × 32768 samples / 8 AVX2 lanes |
| **Graph 7: Cave system 3D** | 0.29ms | 7 octave-equivalents × 32768 samples / 8 AVX2 lanes |
| **Density combination (Rust)** | 0.15ms | 32768 voxels × arithmetic + gradient + quantization |
| **Material assignment** | 0.10ms | Surface voxels only (~20% of chunk), simple branching |
| **Ore noise (optional)** | 0.10ms | 1 octave × solid surface voxels only |
| **Total per surface chunk** | **~1.2ms** | Comfortably under 2ms budget |
| **Meshing (marching cubes)** | ~0.5ms | Separate pipeline stage, parallel |
| **Grand total** | **~1.7ms** | Under 2ms target |

### FN2 graph fusion benefits

All noise within each graph stays in SIMD registers — intermediate values never write to memory. The terrain shape graph with 12 effective octaves runs as a single fused operation, roughly **3× faster** than evaluating low, high, and selector as three separate graphs and combining in scalar Rust code. The cave graph similarly fuses 7 octaves into one pass.

### Cacheable layers

The five 2D noise maps are evaluated once per chunk column (32×32 XZ area) and cached in a `HashMap<(i32, i32), ColumnCache>`. Each column cache stores **5 × 32 × 32 = 5120 f32 values** (20KB) plus derived biome weights. With a typical view distance of 16 chunks, the column cache holds ~256 entries (~5MB) — trivial memory cost for massive savings.

### Range analysis interaction

The 2D heightmap cache enables conservative range analysis: for each chunk, compute the possible terrain height range from biome-blended `base_height ± amplitude_3d / squash_factor`. If the chunk's Y range is entirely above this maximum or entirely below the minimum (accounting for cave carving depth), skip all 3D noise evaluation. In practice, a 20-chunk-tall column has only **2–4 surface chunks** that need full evaluation, with 16–18 chunks skipped.

For the cave graph specifically, if a chunk is classified as "deep solid" (far below surface with no chance of cave penetration), it can also be skipped. The cheese noise range is [-1, 1], and with hollowness 0.15, the minimum output is -0.85. This only carves where terrain density is less than ~0.85 × cave_density × depth_factor, providing another skip criterion for deep chunks with very high base density.

### Thread pool utilization

With a 4-core CPU, the chunk generation pipeline uses a work-stealing thread pool. Each thread processes one chunk at a time through the full pipeline (2D cache check → range analysis → 3D noise → density → materials → mesh). At 1.7ms per surface chunk and ~3 surface chunks per column, a single column takes ~5ms. With 4 threads processing different columns, throughput is **~800 surface chunks/second**, sufficient for aggressive streaming at 60fps.

---

## Implementation roadmap

### Phase 1: Flat-world-with-noise (Week 1–2)

Build the minimum pipeline that produces visible terrain:

1. Implement a single 2D FN2 heightmap graph (Graph 5 only)
2. Convert heightmap to density field: `density = -(y - heightmap[x,z])`
3. Quantize to i8, feed to existing marching cubes mesher
4. Verify meshing, streaming, and chunk loading work with simple terrain
5. **Milestone**: Walk on a procedural landscape with hills and valleys

### Phase 2: 3D noise and drama (Week 3–4)

Add the core 3D density field:

1. Implement Graph 6 (terrain shape: low/high/selector + warp)
2. Replace pure heightmap density with the full Beta-style density function
3. Tune height gradient parameters (squash, asymmetry) for visible overhangs
4. Add floor/ceiling clamping
5. **Milestone**: Overhangs, cliff faces, and occasional floating formations visible

### Phase 3: Biome system (Week 5–6)

Add biome parameterization:

1. Implement Graphs 1–4 (continental, temperature, moisture, erosion)
2. Build biome selection and scattered-data blending system
3. Define the 8 biome parameter structs
4. Wire biome parameters into the density function
5. Add material assignment (basic: biome top/sub/stone only)
6. **Milestone**: Distinct biome regions with different terrain character, smooth transitions

### Phase 4: Cave integration (Week 7–8)

Add the underground:

1. Implement Graph 7 (fused cheese + spaghetti caves)
2. Wire cave contribution into density function with depth masking
3. Tune per-biome cave parameters
4. Add underground layer material transitions
5. Add water/lava fill below sea level and in caves
6. **Milestone**: Explorable cave networks, distinct underground layers

### Phase 5: Polish and optimization (Week 9–10)

Production-readiness:

1. Implement range analysis for chunk skipping
2. Add column cache for 2D noise maps
3. Implement ore placement (Graph 8)
4. Tune all biome parameters for visual quality
5. Add steepness-based material assignment
6. Profile and optimize to meet 2ms budget
7. **Milestone**: Full terrain system at target performance, all biomes differentiated

### Phase 6: Refinement (Ongoing)

- River carving from 2D noise
- Deep underground geode formations (Voronoi cellular noise)
- Biome-specific cave decorations (crystal formations, hanging vines)
- Terrain feature stamping (pre-designed structures placed into density field)
- Player-facing world seed selection with preview

### Iteration strategy

The roadmap prioritizes **playable results first**: each phase produces a complete, testable terrain that is strictly better than the previous. Phase 1 gives a walkable world; Phase 2 adds drama; Phase 3 adds variety; Phase 4 adds depth. No phase requires rework of previous phases — each adds new noise graphs and parameters to the existing pipeline.

The biome parameter structs are designed for rapid iteration: tweaking `squash_factor` from 0.8 to 0.3 instantly transforms rolling hills into shattered peaks. A hot-reload system for biome definition files (TOML or RON) enables live tuning without recompilation.

---

## Why this architecture works

The Parameterized Beta approach succeeds because it preserves the two properties that made each reference system great while avoiding their weaknesses.

From **Beta 1.7.3**, the system inherits the 3D density field driven by the low/high/selector noise blend with an asymmetric height gradient. This is the simplest architecture known to produce emergent overhangs, arches, and floating formations without special-casing any feature type. The three-noise blend creates macro-scale terrain variety (two different "textures" of terrain smoothly transitioning), while the domain warp and fractal octaves provide multi-scale detail. Beta's weakness — globally uniform parameters producing mountains in deserts — is solved by the biome parameterization layer.

From **Hytale V2**, the system inherits the biome-drives-terrain philosophy where each biome's parameter struct fully controls terrain character. The staged pipeline (biome first → terrain second) ensures intentional design: a Shattered Peaks biome always produces dramatic terrain, a Whispering Marsh always produces flat wetland. Hytale's weakness — complex node-editor-driven density functions that require visual tools — is avoided by using a single universal density function with biome parameters rather than per-biome density graphs.

The architecture also aligns naturally with the engine's constraints: all noise is evaluated as FN2 batch calls on fused SIMD graphs, the i8 density field gets full-range utilization through careful quantization scaling, range analysis can propagate through the arithmetic density function, and the data-driven biome system supports iteration without code changes.

For a low-poly stylized isometric game with marching cubes meshing, the **bold, dramatic shapes** produced by low squash factors and high 3D noise amplitudes are ideal. Unlike photorealistic terrain that needs subtle, gentle slopes, Voxulacrum's visual style rewards exactly the kind of exaggerated, surprising terrain that this system naturally produces. The 42 quantized normals and pixel upscaling will make floating islands and deep caverns look stunning rather than uncanny — leveraging the aesthetic rather than fighting it.