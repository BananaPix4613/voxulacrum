use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::params::CrossSectionParams;
use crate::world::chunk::{CHUNK_SIZE, VOXEL_SCALE};
use crate::world::World;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CapVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

impl CapVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CapVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}

pub struct CapPass {
    pub pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: Option<wgpu::Buffer>,
    pub index_buffer: Option<wgpu::Buffer>,
    pub index_count: u32,
    // Invalidation cache
    last_voxel_planes: [Option<i32>; 3],
    last_fog_color: [f32; 3],
    last_clip_dirs: [f32; 3],
    last_enabled: bool,
}

impl CapPass {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        shader_source: &str,
    ) -> Self {
        let pipeline = Self::create_pipeline(device, surface_format, global_bind_group_layout, shader_source);
        Self {
            pipeline,
            vertex_buffer: None,
            index_buffer: None,
            index_count: 0,
            last_voxel_planes: [None; 3],
            last_fog_color: [0.0; 3],
            last_clip_dirs: [0.0; 3],
            last_enabled: false,
        }
    }

    fn create_pipeline(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        shader_source: &str,
    ) -> wgpu::RenderPipeline {
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cap_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cap_pipeline_layout"),
            bind_group_layouts: &[global_bind_group_layout],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cap_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[CapVertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba16Float,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                ],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        })
    }

    /// Try to hot-reload the cap shader.
    pub fn try_reload_shader(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        source: &str,
    ) -> Result<(), String> {
        // Attempt to create module first to catch compile errors
        let _test_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cap_shader_test"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        self.pipeline = Self::create_pipeline(device, surface_format, global_bind_group_layout, source);
        Ok(())
    }

    /// Rebuild the cap mesh if the clip plane crossed a voxel boundary, color changed,
    /// or clip direction changed (camera rotation).
    pub fn maybe_rebuild(
        &mut self,
        device: &wgpu::Device,
        world: &World,
        cs: &CrossSectionParams,
        clip_pos: [f32; 3],
        clip_dirs: [f32; 3],
    ) {
        if !cs.enabled {
            if self.last_enabled {
                self.vertex_buffer = None;
                self.index_buffer = None;
                self.index_count = 0;
                self.last_enabled = false;
                self.last_voxel_planes = [None; 3];
            }
            return;
        }

        // Compute snapped voxel planes for each active axis.
        // For +dir we use floor (cap sits at the low side of the clipped voxel),
        // for -dir we use ceil (cap sits at the high side).
        let planes = [
            if cs.x_offset > 0.0 {
                if clip_dirs[0] > 0.0 {
                    Some((clip_pos[0] / VOXEL_SCALE).floor() as i32)
                } else {
                    Some((clip_pos[0] / VOXEL_SCALE).ceil() as i32)
                }
            } else { None },
            if cs.y_offset > 0.0 {
                Some((clip_pos[1] / VOXEL_SCALE).floor() as i32)
            } else { None },
            if cs.z_offset > 0.0 {
                if clip_dirs[2] > 0.0 {
                    Some((clip_pos[2] / VOXEL_SCALE).floor() as i32)
                } else {
                    Some((clip_pos[2] / VOXEL_SCALE).ceil() as i32)
                }
            } else { None },
        ];

        let color_changed = cs.fog_color != self.last_fog_color;
        let planes_changed = planes != self.last_voxel_planes;
        let dirs_changed = clip_dirs != self.last_clip_dirs;

        if !planes_changed && !color_changed && !dirs_changed && self.last_enabled {
            return; // No rebuild needed
        }

        let (vertices, indices) = generate_cap_mesh(world, &planes, clip_dirs, cs.fog_color);
        self.index_count = indices.len() as u32;

        if vertices.is_empty() {
            self.vertex_buffer = None;
            self.index_buffer = None;
        } else {
            self.vertex_buffer = Some(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("cap_vertex_buffer"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                },
            ));
            self.index_buffer = Some(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("cap_index_buffer"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                },
            ));
        }

        self.last_voxel_planes = planes;
        self.last_fog_color = cs.fog_color;
        self.last_clip_dirs = clip_dirs;
        self.last_enabled = true;
    }
}

