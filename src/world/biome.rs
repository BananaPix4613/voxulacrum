use crate::world::chunk::{CHUNK_SIZE, CHUNK_WORLD_SIZE, VOXEL_SCALE};
use crate::world::generation::{NM_CONTINENTALNESS, NM_EROSION, NM_TEMPERATURE, NM_HUMIDITY};
use crate::world::voxel::*;

// ============================================================================
// Biome identity
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BiomeId {
    Ocean             = 0,
    VerdantHighlands  = 1,
    ShatteredPeaks    = 2,
    SunkenDesert      = 3,
    WhisperingMarsh   = 4,
    AncientForest     = 5,
    VolcanicWastes    = 6,
    FrozenExpanse     = 7,
}

pub const BIOME_COUNT: usize = 8;

impl BiomeId {
    pub const ALL: [BiomeId; BIOME_COUNT] = [
        BiomeId::Ocean,
        BiomeId::VerdantHighlands,
        BiomeId::ShatteredPeaks,
        BiomeId::SunkenDesert,
        BiomeId::WhisperingMarsh,
        BiomeId::AncientForest,
        BiomeId::VolcanicWastes,
        BiomeId::FrozenExpanse,
    ];

    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempBand { Cold, Warm, Hot }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoistureBand { Dry, Wet }

pub fn temp_band(t: f32) -> TempBand {
    if t < -0.3 { TempBand::Cold }
    else if t > 0.3 { TempBand::Hot }
    else { TempBand::Warm }
}

pub fn moisture_band(m: f32) -> MoistureBand {
    if m < 0.0 { MoistureBand::Dry }
    else { MoistureBand::Wet }
}

// ============================================================================
// Per-biome terrain parameters
// ============================================================================

#[derive(Clone, Debug)]
pub struct BiomeParams {
    // Terrain shape
    pub base_height: f32,
    pub squash_factor: f32,
    pub amplitude_3d: f32,
    pub elevation_detail: f32,

    // Underground
    pub cave_density: f32,
    pub cheese_hollowness: f32,
    pub spaghetti_thickness: f32,

    // Water
    pub local_water_level: f32,

    // Surface materials
    pub top_material: u16,
    pub sub_material: u16,
    pub sub_depth: f32,
    pub stone_material: u16,
    pub deep_stone_material: u16,

