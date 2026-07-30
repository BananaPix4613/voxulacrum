use std::sync::Arc;
use glam::IVec3;
use smallvec::SmallVec;
use std::time::Instant;

use super::layers::{DecalLayer, DetailLayers, FluidLayer, LightData, ScatterStore};
use super::overrides::ChunkOverrides;
use super::tags::{BiomeId, ChunkTags, ZoneId};
use super::storage::ChunkStorage;
use super::world_generator::GeneratedChunk;
use super::slab_smoothing::COLUMN_COUNT;
use voxel_core::{ChunkCoord, Voxel};

pub const CHUNK_SIZE: usize = voxel_core::CHUNK_DIM;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;
pub const VOXEL_SCALE: f32 = 1.0;
pub const CHUNK_WORLD_SIZE: f32 = CHUNK_SIZE as f32 * VOXEL_SCALE; // 32.0
pub const SNAP_PAD: usize = 1;
/// Padded snapshot size: CHUNK_SIZE + 2*SNAP_PAD
pub const SNAP_SIZE: usize = CHUNK_SIZE + 2 * SNAP_PAD;
pub const SNAP_VOLUME: usize = SNAP_SIZE * SNAP_SIZE * SNAP_SIZE;
/// When edit count exceeds this, store full chunk instead of delta.
pub const DELTA_THRESHOLD: usize = CHUNK_VOLUME / 4; // 8192

pub struct ChunkMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    /// Vertex + index buffer bytes. Recorded at upload rather than inferred from
    /// `index_count` and quad topology, so the residency panel reports the real
    /// GPU footprint instead of an estimate.
    pub gpu_bytes: u64,
}

/// Serializable chunk content: voxels plus the sidecar layers. This is the
/// data-model `Chunk` from the engine design (§3); persistence, serialization,
/// and the generation pipeline operate on this. Runtime state (meshing, dirty
/// flags) lives on [`LoadedChunk`], which wraps this as its `data` field.
pub struct Chunk {
    pub coord: ChunkCoord,
    pub voxels: Arc<ChunkStorage>,
    pub detail_layers: DetailLayers,
    pub scatter_instances: ScatterStore,
    // Design §3 layers; retained for the fluid/decal/lighting systems that will
    // consume them (not yet wired).
    #[allow(dead_code)]
    pub fluids: FluidLayer,
    #[allow(dead_code)]
    pub decals: DecalLayer,
    #[allow(dead_code)]
    pub lighting: LightData,
    /// Zone / biome / library identity for targeted invalidation. Phase 3 tags
    /// every chunk `ZoneId(0)` + `[BiomeId(0)]`; see [`super::tags::ChunkTags`].
    pub tags: ChunkTags,
    /// Player edits layered on top of generated terrain, or `None` when the
    /// chunk is persisted as a full-storage snapshot (after promotion past
    /// `DELTA_THRESHOLD`, or when loaded from a `Full` save). Canonical edit
    /// model; see [`super::overrides::ChunkOverrides`].
    pub overrides: Option<ChunkOverrides>,
}

impl Chunk {
    /// Build a data chunk from materialized storage with default-empty layers,
    pub fn new(coord: IVec3, voxels: Arc<ChunkStorage>) -> Self {
        Self {
            coord: coord.into(),
            voxels,
            detail_layers: DetailLayers::default(),
            scatter_instances: ScatterStore::default(),
            fluids: FluidLayer::default(),
            decals: DecalLayer::default(),
            lighting: LightData::default(),
            tags: ChunkTags::single_biome(ZoneId(0), BiomeId(0)),
            overrides: None,
        }
    }

    #[inline]
    pub fn voxel_index(x: usize, y: usize, z: usize) -> usize {
        x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE
    }
}

/// A chunk in the live set: the data-model [`Chunk`] plus the runtime state the
/// engine needs while it is loaded (meshing handles, dirty flags, debounce).
pub struct LoadedChunk {
    pub data: Chunk,
    pub mesh_dirty: bool,
    /// Monotonic counter incremented each time mesh_dirty is set to true.
    /// Used to detect stale mesh results: if a neighbor loads while this chunk
    /// is mid-mesh, the returned mesh has incorrect borders. The pipeline records
    /// mesh_seq at snapshot time and only clears mesh_dirty on upload if it matches.
    pub mesh_seq: u64,
    pub mesh: Option<ChunkMesh>,
    pub persist_dirty: bool,
    /// When the last edit occurred. None = generation-triggered dirty (no debounce).
    pub mesh_debounce: Option<Instant>,
    /// True once the cross-chunk seam pass (`world::seam`) has finalized this
    /// chunk's boundary slab demotions. Gates the one-shot pass.
    pub seam_finalized: bool,
    /// Per-column traversal smoothing distances captured at generation, so the
    /// seam pass knows which columns smooth without re-evaluating the graph.
    /// `None` until a generation path sets it (air/failed-gen chunks stay `None`).
    pub smoothing_distances: Option<Box<[u8; COLUMN_COUNT]>>,
}

