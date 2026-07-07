//! Tier-2/3 scatter instance render pass (design doc §6, §11).
//!
//! CPU-generated scatter instances (from each chunk's `ScatterStore`), with player
//! overrides applied) are grouped by `PrefabId` and instance-drawn: one
//! procedural mesh per prefab (built from the `PrefabRegistry`), one instanced
//! draw per prefab. Instance color/scale come from the prefab definition.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::prefabs::{PrefabRegistry, PrefabShape};
use crate::rendering::pipelines::{ScatterInstanceGpu, ScatterVertex};
use crate::rendering::render_context::RenderContext;
use crate::world::chunk::{LoadedChunk, CHUNK_SIZE, VOXEL_SCALE};
use crate::world::layers::PrefabId;
use crate::world::overrides::ChunkOverrides;
use crate::world::World;
use voxel_core::ShapeId;

/// GPU mesh for one prefab shape.
pub struct PrefabMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

/// Per-chunk, per-prefab instance buffer.
pub struct PrefabInstances {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
}

/// Per-chunk scatter: one optional instance buffer per prefab id (index = `PrefabId`).
pub struct ChunkScatter {
    pub by_prefab: Vec<Option<PrefabInstances>>,
}

#[derive(Resource)]
pub struct ScatterPass {
    /// One mesh per prefab id (index = `PrefabId`).
    pub prefab_meshes: Vec<PrefabMesh>,
    /// Prefab registry (color/scale lookup at instance-build time; kept for regen).
    prefabs: Arc<PrefabRegistry>,
    /// Per-chunk instance buffers, keyed by chunk position.
    pub chunk_scatter: HashMap<IVec3, ChunkScatter>,
}

