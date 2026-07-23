# Cloud Shadows — Design Note (Split Envelope/Detail Architecture)

**Status:** Design note for the implementing session. Supersedes the current
*presentation* half of the climate overhaul — the per-tick CPU rasterization of
a camera-centered cloud texture in `cloud_shadow.rs`. The *simulation* half
(`world/climate.rs`: macro-grid advection/relaxation, settle/evict residency,
its tests) is *retained unchanged*; this note only changes how the sim reaches
the screen.

**Verdict being implemented:** the fused design — "repaint one camera-centered
128² texture from sim × noise every tick" — is structurally wrong on two axes:
its sampling lattice is glued to the camera (so panning re-samples the noise at
shifted phases: the persistent shimmer), and its visual content is one octave
of simplex under a wide smoothstep (so no parameters can make it read as
clouds). The fix is to split the two concerns the design fused:

- **Layer A — the envelope** (dynamic, tiny, world-lattice-anchored): what the
  sim knows — *where* is it cloudier.
- **Layer B — the detail** (static, tiling, visually rich): what clouds *look
  like* — baked once, scrolled by wind.

Shadow density at any point is simply `envelope(wp) × detail(wp, wind_t)`.

---

## 1. Post-mortem → architectural properties

Every defect from this arc, and the property of the new shape that prevents
its class (not just its instance):

| Defect (where) | Property that prevents the class |
|---|---|
| Panning shimmer: window re-rasterized at camera-continuous origin, detail resampled at shifted phase every tick (`cloud_shadow.rs::tick` raster loop) | **Nothing is ever re-rasterized.** Detail is a static texture; the envelope's texel lattice is the macro-cell grid itself. All sampling lattices are world-anchored. |
| Sim state destroyed by camera movement (fixed already: settle/evict) | Retained: residency ≠ activity; `pan_and_return_preserves_density` pins it. |
| ClampToEdge smear covering the world beyond the window | Detail uses `Repeat` (infinite by construction); envelope out-of-window falls back to the layer default in-shader (§4), and its window is ~16 km wide anyway. |
| Threshold inversion (legacy noise-waterline semantics) | Kept as fixed: shadow where density is HIGH; direction is stated in this note as the contract. |
| Coverage not areal | Quantile calibration retained, now measured over the *visible region* (§6) — closer to what the eye sees than the old window-based histogram. |
| Blob-noise look | Layer B bake recipe (§5): fBm × inverted Worley + domain warp + steep transfer. |

---

## 2. Data model

Per layer (`CLOUD_LAYER_COUNT = 4`, packed as RGBA channels as today):

**Layer A — envelope texture.** One shared `Rgba8Unorm`, `ENV_SIZE = 32`
texels per side, **one texel = one macro cell** (`climate::CELL_SIZE = 512`),
so the window spans 16,384 world units — far beyond any visible extent.

- Origin: `env_origin_cell = camera_cell − ENV_SIZE/2`, i.e. **snapped to the
  cell lattice**; the uniform carries `env_origin_world` (always a multiple of
  `CELL_SIZE`) and `env_extent_world = ENV_SIZE × CELL_SIZE`.
- Content: `grid.density_at(cell)` per texel — 1,024 lookups × 4 layers per
  climate tick (≈4 KB upload); rebuild unconditionally each tick, no cleverness
  needed. Bilinear sampling across cell texels replaces
  `DensityWindow::sample_bilinear`.
- Sampler: ClampToEdge is acceptable (the window is enormous); the shader
  additionally substitutes the per-layer default density when the sample point
  is outside the window (§4) so extreme zoom-outs degrade to "ambient
  cloudiness," never to a smeared edge row.

**Layer B — detail texture.** One shared `Rgba8Unorm`, 512², **baked once at
startup**, `Repeat` sampler, one channel per layer with distinct character
(§5). World period per layer: `tile_world = 1 / layer.uv_scale` (the existing
param, reclaimed as "detail feature tiling scale").

**Wind displacement accumulator** (per layer, in `CloudShadowState`):

