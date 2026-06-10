//! Stable material registry: numeric [`MaterialId`]s plus snake_case name
//! resolution. Hand-authored for Phase 0; RON/mod-driven loading lands in a
//! later phase (see [`MaterialRegistry::load_from_ron`]).

use std::collections::HashMap;
use std::path::Path;

use crate::error::{VoxelCoreError, VoxelCoreResult};
use crate::material::MaterialId;

/// Authored definition of one material.
#[derive(Clone, Debug)]
pub struct MaterialDef {
    /// Stable snake_case id used by graphs and [`MaterialRegistry::resolve`].
    pub id_name: String,
    /// Human-readable label, for UI only.
    pub display_name: String,
    /// Linear RGB base color.
    pub color: [f32; 3],
    /// Edge-sharpness hint (0..=1).
    pub sharpness: f32,
    /// Mining hardness (0..=1).
    pub hardness: f32,
    /// Whether fluids / roots pass through.
    pub permeable: bool,
    /// Whether flora can root on top.
    pub supports_flora: bool,
}

/// Ordered material table. The index into `entries` is the numeric
/// [`MaterialId`]; ids are stable across runs (Air=0 … Gravel=8).
pub struct MaterialRegistry {
    entries: Vec<MaterialDef>,
    by_id_name: HashMap<String, MaterialId>,
}

impl MaterialRegistry {
    /// Hand-authored Phase 0 registry (moved from the engine's old
    /// `MATERIAL_TABLE`). Panics if two entries share an `id_name`.
    pub fn load_initial() -> Self {
        let entries = vec![
            MaterialDef { id_name: "air".into(), display_name: "Air".into(),
                color: [0.0, 0.0, 0.0], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false },
            MaterialDef { id_name: "limestone".into(), display_name: "Limestone".into(),
                color: [0.95, 0.90, 0.82], sharpness: 0.9, hardness: 0.9, permeable: false, supports_flora: false },
            MaterialDef { id_name: "granite".into(), display_name: "Granite".into(),
                color: [0.50, 0.50, 0.53], sharpness: 0.85, hardness: 0.95, permeable: false, supports_flora: false },
            MaterialDef { id_name: "soil".into(), display_name: "Soil".into(),
                color: [0.40, 0.22, 0.10], sharpness: 0.3, hardness: 0.2, permeable: false, supports_flora: true },
            MaterialDef { id_name: "clay".into(), display_name: "Clay".into(),
                color: [0.62, 0.36, 0.20], sharpness: 0.4, hardness: 0.4, permeable: false, supports_flora: false },
            MaterialDef { id_name: "sand".into(), display_name: "Sand".into(),
                color: [0.90, 0.82, 0.55], sharpness: 0.15, hardness: 0.1, permeable: true, supports_flora: false },
            MaterialDef { id_name: "grass_soil".into(), display_name: "Grass Soil".into(),
                color: [0.30, 0.55, 0.18], sharpness: 0.35, hardness: 0.2, permeable: false, supports_flora: true },
            MaterialDef { id_name: "water".into(), display_name: "Water".into(),
                color: [0.2, 0.35, 0.6], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false },
            MaterialDef { id_name: "gravel".into(), display_name: "Gravel".into(),
                color: [0.52, 0.49, 0.45], sharpness: 0.6, hardness: 0.5, permeable: true, supports_flora: false },
        ];
        Self::from_entries(entries)
    }

    fn from_entries(entries: Vec<MaterialDef>) -> Self {
        let mut by_id_name = HashMap::with_capacity(entries.len());
        for (i, def) in entries.iter().enumerate() {
            let id = MaterialId(i as u16);
            if by_id_name.insert(def.id_name.clone(), id).is_some() {
                panic!("duplicate material id_name: {:?}", def.id_name);
            }
        }
        Self { entries, by_id_name }
    }

    /// Resolve a snake_case `id_name` to its stable numeric id.
    pub fn resolve(&self, id_name: &str) -> Option<MaterialId> {
        self.by_id_name.get(id_name).copied()
    }

    /// Look up the definition for a numeric id.
    pub fn get(&self, id: MaterialId) -> Option<&MaterialDef> {
        self.entries.get(id.0 as usize)
    }

    /// Number of registered materials.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the registry holds no materials.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// RON/mod-driven loading is deferred to a later phase (Phase 5 modding);
    /// future asset path `assets/materials/*.ron`. Always returns
    /// [`VoxelCoreError::RonLoadingDeferredToFuturePhase`] so it can never
    /// silently fall back to a default registry.
    pub fn load_from_ron(_path: &Path) -> VoxelCoreResult<Self> {
        Err(VoxelCoreError::RonLoadingDeferredToFuturePhase)
    }
}
