pub mod voxel;
pub mod chunk;
pub mod generation;
pub mod regen;
pub mod streaming;
pub mod storage;
pub mod persistence;

use std::collections::HashMap;
use wgpu::util::DeviceExt;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use bevy_ecs::prelude::Resource;
use glam::IVec3;

use chunk::{Chunk, ChunkMesh, ChunkNeighbors, VoxelEdit, CHUNK_VOLUME, DELTA_THRESHOLD};
use storage::ChunkStorage;
use generation::TerrainGenerator;
use voxel::{MATERIAL_COUNT, MATERIAL_TABLE};

pub struct World {
    pub chunks: HashMap<IVec3, Chunk>,
    pub generator: TerrainGenerator,
    pub min_chunk_y: i32,
    pub max_chunk_y: i32, // exclusive upper bound
}

use crate::params::TerrainGenParams;
use crate::world::voxel::MAT_AIR;

impl World {
    pub fn generate(params: &TerrainGenParams, min_y: i32, max_y: i32) -> Self {
        let generator = TerrainGenerator::new(params);
        let chunks = generator.generate_world(params, min_y, max_y);

        Self {
            chunks,
            generator,
            min_chunk_y: min_y,
            max_chunk_y: max_y,
        }
    }

    /// Create a World from preloaded chunk data (from cache).
    pub fn from_cached_chunks(
        chunks: HashMap<IVec3, Chunk>,
        params: &TerrainGenParams,
        min_y: i32,
        max_y: i32,
    ) -> Self {
        Self {
            chunks,
            generator: TerrainGenerator::new(params),
            min_chunk_y: min_y,
            max_chunk_y: max_y,
        }
    }

    /// Check whether all 6 face-adjacent neighbor chunks around `pos` are loaded.
    /// The cube mesher only reads face-adjacent voxels for culling, so face
    /// neighbors are sufficient. Neighbors outside [min_chunk_y, max_chunk_y)
    /// are treated as present (implicit air above/below the world).
    pub fn has_face_neighbors(&self, pos: IVec3) -> bool {
        for offset in [IVec3::new(1,0,0), IVec3::new(-1,0,0),
                              IVec3::new(0,1,0), IVec3::new(0,-1,0),
                              IVec3::new(0,0,1), IVec3::new(0,0,-1)] {
            let n = pos + offset;
            if n.y < self.min_chunk_y || n.y >= self.max_chunk_y { continue; }
            if !self.chunks.contains_key(&n) { return false; }
        }
        true
    }

    /// Upload a completed mesh result to the GPU for a specific chunk.
    pub fn upload_mesh_result(
        &mut self,
        chunk_key: IVec3,
        vertices: &[crate::rendering::pipelines::TerrainVertex],
        indices: &[u32],
        device: &wgpu::Device,
        mesh_seq: u64,
    ) {
        let chunk = match self.chunks.get_mut(&chunk_key) {
            Some(c) => c,
            None => return, // Chunk was unloaded while mesh was in flight
        };

        if vertices.is_empty() || indices.is_empty() {
            chunk.mesh = None;
            // Only clear mesh_dirty if the chunk wasn't re-dirtied during meshing
            if chunk.mesh_seq == mesh_seq {
                chunk.mesh_dirty = false;
            }
            return;
        }

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk_vertex_buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk_index_buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        chunk.mesh = Some(ChunkMesh {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        });
        if chunk.mesh_seq == mesh_seq {
            chunk.mesh_dirty = false;
        }
    }