impl LoadedChunk {
    pub fn new(position: IVec3, storage: Arc<ChunkStorage>) -> Self {
        Self {
            data: Chunk::new(position, storage),
            mesh_dirty: true,
            mesh_seq: 0,
            mesh: None,
            persist_dirty: false,
            mesh_debounce: None,
            seam_finalized: false,
            smoothing_distances: None,
        }
    }

    /// Create a chunk with default air storage.
    #[allow(dead_code)] // chunk-construction API; used by tests / future callers
    pub fn new_air(position: IVec3) -> Self {
        Self::new(position, Arc::new(ChunkStorage::new_air()))
    }

    /// Wire the generated sidecar layers (foliage paint, scatter, fluids, and the
    /// per-column slab-smoothing distances) onto this chunk. This is the single
    /// place they are set, so a new `GeneratedChunk` layer becomes a compile error
    /// here that every construction site must satisfy - rather than a field
    /// silently missed at one of the three sites (as `smoothing_distances` was on
    /// the regen path). Storage and tags are set by each caller: streaming applies
    /// overrides to storage and may take DB-persisted tags.
    pub fn set_generated_layers(
        &mut self,
        detail_layers: DetailLayers,
        scatter: ScatterStore,
        fluids: FluidLayer,
        smoothing_distances: Box<[u8; COLUMN_COUNT]>,
    ) {
        self.data.detail_layers = detail_layers;
        self.data.scatter_instances = scatter;
        self.data.fluids = fluids;
        self.smoothing_distances = Some(smoothing_distances);
    }

    /// Build a loaded chunk from a freshly generated one: storage, tags, and every
    /// generated sidecar layer. Used by the initial fill and regen; streaming is
    /// bespoke because it modifies storage with overrides before construction.
    pub fn from_generated(pos: IVec3, generated: GeneratedChunk) -> Self {
        let mut chunk = Self::new(pos, Arc::new(generated.storage));
        chunk.data.tags = generated.tags;
        chunk.set_generated_layers(
            generated.detail_layers,
            generated.scatter,
            generated.fluids,
            generated.smoothing_distances,
        );
        chunk
    }

    /// Mark this chunk as needing re-meshing, incrementing the sequence counter.
    pub fn mark_mesh_dirty(&mut self) {
        self.mesh_dirty = true;
        self.mesh_seq = self.mesh_seq.wrapping_add(1);
    }

    /// Mark dirty due to a voxel edit (applies debounce timer).
    #[allow(dead_code)] // voxel-edit toolkit; wired when editing lands
    pub fn mark_mesh_dirty_from_edit(&mut self) {
        self.mesh_dirty = true;
        self.mesh_seq = self.mesh_seq.wrapping_add(1);
        self.mesh_debounce = Some(Instant::now());
    }

    #[inline]
    pub fn voxel(&self, x: usize, y: usize, z: usize) -> Voxel {
        self.data.voxels.voxel(Chunk::voxel_index(x, y, z))
    }

    #[inline]
    pub fn is_solid(&self, x: usize, y: usize, z: usize) -> bool {
        self.data.voxels.is_solid(Chunk::voxel_index(x, y, z))
    }
}

pub struct ChunkNeighbors<'a> {
    /// 27-entry array: index = (dx+1)*9 + (dy+1)*3 + (dz+1)
    /// where dx,dy,dz ∈ {-1, 0, 1}. Index 13 = (0,0,0) = self, unused.
    pub neighbors: [Option<&'a LoadedChunk>; 27],
}

impl<'a> ChunkNeighbors<'a> {
    pub fn empty() -> Self {
        Self { neighbors: [None; 27] }
    }

    #[inline]
    pub fn set(&mut self, dx: i32, dy: i32, dz: i32, chunk: Option<&'a LoadedChunk>) {
        let idx = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
        self.neighbors[idx] = chunk;
    }

    #[inline]
    pub fn get(&self, dx: i32, dy: i32, dz: i32) -> Option<&'a LoadedChunk> {
        let idx = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
        self.neighbors[idx]
    }
}

// ============================================================================
// ChunkSnapshot — SoA density array + lazy material lookup
// ============================================================================

/// Self-contained snapshot used by the cube mesher. Holds the chunk's 32^3
/// voxels plus a 1-voxel border copied from face/edge/corner neighbors.
/// Missing neighbors fall back to `Voxel::EMPTY` (closed-world boundary).
pub struct ChunkSnapshot {
    pub position: IVec3,
    pub materials: Box<[Voxel; SNAP_VOLUME]>, // 34*34*34 = 39_304
    /// True per axis if this chunk is at the negative world border.
    #[allow(dead_code)] // border-state for boundary-aware meshing; not yet consumed
    pub border_min: [bool; 3],
}

