//! Tier-1 foliage paint render pass (design doc §6, §11).
//!
//! GPU-generates grass blades at draw time from each chunk's `DetailLayers`
//! density map - blades are *not* stored per-blade. Per chunk we upload a
//! storage buffer of `{ density, surface_y }` per column (32x32 = 1024) plus a
//! chunk-origin uniform; the vertex shader indexes the buffer by
//! `instance_index` and places one blade per dense column.
//!
//! Phase 5 4a-2i is the scaffold: a single flat-green blade per dense column,
//! proving the pipeline + per-chunk buffer + lifecycle + cutover. 4a-2ii adds
//! density-driven blade count, wind, tint, and per-blade variation.

use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use bytemuck::{Pod, Zeroable};
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::GrassVertex;
use crate::rendering::render_context::RenderContext;
use crate::world::chunk::{LoadedChunk, CHUNK_SIZE, VOXEL_SCALE};
use crate::world::World;
use voxel_core::ShapeId;

/// Columns per chunk footprint; also the per-chunk instance count (one blade
/// candidate per column - empty columns are culled in the shader).
pub const COLUMN_COUNT: u32 = (CHUNK_SIZE * CHUNK_SIZE) as u32;

/// Max blades generated per column at full density. Must match `MAX_BLADES` in
/// `detail_paint.wgsl`. The shader culls blades beyond the column's
/// density-scaled count.
pub const MAX_BLADES_PER_COLUMN: u32 = 8;

/// Per-chunk instance count: one instance per (column, blade) slot.
pub const INSTANCES_PER_CHUNK: u32 = COLUMN_COUNT * MAX_BLADES_PER_COLUMN;

const CHUNK_AREA: usize = CHUNK_SIZE * CHUNK_SIZE;

/// GPU per-column record: density (0 = no blade) + local surface height.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ColumnTexel {
    /// Packed paint texel: density (bits 0-7) | species (8-15) | tint (16-23).
    /// `density == 0` marks an empty column (no blades).
    packed: u32,
    /// Top-face height of the surface voxel, in voxel units (slab-aware:
    /// a bottom slab's top is `y + 0.5`; a full cube or top slab is `y + 1.0`).
    surface_top: f32,
}


/// GPU per-chunk uniform: world-space chunk origin + voxel scale.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ChunkUniform {
    origin: [f32; 3],
    voxel_scale: f32,
}

/// Per-chunk GPU detail-paint data.
pub struct ChunkDetailPaint {
    pub bind_group: wgpu::BindGroup,
}

#[derive(Resource)]
pub struct DetailPaintPass {
    pub blade_vertex_buffer: wgpu::Buffer,
    pub blade_index_buffer: wgpu::Buffer,
    pub blade_index_count: u32,
    /// Group-1 layout for the per-chunk density buffer + chunk uniform.
    pub chunk_bind_group_layout: wgpu::BindGroupLayout,
    /// Per-chunk paint buffers, keyed by chunk position.
    pub chunk_paint: HashMap<IVec3, ChunkDetailPaint>,
}

