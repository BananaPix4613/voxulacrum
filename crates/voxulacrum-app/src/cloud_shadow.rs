use fastnoise_lite::{
    CellularDistanceFunction, CellularReturnType, DomainWarpType, FastNoiseLite, FractalType,
    NoiseType,
};
use glam::Vec2;
use nodegraph_eval::WorldEvaluator;
use std::collections::HashMap;
use crate::world::climate::{self, ClimateGrid};
use crate::params::{CloudParams, CLOUD_LAYER_COUNT};
use crate::rendering::render_context::RenderContext;

/// Side length of the baked Layer B (detail) texture. Static, baked once at
/// startup, sampled with `Repeat` - see `docs/cloud-shadow-architecture.md`.
/// This is the fix for the panning shimmer: nothing here is ever
/// re-rasterized, so there's no camera-coupled sampling phase to shift.
const DETAIL_TEX_SIZE: u32 = 512;

/// Width (in texels) of the edge-blend region used to make each baked layer
/// tile seamlessly under `Repeat` sampling.
const TILE_MARGIN: f32 = 32.0;

/// Side length (in macro cells) of the envelope (Layer A) texture - one
/// texel per `climate::CELL_SIZE` cell, so the window spans
/// `ENV_SIZE * CELL_SIZE` = 16,384 world units (architecture doc §2).
const ENV_SIZE: i32 = 32;

/// How many cells out from the camera's cell each layer's `ClimateGrid`
/// stays active/seeded - half of `ENV_SIZE`, matching the envelope window.
const ENV_RADIUS_CELLS: i32 = ENV_SIZE / 2;

/// Max number of fresh (uncached) biome/water resolves per tick. Biome is
/// static, so cells resolve once and stay cached; capping fresh resolves
/// keeps first entry to a new area from hitching on a burst of generator
/// evals - the window fills over a few frames instead.
const BIOME_FILL_BUDGET: usize = 64;

/// Radius (in macro cells) around the camera over which cached cells get
/// their water flag re-queried each tick. Only needs to cover the resident-
/// chunk area (a few chunks = well under one 512-unit cell) with margin;
/// beyond it the cheap water lookup returns `None` and is skipped anyway.
const WATER_REFRESH_RADIUS: i32 = 6;

/// Per-layer Layer B bake character (architecture doc §5). Distinct per
/// layer so each altitude band reads as a different cloud type rather than
/// four copies of the same shape at different scales.
struct LayerBakeParams {
    seed: i32,
    /// 0.0 = pure fBm (soft, broad masses), 1.0 = pure inverted-Worley
    /// (puffy, clumped) - blended between for character.
    worley_mix: f32,
    octaves: i32,
    /// Exponent pushing midtones down: higher = more defined masses with
    /// clearer gaps between them, lower = softer/hazier.
    contrast: f32,
    /// Stretches the sampling domain along X before baking - used for
    /// layer 3's cirrus streaks.
    stretch_x: f32,
}

// Contrast is deliberately mild here (see the doc comment on `contrast`
// above) - the original 1.2/1.8/2.2 pushed each layer's baked density so
// hard toward the low end that quantile calibration (§6) had almost no
// mid-range density to select from, so nearly the whole `coverage_dial`
// range mapped to the same threshold and only a thin sliver of dial values
// did anything (reported as "covers the entire world or vanishes, no
// in-between"). Softening these gives the calibrated threshold genuine room
// to move smoothly across the dial's range.
const LAYER_BAKE: [LayerBakeParams; CLOUD_LAYER_COUNT] = [
    // Low stratus: broad, soft masses.
    LayerBakeParams { seed: 9000, worley_mix: 0.2, octaves: 5, contrast: 1.0, stretch_x: 1.0 },
    // Cumulus: the "looks like clouds" layer.
    LayerBakeParams { seed: 9010, worley_mix: 0.65, octaves: 4, contrast: 1.2, stretch_x: 1.0 },
    // High broken: small scattered puffs.
    LayerBakeParams { seed: 9020, worley_mix: 0.5, octaves: 4, contrast: 1.3, stretch_x: 1.0 },
    // Cirrus veil: thin, streaky.
    LayerBakeParams { seed: 9030, worley_mix: 0.1, octaves: 3, contrast: 1.0, stretch_x: 3.0 },
];

