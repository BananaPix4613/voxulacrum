use fastnoise2::Node;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::collections::HashMap;
use glam::IVec3;

use super::chunk::{Chunk, CHUNK_SIZE, CHUNK_VOLUME, CHUNK_WORLD_SIZE, VOXEL_SCALE};
use super::voxel::*;
use super::storage::{self, ChunkStorage};
use super::biome::{ColumnCache, compute_biome_blend, compute_blended_params, underground_fade};
use crate::params::TerrainGenParams;

/// Initial load radius dimensions in chunks
pub const WORLD_CHUNKS_X: usize = 8;
pub const WORLD_CHUNKS_Z: usize = 8;

/// Density threshold for near-surface classification in two-pass generation.
/// Voxels with |density| <= SURFACE_MARGIN are "near-surface" and need detail
/// noise. Must be >= 6.0 (deepest depth that uses material_noise) plus margin
/// for cave carving edge effects.
const SURFACE_MARGIN: f32 = 8.0;

/// Noise-channel indices for biome grid point evaluation.
pub const NM_CONTINENTALNESS: usize = 0;
pub const NM_EROSION: usize = 1;
pub const NM_PEAKS_VALLEYS: usize = 2;
pub const NM_TEMPERATURE: usize = 3;
pub const NM_HUMIDITY: usize = 4;

pub struct GenStats {
    pub skipped_air: AtomicU64,
    pub skipped_solid: AtomicU64,
    pub full_gen: AtomicU64,
    // Two-pass voxel classification counters
    pub voxels_near_surface: AtomicU64,
    pub voxels_deep_solid: AtomicU64,
    pub voxels_clear_air: AtomicU64,
    pub detail_noise_skipped: AtomicU64,  // chunks where near_surface_count == 0
}

impl GenStats {
    pub const fn new() -> Self {
        Self {
            skipped_air: AtomicU64::new(0),
            skipped_solid: AtomicU64::new(0),
            full_gen: AtomicU64::new(0),
            voxels_near_surface: AtomicU64::new(0),
            voxels_deep_solid: AtomicU64::new(0),
            voxels_clear_air: AtomicU64::new(0),
            detail_noise_skipped: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.skipped_air.load(Ordering::Relaxed),
            self.skipped_solid.load(Ordering::Relaxed),
            self.full_gen.load(Ordering::Relaxed),
        )
    }

    /// Extended snapshot including two-pass stats
    pub fn snapshot_detail(&self) -> GenStatsSnapshot {
        GenStatsSnapshot {
            skipped_air: self.skipped_air.load(Ordering::Relaxed),
            skipped_solid: self.skipped_solid.load(Ordering::Relaxed),
            full_gen: self.full_gen.load(Ordering::Relaxed),
            voxels_near_surface: self.voxels_near_surface.load(Ordering::Relaxed),
            voxels_deep_solid: self.voxels_deep_solid.load(Ordering::Relaxed),
            voxels_clear_air: self.voxels_clear_air.load(Ordering::Relaxed),
            detail_noise_skipped: self.detail_noise_skipped.load(Ordering::Relaxed),
        }
    }
}

pub struct GenStatsSnapshot {
    pub skipped_air: u64,
    pub skipped_solid: u64,
    pub full_gen: u64,
    pub voxels_near_surface: u64,
    pub voxels_deep_solid: u64,
    pub voxels_clear_air: u64,
    pub detail_noise_skipped: u64,
}

pub static GEN_STATS: GenStats = GenStats::new();

/// Wrapper around Node that provides Send + Sync.
///
/// SAFETY: FastNoise2's generation methods are reentrant and read-only on the
/// node graph. Multiple threads may call gen_* concurrently on the same node
/// handle. Node construction and set() are NOT thread-safe and must complete
/// before sharing.
struct NoiseGraph(Node);

unsafe impl Send for NoiseGraph {}
unsafe impl Sync for NoiseGraph {}

impl NoiseGraph {
    fn gen_uniform_grid_2d(
        &self,
        out: &mut [f32],
        x_offset: f32, y_offset: f32,
        x_count: i32, y_count: i32,
        x_step: f32, y_step: f32,
        seed: i32,
    ) {
        // SAFETY: graph is fully constructed, all inputs connected.
        // gen methods are read-only and reentrant.
        unsafe {
            self.0.gen_uniform_grid_2d_unchecked(
                out, x_offset, y_offset, x_count, y_count, x_step, y_step, seed,
            );
        }
    }

    fn gen_single_2d(&self, x: f32, y: f32, seed: i32) -> f32 {
        // SAFETY: same as above
        unsafe { self.0.gen_single_2d_unchecked(x, y, seed) }
    }

    fn gen_uniform_grid_3d(
        &self,
        out: &mut [f32],
        x_offset: f32, y_offset: f32, z_offset: f32,
        x_count: i32, y_count: i32, z_count: i32,
        x_step: f32, y_step: f32, z_step: f32,
        seed: i32,
    ) {
        // SAFETY: same as above
        unsafe {
            self.0.gen_uniform_grid_3d_unchecked(
                out, x_offset, y_offset, z_offset,
                x_count, y_count, z_count,
                x_step, y_step, z_step, seed,
            );
        }
    }
}

pub struct TerrainGenerator {
    // Graph 7: fused cave system (cheese + spaghetti, combined on CPU)
    cave_cheese: Arc<NoiseGraph>,       // Simplex→DomainScale(0.012)→FBm(3oct)
    cave_spaghetti_a: Arc<NoiseGraph>,  // Simplex→DomainScale(0.025)→FBm(2oct)
    cave_spaghetti_b: Arc<NoiseGraph>,  // Simplex→DomainScale(0.025)→FBm(2oct)

    // 2D auxiliary (plain SuperSimplex, frequency applied via step sizes)
    material_graph: Arc<NoiseGraph>,
    moisture_graph: Arc<NoiseGraph>,

