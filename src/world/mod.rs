pub mod voxel;
pub mod chunk;
pub mod generation;
pub mod regen;

use wgpu::util::DeviceExt;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use bevy_ecs::prelude::Resource;

use chunk::{Chunk, ChunkMesh, ChunkNeighbors, CHUNK_VOLUME};
use generation::{TerrainGenerator, WORLD_CHUNKS_X, WORLD_CHUNKS_Y, WORLD_CHUNKS_Z};
use voxel::{MATERIAL_COUNT, MATERIAL_TABLE};

pub struct World {
    pub chunks: Vec<Chunk>,
    pub chunks_x: usize,
    pub chunks_y: usize,
    pub chunks_z: usize,
    pub generator: TerrainGenerator,
}

use crate::params::TerrainGenParams;

impl World {
    pub fn generate(params: &TerrainGenParams) -> Self {
        let generator = TerrainGenerator::new(params);
        let chunks = generator.generate_world(params);

        Self {
            chunks,
            chunks_x: WORLD_CHUNKS_X,
            chunks_y: WORLD_CHUNKS_Y,
            chunks_z: WORLD_CHUNKS_Z,
            generator,
        }
    }

    /// Create a World from preloaded chunk data (from cache).
    pub fn from_cached_chunks(chunks: Vec<Chunk>, params: &TerrainGenParams) -> Self {
        Self {
            chunks,
            chunks_x: WORLD_CHUNKS_X,
            chunks_y: WORLD_CHUNKS_Y,
            chunks_z: WORLD_CHUNKS_Z,
            generator: TerrainGenerator::new(params),
        }
    }