/// Bake one layer's Layer B detail texture: fBm blended with inverted-Worley
/// (cellular F1) cloud clumps, domain-warped, contrast-shaped, and
/// edge-blended for seamless tiling under `Repeat` sampling. Baked once at
/// startup - a brief one-time cost, not a per-frame one.
fn bake_layer_b(p: &LayerBakeParams) -> Vec<f32> {
    let mut fbm = FastNoiseLite::with_seed(p.seed);
    fbm.set_noise_type(Some(NoiseType::OpenSimplex2));
    fbm.set_fractal_type(Some(FractalType::FBm));
    fbm.set_fractal_octaves(Some(p.octaves));
    fbm.set_frequency(Some(1.0 / 96.0));

    let mut cellular = FastNoiseLite::with_seed(p.seed + 1);
    cellular.set_noise_type(Some(NoiseType::Cellular));
    cellular.set_cellular_distance_function(Some(CellularDistanceFunction::Euclidean));
    cellular.set_cellular_return_type(Some(CellularReturnType::Distance));
    cellular.set_frequency(Some(1.0 / 128.0));

    let mut warp = FastNoiseLite::with_seed(p.seed + 2);
    warp.set_domain_warp_type(Some(DomainWarpType::OpenSimplex2));
    warp.set_domain_warp_amp(Some(24.0));
    warp.set_frequency(Some(1.0 / 200.0));

    let size = DETAIL_TEX_SIZE as f32;
    let shape_at = |x: f32, y: f32| -> f32 {
        let (wx, wy) = warp.domain_warp_2d(x * p.stretch_x, y);
        let base = fbm.get_noise_2d(wx, wy) * 0.5 + 0.5;
        let cell = 1.0 - (cellular.get_noise_2d(wx, wy) * 0.5 + 0.5);
        let shape = (base * (1.0 - p.worley_mix) + cell * p.worley_mix).clamp(0.0, 1.0);
        shape.powf(p.contrast)
    };

    let mut tile = vec![0.0f32; (DETAIL_TEX_SIZE * DETAIL_TEX_SIZE) as usize];
    for y in 0..DETAIL_TEX_SIZE {
        for x in 0..DETAIL_TEX_SIZE {
            let v = sample_tileable(&shape_at, x as f32, y as f32, size, TILE_MARGIN);
            tile[(y * DETAIL_TEX_SIZE + x) as usize] = v;
        }
    }
    tile
}

/// Sample `shape_fn` with seamless-tiling edge blending: within `margin`
/// texels of any tile edge, blend toward a copy of `shape_fn` sampled a
/// half-tile away (which is smooth exactly where the direct sample has its
/// wrap discontinuity, and vice versa), fading by a smoothstep across the
/// margin. Works for any noise function regardless of whether it's
/// naturally periodic
fn sample_tileable(shape_fn: &impl Fn(f32, f32) -> f32, x: f32, y: f32, size: f32, margin: f32) -> f32 {
    let base = shape_fn(x, y);
    let edge_x = x.min(size - x);
    let edge_y = y.min(size - y);
    if edge_x >= margin && edge_y >= margin {
        return base;
    }
    let half = size * 0.5;
    let shifted = shape_fn((x + half).rem_euclid(size), (y + half).rem_euclid(size));
    let wx = 1.0 - (edge_x / margin).clamp(0.0, 1.0);
    let wy = 1.0 - (edge_y / margin).clamp(0.0, 1.0);
    let w = wx.max(wy);
    let smooth_w = w * w * (3.0 - 2.0 * w);
    base + (shifted - base) * smooth_w
}

