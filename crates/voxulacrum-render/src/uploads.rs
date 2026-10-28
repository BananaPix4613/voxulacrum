//! The per-chunk render data the main side has built and the renderer has not
//! yet taken: one map per kind, keyed by chunk coordinate. A builder queues a
//! chunk's data with the version it was built from, and an unload queues a
//! removal. `chunk_upload_system` drains every map each frame, so nothing here
//! outlives a frame and no chunk's data is held twice. A later write for a chunk
//! replaces and earlier one still queued.

use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;

use voxel_core::ChunkVersion;

use crate::chunk_meshes::TerrainMesh;
use crate::detail_paint_pass::DetailPaintData;
use crate::pipelines::{CapsuleInstanceGpu, LobeInstanceGpu, ScatterInstanceGpu};
use crate::water_pass::WaterSurface;

/// One queued change to a chunk's entry in a pass.
pub enum ChunkUpload<T> {
    /// Replace the entry with `data`, built from the chunk at `version`. `None`
    /// drops it, for a chunk with nothing of this kind to draw.
    Set { version: ChunkVersion, data: Option<T> },
    /// The chunk unloaded.
    Remove,
}

#[derive(Resource, Default)]
pub struct ChunkUploads {
    pub terrain: HashMap<IVec3, ChunkUpload<TerrainMesh>>,
    pub water: HashMap<IVec3, ChunkUpload<WaterSurface>>,
    pub detail_paint: HashMap<IVec3, ChunkUpload<DetailPaintData>>,
    /// One instance list per `PrefabId`.
    pub scatter: HashMap<IVec3, ChunkUpload<Vec<Vec<ScatterInstanceGpu>>>>,
    pub capsules: HashMap<IVec3, ChunkUpload<Vec<CapsuleInstanceGpu>>>,
    pub lobes: HashMap<IVec3, ChunkUpload<Vec<LobeInstanceGpu>>>,
}

impl ChunkUploads {
    /// Queue the removal of everything an unloaded chunk drew.
    pub fn remove(&mut self, pos: IVec3) {
        self.terrain.insert(pos, ChunkUpload::Remove);
        self.water.insert(pos, ChunkUpload::Remove);
        self.detail_paint.insert(pos, ChunkUpload::Remove);
        self.scatter.insert(pos, ChunkUpload::Remove);
        self.capsules.insert(pos, ChunkUpload::Remove);
        self.lobes.insert(pos, ChunkUpload::Remove);
    }
}