impl DetailPaintPass {
    /// Full rebuild - scans all loaded chunks. Used for initial load and regen.
    pub fn new(ctx: &RenderContext, world: &World) -> Self {
        let (vertices, indices) = create_blade_mesh();

        let blade_vertex_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("detail_paint_blade_vertex_buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let blade_index_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("detail_paint_blade_index_buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let chunk_bind_group_layout = create_detail_chunk_layout(&ctx.device);

        let mut chunk_paint = HashMap::new();
        for (&coord, _) in &world.chunks {
            let pos = IVec3::from(coord);
            if let Some(chunk) = world.get_chunk(pos) {
                if let Some((columns, uniform)) = build_chunk_columns(chunk) {
                    chunk_paint.insert(
                        pos,
                        make_chunk_paint(&ctx.device, &chunk_bind_group_layout, &columns, uniform),
                    );
                }
            }
        }
        log::info!("Detail paint: built {} painted chunks", chunk_paint.len());

        Self {
            blade_vertex_buffer,
            blade_index_buffer,
            blade_index_count: indices.len() as u32,
            chunk_bind_group_layout,
            chunk_paint,
        }
    }

    /// Build (or refresh) per-chunk paint buffers for a single chunk.
    pub fn add_chunk(&mut self, pos: IVec3, world: &World, device: &wgpu::Device) {
        let chunk = match world.get_chunk(pos) {
            Some(c) => c,
            None => return,
        };
        match build_chunk_columns(chunk) {
            Some((columns, uniform)) => {
                let entry =
                    make_chunk_paint(device, &self.chunk_bind_group_layout, &columns, uniform);
                self.chunk_paint.insert(pos, entry);
            }
            None => {
                self.chunk_paint.remove(&pos);
            }
        }
    }

    /// Drop a chunk's paint buffers on unload.
    pub fn remove_chunk(&mut self, pos: IVec3) {
        self.chunk_paint.remove(&pos);
    }
}

/// The group-1 bind group layout: a read-only per-column storage buffer and a
/// per-chunk uniform. Called both here (per-chunk bind groups) and by the
/// pipeline (`create_detail_paint_pipeline`); identical descriptors are
/// group-equivalent, so the bind groups are pipeline-compatible.
pub fn create_detail_chunk_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("detail_paint_chunk_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// A 2-segment tapered blade (base quad + tip triangle). double-sided, sized
/// for 0.5m voxels. `uv.y` carries normalized height (0 at base, 1 at tip),
/// used by the shader for wind bend and shading.
fn create_blade_mesh() -> (Vec<GrassVertex>, Vec<u32>) {
    let hw = 0.045_f32; // base half-width
    let h = 0.5_f32;    // height before per-blade scale
    let vertices = vec![
        GrassVertex { position: [-hw,       0.0,     0.0], uv: [0.0, 0.0], _pad: 0.0 }, // 0 base L
        GrassVertex { position: [ hw,       0.0,     0.0], uv: [1.0, 0.0], _pad: 0.0 }, // 1 base R
        GrassVertex { position: [-hw * 0.6, 0.5 * h, 0.0], uv: [0.0, 0.5], _pad: 0.0 }, // 2 mid L
        GrassVertex { position: [ hw * 0.6, 0.5 * h, 0.0], uv: [1.0, 0.5], _pad: 0.0 }, // 3 mid R
        GrassVertex { position: [ 0.0,      h,       0.0], uv: [0.5, 1.0], _pad: 0.0 }, // 4 tip
    ];
    let indices: Vec<u32> = vec![
        // front
        0, 1, 2,  2, 1, 3,  2, 3, 4,
        // back (reversed winding)
        2, 1, 0,  3, 1, 2,  4, 3, 2,
    ];
    (vertices, indices)
}

/// Build the per-column density buffer (+ chunk uniform) from a chunk's first
/// detail layer. Returns `None` if the chunk has no paint to draw.
fn build_chunk_columns(chunk: &LoadedChunk) -> Option<(Vec<ColumnTexel>, ChunkUniform)> {
    let layer = chunk.data.detail_layers.layers.first()?;

    let mut any = false;
    let mut columns = Vec::with_capacity(CHUNK_AREA);
    for idx in 0..CHUNK_AREA {
        let lx = idx % CHUNK_SIZE;
        let lz = idx / CHUNK_SIZE;
        let texel = layer.map[idx];
        if texel.density == 0 {
            columns.push(ColumnTexel { packed: 0, surface_top: 0.0 });
            continue;
        }
        // Topmost solid voxel in this column = the painted surface. Use the
        // voxel's shape so blades sit on a slab's top face, not the cell top.
        let mut surface_top = None;
        for ly in (0..CHUNK_SIZE).rev() {
            if chunk.is_solid(lx, ly, lz) {
                let cell_top = match chunk.voxel(lx, ly, lz).shape {
                    ShapeId::SlabBottom => ly as f32 + 0.5,
                    _ => ly as f32 + 1.0,
                };
                surface_top = Some(cell_top);
                break;
            }
        }
        match surface_top {
            Some(top) => {
                let packed = texel.density as u32
                    | ((texel.species as u32) << 8)
                    | ((texel.tint as u32) << 16);
                columns.push(ColumnTexel { packed, surface_top: top });
                any = true;
            }
            None => columns.push(ColumnTexel { packed: 0, surface_top: 0.0 }),
        }
    }
    if !any {
        return None;
    }

    let c = chunk.data.coord;
    let span = CHUNK_SIZE as f32 * VOXEL_SCALE;
    let uniform = ChunkUniform {
        origin: [c.x as f32 * span, c.y as f32 * span, c.z as f32 * span],
        voxel_scale: VOXEL_SCALE,
    };
    Some((columns, uniform))
}

fn make_chunk_paint(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    columns: &[ColumnTexel],
    uniform: ChunkUniform,
) -> ChunkDetailPaint {
    let column_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("detail_paint_columns"),
        contents: bytemuck::cast_slice(columns),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("detail_paint_chunk_uniform"),
        contents: bytemuck::bytes_of(&uniform),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("detail_paint_chunk_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: column_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: uniform_buffer.as_entire_binding() },
        ],
    });
    ChunkDetailPaint { bind_group }
}