    pub fn build_neighbors(&self, pos: IVec3) -> ChunkNeighbors {
        let mut neighbors = ChunkNeighbors::empty();
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 && dz == 0 { continue; }
                    let n_pos = pos + IVec3::new(dx, dy, dz);
                    neighbors.set(dx, dy, dz, self.chunks.get(&n_pos));
                }
            }
        }
        neighbors
    }

    /// Apply a voxel edit to a chunk, propagating mesh-dirty to border neighbors.
    /// Sets persist_dirty on the edited chunk only.
    pub fn apply_edit(&mut self, chunk_pos: IVec3, edit: VoxelEdit) {
        if let Some(chunk) = self.chunks.get_mut(&chunk_pos) {
            // Apply the edit to storage
            let mut storage = (*chunk.storage).clone();
            storage.set_voxel(edit.index as usize, edit.material_id);
            chunk.storage = Arc::new(storage);

            // Track for persistence - deduplicate by index
            let list = chunk.edit_list.get_or_insert_with(Vec::new);
            if let Some(existing) = list.iter_mut().find(|e| e.index == edit.index) {
                *existing = edit;
            } else {
                list.push(edit);
            }

            // Auto-promote: if over threshold, drop delta list entirely.
            // build_chunk_edits sees edit_list==None + persist_dirty==true -> saves Full.
            if list.len() >= DELTA_THRESHOLD {
                chunk.edit_list = None;
            }

            chunk.persist_dirty = true;
            chunk.mark_mesh_dirty_from_edit();
        }

        // Propagate mesh-dirty to border neighbors
        for offset in chunk::border_dirty_neighbors(edit.index) {
            let neighbor_pos = chunk_pos + offset;
            if let Some(neighbor) = self.chunks.get_mut(&neighbor_pos) {
                neighbor.mark_mesh_dirty_from_edit();
            }
        }
    }

    /// Get a chunk by its chunk-space IVec3 position.
    pub fn get_chunk(&self, pos: IVec3) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    /// Get a mutable chunk by its chunk-space IVec3 position.
    pub fn get_chunk_mut(&mut self, pos: IVec3) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    /// Insert a chunk into the world.
    pub fn insert_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert(chunk.position, chunk);
    }

    /// Remove a chunk. GPU buffers freed on drop.
    pub fn remove_chunk(&mut self, pos: IVec3) -> Option<Chunk> {
        self.chunks.remove(&pos)
    }

    /// Print debug statistics about the generated world.
    pub fn print_debug_stats(&self) {
        let mut total_solid: u64 = 0;
        let mut total_air: u64 = 0;
        let mut material_counts = [0u64; MATERIAL_COUNT];
        let mut uniform_count: usize = 0;
        let mut populated_count: usize = 0;
        let mut total_storage_bytes: usize = 0;

        for chunk in self.chunks.values() {
            if chunk.storage.is_uniform() {
                uniform_count += 1;
            } else {
                populated_count += 1;
            }
            total_storage_bytes += chunk.storage.memory_bytes();

            for idx in 0..CHUNK_VOLUME {
                let m = chunk.storage.material(idx) as usize;
                if chunk.storage.material(idx) != MAT_AIR {
                    total_solid += 1;
                } else {
                    total_air += 1;
                }
                if m < MATERIAL_COUNT {
                    material_counts[m] += 1;
                }
            }
        }

        let total = total_solid + total_air;
        if total == 0 { return; }
        log::info!("=== World Generation Stats ===");
        log::info!("Chunks: {} ({} uniform, {} populated)", self.chunks.len(), uniform_count, populated_count);
        log::info!(
            "Total voxels: {} ({} solid, {} air)",
            total, total_solid, total_air
        );
        log::info!(
            "Solid: {:.1}%, Air: {:.1}%",
            total_solid as f64 / total as f64 * 100.0,
            total_air as f64 / total as f64 * 100.0
        );
        log::info!("--- Material distribution ---");
        for (id, count) in material_counts.iter().enumerate() {
            if *count > 0 {
                log::info!(
                    "  [{}] {}: {} ({:.1}%)",
                    id, MATERIAL_TABLE[id].name, count,
                    *count as f64 / total as f64 * 100.0
                );
            }
        }
        log::info!(
            "Storage memory: ~{} MB",
            total_storage_bytes / (1024 * 1024)
        );
        log::info!("==============================")
    }
}

