use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bevy_ecs::prelude::Resource;
use glam::IVec3;

use crate::meshing::coordinator::MeshingCoordinator;
use crate::params::{StreamingParams, TerrainGenParams};
use crate::world::chunk::CHUNK_WORLD_SIZE;
use crate::world::generation::TerrainGenerator;
use crate::world::World;

struct GenRequest {
    pos: IVec3,
}

struct GenResult {
    chunk: crate::world::chunk::Chunk,
}

/// Camera state needed for frustum-based chunk loading.
pub struct CameraView {
    pub zoom: f32,
    pub aspect: f32,
    pub rotation: f32,
}

impl CameraView {
    /// Compute the chunk-space half-extents of the visible ground area
    /// for an isometric orthographic camera.
    fn visible_chunk_extents(&self) -> (i32, i32) {
        // Isometric pitch: θ = atan(1/√2), sin(θ) = 1/√3
        let sin_theta = (1.0_f32 / 3.0).sqrt();
        let cos_phi = self.rotation.cos().abs();
        let sin_phi = self.rotation.sin().abs();

        let hw = self.zoom * self.aspect; // half-width in world units
        let hh = self.zoom; // half-height in world units

        // Ground footprint AABB half-extents (parallelogram bounding box)
        let ground_half_x = hw * sin_phi + hh * sin_theta * cos_phi;
        let ground_half_z = hw * cos_phi + hh * sin_theta * sin_phi;

        // Convert to chunks, ceiling + 1 for safety margin
        let cx = (ground_half_x / CHUNK_WORLD_SIZE).ceil() as i32 + 1;
        let cz = (ground_half_z / CHUNK_WORLD_SIZE).ceil() as i32 + 1;

        (cx, cz)
    }
}

/// Result of a streaming tick — lists of chunk positions that changed.
pub struct StreamingTickResult {
    pub inserted: Vec<IVec3>,
    pub unloaded: Vec<IVec3>,
}

#[derive(Resource)]
pub struct ChunkStreamingManager {
    pub params: StreamingParams,
    last_camera_chunk: Option<IVec3>,
    gen_request_tx: mpsc::SyncSender<GenRequest>,
    gen_result_rx: Mutex<Receiver<GenResult>>,
    pending_gen: HashSet<IVec3>,
    _workers: Vec<JoinHandle<()>>,
}

impl ChunkStreamingManager {
    pub fn new(
        params: &StreamingParams,
        terrain_params: &TerrainGenParams,
    ) -> Self {
        let num_workers = 4.min(num_cpus::get().saturating_sub(2).max(1));

        let (gen_tx, gen_rx) = mpsc::sync_channel::<GenRequest>(256);
        let (result_tx, result_rx) = mpsc::channel::<GenResult>();

        let gen_rx = Arc::new(std::sync::Mutex::new(gen_rx));
        let terrain_params = Arc::new(terrain_params.clone());

        let mut workers = Vec::with_capacity(num_workers);
        for i in 0..num_workers {
            let gen_rx = gen_rx.clone();
            let result_tx = result_tx.clone();
            let params = terrain_params.clone();

            let handle = std::thread::Builder::new()
                .name(format!("chunk-gen-{}", i))
                .spawn(move || {
                    let generator = TerrainGenerator::new(&params);
                    loop {
                        let req = {
                            let rx = gen_rx.lock().unwrap();
                            match rx.recv() {
                                Ok(r) => r,
                                Err(_) => return,
                            }
                        };

                        let mut chunk = crate::world::chunk::Chunk::new(req.pos);
                        generator.generate_chunk(&mut chunk, &params);

                        if result_tx.send(GenResult { chunk }).is_err() {
                            return;
                        }
                    }
                })
                .expect("Failed to spawn chunk generation worker");

            workers.push(handle);
        }

        log::info!("ChunkStreamingManager: {} generation workers", num_workers);

        Self {
            params: params.clone(),
            last_camera_chunk: None,
            gen_request_tx: gen_tx,
            gen_result_rx: Mutex::new(result_rx),
            pending_gen: HashSet::new(),
            _workers: workers,
        }
    }

    /// Rebuild workers when terrain params change (new generator needed).
    pub fn rebuild_for_new_params(&mut self, terrain_params: &TerrainGenParams) {
        *self = Self::new(&self.params, terrain_params);
        self.last_camera_chunk = None;
    }

