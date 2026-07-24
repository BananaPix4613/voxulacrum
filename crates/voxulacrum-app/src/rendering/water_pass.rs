use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::{Vec2, Vec3, IVec3};
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::WaterVertex;
use crate::world::chunk::{CHUNK_SIZE, VOXEL_SCALE};
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
    /// Per-chunk water meshes, keyed by chunk position.
    pub chunk_meshes: HashMap<IVec3, ChunkWaterMesh>,
    chunk_surface: HashMap<IVec3, f32>,
    pub avg_surface_y: f32,
    pub uniform_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
}

impl WaterPass {
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        scene_copy_view: &wgpu::TextureView,
        scene_depth_view: &wgpu::TextureView,
        reflection_color_view: &wgpu::TextureView,
        reflection_depth_view: &wgpu::TextureView,
    ) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water_uniform_buffer"),
            size: std::mem::size_of::<crate::rendering::uniforms::WaterUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water_scene_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group = crate::rendering::uniforms::create_water_bind_group(
            device, layout, &uniform_buffer, scene_copy_view, &sampler, scene_depth_view,
            reflection_color_view, reflection_depth_view,
        );
        Self {
            chunk_meshes: HashMap::new(),
            chunk_surface: HashMap::new(),
            avg_surface_y: 0.0,
            uniform_buffer,
            bind_group,
            sampler,
        }
    }

    fn recompute_avg(&mut self) {
        if self.chunk_surface.is_empty() {
            return; // keep the last known plane so a momentary empty set doesn't reset it
        }
        let sum: f32 = self.chunk_surface.values().sum();
        self.avg_surface_y = sum / self.chunk_surface.len() as f32;
    }

    /// Rebuild the bind group after render targets are recreated (resize).
    pub fn rebuild_bind_group(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        scene_copy_view: &wgpu::TextureView,
        scene_depth_view: &wgpu::TextureView,
        reflection_color_view: &wgpu::TextureView,
        reflection_depth_view: &wgpu::TextureView,
    ) {
        self.bind_group = crate::rendering::uniforms::create_water_bind_group(
            device, layout, &self.uniform_buffer, scene_copy_view, &self.sampler, scene_depth_view,
            reflection_color_view, reflection_depth_view,
        );
    }

    /// Upload per-frame water uniforms.
    pub fn update_uniforms(
        &self,
        queue: &wgpu::Queue,
        u: &crate::rendering::uniforms::WaterUniforms,
    ) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(std::slice::from_ref(u)));
    }

    /// Build (or refresh) the water surface mesh for a single chunk. Removes the
    /// entry when the chunk has no visible water surface.
    pub fn add_chunk_water(&mut self, pos: IVec3, world: &World, device: &wgpu::Device) {
        if world.get_chunk(pos).is_none() {
            return;
        }
        let (verts, indices, surf_y) = build_water_mesh(pos, world);
        if indices.is_empty() {
            self.chunk_meshes.remove(&pos);
            self.chunk_surface.remove(&pos);
            self.recompute_avg();
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
        self.chunk_surface.insert(pos, surf_y);
        self.recompute_avg();
    }

    /// Remove water mesh for an unloaded chunk. GPU buffers dropped.
    pub fn remove_chunk_water(&mut self, pos: IVec3) {
        self.chunk_meshes.remove(&pos);
        self.chunk_surface.remove(&pos);
        self.recompute_avg();
    }

    /// Clear all water meshes (used during regen).
    pub fn clear_all(&mut self) {
        self.chunk_meshes.clear();
        self.chunk_surface.clear();
    }
}

// Surface welding/flow tuning (voxel units).
const HEIGHT_TOL: f32 = 1.25; // max surface-Y difference to weld adjacent columns into one sheet
const DEPTH_TOL: f32 = 1.5; // max water-depth (terrain-floor) difference to weld across
//const CURTAIN_STEP: f32 = 0.6; // drop to a lower/absent neighbor before we hand a falling sheet
const FALL_MAX: f32 = 5.0; // max curtain height emitted per chunk
const FLOW_GAIN: f32 = 1.5; // surface slope -> flow magnitude (clamped to 1)