    // FN2 2D column noise (frequency baked via DomainScale)
    continental: Arc<NoiseGraph>,      // Simplex->DomainScale(0.0004)->FBm(3oct)
    temperature: Arc<NoiseGraph>,      // Simplex->DomainScale(0.0006)->FBm(2oct)
    humidity: Arc<NoiseGraph>,         // Same as temperature, SeedOffset 200
    erosion: Arc<NoiseGraph>,          // Simplex->DomainScale(0.002)->FBm(3oct)
    elevation_detail: Arc<NoiseGraph>, // Simplex->DomainScale(0.005)->FBm(4oct)->DomainWarpGradient(40)
    terrain_shape: Arc<NoiseGraph>,    // Graph 6: 3D terrain shape

    /// Per-column cache keyed by (chunk_x, chunk_z). Populated once, shared
    /// across all vertical chunks in the same column.
    column_cache: Mutex<HashMap<(i32, i32), Arc<ColumnCache>>>,

    seed: i32,
}

enum HeightClassification {
    Air,
    Solid,
    BelowTerrainCavesEnabled,
    Indeterminate,
}

impl TerrainGenerator {
    pub fn new(params: &TerrainGenParams) -> Self {
        let seed = params.seed;

        // -- Graph 7: Fused cave system (cheese + spaghetti) --
        // Each branch is a simple Simplex->DomainScale->FBm chain.
        // Per-biome hollowness/thickness/density applied on CPU per-voxel.

        let cave_cheese = Arc::new(NoiseGraph(Self::build_scaled_fbm(
            0.012, 3, 800, None, None,
        )));
        let cave_spaghetti_a = Arc::new(NoiseGraph(Self::build_scaled_fbm(
            0.025, 2, 850, None, None,
        )));
        let cave_spaghetti_b = Arc::new(NoiseGraph(Self::build_scaled_fbm(
            0.025, 2, 900, None, None,
        )));

        // -- 2D auxiliary (plain SuperSimplex, frequency via step sizes) --

        let material_graph = Arc::new(NoiseGraph(
            Node::from_name("SuperSimplex").expect("SuperSimplex"),
        ));
        let moisture_graph = Arc::new(NoiseGraph(
            Node::from_name("SuperSimplex").expect("SuperSimplex"),
        ));

        // -- FN2 2D column noise (frequency baked via DomainScale) --

        // Graph 1: Continental
        let continental = Arc::new(NoiseGraph(Self::build_scaled_fbm(
            params.continental_freq, 3, 100, Some(0.45), Some(2.2),
        )));

        // Graph 2: Temperature
        let temperature = Arc::new(NoiseGraph(Self::build_scaled_fbm(
            params.temperature_freq, 2, 150, None, None,
        )));

        // Graph 3: Humidity
        let humidity = Arc::new(NoiseGraph(Self::build_scaled_fbm(
            params.humidity_freq, 2, 200, None, None,
        )));

        // Graph 4: Erosion
        let erosion = Arc::new(NoiseGraph(Self::build_scaled_fbm(
            params.erosion_freq, 3, 250, None, None,
        )));

        // Graph 5: Elevation Detail
        let elevation_detail = Arc::new(NoiseGraph(Self::build_scaled_fbm_warped(
            params.elevation_freq, 4, 300, params.elevation_warp,
        )));

        // Graph 6: 3D terrain shape
        let terrain_shape = Arc::new(NoiseGraph(Self::build_terrain_shape_graph(params)));

        Self {
            cave_cheese, cave_spaghetti_a, cave_spaghetti_b,
            material_graph, moisture_graph,
            continental, temperature, humidity,
            erosion, elevation_detail, terrain_shape,
            column_cache: Mutex::new(HashMap::new()),
            seed,
        }
    }

    // -- Node graph builders (called once at construction) --------------

    /// Build a simple fractal noise: SuperSimplex -> Fractal{type}.
    /// Frequency is NOT baked in - applied via gen_uniform_grid step sizes.
    fn build_fractal(fractal_type: &str, octaves: i32) -> Node {
        let os2 = Node::from_name("SuperSimplex").expect("SuperSimplex");
        let mut fractal = Node::from_name(fractal_type).expect(fractal_type);
        fractal.set("Source", &os2).expect("set Source");
        fractal.set("Octaves", octaves).expect("set Octaves");
        fractal
    }

    // -- FN2 2D column-noise graph builders ----------------------------

    /// Simplex -> DomainScale(freq) -> FractalFBm(octaves).
    /// Optional gain/lacunarity overrides; defaults are FN2 standard (0.5 / 2.0).
    fn build_scaled_fbm(
        freq: f32,
        octaves: i32,
        seed_offset: i32,
        gain: Option<f32>,
        lacunarity: Option<f32>,
    ) -> Node {
        let mut simplex = Node::from_name("Simplex").expect("Simplex");
        simplex.set("SeedOffset", seed_offset).expect("set SeedOffset");

        let mut scale = Node::from_name("DomainScale").expect("DomainScale");
        scale.set("Source", &simplex).expect("set Source");
        scale.set("Scaling", freq).expect("set Scaling");

        let mut fbm = Node::from_name("FractalFBm").expect("FractalFBm");
        fbm.set("Source", &scale).expect("set Source");
        fbm.set("Octaves", octaves).expect("set Octaves");
        if let Some(g) = gain {
            fbm.set("Gain", g).expect("set Gain");
        }
        if let Some(l) = lacunarity {
            fbm.set("Lacunarity", l).expect("set Lacunarity");
        }
        fbm
    }