    // Blending
    pub blend_radius: f32,
}

// ============================================================================
// Biome table - one entry per BiomeId
// ============================================================================

pub static BIOME_TABLE: [BiomeParams; BIOME_COUNT] = [
    // 0: Ocean
    BiomeParams {
        base_height: 20.0,
        squash_factor: 0.8,
        amplitude_3d: 4.0,
        elevation_detail: 0.5,
        cave_density: 0.3,
        cheese_hollowness: 0.2,
        spaghetti_thickness: 0.10,
        local_water_level: 30.0,
        top_material: MAT_SAND,
        sub_material: MAT_CLAY,
        sub_depth: 4.0,
        stone_material: MAT_LIMESTONE,
        deep_stone_material: MAT_DEEP_STONE,
        blend_radius: 8.0,
    },
    // 1: VerdantHighlands
    BiomeParams {
        base_height: 40.0,
        squash_factor: 1.0,
        amplitude_3d: 12.0,
        elevation_detail: 1.0,
        cave_density: 0.5,
        cheese_hollowness: 0.4,
        spaghetti_thickness: 0.15,
        local_water_level: 30.0,
        top_material: MAT_GRASS_SOIL,
        sub_material: MAT_SOIL,
        sub_depth: 3.0,
        stone_material: MAT_LIMESTONE,
        deep_stone_material: MAT_GRANITE,
        blend_radius: 6.0,
    },
    // 2: ShatteredPeaks
    BiomeParams {
        base_height: 60.0,
        squash_factor: 1.4,
        amplitude_3d: 20.0,
        elevation_detail: 1.5,
        cave_density: 0.6,
        cheese_hollowness: 0.5,
        spaghetti_thickness: 0.12,
        local_water_level: 30.0,
        top_material: MAT_GRANITE,
        sub_material: MAT_GRANITE,
        sub_depth: 2.0,
        stone_material: MAT_GRANITE,
        deep_stone_material: MAT_DEEP_STONE,
        blend_radius: 10.0,
    },
    // 3: SunkenDesert
    BiomeParams {
        base_height: 28.0,
        squash_factor: 0.6,
        amplitude_3d: 6.0,
        elevation_detail: 0.3,
        cave_density: 0.2,
        cheese_hollowness: 0.3,
        spaghetti_thickness: 0.08,
        local_water_level: 25.0,
        top_material: MAT_SAND,
        sub_material: MAT_SANDSTONE,
        sub_depth: 6.0,
        stone_material: MAT_SANDSTONE,
        deep_stone_material: MAT_DEEP_STONE,
        blend_radius: 12.0,
    },
    // 4: WhisperingMarsh
    BiomeParams {
        base_height: 29.0,
        squash_factor: 0.5,
        amplitude_3d: 3.0,
        elevation_detail: 0.4,
        cave_density: 0.4,
        cheese_hollowness: 0.6,
        spaghetti_thickness: 0.18,
        local_water_level: 31.0,
        top_material: MAT_CLAY,
        sub_material: MAT_CLAY,
        sub_depth: 5.0,
        stone_material: MAT_LIMESTONE,
        deep_stone_material: MAT_DEEP_STONE,
        blend_radius: 6.0,
    },
    // 5: AncientForest
    BiomeParams {
        base_height: 35.0,
        squash_factor: 0.9,
        amplitude_3d: 10.0,
        elevation_detail: 0.8,
        cave_density: 0.7,
        cheese_hollowness: 0.5,
        spaghetti_thickness: 0.14,
        local_water_level: 30.0,
        top_material: MAT_GRASS_SOIL,
        sub_material: MAT_SOIL,
        sub_depth: 4.0,
        stone_material: MAT_GRANITE,
        deep_stone_material: MAT_DEEP_STONE,
        blend_radius: 5.0,
    },
    // 6: VolcanicWastes
    BiomeParams {
        base_height: 38.0,
        squash_factor: 1.2,
        amplitude_3d: 15.0,
        elevation_detail: 1.2,
        cave_density: 0.8,
        cheese_hollowness: 0.7,
        spaghetti_thickness: 0.20,
        local_water_level: 28.0,
        top_material: MAT_BASALT,
        sub_material: MAT_BASALT,
        sub_depth: 3.0,
        stone_material: MAT_BASALT,
        deep_stone_material: MAT_DEEP_STONE,
        blend_radius: 8.0,
    },
    // 7: FrozenExpanse
    BiomeParams {
        base_height: 34.0,
        squash_factor: 0.7,
        amplitude_3d: 8.0,
        elevation_detail: 0.6,
        cave_density: 0.3,
        cheese_hollowness: 0.3,
        spaghetti_thickness: 0.10,
        local_water_level: 30.0,
        top_material: MAT_SNOW,
        sub_material: MAT_ICE,
        sub_depth: 3.0,
        stone_material: MAT_GRANITE,
        deep_stone_material: MAT_DEEP_STONE,
        blend_radius: 7.0,
    },
];

pub fn select_biome(continental: f32, erosion: f32, temperature: f32, humidity: f32) -> BiomeId {
    // Ocean / cost gate
    if continental < -0.2 {
        return BiomeId::Ocean;
    }
    if continental < 0.05 {
        return BiomeId::Ocean; // Coast -> Ocean (no Coast variant yet)
    }

    // Erosion override: extreme erosion + warm-to-hot -> ShatteredPeaks
    let tb = temp_band(temperature);
    if erosion < -0.5 && matches!(tb, TempBand::Warm | TempBand::Hot) {
        return BiomeId::ShatteredPeaks;
    }

    // Temperature x moisture grid
    let mb = moisture_band(humidity);
    match (tb, mb) {
        (TempBand::Cold, MoistureBand::Dry)  => BiomeId::FrozenExpanse,
        (TempBand::Cold, MoistureBand::Wet)  => BiomeId::AncientForest,
        (TempBand::Warm, MoistureBand::Dry)  => BiomeId::SunkenDesert,
        (TempBand::Warm, MoistureBand::Wet)  => BiomeId::VerdantHighlands,
        (TempBand::Hot,  MoistureBand::Dry)  => BiomeId::VolcanicWastes,
        (TempBand::Hot,  MoistureBand::Wet)  => BiomeId::WhisperingMarsh,
    }
}

// ============================================================================
// Underground layer classification
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndergroundLayer {
    Shallow, // 0..32 blocks below surface
    Mid,     // 32..80 blocks below surface
    Deep,    // 80+ blocks below surface
}

pub fn underground_layer(depth_below_surface: f32) -> UndergroundLayer {
    if depth_below_surface < 32.0 {
        UndergroundLayer::Shallow
    } else if depth_below_surface < 80.0 {
        UndergroundLayer::Mid
    } else {
        UndergroundLayer::Deep
    }
}

// ============================================================================
// Biome blending
// ============================================================================

/// Jittered grid spacing: ~64 voxels = 32 world units (VOXEL_SCALE = 0.5)
const GRID_SPACING: f32 = 8.0;
const GRID_SPACING_INV: f32 = 1.0 / 8.0;
/// Max jitter displacement in world units
const JITTER_AMOUNT: f32 = 6.0;
/// How many grid cells to search in each direction from the column's cell
const SEARCH_RADIUS: i32 = 2;
/// Quartic falloff kernel radius in world units.
/// Must exceed max possible distance from a column to its nearest grid point
/// (GRID_SPACING * 0.707 + JITTER_AMOUNT = ~17.3) to guarantee coverage.
const BLEND_RADIUS: f32 = 20.0;
const BLEND_RADIUS_SQ: f32 = BLEND_RADIUS * BLEND_RADIUS;

/// Per-column blended terrain parameters (result of multi-biome weight mixing).
#[derive(Clone, Copy, Debug, Default)]
pub struct BlendedParams {
    pub base_height: f32,
    pub squash_factor: f32,
    pub amplitude_3d: f32,
    pub elevation_detail: f32,
    pub cave_density: f32,
    pub cheese_hollowness: f32,
    pub spaghetti_thickness: f32,
    pub local_water_level: f32,
}

pub const UNDERGROUND_DEFAULTS: BlendedParams = BlendedParams {
    base_height: 32.0,
    squash_factor: 1.0,
    amplitude_3d: 8.0,
    elevation_detail: 0.5,
    cave_density: 0.5,
    cheese_hollowness: 0.4,
    spaghetti_thickness: 0.12,
    local_water_level: 30.0,
};

/// Depth thresholds in voxel units (not world units).
const FADE_START_VOXELS: f32 = 60.0;
const FADE_RANGE_VOXELS: f32 = 40.0;

/// Fade biome influence toward global underground defaults with depth.
/// `depth_below_surface` is in **voxel units** (world_units / VOXEL_SCALE).
pub fn underground_fade(blended: &BlendedParams, depth_below_surface_voxels: f32) -> BlendedParams {
    let biome_influence = 1.0 - ((depth_below_surface_voxels - FADE_START_VOXELS) / FADE_RANGE_VOXELS).clamp(0.0, 1.0);

    if biome_influence >= 1.0 {
        return *blended; // Copy - BlendedParams is Copy
    }
    if biome_influence <= 0.0 {
        return UNDERGROUND_DEFAULTS;
    }

    let t = 1.0 - biome_influence; // 0 = full biome, 1 = full underground
    BlendedParams {
        base_height:         lerp(blended.base_height, UNDERGROUND_DEFAULTS.base_height, t),
        squash_factor:       lerp(blended.squash_factor, UNDERGROUND_DEFAULTS.squash_factor, t),
        amplitude_3d:        lerp(blended.amplitude_3d, UNDERGROUND_DEFAULTS.amplitude_3d, t),
        elevation_detail:    lerp(blended.elevation_detail, UNDERGROUND_DEFAULTS.elevation_detail, t),
        cave_density:        lerp(blended.cave_density, UNDERGROUND_DEFAULTS.cave_density, t),
        cheese_hollowness:   lerp(blended.cheese_hollowness, UNDERGROUND_DEFAULTS.cheese_hollowness, t),
        spaghetti_thickness: lerp(blended.spaghetti_thickness, UNDERGROUND_DEFAULTS.spaghetti_thickness, t),
        local_water_level:   lerp(blended.local_water_level, UNDERGROUND_DEFAULTS.local_water_level, t),
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

struct GridPoint {
    wx: f32,
    wz: f32,
    biome: BiomeId,
}

fn grid_hash(gx: i32, gz: i32, seed: i32) -> (f32, f32) {
    let mut h = (gx as u32).wrapping_mul(374761393)
        .wrapping_add((gz as u32).wrapping_mul(668265263))
        .wrapping_add(seed as u32);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    h = h ^ (h >> 16);
    let fx = (h & 0xFFFF) as f32 / 32767.5 - 1.0;
    let fy = ((h >> 16) & 0xFFFF) as f32 / 32767.5 - 1.0;
    (fx * JITTER_AMOUNT, fy * JITTER_AMOUNT)
}

#[inline]
fn quartic_weight(dist_sq: f32) -> f32 {
    if dist_sq >= BLEND_RADIUS_SQ { return 0.0; }
    let u = BLEND_RADIUS_SQ - dist_sq;  // (r^2 - d^2)
    u * u                                    // (r^2 - d^2)^2
}

pub fn compute_biome_blend(
    biome_weights: &mut [[f32; CHUNK_SIZE * CHUNK_SIZE]; BIOME_COUNT],
    dominant_biome: &mut [BiomeId; CHUNK_SIZE * CHUNK_SIZE],
    chunk_world_x: f32,
    chunk_world_z: f32,
    seed: i32,
    noise_eval: &dyn Fn(usize, f32, f32) -> f32,
) {
    // Step 1: determine grid cell range covering this chunk + search radius
    let min_gx = (chunk_world_x * GRID_SPACING_INV).floor() as i32 - SEARCH_RADIUS;
    let max_gx = ((chunk_world_x + CHUNK_WORLD_SIZE) * GRID_SPACING_INV).floor() as i32 + SEARCH_RADIUS;
    let min_gz = (chunk_world_z * GRID_SPACING_INV).floor() as i32 - SEARCH_RADIUS;
    let max_gz = ((chunk_world_z + CHUNK_WORLD_SIZE) * GRID_SPACING_INV).floor() as i32 + SEARCH_RADIUS;

    // Step 2: pre-compute all grid points (avoids redundant noise evals)
    let mut grid_points = Vec::with_capacity(36);
    for gx in min_gx..=max_gx {
        for gz in min_gz..=max_gz {
            let (jx, jz) = grid_hash(gx, gz, seed);
            let px = gx as f32 * GRID_SPACING + jx;
            let pz = gz as f32 * GRID_SPACING + jz;

            let cont = noise_eval(NM_CONTINENTALNESS, px, pz);
            let eros = noise_eval(NM_EROSION, px, pz);
            let temp = noise_eval(NM_TEMPERATURE, px, pz);
            let humi = noise_eval(NM_HUMIDITY, px, pz);

            let biome = select_biome(cont, eros, temp, humi);
            grid_points.push(GridPoint { wx: px, wz: pz, biome });
        }
    }

    // Step 3: for each column, accumulate quartic-weighted biome contributions
    for lz in 0..CHUNK_SIZE {
        for lx in 0..CHUNK_SIZE {
            let idx = ColumnCache::xz_index(lx, lz);
            let wx = chunk_world_x + lx as f32 * VOXEL_SCALE;
            let wz = chunk_world_z + lz as f32 * VOXEL_SCALE;

            let mut weights = [0.0f32; BIOME_COUNT];
            let mut total = 0.0f32;

            for gp in &grid_points {
                let dx = wx - gp.wx;
                let dz = wz - gp.wz;
                let dist_sq = dx * dx + dz * dz;
                let w = quartic_weight(dist_sq);
                if w > 0.0 {
                    weights[gp.biome.index()] += w;
                    total += w;
                }
            }

            // Normalize
            if total > 0.0 {
                let inv = 1.0 / total;
                for b in 0..BIOME_COUNT {
                    biome_weights[b][idx] = weights[b] * inv;
                }
            } else {
                // Fallback: direct classification at this column
                let cont = noise_eval(NM_CONTINENTALNESS, wx, wz);
                let eros = noise_eval(NM_EROSION, wx, wz);
                let temp = noise_eval(NM_TEMPERATURE, wx, wz);
                let humi = noise_eval(NM_HUMIDITY, wx, wz);
                let biome = select_biome(cont, eros, temp, humi);
                biome_weights[biome.index()][idx] = 1.0;
            }

            // Find dominant biome (highest weight)
            let mut best_b = 0usize;
            let mut best_w = 0.0f32;
            for b in 0..BIOME_COUNT {
                if biome_weights[b][idx] > best_w {
                    best_w = biome_weights[b][idx];
                    best_b = b;
                }
            }
            dominant_biome[idx] = BiomeId::ALL[best_b];
        }
    }
}

pub fn compute_blended_params(
    blended: &mut [BlendedParams; CHUNK_SIZE * CHUNK_SIZE],
    biome_weights: &[[f32; CHUNK_SIZE * CHUNK_SIZE]; BIOME_COUNT],
) {
    for idx in 0..(CHUNK_SIZE * CHUNK_SIZE) {
        let mut bp = BlendedParams::default();
        for b in 0..BIOME_COUNT {
            let w = biome_weights[b][idx];
            if w == 0.0 { continue; }
            let p = &BIOME_TABLE[b];
            bp.base_height         += w * p.base_height;
            bp.squash_factor       += w * p.squash_factor;
            bp.amplitude_3d        += w * p.amplitude_3d;
            bp.elevation_detail    += w * p.elevation_detail;
            bp.cave_density        += w * p.cave_density;
            bp.cheese_hollowness   += w * p.cheese_hollowness;
            bp.spaghetti_thickness += w * p.spaghetti_thickness;
            bp.local_water_level   += w * p.local_water_level;
        }
        blended[idx] = bp;
    }
}

// ============================================================================
// Column cache - per-chunk-column precomputed noise + blended biome params
// ============================================================================

/// Cached column data for a single chunk-column (32x32 XZ footprint).
/// Populated once per chunk column before voxel generation.
pub struct ColumnCache {
    /// Per-column biome weight (how much each biome contributes).
    /// [BIOME_COUNT][CHUNK_SIZE][CHUNK_SIZE]
    pub biome_weights: [[f32; CHUNK_SIZE * CHUNK_SIZE]; BIOME_COUNT],

    /// Per-column blended parameters [CHUNK_SIZE][CHUNK_SIZE]
    pub blended: [BlendedParams; CHUNK_SIZE * CHUNK_SIZE],

    /// Dominant biome per column (highest weight) [CHUNK_SIZE][CHUNK_SIZE]
    pub dominant_biome: [BiomeId; CHUNK_SIZE * CHUNK_SIZE],
}

impl ColumnCache {
    pub fn new() -> Self {
        Self {
            biome_weights: [[0.0; CHUNK_SIZE * CHUNK_SIZE]; BIOME_COUNT],
            blended: std::array::from_fn(|_| BlendedParams::default()),
            dominant_biome: [BiomeId::Ocean; CHUNK_SIZE * CHUNK_SIZE],
        }
    }

    /// Index into a CHUNK_SIZE x CHUNK_SIZE flat array from local xz.
    #[inline]
    pub fn xz_index(lx: usize, lz: usize) -> usize {
        lz * CHUNK_SIZE + lx
    }
}
