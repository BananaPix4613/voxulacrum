//! Tier-1 foliage paint render pass (design doc §6, §11).
//!
//! GPU-generates grass blades at draw time from each chunk's `DetailLayers`
//! density map - blades are *not* stored per-blade. Per chunk we upload a
//! storage buffer of `{ density, surface_y }` per column (32x32 = 1024) plus a
//! chunk-origin uniform; the vertex shader indexes the buffer by
//! `instance_index` and places one blade per dense column. The per-column
//! records are built on the CPU (`meshing::foliage`).
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
use voxel_core::{ChunkVersion, CHUNK_DIM as CHUNK_SIZE, VOXEL_SCALE};

/// Columns per chunk footprint; also the per-chunk instance count (one blade
/// candidate per column - empty columns are culled in the shader).
pub const COLUMN_COUNT: u32 = (CHUNK_SIZE * CHUNK_SIZE) as u32;

/// Max blades generated per column at full density. Must match `MAX_BLADES` in
/// `detail_paint.wgsl`. The shader culls blades beyond the column's
/// density-scaled count.
pub const MAX_BLADES_PER_COLUMN: u32 = 8;

/// Per-chunk instance count: one instance per (column, blade) slot.
pub const INSTANCES_PER_CHUNK: u32 = COLUMN_COUNT * MAX_BLADES_PER_COLUMN;

/// GPU per-column record: density (0 = no blade) + local surface height.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ColumnTexel {
    /// Packed paint texel: density (bits 0-7) | species (8-15) | tint (16-23).
    /// `density == 0` marks an empty column (no blades).
    pub packed: u32,
    /// Top-face height of the surface voxel, in voxel units (slab-aware:
    /// a bottom slab's top is `y + 0.5`; a full cube or top slab is `y + 1.0`).
    pub surface_top: f32,
}

/// GPU per-chunk uniform: world-space chunk origin, voxel scale, and the slot
/// of the chunk's detail layer in the species color table (`u32::MAX` for a
/// layer the registry does not define, which the shader draws magenta).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ChunkUniform {
    origin: [f32; 3],
    voxel_scale: f32,
    layer_slot: u32,
    _pad: [u32; 3],
}

/// One chunk's paint, built on the CPU (`meshing::foliage`).
pub struct DetailPaintData {
    /// One record per column, `COLUMN_COUNT` of them, indexed `x + z * CHUNK_SIZE`.
    pub columns: Vec<ColumnTexel>,
    /// World-space chunk origin.
    pub origin: [f32; 3],
    /// The chunk's detail layer's slot in the species color table.
    pub layer_slot: u32,
}

/// Per-chunk GPU detail-paint data.
pub struct ChunkDetailPaint {
    pub bind_group: wgpu::BindGroup,
    /// The chunk version this paint was built from.
    pub version: ChunkVersion,
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
    /// Group 2: the species color table, built once from the registry.
    pub species_bind_group: wgpu::BindGroup,
}

impl DetailPaintPass {
    /// The blade mesh and the species color table (`FoliageRegistry::species_color_table`).
    /// Chunks are added as they mesh.
    pub fn new(ctx: &RenderContext, species_colors: &[[f32; 4]]) -> Self {
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

        let species_colors = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("detail_paint_species_colors"),
            contents: bytemuck::cast_slice(species_colors),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let species_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("detail_paint_species_bind_group"),
            layout: &create_detail_species_layout(&ctx.device),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: species_colors.as_entire_binding(),
            }],
        });

        Self {
            blade_vertex_buffer,
            blade_index_buffer,
            blade_index_count: indices.len() as u32,
            chunk_bind_group_layout,
            chunk_paint: HashMap::new(),
            species_bind_group,
        }
    }

    /// Replace `pos`'s paint buffers with `data`, built from the chunk at
    /// `version`, or drop them when the chunk has no paint (`None`).
    pub fn set(
        &mut self,
        pos: IVec3,
        version: ChunkVersion,
        data: Option<&DetailPaintData>,
        device: &wgpu::Device,
    ) {
        debug_assert!(
            self.chunk_paint.get(&pos).map_or(true, |old| old.version <= version),
            "detail paint for chunk {pos} was built from an older version than the one it replaces",
        );
        match data {
            Some(data) => {
                let entry = make_chunk_paint(device, &self.chunk_bind_group_layout, data, version);
                self.chunk_paint.insert(pos, entry);
            }
            None => {
                self.chunk_paint.remove(&pos);
            }
        }
    }

    /// Drop a chunk's paint buffers on unload.
    pub fn remove(&mut self, pos: IVec3) {
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

/// The group-2 bind group layout: the read-only species color table. Like the
/// chunk layout, called both here and by the pipeline.
pub fn create_detail_species_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("detail_paint_species_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
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

fn make_chunk_paint(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    data: &DetailPaintData,
    version: ChunkVersion,
) -> ChunkDetailPaint {
    let uniform = ChunkUniform {
        origin: data.origin,
        voxel_scale: VOXEL_SCALE,
        layer_slot: data.layer_slot,
        _pad: [0; 3],
    };
    let column_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("detail_paint_columns"),
        contents: bytemuck::cast_slice(&data.columns),
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
    ChunkDetailPaint { bind_group, version }
}