    /// Upload a completed mesh result to the GPU for a specific chunk.
    pub fn upload_mesh_result(
        &mut self,
        chunk_index: usize,
        vertices: &[crate::rendering::pipelines::TerrainVertex],
        indices: &[u32],
        device: &wgpu::Device,
    ) {
        if vertices.is_empty() || indices.is_empty() {
            self.chunks[chunk_index].mesh = None;
            self.chunks[chunk_index].mesh_dirty = false;
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

        self.chunks[chunk_index].mesh = Some(ChunkMesh {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        });
        self.chunks[chunk_index].mesh_dirty = false;
    }

    pub fn build_neighbors(&self, cx: usize, cy: usize, cz: usize) -> ChunkNeighbors {
        let mut neighbors = ChunkNeighbors::empty();
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 && dz == 0 { continue; }
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    let nz = cz as i32 + dz;
                    if nx >= 0 && (nx as usize) < self.chunks_x
                        && ny >= 0 && (ny as usize) < self.chunks_y
                        && nz >= 0 && (nz as usize) < self.chunks_z
                    {
                        neighbors.set(dx, dy, dz, self.get_chunk(nx as usize, ny as usize, nz as usize));
                    }
                }
            }
        }
        neighbors
    }

    /// Get a chunk by its chunk-space coordinates, or None if out of bounds.
    pub fn get_chunk(&self, cx: usize, cy: usize, cz: usize) -> Option<&Chunk> {
        if cx >= self.chunks_x || cy >= self.chunks_y || cz >= self.chunks_z {
            return None;
        }
        let index = cx + cy * self.chunks_x + cz * self.chunks_x * self.chunks_y;
        self.chunks.get(index)
    }

    /// Get a mutable chunk by its chunk-space coordinates.
    pub fn get_chunk_mut(&mut self, cx: usize, cy: usize, cz: usize) -> Option<&mut Chunk> {
        if cx >= self.chunks_x || cy >= self.chunks_y || cz >= self.chunks_z {
            return None;
        }
        let index = cx + cy * self.chunks_x + cz * self.chunks_x * self.chunks_y;
        self.chunks.get_mut(index)
    }

    /// Print debug statistics about the generated world.
    pub fn print_debug_stats(&self) {
        let mut total_solid: u64 = 0;
        let mut total_air: u64 = 0;
        let mut material_counts = [0u64; MATERIAL_COUNT];
        let mut min_density: i8 = i8::MAX;
        let mut max_density: i8 = i8::MIN;
        let mut flora_count: u64 = 0;

        for chunk in &self.chunks {
            for voxel in chunk.voxels.iter() {
                if voxel.density > 0 {
                    total_solid += 1;
                } else {
                    total_air += 1;
                }
                let mat = voxel.material as usize;
                if mat < MATERIAL_COUNT {
                    material_counts[mat] += 1;
                }
                if voxel.density < min_density {
                    min_density = voxel.density;
                }
                if voxel.density > max_density {
                    max_density = voxel.density;
                }
                if voxel.flora_id != 0 {
                    flora_count += 1;
                }
            }
        }

        let total = total_solid + total_air;
        log::info!("=== World Generation Stats ===");
        log::info!("Chunks: {}", self.chunks.len());
        log::info!(
            "Total voxels: {} ({} solid, {} air)",
            total,
            total_solid,
            total_air
        );
        log::info!(
            "Solid: {:.1}%, Air: {:.1}%",
            total_solid as f64 / total as f64 * 100.0,
            total_air as f64 / total as f64 * 100.0
        );
        log::info!("Density range: {} to {}", min_density, max_density);
        log::info!("Flora voxels: {}", flora_count);
        log::info!("--- Material distribution ---");
        for (id, count) in material_counts.iter().enumerate() {
            if *count > 0 {
                log::info!(
                    "  [{}] {}: {} ({:.1}%)",
                    id,
                    MATERIAL_TABLE[id].name,
                    count,
                    *count as f64 / total as f64 * 100.0
                );
            }
        }
        log::info!(
            "Voxel memory: ~{} MB",
            self.chunks.len() * CHUNK_VOLUME * 12 / (1024 * 1024)
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
    /// Handle to the background regeneration thread, if active.
    regen_handle: Mutex<Option<JoinHandle<Vec<Chunk>>>>,
    /// Shared progress counter: chunks generated so far.
    regen_progress: Arc<AtomicU32>,
    /// Total chunks to generate (for progress bar denominator).
    regen_total: u32,
    /// The terrain params that triggered this regeneration (kep for cache save
    /// and stale-detection after completion).
    regen_params: Option<TerrainGenParams>,
}

impl WorldManager {
    pub fn new() -> Self {
        Self {
            regen_handle: Mutex::new(None),
            regen_progress: Arc::new(AtomicU32::new(0)),
            regen_total: 0,
            regen_params: None,
        }
    }
    
    /// Returns true if a background regeneration is currently in progress.
    pub fn is_regenerating(&self) -> bool {
        self.regen_handle.lock().unwrap().is_some()
    }
    
    /// Returns (chunks_completed, total_chunks) for the progress bar.
    pub fn progress(&self) -> (u32, u32) {
        if self.regen_handle.lock().unwrap().is_some() {
            (self.regen_progress.load(Ordering::Relaxed), self.regen_total)
        } else {
            (0, 0)
        }
    }
    
    /// Start background regeneration. No-op if one is already in progress.
    pub fn start_regeneration(&mut self, params: &crate::params::TerrainGenParams) {
        let handle = self.regen_handle.get_mut().unwrap();
        if handle.is_some() {
            log::warn!("Regeneration already in progress, ignoring request");
            return;
        }
        
        let total = (WORLD_CHUNKS_X * WORLD_CHUNKS_Y * WORLD_CHUNKS_Z) as u32;
        self.regen_total = total;
        self.regen_progress.store(0, Ordering::Relaxed);
        self.regen_params = Some(params.clone());
        
        let params_clone = params.clone();
        let progress = self.regen_progress.clone();
        
        log::info!("Starting background terrain regeneration ({} chunks)", total);
        
        let new_handle = std::thread::Builder::new()
            .name("terrain-regen".to_string())
            .spawn(move || generate_world_background(&params_clone, &progress))
            .expect("Failed to spawn terrain regeneration thread");
        
        *handle = Some(new_handle);
    }
    
    /// Poll for completion. Returns `Some((chunks, params))` when done.
    /// The caller must swap chunks into the world and rebuild dependent passes.
    pub fn poll_regeneration(
        &mut self,
    ) -> Option<(Vec<Chunk>, crate::params::TerrainGenParams)> {
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

/// Generate the world on a background thread with per-chunk progress reporting.
fn generate_world_background(
    params: &crate::params::TerrainGenParams,
    progress: &Arc<AtomicU32>,
) -> Vec<Chunk> {
    use rayon::prelude::*;
    
    let generator = TerrainGenerator::new(params);
    
    // Pre-allocate all chunks with default (empty) voxel data
    let mut chunks: Vec<Chunk> = Vec::with_capacity(
        WORLD_CHUNKS_X * WORLD_CHUNKS_Y * WORLD_CHUNKS_Z,
    );
    for cz in 0..WORLD_CHUNKS_Z {
        for cy in 0..WORLD_CHUNKS_Y {
            for cx in 0..WORLD_CHUNKS_X {
                chunks.push(Chunk::new(glam::IVec3::new(
                    cx as i32, cy as i32, cz as i32,
                )));
            }
        }
    }
    
    // Generate voxel data in parallel
    let progress_ref = progress.clone();
    chunks.par_iter_mut().for_each(|chunk| {
        generator.generate_chunk(chunk, params);
        progress_ref.fetch_add(1, Ordering::Relaxed);
    });
    
    chunks
}