```rust
// Each climate tick:
let layer_wind = rotate(base_wind, layer.wind_dir_offset) * layer.wind_speed_scale;
self.wind_disp[i] = (self.wind_disp[i] + layer_wind * dt).rem_euclid(Vec2::splat(tile_world));
```

The `rem_euclid` wrap against the tile period keeps the accumulator bounded
forever (no f32 drift over long sessions) and is exact because the texture
repeats with precisely that period. The existing `cloud_offset_N` uniforms are
reused: `offset_N = -wind_disp[i] * uv_scale_N`.

---

## 3. Shader contract (all five cloud-sampling shaders)

Replaces each per-layer block; direction is the corrected one (dense = dark):

```wgsl
// Envelope: where the sim says clouds are (world-lattice-anchored).
let env_uv_0 = (wp.xz - globals.env_origin) / globals.env_extent;
var env_0 = globals.cloud_default_0;
if env_uv_0.x >= 0.0 && env_uv_0.x <= 1.0 && env_uv_0.y >= 0.0 && env_uv_0.y <= 1.0 {
    env_0 = textureSample(cloud_env_texture, cloud_env_sampler, env_uv_0).r;
}
// Detail: what clouds look like (static, tiling, wind-scrolled).
let det_uv_0 = wp.xz * globals.cloud_uv_scale_0 + globals.cloud_offset_0;
let detail_0 = textureSample(cloud_texture, cloud_sampler, det_uv_0).r;

let density_0 = env_0 * detail_0;
let t_0 = smoothstep(globals.cloud_coverage_0 - 0.05, globals.cloud_coverage_0 + 0.05,
                     density_0);
let cloud_factor_0 = mix(1.0, DARK_0, t_0);   // per-shader darkness constants as today
```

Notes: the smoothstep band narrows from ±0.15 to ±0.05 (defined shadow edges;
widen per taste — this is the penumbra knob). `cloud_coverage_N` continues to
carry the **calibrated threshold** (§6), not the user's coverage value.
`cloud_default_N` is a new uniform (the layer's out-of-window ambient density).
Binding note: `cloud_env_texture`/`sampler` are new entries in the global bind
group — `uniforms.rs` layout plus each shader's binding declarations change
together in one substep.

---

## 4. What deletes, what stays

| Item | Disposition |
|---|---|
| `cloud_shadow.rs` per-tick 128² raster loop, `pixels` buffer, `write_texture` of the big window | **Deleted** — the class of sliding-phase bugs goes with it. |
| `climate::DensityWindow` + its test | **Deleted** (envelope rebuild reads `density_at` directly). |
| `bake_detail_tile` (1-octave) + `DETAIL_FLOOR` | **Replaced** by the §5 bake; keep a CPU copy of the baked channels (calibration samples it). |
| `eff_uv_scales` / view-extent window sizing (if applied from the interim fix) | **Deleted** — obsolete; `Repeat` makes window sizing moot. |
| `offsets` semantics | **Reused** as wrapped wind displacements (§2). |
| `climate.rs` sim: advection, relaxation, `seed_new_cells`, `settle_outside_window`, `evict_density_beyond`, all tests incl. `pan_and_return_preserves_density` | **Untouched.** |
| Quantile calibration | **Kept**, re-targeted to the visible region (§6). |
| Shader inversion fix, per-layer darkness constants | **Kept** (restated in §3). |
| `ClimateClock` dt clamp | **Kept.** |

---

## 5. Layer B bake recipe

Per layer channel, sampled over the tile with wrap-blended margins (below).
All via `fastnoise_lite`; every noise seeded from the layer index as today.

```
base    = fbm(p)                    // OpenSimplex2, FractalType::FBm, 4–5 octaves
cells   = 1.0 - cellular_f1(p)      // NoiseType::Cellular, Euclidean F1, normalized:
                                    // inverted Worley = puffy clump centers
shape   = clamp(base * (1 - worley_mix) + cells * worley_mix + bias, 0, 1)
detail  = pow(shape, contrast)      // push midtones down: defined masses, clear gaps
```

Per-layer character (initial values; these are the *real* aesthetic knobs the
old design never had):

| Layer | role | worley_mix | octaves | contrast | notes |
|---|---|---|---|---|---|
| 0 | low stratus | 0.2 | 5 | 1.2 | broad soft masses |
| 1 | cumulus | 0.65 | 4 | 1.8 | the "looks like clouds" layer |
| 2 | high broken | 0.5 | 4 | 2.2 | small scattered puffs |
| 3 | cirrus veil | 0.1 | 3 | 1.0 | stretch p.x by ~3× before sampling: streaks |

**Domain warp:** apply FastNoiseLite's domain warp during the bake (fractal
warp, modest amplitude) — free at runtime, and it is the single largest
"stops looking like noise" contributor. Animated warp at sample time is a
polish option, not in the first build.