    /// Simplex -> DomainScale(freq) -> FractalFBm(octaves) -> DomainWarpGradient(amp).
    fn build_scaled_fbm_warped(
        freq: f32,
        octaves: i32,
        seed_offset: i32,
        warp_amp: f32,
    ) -> Node {
        let mut simplex = Node::from_name("Simplex").expect("Simplex");
        simplex.set("SeedOffset", seed_offset).expect("set SeedOffset");

        let mut scale = Node::from_name("DomainScale").expect("DomainScale");
        scale.set("Source", &simplex).expect("set Source");
        scale.set("Scaling", freq).expect("set Scaling");

        let mut fbm = Node::from_name("FractalFBm").expect("FractalFBm");
        fbm.set("Source", &scale).expect("set Source");
        fbm.set("Octaves", octaves).expect("set Octaves");

        let mut warp = Node::from_name("DomainWarpGradient").expect("DomainWarpGradient");
        warp.set("Source", &fbm).expect("set Source");
        warp.set("WarpAmplitude", warp_amp).expect("set WarpAmplitude");

        warp
    }

    /// Build Graph 6: terrain shape (low/high/selector blend + domain warp).
    ///
    /// Topology:
    ///   low:  Simplex(500) -> DomainScale(0.015) -> DomainAxisScale(Y:0.7) -> FBm(4oct)
    ///   high: Simplex(600) -> DomainScale(0.015) -> DomainAxisScale(Y:0.7) -> FBm(4oct)
    ///   sel:  Simplex(700) -> DomainScale(0.008) -> FBm(2oct) -> Remap[-1,1]->[0,1]
    ///   blend: Fade(A:low, B:high, Fade:sel)
    ///   final: DomainWarpGradient(Source:blend, WarpAmplitude:25.0)
    fn build_terrain_shape_graph(params: &TerrainGenParams) -> Node {
        // -- Low noise path --
        let mut low_src = Node::from_name("Simplex").expect("Simplex");
        low_src.set("SeedOffset", 500i32).expect("set SeedOffset");

        let mut low_scale = Node::from_name("DomainScale").expect("DomainScale");
        low_scale.set("Source", &low_src).expect("set Source");
        low_scale.set("Scaling", params.terrain_freq).expect("set Scaling");

        let mut low_axis = Node::from_name("DomainAxisScale").expect("DomainAxisScale");
        low_axis.set("Source", &low_scale).expect("set Source");
        low_axis.set("Scaling X", 1.0f32).expect("set Scaling X");
        low_axis.set("Scaling Y", params.y_squash).expect("set Scaling Y");
        low_axis.set("Scaling Z", 1.0f32).expect("set Scaling Z");
        low_axis.set("Scaling W", 1.0f32).expect("set Scaling W");

        let mut low_frac = Node::from_name("FractalFBm").expect("FractalFBm");
        low_frac.set("Source", &low_axis).expect("set Source");
        low_frac.set("Octaves", params.terrain_octaves).expect("set Octaves");
        low_frac.set("Gain", params.terrain_gain).expect("set Gain");
        low_frac.set("Lacunarity", 2.0f32).expect("set Lacunarity");

        // -- High noise path --
        let mut high_src = Node::from_name("Simplex").expect("Simplex");
        high_src.set("SeedOffset", 600i32).expect("set SeedOffset");

        let mut high_scale = Node::from_name("DomainScale").expect("DomainScale");
        high_scale.set("Source", &high_src).expect("set Source");
        high_scale.set("Scaling", params.terrain_freq).expect("set Scaling");

        let mut high_axis = Node::from_name("DomainAxisScale").expect("DomainAxisScale");
        high_axis.set("Source", &high_scale).expect("set Source");
        high_axis.set("Scaling X", 1.0f32).expect("set Scaling X");
        high_axis.set("Scaling Y", params.y_squash).expect("set Scaling Y");
        high_axis.set("Scaling Z", 1.0f32).expect("set Scaling Z");
        high_axis.set("Scaling W", 1.0f32).expect("set Scaling W");

        let mut high_frac = Node::from_name("FractalFBm").expect("FractalFBm");
        high_frac.set("Source", &high_axis).expect("set Source");
        high_frac.set("Octaves", params.terrain_octaves).expect("set Octaves");
        high_frac.set("Gain", params.terrain_gain).expect("set Gain");
        high_frac.set("Lacunarity", 2.0f32).expect("set Lacunarity");

        // -- Selector path --
        let mut sel_src = Node::from_name("Simplex").expect("Simplex");
        sel_src.set("SeedOffset", 700i32).expect("set SeedOffset");

        let mut sel_scale = Node::from_name("DomainScale").expect("DomainScale");
        sel_scale.set("Source", &sel_src).expect("set Source");
        sel_scale.set("Scaling", params.selector_freq).expect("set Scaling");

        let mut sel_frac = Node::from_name("FractalFBm").expect("FractalFBm");
        sel_frac.set("Source", &sel_scale).expect("set Source");
        sel_frac.set("Octaves", 2i32).expect("set Octaves");
        sel_frac.set("Gain", 0.5f32).expect("set Gain");
        sel_frac.set("Lacunarity", 2.0f32).expect("set Lacunarity");

        let mut sel_remap = Node::from_name("Remap").expect("Remap");
        sel_remap.set("Source", &sel_frac).expect("set Source");
        sel_remap.set("FromMin", -1.0f32).expect("set FromMin");
        sel_remap.set("FromMax", 1.0f32).expect("set FromMax");
        sel_remap.set("ToMin", 0.0f32).expect("set ToMin");
        sel_remap.set("ToMax", 1.0f32).expect("set ToMax");

        // -- Blend: Fade interpolates low->high controlled by selector --
        let mut blend = Node::from_name("Fade").expect("Fade");
        blend.set("A", &low_frac).expect("set A");
        blend.set("B", &high_frac).expect("set B");
        blend.set("Fade", &sel_remap).expect("set Fade");

        // -- Domain warp for organic distortion --
        let mut warp = Node::from_name("DomainWarpGradient").expect("DomainWarpGradient");
        warp.set("Source", &blend).expect("set Source");
        warp.set("WarpAmplitude", params.terrain_warp).expect("set WarpAmplitude");

        warp
    }

