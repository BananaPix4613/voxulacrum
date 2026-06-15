//! Engine-side material registry loading and the shared [`MaterialRegistryRes`]
//! ECS resource.
//!
//! Materials are data-driven: the table is authored in `assets/materials.ron`
//! and loaded once at startup. If the RON file is missing or invalid the engine
//! logs the error and falls back to the built-in
//! [`MaterialRegistry::load_initial`] table, so it can never fail to start over
//! materials.

use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use voxel_core::MaterialRegistry;

/// Path to the authored material table.
pub fn materials_path() -> PathBuf {
    crate::paths::asset_root().join("assets").join("materials.ron")
}

/// Load the shared material registry: RON-primary, `load_initial` fallback.
///
/// Never fails - a missing or invalid RON file is logged at error level and the
/// hand-authored fallback table is used instead.
pub fn load_registry() -> Arc<MaterialRegistry> {
    let path = materials_path();
    match MaterialRegistry::load_from_ron(&path) {
        Ok(registry) => {
            log::info!("Loaded {} materials from {}", registry.len(), path.display());
            Arc::new(registry)
        }
        Err(e) => {
            log::error!(
                "Failed to load materials from {}: {e}. Falling back to built-in table.",
                path.display()
            );
            Arc::new(MaterialRegistry::load_initial())
        }
    }
}

/// Shared, immutable material registry held as an ECS resource.
#[derive(Resource, Clone)]
pub struct MaterialRegistryRes(pub Arc<MaterialRegistry>);

impl std::ops::Deref for MaterialRegistryRes {
    type Target = MaterialRegistry;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
