pub mod chunk;
pub mod generation;
pub mod fluid_gen;
pub mod fluid_sim;
pub mod climate;
pub mod layers;
pub mod overrides;
pub mod tags;
pub mod seam;
pub mod slab_smoothing;
pub mod regen;
pub mod streaming;
pub mod storage;
pub mod storage_boundary;
pub mod persistence;
pub mod world_generator;
pub mod mutation;
pub mod room;

use std::collections::HashMap;
use wgpu::util::DeviceExt;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use bevy_ecs::prelude::Resource;
use glam::IVec3;

use chunk::{ChunkMesh, ChunkNeighbors, LoadedChunk, CHUNK_SIZE, CHUNK_VOLUME, DELTA_THRESHOLD};
use overrides::ChunkOverrides;
use layers::{PrefabId, ScatterFlags, ScatterInstance, StableInstanceId};
use world_generator::WorldGenerator;
use mutation::{
    EngineMode, MutationCommand, MutationError, MutationOutcome, WorldMutation,
};

pub struct World {
    pub chunks: HashMap<ChunkCoord, LoadedChunk>,
    pub generator: Arc<WorldGenerator>,
    pub min_chunk_y: i32,
    pub max_chunk_y: i32, // exclusive upper bound
    /// The engine's current world-mutation mode (design doc §9). Phase 8 is
    /// always `Authoring`; every mutation flows through [`World::execute`] and is
    /// origin-checked against this. `Play` lands in Phase 9.
    pub mode: EngineMode,
}