impl ChunkSnapshot {
    pub fn extract(chunk: &LoadedChunk, neighbors: &ChunkNeighbors, min_chunk_y: i32, _max_chunk_y: i32) -> Self {
        let mut materials: Box<[Voxel; SNAP_VOLUME]> = unsafe {
            let v: Vec<Voxel> = vec![Voxel::EMPTY; SNAP_VOLUME];
            let boxed_slice = v.into_boxed_slice();
            Box::from_raw(Box::into_raw(boxed_slice) as *mut [Voxel; SNAP_VOLUME])
        };

        // Fill interior: chunk's own 32^3 voxels at snapshot coords [PAD..CHUNK_SIZE+PAD).
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let m = chunk.data.voxels.voxel(Chunk::voxel_index(x, y, z));
                    materials[Self::snap_index(x + SNAP_PAD, y + SNAP_PAD, z + SNAP_PAD)] = m;
                }
            }
        }

        // Fill 1-voxel border from face-adjacent neighbors (missing -> Voxel::EMPTY).
        for sz in 0..SNAP_SIZE {
            for sy in 0..SNAP_SIZE {
                for sx in 0..SNAP_SIZE {
                    let interior = sx >= SNAP_PAD && sx < CHUNK_SIZE + SNAP_PAD
                        && sy >= SNAP_PAD && sy < CHUNK_SIZE + SNAP_PAD
                        && sz >= SNAP_PAD && sz < CHUNK_SIZE + SNAP_PAD;
                    if interior { continue; }
                    let cx = sx as i32 - SNAP_PAD as i32;
                    let cy = sy as i32 - SNAP_PAD as i32;
                    let cz = sz as i32 - SNAP_PAD as i32;
                    materials[Self::snap_index(sx, sy, sz)] =
                        resolve_voxel(neighbors, cx, cy, cz);
                }
            }
        }

        let border_min = [false, chunk.data.coord.y == min_chunk_y, false];

        Self {
            position: chunk.data.coord.into(),
            materials,
            border_min,
        }
    }

    #[inline]
    pub fn snap_index(sx: usize, sy: usize, sz: usize) -> usize {
        sx + sy * SNAP_SIZE + sz * SNAP_SIZE * SNAP_SIZE
    }

    /// Read the voxel at chunk-local coordinates (x in -PAD..CHUNK_SIZE+PAD).
    #[allow(dead_code)] // snapshot read API; used by tests / mesher paths
    #[inline]
    pub fn get_voxel(&self, x: i32, y: i32, z: i32) -> Voxel {
        let pad = SNAP_PAD as i32;
        let sx = (x + pad) as usize;
        let sy = (y + pad) as usize;
        let sz = (z + pad) as usize;
        debug_assert!(
            sx < SNAP_SIZE && sy < SNAP_SIZE && sz < SNAP_SIZE,
            "ChunkSnapshot::get_voxel out of range: ({}, {}, {})", x, y, z
        );
        self.materials[Self::snap_index(sx, sy, sz)]
    }
}

/// Resolve a voxel at a chunk-border coord by reading the appropriate neighbor.
/// Missing neighbors (edge/corner slots) fall back to `Voxel::EMPTY`.
fn resolve_voxel(neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> Voxel {
    let cs = CHUNK_SIZE as i32;
    let (dx, lx) = if x < 0 { (-1, (x + cs) as usize) }
                else if x >= cs { (1, (x - cs) as usize) }
                else { (0, x as usize) };
    let (dy, ly) = if y < 0 { (-1, (y + cs) as usize) }
                else if y >= cs { (1, (y - cs) as usize) }
                else { (0, y as usize) };
    let (dz, lz) = if z < 0 { (-1, (z + cs) as usize) }
                else if z >= cs { (1, (z - cs) as usize) }
                else { (0, z as usize) };

    if dx == 0 && dy == 0 && dz == 0 {
        return Voxel::EMPTY;
    }

    match neighbors.get(dx, dy, dz) {
        Some(neighbor) => neighbor.data.voxels.voxel(Chunk::voxel_index(lx, ly, lz)),
        None => Voxel::EMPTY,
    }
}

/// Given a flat voxel index, return chunk-relative offsets of neighbors
/// whose mesh is also stale due to border proximity (within 1 voxel of face).
#[allow(dead_code)] // used by World::apply_edit (voxel-edit toolkit, not yet reachable)
pub fn border_dirty_neighbors(index: u16) -> SmallVec<[IVec3; 3]> {
    let x = (index as usize % CHUNK_SIZE) as i32;
    let y = ((index as usize / CHUNK_SIZE) % CHUNK_SIZE) as i32;
    let z = (index as usize / (CHUNK_SIZE * CHUNK_SIZE)) as i32;
    let mut neighbors = SmallVec::new();
    if x == 0                       { neighbors.push(IVec3::new(-1, 0, 0)); }
    if x == (CHUNK_SIZE as i32 - 1) { neighbors.push(IVec3::new( 1, 0, 0)); }
    if y == 0                       { neighbors.push(IVec3::new( 0,-1, 0)); }
    if y == (CHUNK_SIZE as i32 - 1) { neighbors.push(IVec3::new( 0, 1, 0)); }
    if z == 0                       { neighbors.push(IVec3::new( 0, 0,-1)); }
    if z == (CHUNK_SIZE as i32 - 1) { neighbors.push(IVec3::new( 0, 0, 1)); }
    neighbors
}