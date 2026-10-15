//! Data-driven scatter prefab registry (Substep 10).
//!
//! Each scatter instance carries a `PrefabId`; the registry maps that id to a
//! [`PrefabDef`] describing the prop's procedural silhouette, color, and base
//! scale. Mirrors the material registry: authored in `assets/prefabs.ron`,
//! RON-primary with a hand-authored fallback so the engine never fails to start
//! over prefabs. This module is pure data - the scatter render pass builds the
//! actual mesh for each [`PrefabShape`] (Substep 10b).
//!
//! The file names each prefab with a bare `id_name`; the loader qualifies it
//! with the namespace the content loads under, as the material registry does.

use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use serde::Deserialize;
use voxel_core::QualifiedName;

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

/// One prefab definition: a stable id, its name, and how the prop looks.
#[derive(Clone, PartialEq, Debug)]
pub struct PrefabDef {
    /// Stable numeric id (contiguous from 0), authoritative over file order.
    pub id: u32,
    /// Identity, qualified with the namespace its content loads under.
    pub name: QualifiedName,
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

    /// Load from a RON file, naming entries in `namespace`. Validates contiguous
    /// ids `0..len` and valid, unique names, and orders by id.
    pub fn load_from_ron(path: &std::path::Path, namespace: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_ron_str(&text, namespace)
    }

    fn from_ron_str(text: &str, namespace: &str) -> Result<Self, String> {
        let file: PrefabFileRon = ron::from_str(text).map_err(|e| e.to_string())?;
        let mut entries = file.prefabs;
        entries.sort_by_key(|p| p.id);
        if entries.is_empty() {
            return Err("prefab registry must have at least one entry".into());
        }
        for (i, p) in entries.iter().enumerate() {
            if p.id as usize != i {
                return Err(format!(
                    "prefab ids must be contiguous from 0; expected {i}, found {}",
                    p.id
                ));
            }
        }
        let mut seen = std::collections::HashSet::with_capacity(entries.len());
        let mut prefabs = Vec::with_capacity(entries.len());
        for e in entries {
            let name = QualifiedName::new(namespace, &e.id_name)
                .map_err(|err| format!("prefab id {}: {err}", e.id))?;
            if !seen.insert(name.clone()) {
                return Err(format!("duplicate prefab id_name: {:?}", e.id_name));
            }
            prefabs.push(PrefabDef { id: e.id, name, shape: e.shape, color: e.color, scale: e.scale });
        }
        Ok(Self { prefabs })
    }

    /// Hand-authored fallback, kept in sync with `assets/prefabs.ron` (locked by a
    /// test), named in `namespace` like a loaded table.
    pub fn load_initial(namespace: &str) -> Self {
        let name = |n: &str| QualifiedName::new(namespace, n).expect("built-in prefab names are valid");
        Self {
            prefabs: vec![
                PrefabDef { id: 0, name: name("grass_tuft"), shape: PrefabShape::GrassTuft, color: [0.30, 0.55, 0.20], scale: 0.65 },
                PrefabDef { id: 1, name: name("bush"),       shape: PrefabShape::Bush,      color: [0.20, 0.42, 0.16], scale: 0.85 },
                PrefabDef { id: 2, name: name("rock"),       shape: PrefabShape::Rock,      color: [0.50, 0.50, 0.53], scale: 0.75 },
                PrefabDef { id: 3, name: name("canopy"),     shape: PrefabShape::Bush,      color: [0.24, 0.46, 0.18], scale: 4.5  },
            ],
        }
    }
}

/// On-disk schema for one prefab in `assets/prefabs.ron`.
#[derive(Deserialize)]
struct PrefabEntryRon {
    id: u32,
    id_name: String,
    shape: PrefabShape,
    color: [f32; 3],
    scale: f32,
}

/// On-disk schema of `assets/prefabs.ron`.
#[derive(Deserialize)]
struct PrefabFileRon {
    prefabs: Vec<PrefabEntryRon>,
}