**Tiling:** bake into a buffer with an `M`-texel margin (M ≈ 32) and
edge-blend: `t[x] = lerp(t[x], t[x + size], smoothstep over the margin)` on
both axes. Soft cloud content hides the blend entirely. (The old 4D-torus hack
is retired; if the repeat ever becomes visible at extreme zoom-out, the fix is
sampling the same channel at two scales and lerping — noted as polish, not
built now.)

---

## 6. Coverage calibration (areal semantics, kept)

Per layer per climate tick, sample the *product* field over the visible
region on the CPU: a 48×48 grid across `visible_extent` centered on the
camera, `density = env_bilinear(grid, p) × detail_cpu(p − wind_disp)` — the
baked CPU copy from §5 serves the detail lookups. Histogram (256 bins) →
threshold = value at quantile `(1 − coverage)`, written to `coverages[i]`
exactly as the current code does. ~9K samples/tick total: negligible.

Coverage `0.3` therefore shadows ~30% of what is *on screen*, by construction,
at any zoom, under any parameter set — the semantics requested at the start of
this arc, now measured where the player is actually looking.

---

## 7. Build order — verification choreography

The arc's core process failure was debugging shape, stability, and dynamics
simultaneously. Each step below is judged alone, and a step is not left until
its check passes:

1. **Bake + static display.** Envelope forced to 1.0, wind frozen. Check: do
   the four channels *look like cloud shadows* on terrain? Iterate §5 knobs
   here and nowhere else. Then the pan/zoom check: pick a landmark, pan and
   zoom hard — shadows must be pinned to the world to the pixel. (This is the
   original complaint's regression test, performed before any dynamics exist.)
2. **Wind scroll on.** Motion must be smooth drift, no shimmer, no seams at
   the tile period. Long-session check: force a huge `wind_disp` (e.g. seed
   the accumulator at 1e6 before wrapping) — identical rendering.
3. **Calibration on.** Sweep coverage 0.1 → 0.9; eyeball (or screenshot-count)
   that shadowed area tracks the dial.
4. **Envelope from the sim.** Regional variation appears; fronts drift.
   `pan_and_return_preserves_density` already pins sim-side stability; the
   visual check is panning across an envelope gradient — the *gradient* moves
   only with the sim, never with the camera.
5. Parameter pass across biomes/times-of-day; retire any layer that isn't
   earning its keep (two strong layers beat four fighting — owner's call).

## 8. Open decisions (owner)

1. **Layer count** — keep 4, or collapse to 2 (low stratus + cumulus) now that
   each layer is expressive?
2. **`coverages`/`cloud_coverage_N` naming** — they carry thresholds; rename
   (`cloud_threshold_N`) in this rework or leave historical.
3. **Whether the envelope modulates only density or also darkness** — deep
   overcast could darken `DARK_N` slightly for weather mood; polish, not core.
4. **Animated domain warp / two-scale anti-repeat** — polish backlog.

---

*End of note. The simulation earned its keep this arc; its presentation
didn't. Layer B carries the look, Layer A carries the weather, and neither
ever re-rasterizes — the whole class of camera-coupled sampling bugs is
structurally unreachable in this shape.*