use crate::params::TerrainGenParams;
use voxel_core::{ChunkCoord, LocalPos, MaterialId, MaterialRegistry, Voxel};
use crate::world::mutation::MutationOrigin;

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
                    (ChunkCoord::from(pos), LoadedChunk::from_generated(pos, generated))
                })
                .collect()
        });
        
        Self {
            chunks,
            generator,
            min_chunk_y: min_y,
            max_chunk_y: max_y,
            mode: EngineMode::Authoring,
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
        vertices: &[crate::rendering::pipelines::FaceVertex],
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

    /// The single entry point for every world-state mutation (design doc §9).
    /// Validates the command's origin against the current [`EngineMode`], then
    /// dispatches its intent to the matching handler. Handlers own persistence
    /// marking, mesh-dirty flagging, override-bucket creation, and invalidation;
    /// the caller constructs a [`MutationCommand`] and services the returned
    /// [`MutationOutcome`]'s GPU-side rebuilds. A command whose origin does not
    /// match the current mode is rejected without touching the world.
    #[allow(dead_code)] // reachable once live call sites migrate (Substeps 1b/1c)
    pub fn execute(
        &mut self,
        command: MutationCommand,
    ) -> Result<MutationOutcome, MutationError> {
        // Actor edits (Authoring/PlayTime) are gated against coexistence; System
        // mutations 9streaming, sim bookkeeping) run in any mode.
        if command.origin != MutationOrigin::System
            && self.mode.required_origin() != command.origin
        {
            return Err(MutationError::WrongMode {
                mode: self.mode,
                origin: command.origin,
            });
        }
        let outcome = match command.mutation {
            WorldMutation::EditVoxel { chunk, index, voxel } => {
                self.handle_edit_voxel(chunk, index, voxel)
            }
            WorldMutation::EditVoxelBatch { chunk, edits } => {
                self.handle_edit_voxel_batch(chunk, edits)
            }
            WorldMutation::RemoveScatter { chunk, anchor } => {
                self.handle_remove_scatter(chunk, anchor)
            }
            WorldMutation::PlaceScatter { chunk, anchor, world_voxel } => {
                self.handle_place_scatter(chunk, anchor, world_voxel)
            }
            WorldMutation::PourFluidColumn { anchor } => self.handle_pour_fluid_column(anchor),
            WorldMutation::MarkFluidDirty { chunks } => self.handle_mark_fluid_dirty(chunks),
            WorldMutation::InsertLoadedChunk { chunk } => self.handle_insert_loaded_chunk(chunk),
            WorldMutation::SwapRegeneratedChunks { chunks, generator } => {
                self.handle_swap_regenerated_chunks(chunks, generator)
            }
        };
        Ok(outcome)
    }

    /// Handler for [`WorldMutation::EditVoxel`]. A convenience over the batched
    /// path: constructs a batch of one and delegates, so the single- and batched-
    /// edit forms share one implementation (and one storage clone).
    fn handle_edit_voxel(&mut self, chunk_pos: IVec3, index: u16, voxel: Voxel) -> MutationOutcome {
        self.handle_edit_voxel_batch(chunk_pos, vec![(LocalPos::from_index(index as usize), voxel)])
    }

    /// Handler for [`WorldMutation::EditVoxelBatch`]. Clones the chunk's storage
    /// exactly once for the whole batch (drift-review 3.1), applies every edit,
    /// records them in the override bucket with a single delta->full promotion
    /// check, and marks persist + mesh dirty once — plus each border neighbor whose
    /// face a batch edit touches, deduplicated. A missing target chunk skips the
    /// storage work but still marks resident border neighbors (matching the prior
    /// single-edit path).
    fn handle_edit_voxel_batch(
        &mut self,
        chunk_pos: IVec3,
        edits: Vec<(LocalPos, Voxel)>,
    ) -> MutationOutcome {
        let mut outcome = MutationOutcome::default();
        if edits.is_empty() {
            return outcome;
        }

        if let Some(chunk) = self.chunks.get_mut(&ChunkCoord::from(chunk_pos)) {
            // One storage clone for the whole batch.
            let mut storage = (*chunk.data.voxels).clone();
            let ovr = chunk.data.overrides.get_or_insert_with(ChunkOverrides::default);
            for &(pos, voxel) in &edits {
                let index = pos.to_index();
                storage.set_voxel(index, voxel);
                ovr.set_voxel(index, voxel);
            }
            chunk.data.voxels = Arc::new(storage);

            // Promotion decided once, after every edit has landed.
            if ovr.voxel_override_count() >= DELTA_THRESHOLD {
                chunk.data.overrides = None;
            }

            chunk.persist_dirty = true;
            chunk.mark_mesh_dirty_from_edit();
            outcome.mesh_invalidated.push(chunk_pos);
        }

        // Border neighbors whose mesh must re-cull, unioned across the batch (each
        // marked at most once), regardless of whether the target chunk is resident.
        let mut border_neighbors: Vec<IVec3> = Vec::new();
        for &(pos, _) in &edits {
            for offset in chunk::border_dirty_neighbors(pos.to_index() as u16) {
                if !border_neighbors.contains(&offset) {
                    border_neighbors.push(offset);
                }
            }
        }
        for offset in border_neighbors {
            let neighbor_pos = chunk_pos + offset;
            if let Some(neighbor) = self.chunks.get_mut(&ChunkCoord::from(neighbor_pos)) {
                neighbor.mark_mesh_dirty_from_edit();
                outcome.mesh_invalidated.push(neighbor_pos);
            }
        }

        outcome
    }

    /// Handler for [`WorldMutation::RemoveScatter`]. Tombstones the generated
    /// instances at `anchor` and drops player-added ones; marks persist-dirty and
    /// reports the chunk for a scatter-buffer rebuild when anything changed.
    fn handle_remove_scatter(&mut self, chunk_pos: IVec3, anchor: LocalPos) -> MutationOutcome {
        let mut outcome = MutationOutcome::default();
        if let Some(chunk) = self.chunks.get_mut(&ChunkCoord::from(chunk_pos)) {
            if remove_scatter_at(chunk, anchor) {
                chunk.persist_dirty = true;
                outcome.scatter_rebuild.push(chunk_pos);
            }
        }
        outcome
    }

    /// Handler for [`WorldMutation::PlaceScatter`]. Appends one player instance at
    /// `anchor` (id derived from `world_voxel`), marks persist_dirty, and reports
    /// the chunk for a scatter-buffer rebuild.
    fn handle_place_scatter(
        &mut self,
        chunk_pos: IVec3,
        anchor: LocalPos,
        world_voxel: IVec3,
    ) -> MutationOutcome {
        let mut outcome = MutationOutcome::default();
        if let Some(chunk) = self.chunks.get_mut(&ChunkCoord::from(chunk_pos)) {
            if place_scatter_at(chunk, anchor, world_voxel) {
                chunk.persist_dirty = true;
                outcome.scatter_rebuild.push(chunk_pos);
            }
        }
        outcome
    }

    /// Handler for [`WorldMutation::PourFluidColumn`]. Inserts a short settled
    /// water column above `anchor` (skipping solid cells), writes each cell as a
    /// player fluid override, activates it for the next tick, marks persist-dirty,
    /// and reports each touched chunk for a water-mesh rebuild.
    fn handle_pour_fluid_column(&mut self, anchor: IVec3) -> MutationOutcome {
        use crate::world::layers::{FluidCell, FluidId};
        let mut outcome = MutationOutcome::default();
        let dim = CHUNK_SIZE as i32;
        for dy in 1..=8 {
            let p = anchor + IVec3::new(0, dy, 0);
            let chunk_pos =
                IVec3::new(p.x.div_euclid(dim), p.y.div_euclid(dim), p.z.div_euclid(dim));
            let Some(chunk) = self.chunks.get_mut(&ChunkCoord::from(chunk_pos)) else { continue };
            let lp = LocalPos::new_unchecked(
                p.x.rem_euclid(dim) as u8,
                p.y.rem_euclid(dim) as u8,
                p.z.rem_euclid(dim) as u8,
            );
            if chunk.data.voxels.voxel(lp.to_index()).is_solid() {
                continue;
            }
            let cell = FluidCell {
                fluid_id: FluidId::WATER,
                mass: crate::world::fluid_gen::FULL_MASS,
                flags: 0,
            };
            chunk.data.fluids.cells.insert(lp, cell);
            chunk.data.fluids.activate(lp);
            // Persist the pour (a player edit) so it survives save/reload; it
            // re-applies as settled and re-flows on load.
            chunk
                .data
                .overrides
                .get_or_insert_with(ChunkOverrides::default)
                .fluid_diffs
                .insert(lp, cell);
            chunk.persist_dirty = true;
            if !outcome.water_rebuild.contains(&chunk_pos) {
                outcome.water_rebuild.push(chunk_pos);
            }
        }
        outcome
    }

    /// Handler for [`WorldMutation::MarkFluidDirty`]. Marks each fluid-changed
    /// chunk persist-dirty and ensures it carries an override bucket, which routes
    /// it through the delta (not full-storage) save path so `build_chunk_edits`
    /// snapshots the fluid field. The sim mutates cells directly (physics, outside
    /// the command path); this centralizes the resulting persistence bookkeeping.
    fn handle_mark_fluid_dirty(&mut self, chunks: Vec<IVec3>) -> MutationOutcome {
        for pos in chunks {
            if let Some(c) = self.chunks.get_mut(&ChunkCoord::from(pos)) {
                c.persist_dirty = true;
                c.data
                    .overrides
                    .get_or_insert_with(ChunkOverrides::default);
            }
        }
        MutationOutcome::default()
    }

    /// Handler for [`WorldMutation::InsertLoadedChunk`]. Inserts the chunk and
    /// marks its six face-adjacent neighbors mesh-dirty so their boundary faces
    /// re-cull against the newly resident chunk. The inserted chunk is already
    /// mesh-dirty from construction, so it is not re-marked here.
    fn handle_insert_loaded_chunk(&mut self, chunk: LoadedChunk) -> MutationOutcome {
        let mut outcome = MutationOutcome::default();
        let pos: IVec3 = chunk.data.coord.into();
        self.insert_chunk(chunk);
        for offset in [
            IVec3::new(1, 0, 0),
            IVec3::new(-1, 0, 0),
            IVec3::new(0, 1, 0),
            IVec3::new(0, -1, 0),
            IVec3::new(0, 0, 1),
            IVec3::new(0, 0, -1),
        ] {
            let npos = pos + offset;
            if let Some(neighbor) = self.get_chunk_mut(npos) {
                neighbor.mark_mesh_dirty();
                outcome.mesh_invalidated.push(npos);
            }
        }
        outcome
    }

    /// Handler for [`WorldMutation::SwapRegeneratedChunks`]. Merges the
    /// regenerated chunk set over the resident set (retaining chunks the edit did
    /// not touch; a full regen overwrites all of them), adopts the new generator,
    /// and marks every resident chunk mesh-dirty. The caller resets the mesh
    /// pipeline and resubmits (`submit_all_dirty`) afterward.
    fn handle_swap_regenerated_chunks(
        &mut self,
        chunks: HashMap<ChunkCoord, LoadedChunk>,
        generator: Arc<WorldGenerator>,
    ) -> MutationOutcome {
        self.chunks.extend(chunks);
        self.generator = generator;
        for chunk in self.chunks.values_mut() {
            chunk.mesh_dirty = true;
        }
        MutationOutcome::default()
    }

    /// Apply a single voxel edit. Thin wrapper over the mutation API's
    /// [`WorldMutation::EditVoxel`] intent, retained for existing/test callers;
    /// new code constructs a [`MutationCommand`] and calls [`World::execute`].
    #[allow(dead_code)] // voxel-edit toolkit entry point; not yet reachable
    pub fn apply_edit(&mut self, chunk_pos: IVec3, index: u16, voxel: Voxel) {
        let _ = self.execute(MutationCommand::authoring(WorldMutation::EditVoxel {
            chunk: chunk_pos,
            index,
            voxel,
        }));
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
            let chunk = LoadedChunk::from_generated(pos, generated);
            progress.fetch_add(1, Ordering::Relaxed);
            chunk
        })
        .collect();
    
    chunks.into_iter().map(|c| (c.data.coord, c)).collect()
}