    /// Main tick: load/unload chunks based on camera position and frustum.
    ///
    /// Returns the list of newly inserted and unloaded chunk positions so the
    /// caller can do incremental work (e.g. vegetation per-chunk).
    pub fn tick(
        &mut self,
        world: &mut World,
        meshing: &mut MeshingCoordinator,
        camera_world_pos: [f32; 3],
        camera_view: &CameraView,
        dt: f32,
    ) -> StreamingTickResult {
        let min_y = self.params.min_chunk_y;
        let max_y = self.params.max_chunk_y;

        // Compute frustum-based load/unload ranges (rectangular, not circular)
        let (vis_cx, vis_cz) = camera_view.visible_chunk_extents();
        let load_margin = self.params.load_distance as i32;
        let unload_margin = self.params.unload_distance as i32;
        let load_range_x = vis_cx + load_margin;
        let load_range_z = vis_cz + load_margin;
        let unload_range_x = vis_cx + unload_margin;
        let unload_range_z = vis_cz + unload_margin;

        // Compute camera chunk position (XZ only)
        let cam_cx = (camera_world_pos[0] / CHUNK_WORLD_SIZE).floor() as i32;
        let cam_cz = (camera_world_pos[2] / CHUNK_WORLD_SIZE).floor() as i32;

        let mut result = StreamingTickResult {
            inserted: Vec::new(),
            unloaded: Vec::new(),
        };

        // --- Poll completed chunk generations ---
        let rx = self.gen_result_rx.get_mut().unwrap();
        while let Ok(gen_result) = rx.try_recv() {
            let pos = gen_result.chunk.position;
            self.pending_gen.remove(&pos);

            // Don't insert if chunk is now outside unload range (camera moved)
            let dx = (pos.x - cam_cx).abs();
            let dz = (pos.z - cam_cz).abs();
            if dx > unload_range_x || dz > unload_range_z {
                continue;
            }

            world.insert_chunk(gen_result.chunk);
            result.inserted.push(pos);

            // Mark face-adjacent neighbors for re-meshing (seam fix)
            for &offset in &[
                IVec3::X, IVec3::NEG_X,
                IVec3::Y, IVec3::NEG_Y,
                IVec3::Z, IVec3::NEG_Z,
            ] {
                let neighbor_pos = pos + offset;
                if let Some(neighbor) = world.chunks.get_mut(&neighbor_pos) {
                    neighbor.mesh_dirty = true;
                }
            }
        }

        if !result.inserted.is_empty() {
            meshing.pipeline.submit_all_dirty(world);
        }

        // --- Check for missing chunks within load range (every frame) ---
        {
            let mut to_generate: Vec<(i32, IVec3)> = Vec::new();

            for dz in -load_range_z..=load_range_z {
                for dx in -load_range_x..=load_range_x {
                    let cx = cam_cx + dx;
                    let cz = cam_cz + dz;
                    for cy in min_y..max_y {
                        let pos = IVec3::new(cx, cy, cz);
                        if !world.chunks.contains_key(&pos) && !self.pending_gen.contains(&pos) {
                            // Chebyshev distance for rectangular priority
                            let dist = dx.abs().max(dz.abs());
                            to_generate.push((dist, pos));
                        }
                    }
                }
            }

            to_generate.sort_by_key(|(dist, _)| *dist);

            let max_gen = self.params.max_gen_per_frame as usize;
            for (_, pos) in to_generate.into_iter().take(max_gen) {
                if self.gen_request_tx.try_send(GenRequest { pos }).is_ok() {
                    self.pending_gen.insert(pos);
                }
            }
        }

        // --- Unload distant chunks (only check when camera crosses chunk boundary) ---
        let camera_chunk = IVec3::new(cam_cx, 0, cam_cz);
        let camera_moved = self.last_camera_chunk != Some(camera_chunk);
        if camera_moved {
            self.last_camera_chunk = Some(camera_chunk);

            let to_unload: Vec<IVec3> = world
                .chunks
                .keys()
                .filter(|pos| {
                    let dx = (pos.x - cam_cx).abs();
                    let dz = (pos.z - cam_cz).abs();
                    dx > unload_range_x || dz > unload_range_z
                })
                .copied()
                .collect();

            for pos in to_unload {
                if meshing.pipeline.is_chunk_in_flight(pos) {
                    continue;
                }
                meshing.pipeline.remove_chunk_state(pos);
                world.remove_chunk(pos);
                result.unloaded.push(pos);
            }
        }

        result
    }

    pub fn pending_gen_count(&self) -> usize {
        self.pending_gen.len()
    }
}
