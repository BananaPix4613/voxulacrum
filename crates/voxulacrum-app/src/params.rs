use serde::{Deserialize, Serialize};
use std::path::Path;

// ============================================================================
// Lighting parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LightingKeyframe {
    pub time: f32,
    pub sun_color: [f32; 3],
    pub ambient_color: [f32; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LightingParams {
    pub keyframes: Vec<LightingKeyframe>,
}

impl Default for LightingParams {
    fn default() -> Self {
        Self {
            keyframes: vec![
                LightingKeyframe { time: 0.00, sun_color: [0.0, 0.0, 0.0],    ambient_color: [0.05, 0.05, 0.12] },
                LightingKeyframe { time: 0.20, sun_color: [0.0, 0.0, 0.0],    ambient_color: [0.08, 0.08, 0.15] },
                LightingKeyframe { time: 0.25, sun_color: [1.0, 0.55, 0.25],  ambient_color: [0.25, 0.2, 0.25]  },
                LightingKeyframe { time: 0.30, sun_color: [1.0, 0.85, 0.65],  ambient_color: [0.35, 0.35, 0.4]  },
                LightingKeyframe { time: 0.50, sun_color: [1.0, 0.98, 0.92],  ambient_color: [0.45, 0.45, 0.5]  },
                LightingKeyframe { time: 0.60, sun_color: [1.0, 0.95, 0.82],  ambient_color: [0.42, 0.42, 0.47] },
                LightingKeyframe { time: 0.68, sun_color: [1.0, 0.85, 0.6],   ambient_color: [0.38, 0.35, 0.4]  },
                LightingKeyframe { time: 0.73, sun_color: [1.0, 0.6, 0.3],    ambient_color: [0.3, 0.22, 0.28]  },
                LightingKeyframe { time: 0.78, sun_color: [0.6, 0.3, 0.15],   ambient_color: [0.18, 0.14, 0.22] },
                LightingKeyframe { time: 0.83, sun_color: [0.15, 0.08, 0.05], ambient_color: [0.1, 0.08, 0.16]  },
                LightingKeyframe { time: 0.87, sun_color: [0.0, 0.0, 0.0],    ambient_color: [0.08, 0.08, 0.15] },
                LightingKeyframe { time: 1.00, sun_color: [0.0, 0.0, 0.0],    ambient_color: [0.05, 0.05, 0.12] },
            ],
        }
    }
}

// ============================================================================
// Wind parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WindParams {
    pub base_magnitude: f32,
    pub magnitude_variation: f32,
    pub magnitude_frequency: f32,
    pub direction_frequency: f32,
    pub gust_strength: f32,
    pub gust_frequency_a: f32,
    pub gust_frequency_b: f32,
}

impl Default for WindParams {
    fn default() -> Self {
        Self {
            base_magnitude: 2.0,
            magnitude_variation: 1.0,
            magnitude_frequency: 0.03,
            direction_frequency: 0.02,
            gust_strength: 3.0,
            gust_frequency_a: 0.7,
            gust_frequency_b: 1.3,
        }
    }
}

// ============================================================================
// Cloud parameters
// ============================================================================