    // -- Public generation API -----------------------------------------

    /// Approximate terrain surface height at world (x, z) using biome-blended
    /// base_height from the column cache. This is the zero-crossing estimate
    /// for the 3D density field — exact for flat terrain, approximate where
    /// 3D noise creates overhangs/underhang.
    pub fn terrain_height(&self, wx: f32, wz: f32, _params: &TerrainGenParams) -> f32 {
        let cx = (wx / CHUNK_WORLD_SIZE).floor() as i32;
        let cz = (wz / CHUNK_WORLD_SIZE).floor() as i32;
        let col_cache = self.get_column_cache(cx, cz);
        let lx = ((wx - cx as f32 * CHUNK_WORLD_SIZE) / VOXEL_SCALE)
            .clamp(0.0, (CHUNK_SIZE - 1) as f32) as usize;
        let lz = ((wz - cz as f32 * CHUNK_WORLD_SIZE) / VOXEL_SCALE)
            .clamp(0.0, (CHUNK_SIZE - 1) as f32) as usize;
        col_cache.blended[ColumnCache::xz_index(lx, lz)].base_height
    }

    /// Retrieve or populate the column cache for chunk column (cx, cz).
    /// Returns an Arc so multiple vertical chunks share the same data.
    pub fn get_column_cache(&self, cx: i32, cz: i32) -> Arc<ColumnCache> {
        let key = (cx, cz);

        // Fast path: already cached
        {
            let map = self.column_cache.lock().unwrap();
            if let Some(cached) = map.get(&key) {
                return Arc::clone(cached);
            }
        }

        // Slow path: evaluate all 5 noise graphs for this column
        let cache = Arc::new(self.populate_column_cache(cx, cz));

        let mut map = self.column_cache.lock().unwrap();
        // Another thread may have populated while we were computing - use
        // entry API to avoid redundant insertion but accept rare double-eval.
        Arc::clone(map.entry(key).or_insert(cache))
    }

    /// Evict column caches outside the given radius (in chunks) from center.
    /// Call periodically to bound memory usage during streaming.
    pub fn evict_column_caches(&self, center_cx: i32, center_cz: i32, radius: i32) {
        let mut map = self.column_cache.lock().unwrap();
        map.retain(|&(cx, cz), _| {
            (cx - center_cx).abs() <= radius && (cz - center_cz).abs() <= radius
        });
    }

    /// Populate biome blend data for a chunk column. Uses gen_single_2d for
    /// all noise queries to ensure cross-chunk consistency at biome grid points.
    fn populate_column_cache(&self, cx: i32, cz: i32) -> ColumnCache {
        let mut cache = ColumnCache::new();

        let chunk_world_x = cx as f32 * CHUNK_WORLD_SIZE;
        let chunk_world_z = cz as f32 * CHUNK_WORLD_SIZE;

        // -- Biome Blend --
        // Always use gen_single_2d for biome grid point noise evaluation.
        // Grid points are at arbitrary jittered world positions shared between
        // adjacent chunks. Using gen_uniform_grid_2d (SIMD batch) for in-chunk
        // points and gen_single_2d (scalar) for out-of-chunk points produces
        // different values for the same coordinates through DomainWarpGradient
        // nodes, causing biome classification discontinuities at chunk borders.
        let noise_eval = |channel: usize, wx: f32, wz: f32| -> f32 {
            match channel {
                NM_CONTINENTALNESS => self.continental.gen_single_2d(wx, wz, self.seed),
                NM_EROSION         => self.erosion.gen_single_2d(wx, wz, self.seed),
                NM_PEAKS_VALLEYS   => self.elevation_detail.gen_single_2d(wx, wz, self.seed),
                NM_TEMPERATURE     => self.temperature.gen_single_2d(wx, wz, self.seed),
                NM_HUMIDITY        => self.humidity.gen_single_2d(wx, wz, self.seed),
                _ => 0.0,
            }
        };

        compute_biome_blend(
            &mut cache.biome_weights,
            &mut cache.dominant_biome,
            chunk_world_x,
            chunk_world_z,
            self.seed,
            &noise_eval,
        );
        compute_blended_params(&mut cache.blended, &cache.biome_weights);

        cache
    }

