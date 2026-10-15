//! Engine-side material registry loading and the shared [`MaterialRegistryRes`]
//! ECS resource.
//!
//! Materials are data-driven: the table is authored in `assets/materials.ron`
//! and loaded once at startup. That file is the only source of the table. A copy
//! of it is also compiled into the binary and used only if the file is missing
//! or invalid at startup, so the engine can never fail to start over materials.

use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use voxel_core::MaterialRegistry;

/// `assets/materials.ron` as it was when this binary was built. Not a second
/// table: the same file, held so a missing or corrupt copy on disk degrades to
/// the shipped data instead of to no data.
const SHIPPED_MATERIALS: &str = include_str!("../../../assets/materials.ron");

/// Path to the authored material table.
pub fn materials_path() -> PathBuf {
    crate::paths::content_root().join("materials.ron")
}

/// The registry built from the copy of `assets/materials.ron` compiled into this
/// binary. The startup fallback, and the fixture for tests that need a registry
/// without touching the filesystem.
pub fn embedded_registry() -> MaterialRegistry {
    MaterialRegistry::from_ron_str(SHIPPED_MATERIALS)
        .expect("the assets/materials.ron compiled into this build is valid")
}

/// Load the shared material registry: the RON file first, the compiled-in copy
/// of it as the fallback.
///
/// Never fails - a missing or invalid RON file is logged at error level and the
/// compiled-in copy is used instead.
pub fn load_registry() -> Arc<MaterialRegistry> {
    let path = materials_path();
    match MaterialRegistry::load_from_ron(&path) {
        Ok(registry) => {
            log::info!("Loaded {} materials from {}", registry.len(), path.display());
            Arc::new(registry)
        }
        Err(e) => {
            log::error!(
                "Failed to load materials from {}: {e}. Using the copy of \
                 assets/materials.ron compiled into this build.",
                path.display()
            );
            Arc::new(embedded_registry())
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

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::MaterialId;
    
    /// The compiled-in copy is what a broken file on disk degrades to, so it
    /// must parse, and it must be the table the file on disk holds.
    #[test]
    fn the_embedded_table_parses_and_matches_the_file() {
        let embedded = embedded_registry();
        let from_disk = MaterialRegistry::load_from_ron(&materials_path())
            .expect("assets/materials.ron loads");
        
        assert_eq!(embedded.len(), from_disk.len());
        for i in 0..embedded.len() {
            let id = MaterialId(i as u16);
            assert_eq!(embedded.get(id), from_disk.get(id), "material {i}");
        }
    }
}