/// Build a 256-bin histogram of a baked tile's own density values. Because
/// Layer B tiles infinitely under `Repeat`, this histogram IS the world's
/// density distribution for this layer - built once here, not resampled
/// from a camera-relative window every tick (see `calibrate_threshold`).
fn build_histogram(tile: &[f32]) -> [u32; 256] {
    const BINS: usize = 256;
    let mut histogram = [0u32; BINS];
    for &v in tile {
        let bin = ((v.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as usize).min(BINS - 1);
        histogram[bin] += 1;
    }
    histogram
}

/// Histogram-based quantile threshold (§6, revised): given a layer's
/// density histogram, find the value such that `coverage` fraction of the
/// tile's own values are at or above it. Writing this (not the raw dial)
/// into `cloud_coverage_N` gives areal semantics without depending on
/// camera position or zoom - see `build_histogram` and the note on `tick`.
fn calibrate_threshold(histogram: &[u32; 256], coverage: f32) -> f32 {
    const BINS: usize = 256;
    let total: u32 = histogram.iter().sum();
    if total == 0 {
        return coverage;
    }
    let target_below = (1.0 - coverage).clamp(0.0, 1.0) * total as f32;

    let mut cumulative = 0.0f32;
    let mut last_populated_bin = -1.0f32;
    for (bin, &count) in histogram.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let next_cumulative = cumulative + count as f32;
        if next_cumulative >= target_below {
            // See the earlier note (now historical) on why this interpolates
            // across gaps rather than snapping: the §5 contrast curve makes
            // each layer's histogram bimodal, and interpolating keeps the
            // threshold a continuous function of `coverage`.
            let t = ((target_below - cumulative) / count as f32).clamp(0.0, 1.0);
            let from = last_populated_bin.max(0.0);
            let interpolated = from + (bin as f32 - from) * t;
            return interpolated / (BINS - 1) as f32;
        }
        cumulative = next_cumulative;
        last_populated_bin = bin as f32;
    }
    1.0
}

pub struct CloudShadowState {
    /// Per-layer histogram of the baked tile's own density values (§6,
    /// revised) - built once in `new` from the tile itself. Since the tile
    /// repeats infinitely, this is exactly the world's density distribution
    /// for that layer, calibrated independent of camera position or zoom.
    histograms: [[u32; 256]; CLOUD_LAYER_COUNT],
    /// Accumulated wind displacement per layer, wrapped against that layer's
    /// tile period every tick (architecture doc §2) - the `rem_euclid` wrap
    /// keeps this bounded forever regardless of session length, and is exact
    /// because the baked Layer B texture repeats with precisely that period.
    wind_disp: [Vec2; CLOUD_LAYER_COUNT],
    /// UV offset per layer, derived from `wind_disp` each tick.
    pub offsets: [Vec2; CLOUD_LAYER_COUNT],
    pub coverages: [f32; CLOUD_LAYER_COUNT],
    /// Out-of-window fallback density per layer (§3's `cloud_default_N`) -
    /// what this layer looks like far from anywhere the sim has actually
    /// simulated.
    pub cloud_defaults: [f32; CLOUD_LAYER_COUNT],
    /// This tick's actual average simulated density per layer, over the
    /// envelope window (§8 "envelope modulates darkness, not just density"):
    /// exposed to the shader as `env_typical_N` so a pixel's darkness can be
    /// scaled relative to how dense the sim currently is *here*, not just a
    /// fixed constant whenever a pixel clears the coverage threshold.
    pub env_typical: [f32; CLOUD_LAYER_COUNT],
    /// Layer A: each layer's own macro-grid simulation (architecture doc
    /// §1-2) - built and tested in `climate.rs`, ticked here for the first
    /// time.
    climate_grids: [ClimateGrid; CLOUD_LAYER_COUNT],
    /// Residency-independent biome + best-effort water per macro cell,
    /// resolved once from the generator and cached (biome is static in world
    /// space). Feeds each layer's condensation target without depending on
    /// which chunks are currently streamed in - see `tick`.
    biome_cache: HashMap<climate::CellPos, (u16, bool)>,
    /// World-space origin of the envelope window's `(0, 0)` texel - always a
    /// multiple of `climate::CELL_SIZE`.
    pub env_origin_world: Vec2,
    pub env_extent_world: f32,
    #[allow(dead_code)] // RAII: owns the storage texture_view borrows
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    #[allow(dead_code)]
    pub env_texture: wgpu::Texture,
    pub env_texture_view: wgpu::TextureView,
    pub env_sampler: wgpu::Sampler,
}