    fn classify_by_height(&self, position: IVec3, params: &TerrainGenParams) -> HeightClassification {
        let chunk_y_bot = position.y as f32 * CHUNK_WORLD_SIZE;
        let chunk_y_top = chunk_y_bot + CHUNK_WORLD_SIZE;

        // FBm(5oct, gain=0.55) output bound: sum of geometric series
        // 1 + 0.55 + 0.3025 + 0.166 + 0.091 ≈ 2.11
        const NOISE_BOUND: f32 = 2.2;
        const WARP_MARGIN: f32 = 5.0;

        // -- Fast global bounds (no column cache lock needed) --
        // Uses worst-case biome params from the biome table:
        //   max base_height = 60 (ShatteredPeaks), max amp = 20, min squash = 0.5
        //   min base_height = 20 (Ocean)
        const GLOBAL_MAX_TERRAIN: f32 = 60.0 + 20.0 * NOISE_BOUND / 0.5 + WARP_MARGIN;
        const GLOBAL_MIN_TERRAIN: f32 = 20.0 - 20.0 * NOISE_BOUND / (4.0 * 0.5) - WARP_MARGIN;

        if chunk_y_bot > GLOBAL_MAX_TERRAIN {
            return HeightClassification::Air;
        }
        if chunk_y_top < GLOBAL_MIN_TERRAIN {
            return HeightClassification::Solid;
        }

        // -- Tighter per-column bounds via 8 representative XZ samples --
        // 4 corners + 4 edge midpoints cover the chunk footprint.
        // Biome params are smoothly blended over ~20 world-unit radius, so 8
        // samples across a 16 world-unit chunk capture the full param range.
        const SAMPLE_POSITIONS: [(usize, usize); 8] = [
            (0, 0),   (0, 31),  (31, 0),  (31, 31),  // corners
            (15, 0),  (31, 15), (15, 31), (0, 15),   // edge midpoints
        ];

        let col_cache = self.get_column_cache(position.x, position.z);

        let mut max_terrain = f32::MIN;
        let mut min_terrain = f32::MAX;

        for &(lx, lz) in &SAMPLE_POSITIONS {
            let idx = ColumnCache::xz_index(lx, lz);
            let bp = &col_cache.blended[idx];
            let squash = bp.squash_factor.max(0.05);

            // Conservative upper bound: terrain noise could push surface up by
            // amplitude_3d * NOISE_BOUND / squash_factor above base_height
            let col_max = bp.base_height + bp.amplitude_3d * NOISE_BOUND / squash + WARP_MARGIN;
            // Conservative lower bound: asymmetric gradient (4x steeper below)
            // means terrain extends less far below base_height
            let col_min = bp.base_height - bp.amplitude_3d * NOISE_BOUND / (4.0 * squash) - WARP_MARGIN;

            max_terrain = max_terrain.max(col_max);
            min_terrain = min_terrain.min(col_min)
        }

        let effective_max = max_terrain.min(params.ceiling_level + 20.0);
        let effective_min = min_terrain.max(params.floor_level - 10.0);

        if chunk_y_bot > effective_max {
            return HeightClassification::Air;
        }
        if chunk_y_top < effective_min {
            // Chunk is fully below terrain - could contain caves
            if params.cave_enabled {
                return HeightClassification::BelowTerrainCavesEnabled;
            }
            return HeightClassification::Solid;
        }
        HeightClassification::Indeterminate
    }

    fn try_classify_caves_solid(&self, position: IVec3, params: &TerrainGenParams) -> Option<ChunkStorage> {
        if !params.cave_enabled {
            return Some(ChunkStorage::Uniform { density: 127, material_id: MAT_GRANITE });
        }

        let chunk_world_x = position.x as f32 * CHUNK_WORLD_SIZE;
        let chunk_world_y = position.y as f32 * CHUNK_WORLD_SIZE;
        let chunk_world_z = position.z as f32 * CHUNK_WORLD_SIZE;

        // -- Derive worst-case cave thresholds from biome-blended params --
        const SAMPLE_POSITIONS: [(usize, usize); 8] = [
            (0, 0),   (0, 31),  (31, 0),  (31, 31),
            (15, 0),  (31, 15), (15, 31), (0, 15),
        ];

        let col_cache = self.get_column_cache(position.x, position.z);

        let mut max_cave_density = 0.0f32;
        let mut max_hollowness = 0.0f32;
        let mut max_thickness = 0.0f32;
        let mut max_surface_h = f32::MIN;

        for &(lx, lz) in &SAMPLE_POSITIONS {
            let idx = ColumnCache::xz_index(lx, lz);
            let bp = &col_cache.blended[idx];
            max_cave_density = max_cave_density.max(bp.cave_density);
            max_hollowness = max_hollowness.max(bp.cheese_hollowness);
            max_thickness = max_thickness.max(bp.spaghetti_thickness);
            max_surface_h = max_surface_h.max(bp.base_height);
        }

        // If cave_density is negligible, chunk is solid
        if max_cave_density < 0.01 {
            return Some(ChunkStorage::Uniform { density: 127, material_id: MAT_GRANITE });
        }

        // If entire chunk is within surface exclusion zone, no caves
        let chunk_y_top = chunk_world_y + CHUNK_WORLD_SIZE;
        if chunk_world_y > max_surface_h - 8.0 {
            return Some(ChunkStorage::Uniform { density: 127, material_id: MAT_GRANITE });
        }

        // If entire chunk is below bedrock taper zone, no caves
        if chunk_y_top < params.floor_level + 10.0 {
            return Some(ChunkStorage::Uniform { density: 127, material_id: MAT_GRANITE });
        }

        // Coarse 8x8x8 evaluation
        const COARSE_3D: usize = 8;
        const COARSE_VOL: usize = COARSE_3D * COARSE_3D * COARSE_3D;
        let coarse_step = CHUNK_WORLD_SIZE / COARSE_3D as f32;
        let cs = COARSE_3D as i32;

        let mut cheese_buf = [0.0f32; COARSE_VOL];
        self.cave_cheese.gen_uniform_grid_3d(
            &mut cheese_buf,
            chunk_world_x, chunk_world_y, chunk_world_z,
            cs, cs, cs,
            coarse_step, coarse_step, coarse_step,
            self.seed,
        );

        let mut spa_a_buf = [0.0f32; COARSE_VOL];
        self.cave_spaghetti_a.gen_uniform_grid_3d(
            &mut spa_a_buf,
            chunk_world_x, chunk_world_y, chunk_world_z,
            cs, cs, cs,
            coarse_step, coarse_step, coarse_step,
            self.seed,
        );

        let mut spa_b_buf = [0.0f32; COARSE_VOL];
        self.cave_spaghetti_b.gen_uniform_grid_3d(
            &mut spa_b_buf,
            chunk_world_x, chunk_world_y, chunk_world_z,
            cs, cs, cs,
            coarse_step, coarse_step, coarse_step,
            self.seed,
        );

        // Conservative margin for coarse-to-fine interpolation error
        let margin = 0.3;

        for i in 0..COARSE_VOL {
            // Worst-case cheese: use minimum threshold (1-max_hollowness)
            let cheese_val = cheese_buf[i] + (1.0 - max_hollowness);
            // Worst-case spaghetti: use max thickness (lowest distance threshold)
            let spa_val = spa_a_buf[i].abs().max(spa_b_buf[i].abs()) - max_thickness;
            let cave_noise = cheese_val.min(spa_val);
            if cave_noise < -margin {
                return None; // Might carve - need full generation
            }
        }

        Some(ChunkStorage::Uniform { density: 127, material_id: MAT_GRANITE })
    }