/// Manages background terrain regeneration.
/// 
/// When `start_regeneration` is called, a background thread generates a fresh
/// `Vec<Chunk>` (with `mesh: None`) using rayon for per-chunk parallelism.
/// The main thread polls `poll_regeneration` each frame: when complete, the
/// caller swaps the new chunks into the active `World`.
#[derive(Resource)]
pub struct WorldManager {
    regen_handle: Mutex<Option<JoinHandle<HashMap<IVec3, Chunk>>>>,
    regen_progress: Arc<AtomicU32>,
    regen_total: u32,
    regen_params: Option<TerrainGenParams>,
    regen_min_y: i32,
    regen_max_y: i32,
}

impl WorldManager {
    pub fn new() -> Self {
        Self {
            regen_handle: Mutex::new(None),
            regen_progress: Arc::new(AtomicU32::new(0)),
            regen_total: 0,
            regen_params: None,
            regen_min_y: 0,
            regen_max_y: 4,
        }
    }

    pub fn is_regenerating(&self) -> bool {
        self.regen_handle.lock().unwrap().is_some()
    }

    pub fn progress(&self) -> (u32, u32) {
        if self.regen_handle.lock().unwrap().is_some() {
            (self.regen_progress.load(Ordering::Relaxed), self.regen_total)
        } else {
            (0, 0)
        }
    }

    /// Start background regeneration for the given chunk set.
    /// `positions` defines which chunks to generate.
    pub fn start_regeneration(
        &mut self,
        params: &TerrainGenParams,
        positions: Vec<IVec3>,
    ) {
        let handle = self.regen_handle.get_mut().unwrap();
        if handle.is_some() {
            log::warn!("Regeneration already in progress, ignoring request");
            return;
        }
        
        let total = positions.len() as u32;
        self.regen_total = total;
        self.regen_progress.store(0, Ordering::Relaxed);
        self.regen_params = Some(params.clone());
        
        let params_clone = params.clone();
        let progress = self.regen_progress.clone();
        
        log::info!("Starting background terrain regeneration ({} chunks)", total);
        
        let new_handle = std::thread::Builder::new()
            .name("terrain-regen".to_string())
            .spawn(move || generate_world_background(&params_clone, &positions, &progress))
            .expect("Failed to spawn terrain regeneration thread");
        
        *handle = Some(new_handle);
    }

    pub fn poll_regeneration(
        &mut self,
    ) -> Option<(HashMap<IVec3, Chunk>, TerrainGenParams)> {
        let handle_opt = self.regen_handle.get_mut().unwrap();
        let handle_ref = handle_opt.as_ref()?;
        
        if !handle_ref.is_finished() {
            return None;
        }
        
        let handle = handle_opt.take().unwrap();
        let params = self.regen_params.take().unwrap();
        
        match handle.join() {
            Ok(chunks) => {
                log::info!(
                    "Background regeneration complete ({} chunks)",
                    chunks.len()
                );
                Some((chunks, params))
            }
            Err(e) => {
                log::error!("Background regeneration thread panicked: {:?}", e);
                None
            }
        }
    }
}

fn generate_world_background(
    params: &TerrainGenParams,
    positions: &[IVec3],
    progress: &Arc<AtomicU32>,
) -> HashMap<IVec3, Chunk> {
    use rayon::prelude::*;
    
    let generator = TerrainGenerator::new(params);
    
    let chunks: Vec<Chunk> = positions
        .par_iter()
        .map(|&pos| {
            let storage = generator.generate_chunk_storage(pos, params);
            let chunk = Chunk::new(pos, std::sync::Arc::new(storage));
            progress.fetch_add(1, Ordering::Relaxed);
            chunk
        })
        .collect();
    
    chunks.into_iter().map(|c| (c.position, c)).collect()
}