impl CloudShadowState {
    pub fn new(ctx: &RenderContext) -> Self {
        let mut pixels = vec![0u8; (DETAIL_TEX_SIZE * DETAIL_TEX_SIZE) as usize * CLOUD_LAYER_COUNT];
        // Calibration histogram (§6) built once here from the baked tile -
        // not resampled from world space every tick. See `tick` for why.
        let histograms: [[u32; 256]; CLOUD_LAYER_COUNT] = std::array::from_fn(|i| {
            let tile = bake_layer_b(&LAYER_BAKE[i]);
            for (texel_idx, &v) in tile.iter().enumerate() {
                pixels[texel_idx * CLOUD_LAYER_COUNT + i] = (v.clamp(0.0, 1.0) * 255.0) as u8;
            }
            build_histogram(&tile)
        });

        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud_shadow_texture"),
            size: wgpu::Extent3d { width: DETAIL_TEX_SIZE, height: DETAIL_TEX_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(DETAIL_TEX_SIZE * CLOUD_LAYER_COUNT as u32),
                rows_per_image: Some(DETAIL_TEX_SIZE),
            },
            wgpu::Extent3d { width: DETAIL_TEX_SIZE, height: DETAIL_TEX_SIZE, depth_or_array_layers: 1 },
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Repeat: this is now a genuinely static, infinite-by-construction
        // tiling texture (baked once, seamlessly, above) - not a
        // camera-centered window, so there's no edge to clamp.
        let sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cloud_shadow_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Layer A - the envelope: shared Rgba8Unorm, one texel per macro
        // cell, rebuilt unconditionally every climate tick from the sim's
        // `ClimateGrid's (architecture doc §2). Unlike Layer B this is
        // genuinely dynamic - ClampToEdge is fine because the shader
        // explicitly substitutes `cloud_default_N` outside the window rather
        // than relying on texture clamping (§3).
        let env_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud_envelope_texture"),
            size: wgpu::Extent3d { width: ENV_SIZE as u32, height: ENV_SIZE as u32, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let env_texture_view = env_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let env_sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cloud_envelope_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            histograms,
            wind_disp: [Vec2::ZERO; CLOUD_LAYER_COUNT],
            offsets: [Vec2::ZERO; CLOUD_LAYER_COUNT],
            coverages: [0.0; CLOUD_LAYER_COUNT],
            cloud_defaults: [0.0; CLOUD_LAYER_COUNT],
            env_typical: [0.05; CLOUD_LAYER_COUNT],
            climate_grids: std::array::from_fn(|_| ClimateGrid::default()),
            biome_cache: HashMap::new(),
            env_origin_world: Vec2::ZERO,
            env_extent_world: ENV_SIZE as f32 * climate::CELL_SIZE,
            texture,
            texture_view,
            sampler,
            env_texture,
            env_texture_view,
            env_sampler,
        }
    }

    /// Rotate `v` by `angle` radians (counter-clockwise).
    fn rotate(v: Vec2, angle: f32) -> Vec2 {
        let (s, c) = angle.sin_cos();
        Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
    }

