use bevy_ecs::prelude::Event;

/// Request a full terrain regeneration from current `TerrainGenParams`.
#[derive(Event)]
pub struct RegenerateWorld;

/// Request a remesh of all chunks.
#[derive(Event)]
pub struct RemeshAll;

/// Request the mesh cache be cleared.
#[derive(Event)]
pub struct ClearMeshCache;

/// Request a palette load by name.
#[derive(Event)]
pub struct LoadPaletteRequest {
    pub name: String,
}