/// Number of independently-moving cloud layers. Each gets its own wind
/// deflection and simulated macro-grid, composited together in the terrain
/// shader. A named constant (not a growable Vec) because both the shared
/// texture (one channel per layer) and the uniform buffer layout are sized
/// for it at compile time.
pub const CLOUD_LAYER_COUNT: usize = 4;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CloudLayerParams {
    pub base_coverage: f32,
    pub coverage_variation: f32,
    pub coverage_frequency: f32,
    /// Radians this layer's wind is rotated from the shared base wind vector
    /// (`WindState::wind_vector`). Higher layers typically deflect further.
    pub wind_dir_offset: f32,
    /// Multiplier on the (rotated) base wind's magnitude, driving this
    /// layer's macro-grid advection - higher layers usually move faster.
    pub wind_speed_scale: f32,
    /// World-to-UV scale for sampling the live cloud texture - `1 / uv_scale`
    /// is the world-space width of the camera-centered render window.
    /// Independent per layer so the layers' window sizes don't line up.
    pub uv_scale: f32,
    /// Relaxation rate toward each cell's condensation target (higher =
    /// clouds build/dissipate faster).
    pub decay_rate: f32,
    /// Fallback humidity for biomes that don't declare `cloud_humidity`.
    pub default_humidity: f32,
    /// Fallback condensation threshold for biomes that don't declare
    /// `cloud_condensation`.
    pub default_condensation: f32,
    /// How much a macro cell's ambient humidity increases when its
    /// representative chunk has water present. Low layers should be much
    /// more responsive to nearby water than high ones.
    pub water_humidity_boost: f32,
    /// Frequency of the fine coherent noise applied at rasterization time to
    /// carve the coarse simulated density into individual cloud-shaped
    /// patches. `1 / blob_frequency` is roughly a single blob's width in
    /// world units - independent of `CELL_SIZE` (the simulation's own
    /// resolution), so this can be tuned purely for visual scale without
    /// touching simulation cost.
    pub blob_frequency: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CloudParams {
    pub layers: [CloudLayerParams; CLOUD_LAYER_COUNT],
}

impl Default for CloudParams {
    fn default() -> Self {
        Self {
            layers: [
                // Layer 0: low stratus - broad, slow regional dimming. The most
                // water/humidity-coupled layer: coasts run noticeably cloudier.
                CloudLayerParams {
                    base_coverage: 0.12,        // occasional big soft masses, not a lid
                    coverage_variation: 0.06,
                    coverage_frequency: 0.010,  // ~10 min swell - weather, not flicker
                    wind_dir_offset: 0.0,
                    wind_speed_scale: 0.5,      // target ~2.5 u/s drift (see check below)
                    uv_scale: 1.0 / 2048.0,     // masses 150-300 u: several screens wide
                    decay_rate: 0.25,           // stately fronts
                    default_humidity: 0.75,
                    default_condensation: 0.24, // just above 0.3*h: rare clear holes only
                    water_humidity_boost: 0.20,
                    blob_frequency: 1.0 / 80.0, // unused post-rework; kept for serde
                },
                // Layer 1: cumulus - the picturesque layer, carries the look.
                CloudLayerParams {
                    base_coverage: 0.28,        // the main visible coverage dial
                    coverage_variation: 0.10,
                    coverage_frequency: 0.018,  // ~6 min
                    wind_dir_offset: 0.5,
                    wind_speed_scale: 0.8,      // target ~4 u/s: walking pace
                    uv_scale: 1.0 / 1024.0,     // puffs 35-60 u: 3-4 per screen width
                    decay_rate: 0.35,
                    default_humidity: 0.70,
                    default_condensation: 0.20, // essentially no holes; placement bias only
                    water_humidity_boost: 0.12,
                    blob_frequency: 1.0 / 56.0,
                },
                // Layer 2: high broken - sparse fast accents; subtle at 0.8 darkness.
                CloudLayerParams {
                    base_coverage: 0.15,
                    coverage_variation: 0.08,
                    coverage_frequency: 0.030,
                    wind_dir_offset: 1.1,
                    wind_speed_scale: 1.2,      // target ~6 u/s
                    uv_scale: 1.0 / 768.0,      // puffs 18-30 u
                    decay_rate: 0.5,
                    default_humidity: 0.65,
                    default_condensation: 0.18,
                    water_humidity_boost: 0.05,
                    blob_frequency: 1.0 / 40.0,
                },
                // Layer 3: cirrus veil - motion texture at 0.92 darkness; barely
                // a shadow, mostly life in the light.
                CloudLayerParams {
                    base_coverage: 0.20,
                    coverage_variation: 0.10,
                    coverage_frequency: 0.045,
                    wind_dir_offset: 1.8,
                    wind_speed_scale: 1.8,      // target ~9 u/s
                    uv_scale: 1.0 / 1536.0,     // long period for long streaks
                    decay_rate: 0.7,
                    default_humidity: 0.60,
                    default_condensation: 0.15,
                    water_humidity_boost: 0.02,
                    blob_frequency: 1.0 / 32.0,
                },
            ],
        }
    }
}

// ============================================================================
// Water visual parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WaterVisualParams {
    pub water_level: f32,
}

impl Default for WaterVisualParams {
    fn default() -> Self {
        Self { water_level: 30.0 }
    }
}

// ============================================================================
// Post-process parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PostProcessParams {
    pub vignette_strength: f32,
    pub exposure: f32,
    pub overcast_desaturation_factor: f32,
}

impl Default for PostProcessParams {
    fn default() -> Self {
        Self {
            vignette_strength: 0.0,
            exposure: 1.1,
            overcast_desaturation_factor: 0.3,
        }
    }
}

// ============================================================================
// Palette parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PaletteParams {
    pub enabled: bool,
    pub mode: u32,
    pub selected_palette: String,
    pub l_levels: u32,
    pub ab_levels: u32,
    pub l_gamma: f32,
}

impl Default for PaletteParams {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: 2,
            selected_palette: String::new(),
            l_levels: 64,
            ab_levels: 64,
            l_gamma: 0.5,
        }
    }
}

