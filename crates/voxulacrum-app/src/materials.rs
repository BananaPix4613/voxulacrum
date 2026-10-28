//! Engine-side material registry loading and the shared [`MaterialRegistryRes`]
//! ECS resource.
//!
//! Materials are data-driven: the table is authored in `packages/sandbox/materials.ron`
//! and loaded once at startup. That file is the only source of the table. A copy
//! of it is also compiled into the binary and used only if the file is missing
//! or invalid at startup, so the engine can never fail to start over materials.

use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use voxel_core::MaterialRegistry;

/// `packages/sandbox/materials.ron` as it was when this binary was built. Not a second
/// table: the same file, held so a missing or corrupt copy on disk degrades to
/// the shipped data instead of to no data.
const SHIPPED_MATERIALS: &str = include_str!("../../../packages/sandbox/materials.ron");

/// Path to the authored material table.
pub fn materials_path() -> PathBuf {
    crate::paths::content_root().join("materials.ron")
}

/// The registry built from the copy of `packages/sandbox/materials.ron` compiled into this
/// binary. The startup fallback, and the fixture for tests that need a registry
/// without touching the filesystem.
pub fn embedded_registry() -> MaterialRegistry {
    MaterialRegistry::from_ron_str(SHIPPED_MATERIALS, crate::paths::namespace())
        .expect("the packages/sandbox/materials.ron compiled into this build is valid")
}

/// Load the shared material registry: the RON file first, the compiled-in copy
/// of it as the fallback.
///
/// Never fails - a missing or invalid RON file is logged at error level and the
/// compiled-in copy is used instead.
pub fn load_registry() -> Arc<MaterialRegistry> {
    let path = materials_path();
    match MaterialRegistry::load_from_ron(&path, crate::paths::namespace()) {
        Ok(registry) => {
            log::info!("Loaded {} materials from {}", registry.len(), path.display());
            Arc::new(registry)
        }
        Err(e) => {
            log::error!(
                "Failed to load materials from {}: {e}. Using the copy of \
                 packages/sandbox/materials.ron compiled into this build.",
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
    use crate::rendering::uniforms::{material_color_table, MATERIAL_COLOR_SLOTS};
    
    /// The compiled-in copy is what a broken file on disk degrades to, so it
    /// must parse, and it must be the table the file on disk holds.
    #[test]
    fn the_embedded_table_parses_and_matches_the_file() {
        let embedded = embedded_registry();
        let from_disk = MaterialRegistry::load_from_ron(&materials_path(), crate::paths::namespace())
            .expect("packages/sandbox/materials.ron loads");
        
        assert_eq!(embedded.len(), from_disk.len());
        for i in 0..embedded.len() {
            let id = MaterialId(i as u16);
            assert_eq!(embedded.get(id), from_disk.get(id), "material {i}");
        }
    }

    /// The shipped table, read strictly: no fallback, so a broken file fails here
    /// instead of quietly comparing the built-in copy to itself.
    fn shipped_registry() -> MaterialRegistry {
        MaterialRegistry::load_from_ron(&materials_path(), crate::paths::namespace())
            .expect("packages/sandbox/materials.ron loads")
    }

    /// The table the shader should see for `registry`: each material's color in
    /// its own slot with alpha 1, and magenta in every slot past the end.
    fn expected_table(registry: &MaterialRegistry) -> [[f32; 4]; MATERIAL_COLOR_SLOTS] {
        let mut table = [[1.0, 0.0, 1.0, 1.0]; MATERIAL_COLOR_SLOTS];
        for i in 0..registry.len().min(MATERIAL_COLOR_SLOTS) {
            let color = registry.get(MaterialId(i as u16)).expect("id in range").color;
            table[i] = [color[0], color[1], color[2], 1.0];
        }
        table
    }

    /// The uploaded table is the registry laid out by id: each material's color
    /// in its own slot with alpha 1, and magenta in every slot past the end.
    /// Substep 1 showed the old `params.rs` copy produced `expected_table`
    /// exactly, so this is the proof that no color moved.
    #[test]
    fn the_color_table_is_the_registry_laid_out_by_id() {
        let registry = shipped_registry();
        let table = material_color_table(&registry);

        assert_eq!(table, expected_table(&registry), "slot for slot");
        // The padding is visibly wrong on purpose.
        assert_eq!(table[registry.len()], [1.0, 0.0, 1.0, 1.0], "first slot past the registry");
        assert_eq!(table[MATERIAL_COLOR_SLOTS - 1], [1.0, 0.0, 1.0, 1.0], "last slot");
        // One material by name rather than by position.
        let grass = registry
            .resolve_str("grass_soil", crate::paths::namespace())
            .expect("grass_soil is registered");
        let color = registry.get(grass).expect("grass_soil resolves to a definition").color;
        assert_eq!(table[grass.0 as usize], [color[0], color[1], color[2], 1.0]);
    }
}