#[inline]
fn weldable(c: Col, ref_y: f32, ref_depth: f32) -> bool {
    (c.y - ref_y).abs() <= HEIGHT_TOL && (c.depth - ref_depth).abs() <= DEPTH_TOL
}

/// One column's water surface within this chunk's Y band.
#[derive(Copy, Clone)]
struct Col {
    y: f32,     // world-space surface height
    depth: f32, // contiguous water cells downward, voxels
}

/// Floor division (correct for negative coordinates).
fn floor_div(a: i32, b: i32) -> i32 {
    let d = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) { d - 1 } else { d }
}

/// Whether the terrain voxel at center-chunk-local column `(gx, gz)`, world-Y `wy`,
/// is solid (resolving into neighbor chunks). Unloaded/out-of-range reads as air.
/// This is what separates a genuine spill edge (open air at the waterline) from a
/// solid bank/dam (which must contain the water, not shed a curtain into it).
fn terrain_solid_at(world: &World, base: IVec3, gx: i32, gz: i32, wy: i32) -> bool {
    let dim = CHUNK_SIZE as i32;
    let wx = base.x * dim + gx;
    let wz = base.z * dim + gz;
    let cx = floor_div(wx, dim);
    let cy = floor_div(wy, dim);
    let cz = floor_div(wz, dim);
    let lx = (wx - cx * dim) as u8;
    let ly = (wy - cy * dim) as u8;
    let lz = (wz - cz * dim) as u8;
    match world.get_chunk(IVec3::new(cx, cy, cz)) {
        Some(chunk) => chunk
            .data
            .voxels
            .voxel(LocalPos::new_unchecked(lx, ly, lz).to_index())
            .is_solid(),
        None => false,
    }
}

/// Water surface of the column at center-chunk-local `(gx, gz)` (which may fall
/// into a horizontal neighbor chunk), searched within the center chunk's Y layer.
/// `None` when the column has no visible water surface here.
fn column_surface(world: &World, base: IVec3, gx: i32, gz: i32) -> Option<Col> {
    let dim = CHUNK_SIZE as i32;
    let wx = base.x * dim + gx;
    let wz = base.z * dim + gz;
    let cx = floor_div(wx, dim);
    let cz = floor_div(wz, dim);
    let lx = (wx - cx * dim) as usize;
    let lz = (wz - cz * dim) as usize;

    let chunk = world.get_chunk(IVec3::new(cx, base.y, cz))?;
    let fluids = &chunk.data.fluids;
    let storage = &chunk.data.voxels;
    let submerged = matches!(fluids.fill_mode, FluidFillMode::Submerged(_));
    if !submerged && fluids.cells.is_empty() {
        return None;
    }

    let mass_at = |y: usize| -> u16 {
        let lp = LocalPos::new_unchecked(lx as u8, y as u8, lz as u8);
        if let Some(c) = fluids.cells.get(&lp) {
            c.mass
        } else if submerged {
            fluid_capacity(storage.voxel(lp.to_index()))
        } else {
            0
        }
    };

    for y in (0..CHUNK_SIZE).rev() {
        let m = mass_at(y);
        if m == 0 {
            continue;
        }
        let above_open = if y + 1 >= CHUNK_SIZE {
            !submerged
        } else {
            mass_at(y + 1) == 0
                && !storage
                    .voxel(LocalPos::new_unchecked(lx as u8, (y + 1) as u8, lz as u8).to_index())
                    .is_solid()
        };
        if !above_open {
            continue;
        }

        // Slab-aware fill fraction, so a brim-full slab lines up flush with a full cell.
        let shape = storage
            .voxel(LocalPos::new_unchecked(lx as u8, y as u8, lz as u8).to_index())
            .shape;
        let (base_frac, span_frac, capacity) = match shape {
            ShapeId::SlabBottom => (0.5f32, 0.5f32, SLAB_MASS),
            ShapeId::SlabTop => (0.0, 0.5, SLAB_MASS),
            _ => (0.0, 1.0, FULL_MASS),
        };
        let fill = m as f32 / capacity as f32;
        let world_y = ((base.y * dim + y as i32) as f32 + base_frac + fill * span_frac) * VOXEL_SCALE;

        let mut depth = 0u32;
        let mut yy = y as i32;
        while yy >= 0 && mass_at(yy as usize) > 0 {
            depth += 1;
            yy -= 1;
        }
        return Some(Col { y: world_y, depth: depth as f32 });
    }
    None
}