// ============================================================================
// Outline parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OutlineParams {
    pub enabled: bool,
    pub depth_threshold: f32,
    pub depth_strength: f32,
    pub normal_threshold: f32,
    pub normal_strength: f32,
    pub darken_strength: f32,
    pub brighten_strength: f32,
}

impl Default for OutlineParams {
    fn default() -> Self {
        Self {
            enabled: true,
            depth_threshold: 0.0005,
            depth_strength: 1.0,
            normal_threshold: 0.3,
            normal_strength: 0.8,
            darken_strength: 0.4,
            brighten_strength: 0.3,
        }
    }
}

// ============================================================================
// Meshing parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MeshingParams {
    pub edge_strength: f32,
    pub ortho_ao_enabled: bool,
    pub ortho_ao_strength: f32,
    /// Chunk snapshots extracted for meshing per frame. This is a *main-thread*
    /// budget - each submission copies a 34^3 voxel snapshot - and is separate
    /// from how many mesh jobs may run concurrently, which the scheduler owns.
    pub max_mesh_per_frame: u32,
}

impl Default for MeshingParams {
    fn default() -> Self {
        Self {
            edge_strength: 0.15,
            ortho_ao_enabled: true,
            ortho_ao_strength: 0.3,
            max_mesh_per_frame: 32,
        }
    }
}

// ============================================================================
// Render pipeline parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RenderPipelineParams {
    pub world_pixel_density: f32,
    pub camera_snap_enabled: bool,
    pub sky_color: [f32; 3],
}

impl Default for RenderPipelineParams {
    fn default() -> Self {
        Self {
            world_pixel_density: 16.0,
            camera_snap_enabled: true,
            sky_color: [0.5, 0.65, 0.8],
        }
    }
}

// ============================================================================
// Time control parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TimeControlParams {
    pub day_duration_seconds: f32,
    pub speed_multiplier: f32,
    pub paused: bool,
    pub manual_time: f32,
}

impl Default for TimeControlParams {
    fn default() -> Self {
        Self {
            day_duration_seconds: 300.0,
            speed_multiplier: 1.0,
            paused: false,
            manual_time: 0.30,
        }
    }
}

// ============================================================================
// Camera parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CameraParams {
    pub zoom_min: f32,
    pub zoom_max: f32,
    pub scroll_speed: f32,
    pub pan_speed: f32,
    pub initial_zoom: f32,
    /// Exponential smoothing speed (s⁻¹) applied to the render target.
    /// Higher = snappier but more jitter; lower = smoother but more lag.
    pub smooth_speed: f32,
}

impl Default for CameraParams {
    fn default() -> Self {
        Self {
            zoom_min: 1.0,
            zoom_max: 100.0,
            scroll_speed: 2.0,
            pan_speed: 12.0,
            initial_zoom: 40.0,
            smooth_speed: 20.0,
        }
    }
}

// ============================================================================
// Debug parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DebugParams {
    pub show_wireframe: bool,
    pub show_chunk_boundaries: bool,
    pub show_material_ids: bool,
    pub show_ao_only: bool,
    pub show_normals: bool,
    pub show_greedy_debug: bool,
    pub show_water_debug: bool,
    pub show_performance: bool,
    pub freeze_culling: bool,
    pub hide_water: bool,
    pub hide_foliage: bool,
    pub show_canopy_cores: bool,
}

impl Default for DebugParams {
    fn default() -> Self {
        Self {
            show_wireframe: false,
            show_chunk_boundaries: false,
            show_material_ids: false,
            show_ao_only: false,
            show_normals: false,
            show_greedy_debug: false,
            show_water_debug: false,
            show_performance: true,
            freeze_culling: false,
            hide_water: false,
            hide_foliage: false,
            show_canopy_cores: false,
        }
    }
}

// ============================================================================
// Cross-section Params
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CrossSectionParams {
    pub enabled: bool,
    /// Per-axis offset from camera center into the visible volume.
    /// Positive = plane moves inward (clips more). Default 0.0 = no clipping.
    /// Range: 0.0 (disabled) to ~half-world-size
    pub x_offset: f32,  // clips on the +X side of camera
    pub y_offset: f32,  // clips on the +Y (top) side of camera
    pub z_offset: f32,  // clips on the +Z side of camera
    /// Fog applied at clip boundary to show terrain interior
    pub fog_density: f32,    // how quickly fog reaches full opacity (default: 2.0)
    pub fog_color: [f32; 3], // default: [0.4, 0.35, 0.3] earthy fog
    pub show_edges: bool,    // debug: highlight clip plane edge in color
}