    pub fn generate_world(
        &self,
        params: &TerrainGenParams,
        min_y: i32,
        max_y: i32,
    ) -> std::collections::HashMap<IVec3, Chunk> {
        let half_x = WORLD_CHUNKS_X as i32 / 2;
        let half_z = WORLD_CHUNKS_Z as i32 / 2;
        let mut chunks = std::collections::HashMap::new();

        for cz in -half_z..half_z {
            for cy in min_y..max_y {
                for cx in -half_x..half_x {
                    let pos = IVec3::new(cx, cy, cz);
                    let storage = self.generate_chunk_storage(pos, params);
                    let chunk = Chunk::new(pos, std::sync::Arc::new(storage));
                    chunks.insert(pos, chunk);
                }
            }
        }

        chunks
    }

    pub fn generate_chunk(&self, chunk: &mut Chunk, params: &TerrainGenParams) {
        let storage = self.generate_chunk_storage(chunk.position, params);
        chunk.storage = std::sync::Arc::new(storage);
    }

    /// Generate a ChunkStorage directly from terrain parameters.
    /// Used by streaming workers and background regeneration.
    pub fn generate_chunk_storage(&self, position: IVec3, params: &TerrainGenParams) -> ChunkStorage {
        // -- Range analysis early termination --
        match self.classify_by_height(position, params) {
            HeightClassification::Air => {
                GEN_STATS.skipped_air.fetch_add(1, Ordering::Relaxed);
                return ChunkStorage::Uniform { density: -128, material_id: MAT_AIR };
            }
            HeightClassification::Solid => {
                GEN_STATS.skipped_solid.fetch_add(1, Ordering::Relaxed);
                return ChunkStorage::Uniform { density: 127, material_id: MAT_GRANITE };
            }
            HeightClassification::BelowTerrainCavesEnabled => {
                if let Some(storage) = self.try_classify_caves_solid(position, params) {
                    GEN_STATS.skipped_solid.fetch_add(1, Ordering::Relaxed);
                    return storage;
                }
            }
            HeightClassification::Indeterminate => {}
        }
        GEN_STATS.full_gen.fetch_add(1, Ordering::Relaxed);

        let chunk_world_x = position.x as f32 * CHUNK_WORLD_SIZE;
        let chunk_world_y = position.y as f32 * CHUNK_WORLD_SIZE;
        let chunk_world_z = position.z as f32 * CHUNK_WORLD_SIZE;

        // ==============================================================
        //  STRUCTURAL NOISE
        // ==============================================================

        let col_cache = self.get_column_cache(position.x, position.z);
        let cs = CHUNK_SIZE as i32;

        // Evaluate terrain shape over full 32^3 volume
        let mut terrain_noise = vec![0.0f32; CHUNK_VOLUME];
        self.terrain_shape.gen_uniform_grid_3d(
            &mut terrain_noise,
            chunk_world_x, chunk_world_y, chunk_world_z,
            cs, cs, cs,
            VOXEL_SCALE, VOXEL_SCALE, VOXEL_SCALE,
            self.seed,
        );

        // ==============================================================
        //  CAVE NOISE (Graph 7: fused cheese + spaghetti)
        // ==============================================================

        let (cave_cheese_buf, cave_spa_a_buf, cave_spa_b_buf);
        let caves_active = params.cave_enabled;

        if caves_active {
            let mut cheese = vec![0.0f32; CHUNK_VOLUME];
            self.cave_cheese.gen_uniform_grid_3d(
                &mut cheese,
                chunk_world_x, chunk_world_y, chunk_world_z,
                cs, cs, cs,
                VOXEL_SCALE, VOXEL_SCALE, VOXEL_SCALE,
                self.seed,
            );

            let mut spa_a = vec![0.0f32; CHUNK_VOLUME];
            self.cave_spaghetti_a.gen_uniform_grid_3d(
                &mut spa_a,
                chunk_world_x, chunk_world_y, chunk_world_z,
                cs, cs, cs,
                VOXEL_SCALE, VOXEL_SCALE, VOXEL_SCALE,
                self.seed,
            );

            let mut spa_b = vec![0.0f32; CHUNK_VOLUME];
            self.cave_spaghetti_b.gen_uniform_grid_3d(
                &mut spa_b,
                chunk_world_x, chunk_world_y, chunk_world_z,
                cs, cs, cs,
                VOXEL_SCALE, VOXEL_SCALE, VOXEL_SCALE,
                self.seed,
            );

            cave_cheese_buf = cheese;
            cave_spa_a_buf = spa_a;
            cave_spa_b_buf = spa_b;
        } else {
            cave_cheese_buf = Vec::new();
            cave_spa_a_buf = Vec::new();
            cave_spa_b_buf = Vec::new();
        }

        // ==============================================================
        //  PASS 1: 3D density computation
        // ==============================================================

        let mut density_arr = vec![0i8; CHUNK_VOLUME].into_boxed_slice();
        let mut has_near_surface = false;

        // Track approximate surface height per column for material assignment
        let mut approx_surface = [f32::MIN; CHUNK_SIZE * CHUNK_SIZE];

        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let xz_idx = ColumnCache::xz_index(lx, lz);
                let bp = &col_cache.blended[xz_idx];

                for ly in 0..CHUNK_SIZE {
                    let wy = chunk_world_y + ly as f32 * VOXEL_SCALE;
                    let idx = Chunk::voxel_index(lx, ly, lz);

                    // Asymmetric height gradient (Beta 1.7.3)
                    let mut gradient = (wy - bp.base_height) * bp.squash_factor;
                    if gradient < 0.0 {
                        gradient *= 4.0;
                    }

                    // Core density: 3D noise * amplitude - gradient
                    let mut d = terrain_noise[idx] * bp.amplitude_3d - gradient;

                    // -- Cave carving (Graph 7) --
                    // Uses min-override instead of additive: when cave_noise < 0,
                    // density is clamped to the cave signal. This ensures caves
                    // carve through arbitrarily solid deep underground rock.
                    if caves_active {
                        let mask = depth_cave_mask(wy, bp.base_height, params.floor_level);
                        if mask > 0.0 {
                            let depth_voxels = (bp.base_height - wy) / VOXEL_SCALE;
                            let cave_bp = underground_fade(bp, depth_voxels);

                            // Cheese: large round chambers where noise dips below threshold.
                            // (1-hollowness) sets the threshold; higher hollowness = lower
                            // threshold = more chambers carved.
                            let cheese_val = cave_cheese_buf[idx]
                                + (1.0 - cave_bp.cheese_hollowness);

                            // Spaghetti: narrow tunnels at the intersection of two
                            // zero-isosurfaces. Carves only when BOTH |a| and |b| are
                            // within `thickness` of zero — creating tube-shaped passages.
                            let spa_val = cave_spa_a_buf[idx].abs()
                                .max(cave_spa_b_buf[idx].abs())
                                - cave_bp.spaghetti_thickness;

                            let cave_noise = cheese_val.min(spa_val);
                            if cave_noise < 0.0 {
                                // Amplify so cave walls quantize cleanly (raw signal
                                // is small, ~[-0.7, 0); ×5 gives i8 values of -8..0).
                                d = d.min(cave_noise * cave_bp.cave_density * 5.0 * mask);
                            }
                        }
                    }

                    // Floor clamp: force solid below floor_level
                    if wy < params.floor_level {
                        d += (params.floor_level - wy) * 2.0;
                    }
                    // Ceiling fade: force air above ceiling_level
                    if wy > params.ceiling_level {
                        d -= (wy - params.ceiling_level) * 0.5;
                    }

                    let density = quantize_density(d, params.density_scale);
                    density_arr[idx] = density;

                    // Track surface for material assignment
                    if density > 0 && wy > approx_surface[xz_idx] {
                        approx_surface[xz_idx] = wy;
                    }

                    // Near-surface detection for detail noise pass
                    if !has_near_surface && (density as i16).abs() <= SURFACE_MARGIN as i16 {
                        has_near_surface = true;
                    }
                }

                // Fallback: if no solid voxel found in this chunk, use base_height
                if approx_surface[xz_idx] == f32::MIN {
                    approx_surface[xz_idx] = bp.base_height;
                }
            }
        }

        // ==============================================================
        //  PASS 2: Detail noise + material/moisture assignment
        // ==============================================================

        let mut material_arr = [MAT_AIR; CHUNK_VOLUME];
        let mut moisture_arr = [0u8; CHUNK_VOLUME];

        // Only batch-evaluate detail noise if there are near-surface voxels
        // that actually need it (solid voxels with depth <= 6.0).
        // The has_near_surface flag is conservative — if ANY voxel could
        // need detail noise, we evaluate the full 32x32 grids.
        let material_noise_map;
        let moisture_noise_map;

        if has_near_surface {
            let mut mat_buf = [0.0f32; CHUNK_SIZE * CHUNK_SIZE];
            self.material_graph.gen_uniform_grid_2d(
                &mut mat_buf,
                chunk_world_x * 0.05, chunk_world_z * 0.05,
                cs, cs,
                VOXEL_SCALE * 0.05, VOXEL_SCALE * 0.05,
                self.seed + 4,
            );
            material_noise_map = mat_buf;

            let mut moist_buf = [0.0f32; CHUNK_SIZE * CHUNK_SIZE];
            self.moisture_graph.gen_uniform_grid_2d(
                &mut moist_buf,
                chunk_world_x * 0.02, chunk_world_z * 0.02,
                cs, cs,
                VOXEL_SCALE * 0.02, VOXEL_SCALE * 0.02,
                self.seed + 5,
            );
            moisture_noise_map = moist_buf;
        } else {
            // All solid voxels are deep interior — skip detail noise entirely
            GEN_STATS.detail_noise_skipped.fetch_add(1, Ordering::Relaxed);
            material_noise_map = [0.0f32; CHUNK_SIZE * CHUNK_SIZE];
            moisture_noise_map = [0.0f32; CHUNK_SIZE * CHUNK_SIZE];
        }

        // Voxel classification counters for this chunk
        let mut count_near_surface: u32 = 0;
        let mut count_deep_solid: u32 = 0;
        let mut count_clear_air: u32 = 0;

        for lz in 0..CHUNK_SIZE {
            for ly in 0..CHUNK_SIZE {
                let wy = chunk_world_y + ly as f32 * VOXEL_SCALE;
                for lx in 0..CHUNK_SIZE {
                    let idx = Chunk::voxel_index(lx, ly, lz);
                    let density = density_arr[idx];

                    if density <= 0 {
                        // Air (including carved voxels) — no material needed
                        count_clear_air += 1;
                        continue;
                    }

                    let xz_idx = ColumnCache::xz_index(lx, lz);
                    let surface_h = approx_surface[xz_idx];
                    let depth_below_surface = surface_h - wy;

                    // Deep solid: always granite, but include moisture noise for identity
                    if depth_below_surface > 6.0 {
                        material_arr[idx] = MAT_GRANITE;
                        let base_moisture =
                            ((params.water_level - wy) / 20.0 + 0.5).clamp(0.0, 1.0);
                        let noise_val = moisture_noise_map[lz * CHUNK_SIZE + lx];
                        let moisture = ((base_moisture + noise_val * 0.3) * 255.0)
                            .clamp(0.0, 255.0) as u8;
                        moisture_arr[idx] = moisture;
                        count_deep_solid += 1;
                        continue;
                    }

                    // Near-surface: full material assignment with detail noise
                    count_near_surface += 1;

                    let steepness = {
                        // Use biome-blended base_height for steepness gradient.
                        // Unlike approx_surface (per-chunk, noisy), base_height is
                        // deterministic and identical across chunk borders, eliminating
                        // material seams at chunk edges.
                        let dx = if lx > 0 && lx < CHUNK_SIZE - 1 {
                            (col_cache.blended[ColumnCache::xz_index(lx + 1, lz)].base_height
                                - col_cache.blended[ColumnCache::xz_index(lx - 1, lz)].base_height)
                                / (2.0 * VOXEL_SCALE)
                        } else if lx == 0 {
                            (col_cache.blended[ColumnCache::xz_index(1, lz)].base_height
                                - col_cache.blended[ColumnCache::xz_index(0, lz)].base_height)
                                / VOXEL_SCALE
                        } else {
                            (col_cache.blended[ColumnCache::xz_index(CHUNK_SIZE - 1, lz)].base_height
                                - col_cache.blended[ColumnCache::xz_index(CHUNK_SIZE - 2, lz)].base_height)
                                / VOXEL_SCALE
                        };
                        let dz = if lz > 0 && lz < CHUNK_SIZE - 1 {
                            (col_cache.blended[ColumnCache::xz_index(lx, lz + 1)].base_height
                                - col_cache.blended[ColumnCache::xz_index(lx, lz - 1)].base_height)
                                / (2.0 * VOXEL_SCALE)
                        } else if lz == 0 {
                            (col_cache.blended[ColumnCache::xz_index(lx, 1)].base_height
                                - col_cache.blended[ColumnCache::xz_index(lx, 0)].base_height)
                                / VOXEL_SCALE
                        } else {
                            (col_cache.blended[ColumnCache::xz_index(lx, CHUNK_SIZE - 1)].base_height
                                - col_cache.blended[ColumnCache::xz_index(lx, CHUNK_SIZE - 2)].base_height)
                                / VOXEL_SCALE
                        };
                        (dx * dx + dz * dz).sqrt()
                    };
                    let mat_noise = material_noise_map[lz * CHUNK_SIZE + lx];
                    let cliff = 2.0f32;

                    // ── Material assignment (UNCHANGED logic) ──
                    let material = if steepness > cliff && depth_below_surface < 6.0 {
                        MAT_LIMESTONE
                    } else if steepness > cliff * 0.6 && depth_below_surface < 4.0 {
                        MAT_GRANITE
                    } else if wy < params.water_level + 1.5
                        && depth_below_surface < 2.0
                    {
                        MAT_SAND
                    } else if wy < params.water_level + 3.0
                        && depth_below_surface < 3.0
                        && steepness < cliff * 0.3
                    {
                        MAT_GRAVEL
                    } else if depth_below_surface < 1.5 && steepness < cliff * 0.5 {
                        MAT_GRASS_SOIL
                    } else if depth_below_surface < 1.5 && steepness >= cliff * 0.5 {
                        MAT_SOIL
                    } else if depth_below_surface < 3.0 {
                        MAT_SOIL
                    } else if depth_below_surface < 6.0 {
                        if mat_noise > 0.3 { MAT_CLAY } else { MAT_SOIL }
                    } else {
                        MAT_GRANITE
                    };

                    material_arr[idx] = material;

                    // -- Moisture (with noise) --
                    let base_moisture =
                        ((params.water_level - wy) / 20.0 + 0.5).clamp(0.0, 1.0);
                    let noise_val = moisture_noise_map[lz * CHUNK_SIZE + lx];
                    let moisture = ((base_moisture + noise_val * 0.3) * 255.0)
                        .clamp(0.0, 255.0) as u8;
                    moisture_arr[idx] = moisture;
                }
            }
        }

        // Update global classification counters
        GEN_STATS.voxels_near_surface.fetch_add(count_near_surface as u64, Ordering::Relaxed);
        GEN_STATS.voxels_deep_solid.fetch_add(count_deep_solid as u64, Ordering::Relaxed);
        GEN_STATS.voxels_clear_air.fetch_add(count_clear_air as u64, Ordering::Relaxed);

        // SAFETY: Vec guarantees length == CHUNK_VOLUME
        let density_box: Box<[i8; CHUNK_VOLUME]> = unsafe {
            Box::from_raw(Box::into_raw(density_arr) as *mut [i8; CHUNK_VOLUME])
        };

        storage::storage_from_arrays_with_moisture(density_box, &material_arr, &moisture_arr)
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Quantize floating-point density to i8. Scale factor 12.7 maps +/-10
/// raw density to the full i8 range, preserving precision near the
/// zero-crossing where marching cubes interpolates.
#[inline]
fn quantize_density(d: f32, scale: f32) -> i8 {
    (d * scale).clamp(-128.0, 127.0) as i8
}

/// Depth-based cave attenuation mask.
/// - No caves within 8 world units of the surface
/// - Linear taper from 8..16 world units depth
/// - Full cave strength below 16 world units
/// - Taper to zero within 10 world units of floor_level (bedrock)
#[inline]
fn depth_cave_mask(wy: f32, surface_h: f32, floor_level: f32) -> f32 {
    let depth = surface_h - wy;
    if depth < 8.0 {
        return 0.0;
    }
    let top_fade = ((depth - 8.0) / 8.0).min(1.0);
    let bottom_fade = ((wy - floor_level) / 10.0).clamp(0.0, 1.0);
    top_fade * bottom_fade
}
