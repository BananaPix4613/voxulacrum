use serde::{Deserialize, Serialize};
use std::path::Path;

// ============================================================================
// Material parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MaterialEntry {
    pub name: String,
    pub color: [f32; 3],
    pub sharpness: f32,
    pub hardness: f32,
    pub permeable: bool,
    pub supports_flora: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MaterialParams {
    pub entries: Vec<MaterialEntry>,
}

impl MaterialParams {
    #[allow(dead_code)] // material-change detector; used when remesh-on-edit lands
    pub fn sharpness_changed(&self, other: &MaterialParams) -> bool {
        if self.entries.len() != other.entries.len() {
            return true;
        }
        self.entries
            .iter()
            .zip(other.entries.iter())
            .any(|(a, b)| (a.sharpness - b.sharpness).abs() > f32::EPSILON)
    }

    pub fn materials_changed(&self, other: &MaterialParams) -> bool {
        if self.entries.len() != other.entries.len() {
            return true;
        }
        self.entries
            .iter()
            .zip(other.entries.iter())
            .any(|(a, b)| {
            (a.sharpness - b.sharpness).abs() > f32::EPSILON
                || a.color != b.color
            })
    }
}

impl Default for MaterialParams {
    fn default() -> Self {
        Self {
            entries: vec![
                MaterialEntry { name: "Air".into(),        color: [0.0, 0.0, 0.0],     sharpness: 0.0,  hardness: 0.0,  permeable: true,  supports_flora: false },
                MaterialEntry { name: "Limestone".into(),  color: [0.95, 0.90, 0.82],  sharpness: 0.9,  hardness: 0.9,  permeable: false, supports_flora: false },
                MaterialEntry { name: "Granite".into(),    color: [0.50, 0.50, 0.53],  sharpness: 0.85, hardness: 0.95, permeable: false, supports_flora: false },
                MaterialEntry { name: "Soil".into(),       color: [0.40, 0.22, 0.10],  sharpness: 0.3,  hardness: 0.2,  permeable: false, supports_flora: true  },
                MaterialEntry { name: "Clay".into(),       color: [0.62, 0.36, 0.20],  sharpness: 0.4,  hardness: 0.4,  permeable: false, supports_flora: false },
                MaterialEntry { name: "Sand".into(),       color: [0.90, 0.82, 0.55],  sharpness: 0.15, hardness: 0.1,  permeable: true,  supports_flora: false },
                MaterialEntry { name: "Grass Soil".into(), color: [0.30, 0.55, 0.18],  sharpness: 0.35, hardness: 0.2,  permeable: false, supports_flora: true  },
                MaterialEntry { name: "Water".into(),      color: [0.2, 0.35, 0.6],    sharpness: 0.0,  hardness: 0.0,  permeable: true,  supports_flora: false },
                MaterialEntry { name: "Gravel".into(),     color: [0.52, 0.49, 0.45],  sharpness: 0.6,  hardness: 0.5,  permeable: true,  supports_flora: false },
            ],
        }
    }
}

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CloudParams {
    pub base_coverage: f32,
    pub coverage_variation: f32,
    pub coverage_frequency: f32,
    pub scroll_speed: f32,
    pub noise_freq_1: f32,
    pub noise_freq_2: f32,
    pub noise_freq_3: f32,
}

