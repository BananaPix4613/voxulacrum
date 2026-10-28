//! The renderer's settings: the groups only rendering reads, which
//! `EngineParams` holds flattened so presets keep their shape, and the values
//! it reads from groups the main side shares with other systems.

use serde::{Deserialize, Serialize};

/// The settings groups only rendering needs. `EngineParams` holds them with
/// `#[serde(flatten)]`, so each group stays a top-level key of a preset.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RenderSettings {
    pub post_process: PostProcessParams,
    pub outline: OutlineParams,
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
// Scene options
// ============================================================================

/// What the renderer reads from settings groups the main side shares with
/// other systems: meshing, the cross-section and the debug views. Filled at
/// Extract each frame.
#[derive(Clone, Copy, Debug)]
pub struct SceneOptions {
    /// Strength of the terrain's face-edge darkening.
    pub edge_strength: f32,
    /// Orthographic ambient occlusion strength; 0 when it is off.
    pub ao_strength: f32,
    /// Whether the cross-section is on, which draws the cap and the clip fog.
    pub cross_section: bool,
    /// Tint the clip plane's edge, a cross-section debug view.
    pub clip_edges: bool,
    pub clip_fog_color: [f32; 3],
    pub clip_fog_density: f32,
    pub wireframe: bool,
    pub chunk_boundaries: bool,
    pub canopy_cores: bool,
    pub hide_foliage: bool,
    pub hide_water: bool,
}
