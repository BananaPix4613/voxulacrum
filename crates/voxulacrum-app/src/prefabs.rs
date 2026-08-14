//! Data-driven scatter prefab registry (Substep 10).
//!
//! Each scatter instance carries a `PrefabId`; the registry maps that id to a
//! [`PrefabDef`] describing the prop's procedural silhouette, color, and base
//! scale. Mirrors the material registry: authored in `assets/prefabs.ron`,
//! RON-primary with a hand-authored fallback so the engine never fails to start
//! over prefabs. This module is pure data - the scatter render pass builds the
//! actual mesh for each [`PrefabShape`] (Substep 10b).

use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use serde::Deserialize;

use crate::world::layers::PrefabId;

/// Procedural prop silhouette. The scatter pass turns each into a small mesh.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Deserialize)]
pub enum PrefabShape {
    /// A low, wide faceted stone.
    Rock,
    /// A rounded leafy clump (crossed quads).
    Bush,
    /// A few upright blades (crossed quads).
    GrassTuft,
}

/// One prefab definition: a stable id plus how the prop looks.
#[derive(Clone, PartialEq, Debug, Deserialize)]
pub struct PrefabDef {
    /// Stable numeric id (contiguous from 0), authoritative over file order.
    pub id: u32,
    /// Stable string handle.
    pub id_name: String,
    /// Procedural silhouette.
    pub shape: PrefabShape,
    /// Base color (linear RGB).
    pub color: [f32; 3],
    /// Base world scale.
    pub scale: f32,
}

/// The set of scatter prefabs, ordered so index == `PrefabId`.
#[derive(Clone, Debug)]
pub struct PrefabRegistry {
    prefabs: Vec<PrefabDef>,
}

impl PrefabRegistry {
    /// Number of prefabs.
    pub fn len(&self) -> usize {
        self.prefabs.len()
    }
    
    /// Look up a prefab by id (`None` if out of range).
    pub fn get(&self, id: PrefabId) -> Option<&PrefabDef> {
        self.prefabs.get(id.0 as usize)
    }
    
    /// All prefabs in id order.
    pub fn iter(&self) -> impl Iterator<Item = &PrefabDef> + '_ {
        self.prefabs.iter()
    }

    /// Load from a RON file. Validates contiguous ids `0..len` and orders by id.
    pub fn load_from_ron(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_ron_str(&text)
    }

    fn from_ron_str(text: &str) -> Result<Self, String> {
        let file: PrefabFileRon = ron::from_str(text).map_err(|e| e.to_string())?;
        let mut prefabs = file.prefabs;
        prefabs.sort_by_key(|p| p.id);
        if prefabs.is_empty() {
            return Err("prefab registry must have at least one entry".into());
        }
        for (i, p) in prefabs.iter().enumerate() {
            if p.id as usize != i {
                return Err(format!(
                    "prefab ids must be contiguous from 0; expected {i}, found {}",
                    p.id
                ));
            }
        }
        Ok(Self { prefabs })
    }

    /// Hand-authored fallback, kept in sync with `assets/prefabs.ron` (locked by a test).
    pub fn load_initial() -> Self {
        Self {
            prefabs: vec![
                PrefabDef { id: 0, id_name: "grass_tuft".into(), shape: PrefabShape::GrassTuft, color: [0.30, 0.55, 0.20], scale: 0.65 },
                PrefabDef { id: 1, id_name: "bush".into(),       shape: PrefabShape::Bush,      color: [0.20, 0.42, 0.16], scale: 0.85 },
                PrefabDef { id: 2, id_name: "rock".into(),       shape: PrefabShape::Rock,      color: [0.50, 0.50, 0.53], scale: 0.75 },
                PrefabDef { id: 3, id_name: "canopy".into(),     shape: PrefabShape::Bush,      color: [0.24, 0.46, 0.18], scale: 4.5 },
            ],
        }
    }
}

/// On-disk schema of `assets/prefabs.ron`.
#[derive(Deserialize)]
struct PrefabFileRon {
    prefabs: Vec<PrefabDef>,
}

/// Path to the authored prefab table.
pub fn prefabs_path() -> PathBuf {
    crate::paths::asset_root().join("assets").join("prefabs.ron")
}

/// Load the shared prefab registry: RON-primary, `load_initial` fallback.
///
/// Never fails - a missing or invalid RON file is logged at error level and the
/// hand-authored fallback table is used instead.
pub fn load_prefab_registry() -> Arc<PrefabRegistry> {
    let path = prefabs_path();
    match PrefabRegistry::load_from_ron(&path) {
        Ok(reg) => {
            log::info!("Loaded {} prefabs from {}", reg.len(), path.display());
            Arc::new(reg)
        }
        Err(e) => {
            log::error!(
                "Failed to load prefabs from {}: {e}. Falling back to built-in table.",
                path.display()
            );
            Arc::new(PrefabRegistry::load_initial())
        }
    }
}

/// Shared, immutable graph registry held as an ECS resource.
#[derive(Resource, Clone)]
pub struct PrefabRegistryRes(pub Arc<PrefabRegistry>);

impl std::ops::Deref for PrefabRegistryRes {
    type Target = PrefabRegistry;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ron_and_orders_by_id() {
        let text = r#"(
            prefabs: [
                (id: 2, id_name: "rock", shape: Rock, color: (0.5, 0.5, 0.53), scale: 0.45),
                (id: 0, id_name: "grass_tuft", shape: GrassTuft, color: (0.3, 0.55, 0.2), scale: 0.35),
                (id: 1, id_name: "bush", shape: Bush, color: (0.2, 0.42, 0.16), scale: 0.55),
            ],
        )"#;
        let reg = PrefabRegistry::from_ron_str(text).expect("valid RON");
        assert_eq!(reg.len(), 3);
        // Field is private but the test is in-module: ordered by id.
        assert_eq!(reg.prefabs[0].id_name, "grass_tuft");
        assert_eq!(reg.prefabs[1].shape, PrefabShape::Bush);
        assert_eq!(reg.prefabs[2].shape, PrefabShape::Rock);
    }

    #[test]
    fn non_contiguous_ids_rejected() {
        let text = r#"( prefabs: [
            (id: 0, id_name: "a", shape: Rock, color: (0.0, 0.0, 0.0), scale: 1.0),
            (id: 2, id_name: "b", shape: Rock, color: (0.0, 0.0, 0.0), scale: 1.0),
        ] )"#;
        assert!(PrefabRegistry::from_ron_str(text).is_err());
    }

    #[test]
    fn fallback_matches_shipped_ron() {
        // The built-in fallback must match the authored file so the two never drift.
        const RON: &str = include_str!("../../../assets/prefabs.ron");
        let from_file = PrefabRegistry::from_ron_str(RON).expect("prefabs.ron valid");
        assert_eq!(
            from_file.prefabs,
            PrefabRegistry::load_initial().prefabs,
            "prefabs.ron and load_initial() drifted"
        );
    }
}