/// Check if a voxel at absolute voxel coordinates is solid.
fn is_solid_at(world: &World, vx: usize, vy: usize, vz: usize) -> bool {
    let cx = vx / CHUNK_SIZE;
    let cy = vy / CHUNK_SIZE;
    let cz = vz / CHUNK_SIZE;
    let lx = vx % CHUNK_SIZE;
    let ly = vy % CHUNK_SIZE;
    let lz = vz % CHUNK_SIZE;
    match world.get_chunk(cx, cy, cz) {
        Some(chunk) => chunk.get_voxel(lx, ly, lz).is_solid(),
        None => false,
    }
}

/// Check if the marching cubes cell at absolute voxel coordinates has surface
/// geometry (at least one solid and one non-solid corner among its 8 corners).
/// This detects transitional cells where MC creates diagonal faces that extend
/// beyond the solid voxel center into "air" voxels.
fn has_mc_surface_at(world: &World, vx: usize, vy: usize, vz: usize) -> bool {
    let max_vx = world.chunks_x * CHUNK_SIZE;
    let max_vy = world.chunks_y * CHUNK_SIZE;
    let max_vz = world.chunks_z * CHUNK_SIZE;

    let mut has_solid = false;
    let mut has_air = false;

    // Check all 8 corners of the MC cell at (vx, vy, vz).
    // Corners span from (vx, vy, vz) to (vx+1, vy+1, vz+1),
    // matching CORNER_OFFSETS in marching_cubes.rs.
    for dz in 0..=1usize {
        for dy in 0..=1usize {
            for dx in 0..=1usize {
                let cx = vx + dx;
                let cy = vy + dy;
                let cz = vz + dz;

                if cx >= max_vx || cy >= max_vy || cz >= max_vz {
                    has_air = true;
                } else if is_solid_at(world, cx, cy, cz) {
                    has_solid = true;
                } else {
                    has_air = true;
                }

                if has_solid && has_air {
                    return true;
                }
            }
        }
    }

    false
}

/// Generate cap mesh geometry for all active clip planes.
fn generate_cap_mesh(
    world: &World,
    planes: &[Option<i32>; 3],
    clip_dirs: [f32; 3],
    fog_color: [f32; 3],
) -> (Vec<CapVertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let world_voxels_x = world.chunks_x * CHUNK_SIZE;
    let world_voxels_y = world.chunks_y * CHUNK_SIZE;
    let world_voxels_z = world.chunks_z * CHUNK_SIZE;

    // X clip plane
    if let Some(vx) = planes[0] {
        if vx >= 0 && (vx as usize) < world_voxels_x {
            generate_plane_x(
                world, vx as usize, world_voxels_y, world_voxels_z,
                clip_dirs[0], fog_color, &mut vertices, &mut indices,
            );
        }
    }

    // Y clip plane
    if let Some(vy) = planes[1] {
        if vy >= 0 && (vy as usize) < world_voxels_y {
            generate_plane_y(
                world, vy as usize, world_voxels_x, world_voxels_z,
                clip_dirs[1], fog_color, &mut vertices, &mut indices,
            );
        }
    }

    // Z clip plane
    if let Some(vz) = planes[2] {
        if vz >= 0 && (vz as usize) < world_voxels_z {
            generate_plane_z(
                world, vz as usize, world_voxels_x, world_voxels_y,
                clip_dirs[2], fog_color, &mut vertices, &mut indices,
            );
        }
    }

    (vertices, indices)
}

