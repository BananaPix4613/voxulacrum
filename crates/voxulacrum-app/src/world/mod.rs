pub mod chunk;
pub mod generation;
pub mod layers;
pub mod overrides;
pub mod tags;
pub mod walkability;
pub mod slab_smoothing;
pub mod regen;
pub mod streaming;
pub mod storage;
pub mod storage_boundary;
pub mod persistence;
pub mod world_generator;

use std::collections::HashMap;
use wgpu::util::DeviceExt;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use bevy_ecs::prelude::Resource;
use glam::IVec3;

use chunk::{ChunkMesh, ChunkNeighbors, LoadedChunk, CHUNK_VOLUME, DELTA_THRESHOLD};
use overrides::ChunkOverrides;
use world_generator::WorldGenerator;

pub struct World {
    pub chunks: HashMap<ChunkCoord, LoadedChunk>,
    pub generator: Arc<WorldGenerator>,
    pub min_chunk_y: i32,
    pub max_chunk_y: i32, // exclusive upper bound
}

use crate::params::TerrainGenParams;
use voxel_core::{ChunkCoord, MaterialId, MaterialRegistry, Voxel};

impl World {
    /// Build the initial world synchronously (blocks boot), fanning the per-chunk
    /// graph evaluation across the shared generation pool. This runs outside the
    /// frame schedule, so it is exempt from the schedule-thread eval guard.
    pub fn generate(
        generator: Arc<WorldGenerator>,
        pool: &rayon::ThreadPool,
        min_y: i32,
        max_y: i32
    ) -> Self {
        use rayon::prelude::*;

        let half_x = generation::WORLD_CHUNKS_X as i32 / 2;
        let half_z = generation::WORLD_CHUNKS_Z as i32 / 2;

        let mut positions = Vec::new();
        for cz in -half_z..half_z {
            for cy in min_y..max_y {
                for cx in -half_x..half_x {
                    positions.push(IVec3::new(cx, cy, cz));
                }
            }
        }

        let chunks: HashMap<ChunkCoord, LoadedChunk> = pool.install(|| {
            positions
                .par_iter()
                .map(|&pos| {
                    let generated = generator.generate_chunk(pos);
                    let mut chunk = LoadedChunk::new(pos, Arc::new(generated.storage));
                    chunk.data.tags = generated.tags;
                    chunk.data.detail_layers = generated.detail_layers;
                    chunk.data.scatter_instances = generated.scatter;
                    (ChunkCoord::from(pos), chunk)
                })
                .collect()
        });
        
        Self {
            chunks,
            generator,
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
            if !self.chunks.contains_key(&ChunkCoord::from(n)) { return false; }
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
        let chunk = match self.chunks.get_mut(&ChunkCoord::from(chunk_key)) {
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

    pub fn build_neighbors(&self, pos: IVec3) -> ChunkNeighbors<'_> {
        let mut neighbors = ChunkNeighbors::empty();
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 && dz == 0 { continue; }
                    let n_pos = pos + IVec3::new(dx, dy, dz);
                    neighbors.set(dx, dy, dz, self.chunks.get(&ChunkCoord::from(n_pos)));
                }
            }
        }
        neighbors
    }

    /// Apply a voxel edit to a chunk, propagating mesh-dirty to border neighbors.
    /// Sets persist_dirty on the edited chunk only.
    #[allow(dead_code)] // voxel-edit toolkit entry point; not yet reachable
    pub fn apply_edit(&mut self, chunk_pos: IVec3, index: u16, voxel: Voxel) {
        if let Some(chunk) = self.chunks.get_mut(&ChunkCoord::from(chunk_pos)) {
            // Apply the edit to storage
            let mut storage = (*chunk.data.voxels).clone();
            storage.set_voxel(index as usize, voxel);
            chunk.data.voxels = Arc::new(storage);

            // Track for persistence. voxel_diffs is a map, so repeated edits to
            // the same cell are last-write-wins for free.
            let ovr = chunk.data.overrides.get_or_insert_with(ChunkOverrides::default);
            ovr.set_voxel(index as usize, voxel);

            // Auto-promote: past the threshold, drop the per-voxel overrides and
            // let build_chunk_edits persist the whole storage as a Full snapshot
            // (overrides == None + persist_dirty == true -> Full).
            if ovr.voxel_override_count() >= DELTA_THRESHOLD {
                chunk.data.overrides = None;
            }

            chunk.persist_dirty = true;
            chunk.mark_mesh_dirty_from_edit();
        }

        // Propagate mesh-dirty to border neighbors
        for offset in chunk::border_dirty_neighbors(index) {
            let neighbor_pos = chunk_pos + offset;
            if let Some(neighbor) = self.chunks.get_mut(&ChunkCoord::from(neighbor_pos)) {
                neighbor.mark_mesh_dirty_from_edit();
            }
        }
    }

    /// Get a chunk by its chunk-space IVec3 position.
    pub fn get_chunk(&self, pos: IVec3) -> Option<&LoadedChunk> {
        self.chunks.get(&ChunkCoord::from(pos))
    }

    /// Get a mutable chunk by its chunk-space IVec3 position.
    pub fn get_chunk_mut(&mut self, pos: IVec3) -> Option<&mut LoadedChunk> {
        self.chunks.get_mut(&ChunkCoord::from(pos))
    }

    /// Insert a chunk into the world.
    pub fn insert_chunk(&mut self, chunk: LoadedChunk) {
        self.chunks.insert(chunk.data.coord, chunk);
    }

    /// Remove a chunk. GPU buffers freed on drop.
    pub fn remove_chunk(&mut self, pos: IVec3) -> Option<LoadedChunk> {
        self.chunks.remove(&ChunkCoord::from(pos))
    }

    /// Print debug statistics about the generated world.
    pub fn print_debug_stats(&self, registry: &MaterialRegistry) {
        let mut total_solid: u64 = 0;
        let mut total_air: u64 = 0;
        let mut material_counts = vec![0u64; registry.len()];
        let mut uniform_count: usize = 0;
        let mut populated_count: usize = 0;
        let mut total_storage_bytes: usize = 0;

        for chunk in self.chunks.values() {
            if chunk.data.voxels.is_uniform() {
                uniform_count += 1;
            } else {
                populated_count += 1;
            }
            total_storage_bytes += chunk.data.voxels.memory_bytes();

            for idx in 0..CHUNK_VOLUME {
                let v = chunk.data.voxels.voxel(idx);
                let m = v.material.0 as usize;
                if v.is_solid() {
                    total_solid += 1;
                } else {
                    total_air += 1;
                }
                if m < material_counts.len() {
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
                let name = registry
                    .get(MaterialId(id as u16))
                    .map(|d| d.display_name.as_str())
                    .unwrap_or("?");
                log::info!(
                    "  [{}] {}: {} ({:.1}%)",
                    id, name, count,
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
/// `start_regeneration` submits one fan-out task to the shared generation pool;
/// it builds a fresh `HashMap<IVec3, Chunk>` (with `mesh: None`) using rayon for
/// per-chunk parallelism and delivers the result over a channel. The main thread
/// polls `poll_regeneration` each frame and, when a result arrives, swaps the new
/// chunks into the active `World`.
#[derive(Resource)]
pub struct WorldManager {
    /// Shared chunk-generation pool (also used by the startup fill and streaming).
    pool: Arc<rayon::ThreadPool>,
    /// Receiver for the in-flight regeneration's finished chunk set. `Some` while
    /// a regen is running, `None` when idle. Wrapped in a `Mutex` because
    /// `mpsc::Receiver` is `Send` but not `Sync`, and `WorldManager` is a bevy_ecs
    /// `Resource` (which requires `Sync`); the `Mutex` supplies the `Sync` bound.
    regen_rx: Mutex<Option<mpsc::Receiver<HashMap<ChunkCoord, LoadedChunk>>>>,
    regen_progress: Arc<AtomicU32>,
    regen_total: u32,
    regen_params: Option<TerrainGenParams>,
    /// Generator the in-flight regen is using; adopted by `World` + streaming
    /// on completion so all three share one `Arc`.
    regen_generator: Option<Arc<WorldGenerator>>,
    // Y-bounds of the in-flight regen; retained for bounded-regen wiring.
    #[allow(dead_code)]
    regen_min_y: i32,
    #[allow(dead_code)]
    regen_max_y: i32,
}

impl WorldManager {
    pub fn new(pool: Arc<rayon::ThreadPool>) -> Self {
        Self {
            pool,
            regen_rx: Mutex::new(None),
            regen_progress: Arc::new(AtomicU32::new(0)),
            regen_total: 0,
            regen_params: None,
            regen_generator: None,
            regen_min_y: 0,
            regen_max_y: 4,
        }
    }

    pub fn is_regenerating(&self) -> bool {
        self.regen_rx.lock().unwrap().is_some()
    }

    pub fn progress(&self) -> (u32, u32) {
        if self.regen_rx.lock().unwrap().is_some() {
            (self.regen_progress.load(Ordering::Relaxed), self.regen_total)
        } else {
            (0, 0)
        }
    }

    /// Start background regeneration for the given chunk set.
    /// `positions` defines which chunks to generate; `generator` is the freshly
    /// built generator the new world will adopt on completion.
    pub fn start_regeneration(
        &mut self,
        params: &TerrainGenParams,
        generator: Arc<WorldGenerator>,
        positions: Vec<IVec3>,
    ) {
        if self.regen_rx.get_mut().unwrap().is_some() {
            log::warn!("Regeneration already in progress, ignoring request");
            return;
        }
        
        let total = positions.len() as u32;
        self.regen_total = total;
        self.regen_progress.store(0, Ordering::Relaxed);
        self.regen_params = Some(params.clone());
        self.regen_generator = Some(generator.clone());
        
        let progress = self.regen_progress.clone();
        let gen_for_pool = generator;
        let (tx, rx) = mpsc::channel();
        *self.regen_rx.get_mut().unwrap() = Some(rx);
        
        log::info!("Starting background terrain regeneration ({} chunks)", total);
        
        // One fan-out task on the shared pool: its internal `par_iter` runs on the
        // same pool. Result is delivered over the channel; the main thread polls.
        self.pool.spawn(move || {
            let chunks = generate_world_background(gen_for_pool, &positions, &progress);
            let _ = tx.send(chunks);
        });
    }

    pub fn poll_regeneration(
        &mut self,
    ) -> Option<(HashMap<ChunkCoord, LoadedChunk>, TerrainGenParams, Arc<WorldGenerator>)> {
        use std::sync::mpsc::TryRecvError;

        // What the channel told us. Computed inside a scope that holds a mutable
        // borrow of `regen_rx`; we resolve to one of these before releasing it so
        // the sibling-field cleanup below isn't fighting the borrow checker.
        enum Outcome {
            Got(HashMap<ChunkCoord, LoadedChunk>),
            Empty,
            Done,
        }

        let outcome = {
            let slot = self.regen_rx.get_mut().unwrap();
            match slot.as_ref() {
                Some(rx) => match rx.try_recv() {
                    Ok(chunks) => {
                        *slot = None;
                        Outcome::Got(chunks)
                    }
                    Err(TryRecvError::Empty) => Outcome::Empty,
                    Err(TryRecvError::Disconnected) => {
                        *slot = None;
                        Outcome::Done
                    }
                },
                None => Outcome::Empty,
            }
        };

        match outcome {
            Outcome::Got(chunks) => {
                let params = self.regen_params.take().unwrap();
                let generator = self.regen_generator.take().unwrap();
                log::info!("Background regeneration complete ({} chunks)", chunks.len());
                Some((chunks, params, generator))
            }
            Outcome::Done => {
                log::error!("Background regeneration task ended without a result");
                self.regen_params = None;
                self.regen_generator = None;
                None
            }
            Outcome::Empty => None,
        }
    }
}

fn generate_world_background(
    generator: Arc<WorldGenerator>,
    positions: &[IVec3],
    progress: &Arc<AtomicU32>,
) -> HashMap<ChunkCoord, LoadedChunk> {
    use rayon::prelude::*;
    
    let chunks: Vec<LoadedChunk> = positions
        .par_iter()
        .map(|&pos| {
            let generated = generator.generate_chunk(pos);
            let mut chunk = LoadedChunk::new(pos, std::sync::Arc::new(generated.storage));
            chunk.data.tags = generated.tags;
            chunk.data.detail_layers = generated.detail_layers;
            chunk.data.scatter_instances = generated.scatter;
            progress.fetch_add(1, Ordering::Relaxed);
            chunk
        })
        .collect();
    
    chunks.into_iter().map(|c| (c.data.coord, c)).collect()
}