    /// Stage 4 (architecture doc §7): the envelope (Layer A) now comes from
    /// the sim - each layer's `ClimateGrid` is ticked (seed -> simulate ->
    /// settle -> evict, the same rhythm `pan_and_return_preserves_density`
    /// exercises in `climate.rs`) using the same per-layer wind already
    /// driving Layer B's scroll, then all four grids' `density_at` are
    /// packed into one shared 32x32 envelope texture centered on the
    /// camera's cell and re-uploaded unconditionally every tick - small
    /// enough (1,024 lookups x 4 layers) that there's no reason to diff it.
    pub fn tick(
        &mut self,
        ctx: &RenderContext,
        params: &CloudParams,
        camera_xz: Vec2,
        base_wind: Vec2,
        dt: f32,
        elapsed: f32,
        world_eval: &WorldEvaluator,
        mut regional_at: impl FnMut(Vec2) -> Option<(u16, bool)>,
        mut water_at: impl FnMut(Vec2) -> Option<bool>,
    ) {
        // Cell-snapped window origin (architecture doc §2) - computed once
        // up front since both the per-layer calibration below and the final
        // texture pack need the same window.
        let center_cell = (
            (camera_xz.x / climate::CELL_SIZE).floor() as i32,
            (camera_xz.y / climate::CELL_SIZE).floor() as i32,
        );
        let origin_cell = (center_cell.0 - ENV_SIZE / 2, center_cell.1 - ENV_SIZE / 2);
        self.env_origin_world = Vec2::new(origin_cell.0 as f32, origin_cell.1 as f32) * climate::CELL_SIZE;

        // --- Residency-independent biome/water field ---
        // Resolve each window cell's biome + water once, from the generator
        // (biome via a cheap column eval that ignores chunk streaming; water
        // as a best-effort refinement only where a chunk is resident). Biome
        // is static in world space, so a cell is resolved once and cached;
        // only a bounded number of fresh resolves run per tick so first entry
        // to an area fills over a few frames instead of hitching. This is what
        // stops large cloud regions from receding when zoom shrinks the
        // chunk-load radius.
        let mut fills_remaining = BIOME_FILL_BUDGET;
        for cz in 0..ENV_SIZE {
            if fills_remaining == 0 {
                break;
            }
            for cx in 0..ENV_SIZE {
                if fills_remaining == 0 {
                    break;
                }
                let pos = (origin_cell.0 + cx, origin_cell.1 + cz);
                if self.biome_cache.contains_key(&pos) {
                    continue;
                }
                let cell_center =
                    (Vec2::new(pos.0 as f32, pos.1 as f32) + 0.5) * climate::CELL_SIZE;
                if let Some(data) = regional_at(cell_center) {
                    self.biome_cache.insert(pos, data);
                    fills_remaining -= 1;
                }
            }
        }
        // Bound memory: drop cached cells far outside the window (re-resolved
        // deterministically on return, mirroring the sim's own eviction).
        let keep = ENV_SIZE * 3;
        self.biome_cache.retain(|&(cx, cz), _| {
            (cx - center_cell.0).abs() <= keep && (cz - center_cell.1).abs() <= keep
        });
        
        // Keep near-camera water fresh. Biome is static (resolved once above),
        // but water state changes as chunks stream in/out with zoom/pan - so
        // re-query just the cheap water-only lookup for cached cells near the
        // camera and update their flag in place, leaving biome untouched.
        // `None` (no resident chunk) leaves the last-known value rather than
        // clobbering a previously-observed `true` with "unknown", so zooming
        // out doesn't erase water the sim already saw.
        for cz in -WATER_REFRESH_RADIUS..=WATER_REFRESH_RADIUS {
            for cx in -WATER_REFRESH_RADIUS..=WATER_REFRESH_RADIUS {
                let pos = (center_cell.0 + cx, center_cell.1 + cz);
                if let Some(entry) = self.biome_cache.get_mut(&pos) {
                    let cell_center =
                        (Vec2::new(pos.0 as f32, pos.1 as f32) + 0.5) * climate::CELL_SIZE;
                    if let Some(w) = water_at(cell_center) {
                        entry.1 = w;
                    }
                }
            }
        }

        for (i, layer) in params.layers.iter().enumerate() {
            let tile_world = 1.0 / layer.uv_scale;
            let layer_wind = Self::rotate(base_wind, layer.wind_dir_offset) * layer.wind_speed_scale;
            self.wind_disp[i] =
                (self.wind_disp[i] + layer_wind * dt).rem_euclid(Vec2::splat(tile_world));
            self.offsets[i] = -self.wind_disp[i] * layer.uv_scale;

            self.cloud_defaults[i] =
                climate::condensation_target(layer.default_humidity, layer.default_condensation);

            let default_humidity = layer.default_humidity;
            let default_condensation = layer.default_condensation;
            let water_humidity_boost = layer.water_humidity_boost;
            let biome_cache = &self.biome_cache;
            let grid = &mut self.climate_grids[i];
            let mut condensation_at = |pos: climate::CellPos| {
                // Read this cell's pre-resolved biome/water (filled above,
                // residency-independent); a cell the bounded fill hasn't
                // reached yet falls back to biome 0 for a frame.
                let (biome_id, has_water) =
                    biome_cache.get(&pos).copied().unwrap_or((0, false));
                climate::regional_condensation_target(
                    world_eval,
                    biome_id,
                    pos,
                    has_water,
                    default_humidity,
                    default_condensation,
                    water_humidity_boost,
                )
            };
            climate::seed_new_cells(grid, camera_xz, ENV_RADIUS_CELLS, &mut condensation_at);
            climate::simulate(grid, layer_wind, dt, layer.decay_rate, &mut condensation_at);
            climate::settle_outside_window(grid, camera_xz, ENV_RADIUS_CELLS);
            climate::evict_density_beyond(grid, camera_xz, ENV_RADIUS_CELLS * 3);

            // Typical envelope magnitude this tick, averaged over the same
            // window the envelope texture covers (absent cells count as 0,
            // matching `density_at`) - the sim's actual output, not a flat
            // guess. `cloud_defaults[i]` was tried here previously and was
            // too optimistic: it's the flat, no-water, no-regional-variation
            // baseline, but `ambient_humidity`'s regional term averages well
            // below its own flat multiplier and plenty of cells condense to
            // exactly 0 - so that scaling left the threshold roughly 2x too
            // high, and only extreme `base_coverage` values (and only the
            // crest of `coverage_variation`'s sine wave) ever cleared it.
            let mut env_sum = 0.0f32;
            for cz in 0..ENV_SIZE {
                for cx in 0..ENV_SIZE {
                    env_sum += grid.density_at((origin_cell.0 + cx, origin_cell.1 + cz));
                }
            }
            self.env_typical[i] = (env_sum / (ENV_SIZE * ENV_SIZE) as f32).max(0.05);

            let coverage_dial = layer.base_coverage
                + layer.coverage_variation * (elapsed * layer.coverage_frequency).sin();
            let detail_threshold = calibrate_threshold(&self.histograms[i], coverage_dial);
            let calibrated = detail_threshold * self.env_typical[i];

            // Smoothed (camera.rs idiom) so coverage_dial's sin() drift
            // crossing a histogram gap still transitions gradually.
            const COVERAGE_SMOOTH_RATE: f32 = 4.0;
            let alpha = (COVERAGE_SMOOTH_RATE * dt).exp().recip().mul_add(-1.0, 1.0).clamp(0.0, 1.0);
            self.coverages[i] += (calibrated - self.coverages[i]) * alpha;
        }

        // Pack all four grids' density into the shared envelope texture.
        let mut env_pixels = vec![0u8; (ENV_SIZE * ENV_SIZE) as usize * CLOUD_LAYER_COUNT];
        for i in 0..CLOUD_LAYER_COUNT {
            for cz in 0..ENV_SIZE {
                for cx in 0..ENV_SIZE {
                    let d = self.climate_grids[i].density_at((origin_cell.0 + cx, origin_cell.1 + cz));
                    let texel_idx = (cz * ENV_SIZE + cx) as usize;
                    env_pixels[texel_idx * CLOUD_LAYER_COUNT + i] = (d.clamp(0.0, 1.0) * 255.0) as u8;
                }
            }
        }
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.env_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &env_pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ENV_SIZE as u32 * CLOUD_LAYER_COUNT as u32),
                rows_per_image: Some(ENV_SIZE as u32),
            },
            wgpu::Extent3d { width: ENV_SIZE as u32, height: ENV_SIZE as u32, depth_or_array_layers: 1 },
        );
    }
}