impl Default for CrossSectionParams {
    fn default() -> Self {
        Self {
            enabled: false,
            x_offset: 0.0,
            y_offset: 0.0,
            z_offset: 0.0,
            fog_density: 0.5,
            fog_color: [0.235, 0.216, 0.2],
            show_edges: false,
        }
    }
}

// ============================================================================
// Mesh Cache Params
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MeshCacheParams {
    pub max_size_bytes: u64,
    pub eviction_batch_size: usize,
    /// Bypass the mesh cache entirely: no lookup, no write. Read once, when the
    /// meshing pipeline is built, so a change needs a relaunch.
    pub enabled: bool,
}

impl Default for MeshCacheParams {
    fn default() -> Self {
        Self {
            max_size_bytes: 256 * 1024 * 1024,
            eviction_batch_size: 64,
            enabled: true,
        }
    }
}

// ============================================================================
// Top-level EngineParams
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EngineParams {
    pub lighting: LightingParams,
    pub wind: WindParams,
    pub cloud: CloudParams,
    pub water: WaterVisualParams,
    pub post_process: PostProcessParams,
    pub palette: PaletteParams,
    pub time_control: TimeControlParams,
    pub camera: CameraParams,
    pub debug: DebugParams,
    pub meshing: MeshingParams,
    pub render_pipeline: RenderPipelineParams,
    pub outline: OutlineParams,
    pub cross_section: CrossSectionParams,
    pub mesh_cache: MeshCacheParams,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            lighting: LightingParams::default(),
            wind: WindParams::default(),
            cloud: CloudParams::default(),
            water: WaterVisualParams::default(),
            post_process: PostProcessParams::default(),
            palette: PaletteParams::default(),
            time_control: TimeControlParams::default(),
            camera: CameraParams::default(),
            debug: DebugParams::default(),
            meshing: MeshingParams::default(),
            render_pipeline: RenderPipelineParams::default(),
            outline: OutlineParams::default(),
            cross_section: CrossSectionParams::default(),
            mesh_cache: MeshCacheParams::default(),
        }
    }
}

// ============================================================================
// Save / Load
// ============================================================================

impl EngineParams {
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let json = std::fs::read_to_string(path)?;
        let params: Self = serde_json::from_str(&json)?;
        Ok(params)
    }

    pub fn list_presets(dir: &Path) -> Vec<String> {
        let mut presets = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "json") {
                    if let Some(stem) = path.file_stem() {
                        presets.push(stem.to_string_lossy().into_owned());
                    }
                }
            }
        }
        presets.sort();
        presets
    }
}

// ============================================================================
// Change detection
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamChangeKind {
    None,
    UniformOnly,
    /// No parameter needs a remesh today: the material table, its only
    /// producer, left `EngineParams`. Kept for the remesh-on-edit work.
    #[allow(dead_code)]
    MeshInvalidating,
}

pub struct ParamChangeDetector {
    previous: EngineParams,
}

impl ParamChangeDetector {
    pub fn new(initial: &EngineParams) -> Self {
        Self {
            previous: initial.clone(),
        }
    }
    
    pub fn detect(&self, current: &EngineParams) -> ParamChangeKind {
        if (current.render_pipeline.world_pixel_density
            - self.previous.render_pipeline.world_pixel_density).abs() > f32::EPSILON {
            return ParamChangeKind::UniformOnly;
        }
        if current != &self.previous {
            return ParamChangeKind::UniformOnly;
        }
        ParamChangeKind::None
    }
    
    pub fn snapshot(&mut self, current: &EngineParams) {
        self.previous = current.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    /// Presets written before the material table left `EngineParams` carry a
    /// `materials` block. They must still load, with that block ignored: the
    /// registry owns the table now, so a preset must not be able to recolor it.
    #[test]
    fn a_preset_with_a_materials_block_still_loads_and_the_block_is_ignored() {
        let json = r#"{
            "materials": { "entries": [
                { "name": "Air", "color": [0.0, 0.0, 0.0], "sharpness": 0.0,
                  "hardness": 0.0, "permeable": true, "supports_flora": false }
            ] }
        }"#;
        let params: EngineParams = serde_json::from_str(json).expect("an old preset loads");
        assert_eq!(params, EngineParams::default());
    }
    
    /// Presets written while `EngineParams` carried terrain-generation sliders
    /// have a `terrain_gen` block. Those sliders reached no generation code, so
    /// the block went; such a preset must still load, with the block ignored.
    #[test]
    fn a_preset_with_a_terrain_gen_block_still_loads_and_the_block_is_ignored() {
        let json = r#"{ "terrain_gen": { "base_height": 40.0, "cave_enabled": false } }"#;
        let params: EngineParams = serde_json::from_str(json).expect("an old preset loads");
        assert_eq!(params, EngineParams::default());
    }
}