#[inline]
fn col_at(cols: &[Option<Col>], stride: usize, gx: i32, gz: i32) -> Option<Col> {
    cols[(gz + 1) as usize * stride + (gx + 1) as usize]
}

fn corner_height(
    cols: &[Option<Col>],
    stride: usize,
    x: i32,
    z: i32,
    ax: i32,
    az: i32,
    ref_y: f32,
    ref_depth: f32,
) -> f32 {
    let touching = [
        (x + ax - 1, z + az - 1),
        (x + ax, z + az - 1),
        (x + ax - 1, z + az),
        (x + ax, z + az),
    ];
    let mut sum = 0.0;
    let mut n = 0.0;
    for (cx, cz) in touching {
        if let Some(c) = col_at(cols, stride, cx, cz) {
            if weldable(c, ref_y, ref_depth) {
                sum += c.y;
                n += 1.0;
            }
        }
    }
    if n > 0.0 { sum / n } else { ref_y }
}

fn slope(pos_n: Option<Col>, neg_n: Option<Col>, ref_y: f32, ref_depth: f32) -> f32 {
    let a = pos_n.filter(|c| weldable(*c, ref_y, ref_depth)).map(|c| c.y).unwrap_or(ref_y);
    let b = neg_n.filter(|c| weldable(*c, ref_y, ref_depth)).map(|c| c.y).unwrap_or(ref_y);
    (a - b) * 0.5
}

