use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::WaterVertex;
use crate::world::chunk::{LoadedChunk, CHUNK_SIZE, VOXEL_SCALE};
use crate::world::fluid_gen::{fluid_capacity, FULL_MASS, SLAB_MASS};
use crate::world::layers::FluidFillMode;
use crate::world::World;
use voxel_core::{LocalPos, ShapeId};

/// Per-chunk water surface mesh on GPU.
pub struct ChunkWaterMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

/// Water surface pass: holds one surface mesh per chunk that has visible water.
///
/// Geometry is (re)built from a chunk's `FluidLayer` when the chunk meshes; the
/// pipeline + shader (`water.wgsl`) handle transparency, depth shading, and
/// waves. Deep (fully `Submerged`) chunks produce no mesh - their surface is
/// rendered by the chunk that straddles `sea_level`.
#[derive(Resource)]
pub struct WaterPass {
    /// Per-chunk water meshes, keyed by chunk position. Chunks with no visible
    /// water surface have no entry.
    pub chunk_meshes: HashMap<IVec3, ChunkWaterMesh>,
}

impl WaterPass {
    /// Create an empty water pass. Meshes are added as chunks mesh.
    pub fn new() -> Self {
        Self { chunk_meshes: HashMap::new() }
    }

    /// Build (or refresh) the water surface mesh for a single chunk. Removes the
    /// entry when the chunk has no visible water surface.
    pub fn add_chunk_water(&mut self, pos: IVec3, world: &World, device: &wgpu::Device) {
        let chunk = match world.get_chunk(pos) {
            Some(c) => c,
            None => return,
        };
        let (verts, indices) = build_water_mesh(pos, chunk);
        if indices.is_empty() {
            self.chunk_meshes.remove(&pos);
            return;
        }
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water_vertex_buffer"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water_index_buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.chunk_meshes.insert(pos, ChunkWaterMesh {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        });
    }

    /// Remove water mesh for an unloaded chunk. GPU buffers dropped.
    pub fn remove_chunk_water(&mut self, pos: IVec3) {
        self.chunk_meshes.remove(&pos);
    }

    /// Clear all water meshes (used during regen).
    pub fn clear_all(&mut self) {
        self.chunk_meshes.clear();
    }
}

/// Build the water surface mesh for one chunk from its `FluidLayer`.
///
/// Emits an upward top-face quad for each water cell whose cell directly above
/// is open air, so a filled column yields exactly one quad at its top. Per-
/// vertex `depth` is the contiguous water depth below the surface in voxels,
/// which the shader maps to color + alpha. Returns empty vecs when the chunk has
/// no visible water surface.
fn build_water_mesh(pos: IVec3, chunk: &LoadedChunk) -> (Vec<WaterVertex>, Vec<u32>) {
    let fluids = &chunk.data.fluids;
    let storage = &chunk.data.voxels;
    let submerged = matches!(fluids.fill_mode, FluidFillMode::Submerged(_));

    // Fully dry chunk: nothing to build.
    if !submerged && fluids.cells.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let dim = CHUNK_SIZE;
    // Mass at a local cell: an explicit cell wins; otherwise the Submerged
    // default fills empty voxels; otherwise the cell is dry.
    let mass_at = |x: usize, y: usize, z: usize| -> u16 {
        let lp = LocalPos::new_unchecked(x as u8, y as u8, z as u8);
        if let Some(cell) = fluids.cells.get(&lp) {
            cell.mass
        } else if submerged {
            fluid_capacity(storage.voxel(lp.to_index()))
        } else {
            0
        }
    };

    let origin = [
        pos.x as f32 * dim as f32 * VOXEL_SCALE,
        pos.y as f32 * dim as f32 * VOXEL_SCALE,
        pos.z as f32 * dim as f32 * VOXEL_SCALE,
    ];

    let mut verts: Vec<WaterVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for z in 0..dim {
        for y in 0..dim {
            for x in 0..dim {
                let m = mass_at(x, y, z);
                if m == 0 {
                    continue;
                }
                // A surface exists only where the cell directly above is open air.
                let above_open = if y + 1 >= dim {
                    // Top row: deep water continues into the chunk above, so a
                    // Submerged chunk emits no surface there; a cells-mode chunk
                    // (pond) treats the ceiling as open.
                    !submerged
                } else {
                    let above = LocalPos::new_unchecked(x as u8, (y + 1) as u8, z as u8);
                    mass_at(x, y + 1, z) == 0 && !storage.voxel(above.to_index()).is_solid()
                };
                if !above_open {
                    continue;
                }

                // Surface height: top of this (possibly partial) cell. A slab's
                // fluid lives in its empty half only, so the fill fraction maps
                // onto that half instead of the whole cell - the top half for a
                // bottom slab, the bottom half for a top slab - so a brim-full
                // slab's surface lines up flush with a neighboring full cell.
                let shape = storage
                    .voxel(LocalPos::new_unchecked(x as u8, y as u8, z as u8).to_index())
                    .shape;
                let (base_frac, span_frac, capacity) = match shape {
                    ShapeId::SlabBottom => (0.5, 0.5, SLAB_MASS),
                    ShapeId::SlabTop => (0.0, 0.5, SLAB_MASS),
                    _ => (0.0, 1.0, FULL_MASS),
                };
                let fill_frac = m as f32 / capacity as f32;
                let surf_y = origin[1] + (y as f32 + base_frac + fill_frac * span_frac) * VOXEL_SCALE;
                // Depth: contiguous water cells downward from here, in voxels.
                let mut depth = 0u32;
                let mut yy = y as i32;
                while yy >= 0 && mass_at(x, yy as usize, z) > 0 {
                    depth += 1;
                    yy -= 1;
                }
                let d = depth as f32;

                let x0 = origin[0] + x as f32 * VOXEL_SCALE;
                let x1 = x0 + VOXEL_SCALE;
                let z0 = origin[2] + z as f32 * VOXEL_SCALE;
                let z1 = z0 + VOXEL_SCALE;
                let base = verts.len() as u32;
                // Top-face corners (CCW from above; culling is disabled).
                verts.push(WaterVertex { position: [x0, surf_y, z0], flow: [0.0, 0.0], depth: d });
                verts.push(WaterVertex { position: [x1, surf_y, z0], flow: [0.0, 0.0], depth: d });
                verts.push(WaterVertex { position: [x1, surf_y, z1], flow: [0.0, 0.0], depth: d });
                verts.push(WaterVertex { position: [x0, surf_y, z1], flow: [0.0, 0.0], depth: d });
                indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
    }

    (verts, indices)
}