/// Generate cap quads for the X clip plane.
/// `dir` is +1.0 (clip +X side, normal faces +X) or -1.0 (clip -X side, normal faces -X).
fn generate_plane_x(
    world: &World,
    vx: usize,
    max_vy: usize,
    max_vz: usize,
    dir: f32,
    color: [f32; 3],
    vertices: &mut Vec<CapVertex>,
    indices: &mut Vec<u32>,
) {
    let normal = [dir, 0.0, 0.0];
    let wx = vx as f32 * VOXEL_SCALE;

    for vz in 0..max_vz {
        for vy in 0..max_vy {
            if is_solid_at(world, vx, vy, vz) || has_mc_surface_at(world, vx, vy, vz) {
                let y0 = vy as f32 * VOXEL_SCALE;
                let y1 = y0 + VOXEL_SCALE;
                let z0 = vz as f32 * VOXEL_SCALE;
                let z1 = z0 + VOXEL_SCALE;

                let base = vertices.len() as u32;
                vertices.push(CapVertex { position: [wx, y0, z0], normal, color });
                vertices.push(CapVertex { position: [wx, y1, z0], normal, color });
                vertices.push(CapVertex { position: [wx, y1, z1], normal, color });
                vertices.push(CapVertex { position: [wx, y0, z1], normal, color });

                if dir > 0.0 {
                    // CCW winding when viewed from +X direction
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                } else {
                    // Reversed winding for -X direction
                    indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
                }
            }
        }
    }
}

/// Generate cap quads for the Y clip plane.
/// `dir` is +1.0 (clip +Y side, normal faces +Y) or -1.0 (clip -Y side, normal faces -Y).
fn generate_plane_y(
    world: &World,
    vy: usize,
    max_vx: usize,
    max_vz: usize,
    dir: f32,
    color: [f32; 3],
    vertices: &mut Vec<CapVertex>,
    indices: &mut Vec<u32>,
) {
    let normal = [0.0, dir, 0.0];
    let wy = vy as f32 * VOXEL_SCALE;

    for vz in 0..max_vz {
        for vx in 0..max_vx {
            if is_solid_at(world, vx, vy, vz) || has_mc_surface_at(world, vx, vy, vz) {
                let x0 = vx as f32 * VOXEL_SCALE;
                let x1 = x0 + VOXEL_SCALE;
                let z0 = vz as f32 * VOXEL_SCALE;
                let z1 = z0 + VOXEL_SCALE;

                let base = vertices.len() as u32;
                vertices.push(CapVertex { position: [x0, wy, z0], normal, color });
                vertices.push(CapVertex { position: [x0, wy, z1], normal, color });
                vertices.push(CapVertex { position: [x1, wy, z1], normal, color });
                vertices.push(CapVertex { position: [x1, wy, z0], normal, color });

                if dir > 0.0 {
                    // CCW winding when viewed from +Y direction
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                } else {
                    // Reversed winding for -Y direction
                    indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
                }
            }
        }
    }
}

/// Generate cap quads for the Z clip plane.
/// `dir` is +1.0 (clip +Z side, normal faces +Z) or -1.0 (clip -Z side, normal faces -Z).
fn generate_plane_z(
    world: &World,
    vz: usize,
    max_vx: usize,
    max_vy: usize,
    dir: f32,
    color: [f32; 3],
    vertices: &mut Vec<CapVertex>,
    indices: &mut Vec<u32>,
) {
    let normal = [0.0, 0.0, dir];
    let wz = vz as f32 * VOXEL_SCALE;

    for vy in 0..max_vy {
        for vx in 0..max_vx {
            if is_solid_at(world, vx, vy, vz) || has_mc_surface_at(world, vx, vy, vz) {
                let x0 = vx as f32 * VOXEL_SCALE;
                let x1 = x0 + VOXEL_SCALE;
                let y0 = vy as f32 * VOXEL_SCALE;
                let y1 = y0 + VOXEL_SCALE;

                let base = vertices.len() as u32;
                vertices.push(CapVertex { position: [x0, y0, wz], normal, color });
                vertices.push(CapVertex { position: [x1, y0, wz], normal, color });
                vertices.push(CapVertex { position: [x1, y1, wz], normal, color });
                vertices.push(CapVertex { position: [x0, y1, wz], normal, color });

                if dir > 0.0 {
                    // CCW winding when viewed from +Z direction
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                } else {
                    // Reversed winding for -Z direction
                    indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
                }
            }
        }
    }
}