impl Default for CloudParams {
    fn default() -> Self {
        Self {
            base_coverage: 0.45,
            coverage_variation: 0.10,
            coverage_frequency: 0.05,
            scroll_speed: 0.005,
            noise_freq_1: 0.8,
            noise_freq_2: 1.6,
            noise_freq_3: 3.2,
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
            vignette_strength: 0.35,
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
            depth_threshold: 0.005,
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
}

impl Default for MeshingParams {
    fn default() -> Self {
        Self {
            edge_strength: 0.15,
            ortho_ao_enabled: true,
            ortho_ao_strength: 0.3,
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
// Terrain generation parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerrainGenParams {
    pub base_height: f32,
    pub cliff_threshold: f32,
    pub hill_amplitude: f32,
    pub hill_frequency: f32,
    pub ridge_amplitude: f32,
    pub ridge_frequency: f32,
    pub detail_amplitude: f32,
    pub detail_frequency: f32,
    // Cave system
    pub cave_enabled: bool,
    pub cave_spaghetti_freq: f32,      // Frequency for main tunnel noise (default: 0.02)
    pub cave_spaghetti_thickness: f32, // Threshold width - higher = wider tunnels (default: 0.12)
    pub cave_noodle_freq: f32,         // Frequency for thin passages (default: 0.04)
    pub cave_noodle_thickness: f32,    // Threshold for thin passages (default: 0.06)
    pub cave_cheese_freq: f32,         // Frequency for large chambers (default: 0.008)
    pub cave_cheese_threshold: f32,    // Threshold - how much noise must exceed to carve (default: 0.6)
    pub cave_warp_amp: f32,            // Domain warp amplitude for organic shapes (default: 30.0)
    pub cave_surface_margin: f32,      // Depth below surface before caves begin (default: 4.0)
    pub cave_y_squash: f32,            // Y-axis frequency multiplier - <1.0 = horizontal bias (default: 0.5)
    pub water_level: f32,
    pub seed: i32,
}

impl Default for TerrainGenParams {
    fn default() -> Self {
        Self {
            base_height: 32.0,
            cliff_threshold: 2.0,
            hill_amplitude: 20.0,
            hill_frequency: 0.0025,
            ridge_amplitude: 8.0,
            ridge_frequency: 0.01,
            detail_amplitude: 2.0,
            detail_frequency: 0.05,
            cave_enabled: true,
            cave_spaghetti_freq: 0.01,
            cave_spaghetti_thickness: 0.25,
            cave_noodle_freq: 0.015,
            cave_noodle_thickness: 0.2,
            cave_cheese_freq: 0.008,
            cave_cheese_threshold: 0.6,
            cave_warp_amp: 15.0,
            cave_surface_margin: 2.0,
            cave_y_squash: 0.5,
            water_level: 30.0,
            seed: 54321,
        }
    }
}

// ============================================================================
// Debug parameters
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
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
// Streaming Params
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StreamingParams {
    /// Proportional load margin: fraction of visible extent added as buffer.
    /// E.g., 0.15 = 15% buffer on each side beyond the visible view rect.
    /// A minimum of 2 chunks is always enforced regardless of zoom.
    pub load_margin: f32,
    /// Proportional unload margin: fraction of visible extent beyond which
    /// chunks are unloaded. Should be larger than load_margin to prevent
    /// load/unload thrashing (hysteresis).
    pub unload_margin: f32,
    pub min_chunk_y: i32,
    pub max_chunk_y: i32,
    pub max_gen_per_frame: u32,
    pub max_mesh_per_frame: u32,
}

impl Default for StreamingParams {
    fn default() -> Self {
        Self {
            load_margin: 0.40,
            unload_margin: 0.55,
            min_chunk_y: 0,
            max_chunk_y: 4,
            max_gen_per_frame: 64,
            max_mesh_per_frame: 64,
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
    pub materials: MaterialParams,
    pub lighting: LightingParams,
    pub wind: WindParams,
    pub cloud: CloudParams,
    pub water: WaterVisualParams,
    pub post_process: PostProcessParams,
    pub palette: PaletteParams,
    pub time_control: TimeControlParams,
    pub camera: CameraParams,
    pub terrain_gen: TerrainGenParams,
    pub debug: DebugParams,
    pub meshing: MeshingParams,
    pub render_pipeline: RenderPipelineParams,
    pub outline: OutlineParams,
    pub cross_section: CrossSectionParams,
    pub streaming: StreamingParams,
    pub mesh_cache: MeshCacheParams,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            materials: MaterialParams::default(),
            lighting: LightingParams::default(),
            wind: WindParams::default(),
            cloud: CloudParams::default(),
            water: WaterVisualParams::default(),
            post_process: PostProcessParams::default(),
            palette: PaletteParams::default(),
            time_control: TimeControlParams::default(),
            camera: CameraParams::default(),
            terrain_gen: TerrainGenParams::default(),
            debug: DebugParams::default(),
            meshing: MeshingParams::default(),
            render_pipeline: RenderPipelineParams::default(),
            outline: OutlineParams::default(),
            cross_section: CrossSectionParams::default(),
            streaming: StreamingParams::default(),
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
    MeshInvalidating,
    RegenerationRequired,
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
        if current.terrain_gen != self.previous.terrain_gen {
            return ParamChangeKind::RegenerationRequired;
        }
        if current.materials.materials_changed(&self.previous.materials) {
            return ParamChangeKind::MeshInvalidating;
        }
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