impl ScatterPass {
    /// Full rebuild - scans all loaded chunks. Used for initial load and regen.
    pub fn new(ctx: &RenderContext, world: &World, prefabs: &Arc<PrefabRegistry>) -> Self {
        let prefab_meshes: Vec<PrefabMesh> = prefabs
            .iter()
            .map(|def| {
                let (verts, indices) = prefab_mesh(def.shape);
                let vertex_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("scatter_prefab_vertex_buffer"),
                    contents: bytemuck::cast_slice(&verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let index_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("scatter_prefab_index_buffer"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                PrefabMesh { vertex_buffer, index_buffer, index_count: indices.len() as u32 }
            })
            .collect();

        let mut chunk_scatter = HashMap::new();
        for (&coord, _) in &world.chunks {
            let pos = IVec3::from(coord);
            if let Some(chunk) = world.get_chunk(pos) {
                if let Some(cs) = build_chunk_scatter(chunk, prefabs, &ctx.device) {
                    chunk_scatter.insert(pos, cs);
                }
            }
        }
        log::info!("Scatter: built instances for {} chunks", chunk_scatter.len());

        Self { prefab_meshes, prefabs: prefabs.clone(), chunk_scatter }
    }

    /// Build (or refresh) the per-prefab instance buffer for a single chunk.
    pub fn add_chunk(&mut self, pos: IVec3, world: &World, device: &wgpu::Device) {
        let chunk = match world.get_chunk(pos) {
            Some(c) => c,
            None => return,
        };
        match build_chunk_scatter(chunk, &self.prefabs, device) {
            Some(cs) => {
                self.chunk_scatter.insert(pos, cs);
            }
            None => {
                self.chunk_scatter.remove(&pos);
            }
        }
    }

    /// Drop a chunk's instances on unload.
    pub fn remove_chunk(&mut self, pos: IVec3) {
        self.chunk_scatter.remove(&pos);
    }

    /// The prefab registry this pass was built with (used to rebuild on regen).
    pub fn prefabs(&self) -> &Arc<PrefabRegistry> {
        &self.prefabs
    }
}

/// Build per-prefab instance buffers for one chunk. `None` if it has no scatter.
fn build_chunk_scatter(
    chunk: &LoadedChunk,
    prefabs: &PrefabRegistry,
    device: &wgpu::Device,
) -> Option<ChunkScatter> {
    let grouped = group_instances_by_prefab(chunk, prefabs);
    if grouped.iter().all(|g| g.is_empty()) {
        return None;
    }
    let by_prefab = grouped
        .into_iter()
        .map(|insts| {
            if insts.is_empty() {
                None
            } else {
                let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("scatter_instance_buffer_chunk"),
                    contents: bytemuck::cast_slice(&insts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                Some(PrefabInstances { instance_buffer, instance_count: insts.len() as u32 })
            }
        })
        .collect();
    Some(ChunkScatter { by_prefab })
}

/// Group a chunk's effective scatter into per-prefab GPU instance vecs
/// (index = `PrefabId`). Instances with an unknown prefab fall back to id 0.
fn group_instances_by_prefab(
    chunk: &LoadedChunk,
    prefabs: &PrefabRegistry,
) -> Vec<Vec<ScatterInstanceGpu>> {
    let mut grouped: Vec<Vec<ScatterInstanceGpu>> = vec![Vec::new(); prefabs.len()];

    let generated = &chunk.data.scatter_instances;
    let no_overrides = ChunkOverrides::default();
    let overrides = chunk.data.overrides.as_ref().unwrap_or(&no_overrides);
    let c = chunk.data.coord;
    let span = CHUNK_SIZE as f32 * VOXEL_SCALE;
    let origin = [c.x as f32 * span, c.y as f32 * span, c.z as f32 * span];

    for si in overrides.effective_scatter(generated) {
        // Resolve the prefab, falling back to id 0 for unknown ids.
        let (prefab_idx, def) = match prefabs.get(si.prefab_id) {
            Some(def) => (si.prefab_id.0 as usize, def),
            None => (0, prefabs.get(PrefabId(0)).expect("prefab 0 present")),
        };

        let (ax, ay, az) = (si.anchor.x as usize, si.anchor.y as usize, si.anchor.z as usize);
        // sub_offset encodes the in-cell fraction: frac = (s + 128) / 256.
        let frac_x = (si.sub_offset[0] as f32 + 128.0) / 256.0;
        let frac_z = (si.sub_offset[2] as f32 + 128.0) / 256.0;
        let top = match chunk.voxel(ax, ay, az).shape {
            ShapeId::SlabBottom => ay as f32 + 0.5,
            _ => ay as f32 + 1.0,
        };
        let position = [
            origin[0] + (ax as f32 + frac_x) * VOXEL_SCALE,
            origin[1] + top * VOXEL_SCALE,
            origin[2] + (az as f32 + frac_z) * VOXEL_SCALE,
        ];
        let rotation_y = si.rotation_y as f32 / 256.0 * std::f32::consts::TAU;
        // Prefab base scale plus a little per-instance variation.
        let scale = def.scale * (0.85 + (si.scale_variant as f32 / 255.0) * 0.3);

        grouped[prefab_idx].push(ScatterInstanceGpu {
            position,
            rotation_y,
            color: def.color,
            scale,
        });
    }
    grouped
}

/// Procedural mesh for a prefab shape (position + normal; color is per-instance).
/// Local coords sit in x,z ∈ [-0.5, 0.5], y ∈ [0, ~1]; the instance scale is
/// applied at draw time. Drawn with cull disabled so crossed quads show both sides.
fn prefab_mesh(shape: PrefabShape) -> (Vec<ScatterVertex>, Vec<u32>) {
    match shape {
        PrefabShape::Rock => rock_mesh(),
        PrefabShape::Bush => crossed_quads(2, 0.9, 0.75),
        PrefabShape::GrassTuft => crossed_quads(3, 0.28, 1.0),
    }
}

/// `n` vertical quads evenly rotated around Y - a crossed-quad clump. Normal
/// points up for soft foliage shading.
fn crossed_quads(n: usize, width: f32, height: f32) -> (Vec<ScatterVertex>, Vec<u32>) {
    let hw = width * 0.5;
    let normal = [0.0, 1.0, 0.0];
    let mut verts = Vec::with_capacity(n * 4);
    let mut indices = Vec::with_capacity(n * 6);
    for i in 0..n {
        let theta = i as f32 * std::f32::consts::PI / n as f32;
        let (s, c) = theta.sin_cos();
        let (dx, dz) = (c * hw, s * hw);
        let base = verts.len() as u32;
        verts.push(ScatterVertex { position: [-dx, 0.0, -dz], normal });
        verts.push(ScatterVertex { position: [dx, 0.0, dz], normal });
        verts.push(ScatterVertex { position: [dx, height, dz], normal });
        verts.push(ScatterVertex { position: [-dx, height, -dz], normal });
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (verts, indices)
}

/// A low truncated pyramid - a chunky boulder. Normals point outward from the center.
fn rock_mesh() -> (Vec<ScatterVertex>, Vec<u32>) {
    let (b, t, h) = (0.5, 0.32, 0.55);
    let base = [[-b, 0.0, -b], [b, 0.0, -b], [b, 0.0, b], [-b, 0.0, b]];
    let top = [[-t, h, -t], [t, h, -t], [t, h, t], [-t, h, t]];
    let center = [0.0, h * 0.5, 0.0];

    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for i in 0..4 {
        let j = (i + 1) % 4;
        push_outward_quad(&mut verts, &mut indices, [base[i], base[j], top[j], top[i]], center);
    }
    push_outward_quad(&mut verts, &mut indices, [top[0], top[1], top[2], top[3]], center);
    (verts, indices)
}

fn push_outward_quad(
    verts: &mut Vec<ScatterVertex>,
    indices: &mut Vec<u32>,
    quad: [[f32; 3]; 4],
    center: [f32; 3],
) {
    let i0 = verts.len() as u32;
    for p in quad {
        let d = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
        verts.push(ScatterVertex { position: p, normal: [d[0] / len, d[1] / len, d[2] / len] });
    }
    indices.extend_from_slice(&[i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
}
