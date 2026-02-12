use wgpu::util::DeviceExt;

use crate::rendering::pipelines::WaterVertex;
use crate::simulation::water::StaticWater;

pub struct WaterPass {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

impl WaterPass {
    pub fn new(device: &wgpu::Device, water: &StaticWater) -> Self {
        let (vertices, indices) = generate_water_mesh(water);

        let vertex_buffer = if vertices.is_empty() {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("water_vertex_buffer_empty"),
                size: std::mem::size_of::<WaterVertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            })
        } else {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("water_vertex_buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            })
        };

        let index_buffer = if indices.is_empty() {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("water_index_buffer_empty"),
                size: 4,
                usage: wgpu::BufferUsages::INDEX,
                mapped_at_creation: false,
            })
        } else {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("water_index_buffer"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            })
        };

        let index_count = indices.len() as u32;
        log::info!("Water mesh: {} vertices, {} indices", vertices.len(), index_count);

        Self {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        }
    }
}

fn generate_water_mesh(water: &StaticWater) -> (Vec<WaterVertex>, Vec<u32>) {
    let w = water.width;
    let d = water.depth;
    let vw = w + 1;
    let vd = d + 1;
    let water_y = water.water_level();

    // Vertex data: average depth from adjacent cells at each grid intersection
    let mut vertex_depths = vec![0.0f32; vw * vd];
    let mut vertex_index_map = vec![u32::MAX; vw * vd];

    for vz in 0..vd {
        for vx in 0..vw {
            let vidx = vz * vw + vx;
            let mut depth_sum = 0.0f32;
            let mut count = 0u32;

            for &(dx, dz) in &[(0i32, 0i32), (-1, 0), (0, -1), (-1, -1)] {
                let cx = vx as i32 + dx;
                let cz = vz as i32 + dz;
                if cx >= 0 && cx < w as i32 && cz >= 0 && cz < d as i32 {
                    if water.has_water(cx as usize, cz as usize) {
                        depth_sum += water.water_depth(cx as usize, cz as usize);
                        count += 1;
                    }
                }
            }

            if count > 0 {
                vertex_depths[vidx] = depth_sum / count as f32;
            }
        }
    }

    let mut vertices: Vec<WaterVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for cz in 0..d {
        for cx in 0..w {
            if !water.has_water(cx, cz) { continue; }
            let corners = [(cx, cz), (cx+1, cz), (cx+1, cz+1), (cx, cz+1)];
            let mut ci = [0u32; 4];
            for (i, &(vx, vz)) in corners.iter().enumerate() {
                let vidx = vz * vw + vx;
                if vertex_index_map[vidx] == u32::MAX {
                    vertex_index_map[vidx] = vertices.len() as u32;
                    vertices.push(WaterVertex {
                        position: [vx as f32, water_y, vz as f32],
                        flow: [0.0, 0.0],  // Static — no flow
                        depth: vertex_depths[vidx].clamp(0.0, 10.0),
                    });
                }
                ci[i] = vertex_index_map[vidx];
            }
            indices.extend_from_slice(&[ci[0], ci[1], ci[2], ci[0], ci[2], ci[3]]);
        }
    }

    (vertices, indices)
}