/// Path to the authored prefab table.
pub fn prefabs_path() -> PathBuf {
    crate::paths::content_root().join("prefabs.ron")
}

/// Load the shared prefab registry: RON-primary, `load_initial` fallback.
///
/// Never fails - a missing or invalid RON file is logged at error level and the
/// hand-authored fallback table is used instead.
pub fn load_prefab_registry() -> Arc<PrefabRegistry> {
    let path = prefabs_path();
    let namespace = crate::paths::namespace();
    match PrefabRegistry::load_from_ron(&path, namespace) {
        Ok(reg) => {
            log::info!("Loaded {} prefabs from {}", reg.len(), path.display());
            Arc::new(reg)
        }
        Err(e) => {
            log::error!(
                "Failed to load prefabs from {}: {e}. Falling back to built-in table.",
                path.display()
            );
            Arc::new(PrefabRegistry::load_initial(namespace))
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
    use voxel_core::ENGINE_NAMESPACE;

    #[test]
    fn parses_ron_and_orders_by_id() {
        let text = r#"(
            prefabs: [
                (id: 2, id_name: "rock", shape: Rock, color: (0.5, 0.5, 0.53), scale: 0.45),
                (id: 0, id_name: "grass_tuft", shape: GrassTuft, color: (0.3, 0.55, 0.2), scale: 0.35),
                (id: 1, id_name: "bush", shape: Bush, color: (0.2, 0.42, 0.16), scale: 0.55),
            ],
        )"#;
        let reg = PrefabRegistry::from_ron_str(text, ENGINE_NAMESPACE).expect("valid RON");
        assert_eq!(reg.len(), 3);
        // Field is private but the test is in-module: ordered by id.
        assert_eq!(reg.prefabs[0].name.to_string(), "voxulacrum:grass_tuft");
        assert_eq!(reg.prefabs[1].shape, PrefabShape::Bush);
        assert_eq!(reg.prefabs[2].shape, PrefabShape::Rock);
    }

    #[test]
    fn non_contiguous_ids_rejected() {
        let text = r#"( prefabs: [
            (id: 0, id_name: "a", shape: Rock, color: (0.0, 0.0, 0.0), scale: 1.0),
            (id: 2, id_name: "b", shape: Rock, color: (0.0, 0.0, 0.0), scale: 1.0),
        ] )"#;
        let err = PrefabRegistry::from_ron_str(text, ENGINE_NAMESPACE).expect_err("must refuse");
        assert!(err.contains("contiguous"), "{err}");
    }

    #[test]
    fn duplicate_names_rejected() {
        let text = r#"( prefabs: [
            (id: 0, id_name: "a", shape: Rock, color: (0.0, 0.0, 0.0), scale: 1.0),
            (id: 1, id_name: "a", shape: Rock, color: (0.0, 0.0, 0.0), scale: 1.0),
        ] )"#;
        let err = PrefabRegistry::from_ron_str(text, ENGINE_NAMESPACE).expect_err("must refuse");
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn a_name_that_is_not_a_name_is_rejected() {
        let text = r#"( prefabs: [
            (id: 0, id_name: "Big Rock", shape: Rock, color: (0.0, 0.0, 0.0), scale: 1.0),
        ] )"#;
        let err = PrefabRegistry::from_ron_str(text, ENGINE_NAMESPACE).expect_err("must refuse");
        assert!(err.contains("Big Rock"), "{err}");
    }

    #[test]
    fn fallback_matches_shipped_ron() {
        // The built-in fallback must match the authored file so the two never drift.
        const RON: &str = include_str!("../../../assets/prefabs.ron");
        let from_file = PrefabRegistry::from_ron_str(RON, ENGINE_NAMESPACE).expect("prefabs.ron valid");
        assert_eq!(
            from_file.prefabs,
            PrefabRegistry::load_initial(ENGINE_NAMESPACE).prefabs,
            "prefabs.ron and load_initial() drifted"
        );
    }
}