/// Remove every effective scatter instance anchored at `anchor`: generated ones
/// are recorded in `scatter_removed`, player-added ones are dropped.
fn remove_scatter_at(chunk: &mut LoadedChunk, anchor: LocalPos) -> bool {
    let generated_ids: Vec<StableInstanceId> = chunk
        .data
        .scatter_instances
        .by_type
        .values()
        .flatten()
        .filter(|si| si.anchor == anchor)
        .map(|si| si.stable_id)
        .collect();

    let overrides = chunk.data.overrides.get_or_insert_with(ChunkOverrides::default);
    let mut changed = false;
    for id in generated_ids {
        if overrides.scatter_removed.insert(id) {
            changed = true;
        }
    }
    let before = overrides.scatter_added.len();
    overrides.scatter_added.retain(|si| si.anchor != anchor);
    if overrides.scatter_added.len() != before {
        changed = true;
    }
    changed
}

/// Place one player scatter instance at `anchor`, jittered within the cell.
fn place_scatter_at(chunk: &mut LoadedChunk, anchor: LocalPos, world_voxel: IVec3) -> bool {
    let seq = effective_count_at(chunk, anchor);
    let h = player_stable_id(world_voxel, seq);
    let overrides = chunk.data.overrides.get_or_insert_with(ChunkOverrides::default);
    overrides.scatter_added.push(ScatterInstance {
        anchor,
        sub_offset: [(h & 0xFF) as u8 as i8, 0, ((h >> 8) & 0xFF) as u8 as i8],
        rotation_y: ((h >> 16) & 0xFF) as u8,
        scale_variant: 0,
        prefab_id: PrefabId(0),
        flags: ScatterFlags(ScatterFlags::PLAYER_PLACED),
        stable_id: StableInstanceId(h),
    });
    true
}