/// Build the water surface mesh for one chunk, welded continuously across cell and
/// chunk seams. Each surface column emits one quad whose corners are averaged with
/// its neighbors (continuous sloped surface, not per-cell plates), carrying a smoothed
/// normal and a downhill flow vector. Sharp drops to lower/absent neighbors hang a
/// vertical "curtain" so cascades read as connected sheets.
fn build_water_mesh(pos: IVec3, world: &World) -> (Vec<WaterVertex>, Vec<u32>, f32) {
    let dim = CHUNK_SIZE as i32;
    let stride = CHUNK_SIZE + 2;

    // Sample center chunk + 1-column ring of horizontal neighbors.
    let mut cols: Vec<Option<Col>> = Vec::with_capacity(stride * stride);
    for gz in -1..=dim {
        for gx in -1..=dim {
            cols.push(column_surface(world, pos, gx, gz));
        }
    }

    let mut verts: Vec<WaterVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut surf_sum = 0.0f32;
    let mut surf_count = 0u32;

    for z in 0..dim {
        for x in 0..dim {
            let here = match col_at(&cols, stride, x, z) {
                Some(c) => c,
                None => continue,
            };
            let hy = here.y;
            surf_sum += hy;
            surf_count += 1;

            let dhx = slope(col_at(&cols, stride, x + 1, z), col_at(&cols, stride, x - 1, z), hy, here.depth);
            let dhz = slope(col_at(&cols, stride, x, z + 1), col_at(&cols, stride, x, z - 1), hy, here.depth);
            let normal = Vec3::new(-dhx, 1.0, -dhz).normalize();
            let g = Vec2::new(dhx, dhz);
            let flow = if g.length() > 1e-4 {
                (-g).normalize() * (g.length() * FLOW_GAIN).min(1.0)
            } else {
                Vec2::ZERO
            };
            let nrm = [normal.x, normal.y, normal.z];
            let fl = [flow.x, flow.y];
            let d = here.depth;

            let x0 = (pos.x * dim + x) as f32 * VOXEL_SCALE;
            let z0 = (pos.z * dim + z) as f32 * VOXEL_SCALE;
            let x1 = x0 + VOXEL_SCALE;
            let z1 = z0 + VOXEL_SCALE;

            let c00 = corner_height(&cols, stride, x, z, 0, 0, hy, here.depth);
            let c10 = corner_height(&cols, stride, x, z, 1, 0, hy, here.depth);
            let c11 = corner_height(&cols, stride, x, z, 1, 1, hy, here.depth);
            let c01 = corner_height(&cols, stride, x, z, 0, 1, hy, here.depth);

            let base = verts.len() as u32;
            verts.push(WaterVertex { position: [x0, c00, z0], normal: nrm, flow: fl, depth: d });
            verts.push(WaterVertex { position: [x1, c10, z0], normal: nrm, flow: fl, depth: d });
            verts.push(WaterVertex { position: [x1, c11, z1], normal: nrm, flow: fl, depth: d });
            verts.push(WaterVertex { position: [x0, c01, z1], normal: nrm, flow: fl, depth: d });
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);

            // Cascades: hang a falling sheet toward each 4-neighbor the water genuinely
            // spills over. Gated on terrain solidity at the waterline cell, so a lake/
            // ocean shoreline (a bank rising to the surface) or a dam wall never
            // curtains - that was the old z-fighting - while a pond lip or a step down
            // onto lower water does.
            let wy = hy.floor() as i32; // world cell the waterline sits in
            // (dx, dz, top corner A, top corner B, edge endpoints, world normal, flow dir)
            let sides: [(i32, i32, f32, f32, (f32, f32, f32, f32), [f32; 3], [f32; 2]); 4] = [
                (1, 0, c10, c11, (x1, z0, x1, z1), [1.0, 0.0, 0.0], [1.0, 0.0]),
                (-1, 0, c00, c01, (x0, z0, x0, z1), [-1.0, 0.0, 0.0], [-1.0, 0.0]),
                (0, 1, c01, c11, (x0, z1, x1, z1), [0.0, 0.0, 1.0], [0.0, 1.0]),
                (0, -1, c00, c10, (x0, z0, x1, z0), [0.0, 0.0, -1.0], [0.0, -1.0]),
            ];
            for (dx, dz, top_a, top_b, (ax0, az0, ax1, az1), cn, cflow) in sides {
                // Solid neighbor at the waterline = bank/dam: contain, don't spill.
                if terrain_solid_at(world, pos, x + dx, z + dz, wy) {
                    continue;
                }
                let neighbor = col_at(&cols, stride, x + dx, z + dz);
                let target = match neighbor {
                    Some(c) if hy - c.y > HEIGHT_TOL => c.y, // step down onto lower water
                    Some(_) => continue,                       // ~level: welded by the top quads
                    None => hy - FALL_MAX,                     // spill into open air
                };
                let target = target.max(hy - FALL_MAX);
                if top_a.min(top_b) - target <= 0.05 {
                    continue; // negligible drop
                }
                let b = verts.len() as u32;
                verts.push(WaterVertex { position: [ax0, top_a, az0], normal: cn, flow: cflow, depth: 1.0 });
                verts.push(WaterVertex { position: [ax1, top_b, az1], normal: cn, flow: cflow, depth: 1.0 });
                verts.push(WaterVertex { position: [ax1, target, az1], normal: cn, flow: cflow, depth: 1.0 });
                verts.push(WaterVertex { position: [ax0, target, az0], normal: cn, flow: cflow, depth: 1.0 });
                indices.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
            }
        }
    }

    let avg_surface_y = if surf_count > 0 { surf_sum / surf_count as f32 } else { 0.0 };
    (verts, indices, avg_surface_y)
}