/// Count affective instances at `anchor` (generated-minus-removed + added).
fn effective_count_at(chunk: &LoadedChunk, anchor: LocalPos) -> u32 {
    let ovr = chunk.data.overrides.as_ref();
    let gen = chunk
        .data
        .scatter_instances
        .by_type
        .values()
        .flatten()
        .filter(|si| {
            si.anchor == anchor
                && !ovr.map_or(false, |o| o.scatter_removed.contains(&si.stable_id))
        })
        .count();
    let added = ovr.map_or(0, |o| {
        o.scatter_added.iter().filter(|si| si.anchor == anchor).count()
    });
    (gen + added) as u32
}

/// Stable id for a player-placed instance. Salted so it can't collide with a
/// generated id (which derives from the world seed), and varied by `seq` so
/// repeated placements on one anchor get distinct ids.
fn player_stable_id(v: IVec3, seq: u32) -> u64 {
    let mut h: u64 = 0xA11C_E1A5_0FF1_CE11; // player-placed salt
    h = mix64(h ^ (v.x as i64 as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
    h = mix64(h ^ (v.y as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    h = mix64(h ^ (v.z as i64 as u64).wrapping_mul(0xABC9_8388_FB8F_AC03));
    h = mix64(h ^ (seq as u64).wrapping_mul(0xC4CE_B9FE_1A85_EC53));
    h
}

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    x ^= x >> 33;
    x
}

#[cfg(test)]
mod mutation_tests {
    use super::*;
    use crate::world::mutation::{EngineMode, MutationCommand, MutationError, MutationOrigin, WorldMutation};
    use glam::IVec3;
    use voxel_core::{ChunkCoord, MaterialId, Voxel};

    /// A minimal world holding a single air chunk at the origin, in `mode`.
    fn world_with_air_chunk(mode: EngineMode) -> World {
        let generator = crate::world::world_generator::load_default(
            &crate::params::TerrainGenParams::default(),
        )
        .expect("default generator loads");
        let mut chunks = HashMap::new();
        chunks.insert(ChunkCoord::from(IVec3::ZERO), LoadedChunk::new_air(IVec3::ZERO));
        World { chunks, generator, min_chunk_y: 0, max_chunk_y: 4, mode }
    }

    #[test]
    fn authoring_command_applies_voxel_edit() {
        let mut world = world_with_air_chunk(EngineMode::Authoring);
        let voxel = Voxel::cube(MaterialId(3));
        let outcome = world
            .execute(MutationCommand::authoring(WorldMutation::EditVoxel {
                chunk: IVec3::ZERO,
                index: 5,
                voxel,
            }))
            .expect("authoring edit accepted in authoring mode");

        let chunk = world.get_chunk(IVec3::ZERO).unwrap();
        assert_eq!(chunk.data.voxels.voxel(5), voxel, "voxel written");
        assert!(chunk.persist_dirty, "edit marks persist dirty");
        assert!(chunk.mesh_dirty, "edit marks mesh dirty");
        assert!(
            outcome.mesh_invalidated.contains(&IVec3::ZERO),
            "outcome reports the edited chunk mesh-invalidated"
        );
    }

    #[test]
    fn play_time_command_rejected_in_authoring_mode() {
        let mut world = world_with_air_chunk(EngineMode::Authoring);
        let before = world.get_chunk(IVec3::ZERO).unwrap().data.voxels.voxel(5);

        let err = world
            .execute(MutationCommand::play(WorldMutation::EditVoxel {
                chunk: IVec3::ZERO,
                index: 5,
                voxel: Voxel::cube(MaterialId(3)),
            }))
            .expect_err("play-time command rejected in authoring mode");
        assert_eq!(
            err,
            MutationError::WrongMode {
                mode: EngineMode::Authoring,
                origin: MutationOrigin::PlayTime,
            }
        );

        // Rejection must leave the world untouched.
        let chunk = world.get_chunk(IVec3::ZERO).unwrap();
        assert_eq!(chunk.data.voxels.voxel(5), before, "rejected edit did not apply");
        assert!(!chunk.persist_dirty, "rejected edit did not mark persist dirty");
    }

    #[test]
    fn authoring_command_rejected_in_play_mode() {
        // Symmetric enforcement: an authoring command in Play mode is also rejected
        // (this is the machinery Phase 9 will lean on; no live Play mode yet).
        let mut world = world_with_air_chunk(EngineMode::Play);
        let err = world
            .execute(MutationCommand::authoring(WorldMutation::EditVoxel {
                chunk: IVec3::ZERO,
                index: 5,
                voxel: Voxel::cube(MaterialId(3)),
            }))
            .expect_err("authoring command rejected in play mode");
        assert_eq!(
            err,
            MutationError::WrongMode {
                mode: EngineMode::Play,
                origin: MutationOrigin::Authoring,
            }
        );
    }

    #[test]
    fn place_scatter_marks_persist_and_rebuild() {
        let mut world = world_with_air_chunk(EngineMode::Authoring);
        let anchor = LocalPos::new_unchecked(5, 0, 5);
        let outcome = world
            .execute(MutationCommand::authoring(WorldMutation::PlaceScatter {
                chunk: IVec3::ZERO,
                anchor,
                world_voxel: IVec3::new(5, 0, 5),
            }))
            .expect("authoring place accepted");
        assert!(outcome.scatter_rebuild.contains(&IVec3::ZERO));
        let chunk = world.get_chunk(IVec3::ZERO).unwrap();
        assert!(chunk.persist_dirty);
        let ovr = chunk.data.overrides.as_ref().expect("bucket created");
        assert_eq!(ovr.scatter_added.len(), 1);
        assert_eq!(ovr.scatter_added[0].anchor, anchor);
    }

    #[test]
    fn pour_fluid_column_writes_overrides_and_activates() {
        use crate::world::layers::FluidId;
        let mut world = world_with_air_chunk(EngineMode::Authoring);
        let outcome = world
            .execute(MutationCommand::authoring(WorldMutation::PourFluidColumn {
                anchor: IVec3::new(5, 0, 5),
            }))
            .expect("authoring pour accepted");
        assert!(outcome.water_rebuild.contains(&IVec3::ZERO));
        let chunk = world.get_chunk(IVec3::ZERO).unwrap();
        assert!(chunk.persist_dirty);
        // 8 cells poured above the anchor (all air), each an override + active.
        assert_eq!(chunk.data.fluids.cells.len(), 8);
        assert_eq!(chunk.data.fluids.active.len(), 8);
        let ovr = chunk.data.overrides.as_ref().expect("bucket created");
        assert_eq!(ovr.fluid_diffs.len(), 8);
        let top = LocalPos::new_unchecked(5, 1, 5);
        assert_eq!(ovr.fluid_diffs[&top].fluid_id, FluidId::WATER);

        // Deterministic: a fresh world poured identically matches.
        let mut world2 = world_with_air_chunk(EngineMode::Authoring);
        world2
            .execute(MutationCommand::authoring(WorldMutation::PourFluidColumn {
                anchor: IVec3::new(5, 0, 5),
            }))
            .unwrap();
        let c2 = world2.get_chunk(IVec3::ZERO).unwrap();
        assert_eq!(chunk.data.fluids.cells, c2.data.fluids.cells);
    }

    #[test]
    fn mark_fluid_dirty_sets_persist_and_bucket() {
        let mut world = world_with_air_chunk(EngineMode::Authoring);
        world
            .execute(MutationCommand::authoring(WorldMutation::MarkFluidDirty {
                chunks: vec![IVec3::ZERO],
            }))
            .expect("authoring mark accepted");
        let chunk = world.get_chunk(IVec3::ZERO).unwrap();
        assert!(chunk.persist_dirty);
        assert!(chunk.data.overrides.is_some(), "bucket ensured for snapshot");
    }

    #[test]
    fn insert_loaded_chunk_marks_face_neighbors() {
        let mut world = world_with_air_chunk(EngineMode::Authoring);
        // Clear the origin chunk's generation-dirty flag so the mark is observable.
        world.get_chunk_mut(IVec3::ZERO).unwrap().mesh_dirty = false;
        let seq_before = world.get_chunk(IVec3::ZERO).unwrap().mesh_seq;

        let new_chunk = LoadedChunk::new_air(IVec3::new(1, 0, 0));
        let outcome = world
            .execute(MutationCommand::authoring(WorldMutation::InsertLoadedChunk {
                chunk: new_chunk,
            }))
            .expect("authoring insert accepted");

        assert!(world.get_chunk(IVec3::new(1, 0, 0)).is_some(), "chunk inserted");
        let origin = world.get_chunk(IVec3::ZERO).unwrap();
        assert!(origin.mesh_dirty, "face neighbor marked mesh-dirty");
        assert_eq!(origin.mesh_seq, seq_before + 1, "neighbor mesh_seq bumped");
        assert!(outcome.mesh_invalidated.contains(&IVec3::ZERO));
    }

    #[test]
    fn swap_regenerated_chunks_extends_and_marks_all_dirty() {
        let mut world = world_with_air_chunk(EngineMode::Authoring);
        world.get_chunk_mut(IVec3::ZERO).unwrap().mesh_dirty = false;
        let gen = world.generator.clone();

        let mut new_chunks = HashMap::new();
        new_chunks.insert(
            ChunkCoord::from(IVec3::new(2, 0, 0)),
            LoadedChunk::new_air(IVec3::new(2, 0, 0)),
        );

        world
            .execute(MutationCommand::authoring(WorldMutation::SwapRegeneratedChunks {
                chunks: new_chunks,
                generator: gen,
            }))
            .expect("authoring swap accepted");

        assert!(world.get_chunk(IVec3::new(2, 0, 0)).is_some(), "regenerated chunk merged");
        assert!(world.chunks.values().all(|c| c.mesh_dirty), "every resident chunk mesh-dirty");
    }

    #[test]
    fn batch_equals_sequential_singles() {
        use voxel_core::LocalPos;
        let edits: Vec<(LocalPos, Voxel)> = vec![
            (LocalPos::from_index(0), Voxel::cube(MaterialId(1))),
            (LocalPos::from_index(500), Voxel::cube(MaterialId(2))),
            (LocalPos::from_index(9000), Voxel::EMPTY),
            (LocalPos::from_index(500), Voxel::cube(MaterialId(3))), // last-write-wins on 500
        ];

        // Batched application (one storage clone).
        let mut batched = world_with_air_chunk(EngineMode::Authoring);
        batched
            .execute(MutationCommand::authoring(WorldMutation::EditVoxelBatch {
                chunk: IVec3::ZERO,
                edits: edits.clone(),
            }))
            .unwrap();

        // Sequential single edits (one clone each).
        let mut singles = world_with_air_chunk(EngineMode::Authoring);
        for (pos, voxel) in &edits {
            singles
                .execute(MutationCommand::authoring(WorldMutation::EditVoxel {
                    chunk: IVec3::ZERO,
                    index: pos.to_index() as u16,
                    voxel: *voxel,
                }))
                .unwrap();
        }

        let a = batched.get_chunk(IVec3::ZERO).unwrap();
        let b = singles.get_chunk(IVec3::ZERO).unwrap();
        let diffs = (0..CHUNK_VOLUME)
            .filter(|&i| a.data.voxels.voxel(i) != b.data.voxels.voxel(i))
            .count();
        assert_eq!(diffs, 0, "batch and sequential singles produce identical storage");
        assert_eq!(a.data.overrides, b.data.overrides, "and identical override buckets");
        assert!(a.persist_dirty && b.persist_dirty);
        assert!(a.mesh_dirty && b.mesh_dirty);
    }

    #[test]
    fn empty_batch_is_noop() {
        let mut world = world_with_air_chunk(EngineMode::Authoring);
        let outcome = world
            .execute(MutationCommand::authoring(WorldMutation::EditVoxelBatch {
                chunk: IVec3::ZERO,
                edits: vec![],
            }))
            .unwrap();
        assert!(outcome.mesh_invalidated.is_empty());
        assert!(!world.get_chunk(IVec3::ZERO).unwrap().persist_dirty, "empty batch marks nothing");
    }
    
    #[test]
    fn system_command_accepted_in_both_modes() {
        for mode in [EngineMode::Authoring, EngineMode::Play] {
            let mut world = world_with_air_chunk(mode);
            let chunk = crate::world::chunk::LoadedChunk::new_air(IVec3::new(9, 0, 0));
            world
                .execute(MutationCommand::system(WorldMutation::InsertLoadedChunk { chunk }))
                .expect("system command accepted regardless of mode");
        }
    }
}
