use super::pipelines::TerrainVertex;
use wgpu::util::DeviceExt;

pub struct DebugCube {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

impl DebugCube {
    pub fn new(device: &wgpu::Device) -> Self {
        // Unit cube from -0.5 to +0.5, 24 vertices (4 per face with unique normals/colors)
        #[rustfmt::skip]
        let vertices: Vec<TerrainVertex> = vec![
            // Top face (Y+), green
            TerrainVertex { position: [-0.5,  0.5, -0.5], normal: [0.0, 1.0, 0.0], color: [0.3, 0.7, 0.3], ao: 1.0 },
            TerrainVertex { position: [ 0.5,  0.5, -0.5], normal: [0.0, 1.0, 0.0], color: [0.3, 0.7, 0.3], ao: 1.0 },
            TerrainVertex { position: [ 0.5,  0.5,  0.5], normal: [0.0, 1.0, 0.0], color: [0.3, 0.7, 0.3], ao: 1.0 },
            TerrainVertex { position: [-0.5,  0.5,  0.5], normal: [0.0, 1.0, 0.0], color: [0.3, 0.7, 0.3], ao: 1.0 },
            // Bottom face (Y-), dark brown
            TerrainVertex { position: [-0.5, -0.5,  0.5], normal: [0.0, -1.0, 0.0], color: [0.35, 0.2, 0.1], ao: 1.0 },
            TerrainVertex { position: [ 0.5, -0.5,  0.5], normal: [0.0, -1.0, 0.0], color: [0.35, 0.2, 0.1], ao: 1.0 },
            TerrainVertex { position: [ 0.5, -0.5, -0.5], normal: [0.0, -1.0, 0.0], color: [0.35, 0.2, 0.1], ao: 1.0 },
            TerrainVertex { position: [-0.5, -0.5, -0.5], normal: [0.0, -1.0, 0.0], color: [0.35, 0.2, 0.1], ao: 1.0 },
            // Front face (Z+), warm red
            TerrainVertex { position: [-0.5, -0.5,  0.5], normal: [0.0, 0.0, 1.0], color: [0.75, 0.35, 0.3], ao: 1.0 },
            TerrainVertex { position: [-0.5,  0.5,  0.5], normal: [0.0, 0.0, 1.0], color: [0.75, 0.35, 0.3], ao: 1.0 },
            TerrainVertex { position: [ 0.5,  0.5,  0.5], normal: [0.0, 0.0, 1.0], color: [0.75, 0.35, 0.3], ao: 1.0 },
            TerrainVertex { position: [ 0.5, -0.5,  0.5], normal: [0.0, 0.0, 1.0], color: [0.75, 0.35, 0.3], ao: 1.0 },
            // Back face (Z-), teal
            TerrainVertex { position: [ 0.5, -0.5, -0.5], normal: [0.0, 0.0, -1.0], color: [0.3, 0.6, 0.6], ao: 1.0 },
            TerrainVertex { position: [ 0.5,  0.5, -0.5], normal: [0.0, 0.0, -1.0], color: [0.3, 0.6, 0.6], ao: 1.0 },
            TerrainVertex { position: [-0.5,  0.5, -0.5], normal: [0.0, 0.0, -1.0], color: [0.3, 0.6, 0.6], ao: 1.0 },
            TerrainVertex { position: [-0.5, -0.5, -0.5], normal: [0.0, 0.0, -1.0], color: [0.3, 0.6, 0.6], ao: 1.0 },
            // Right face (X+), blue-purple
            TerrainVertex { position: [ 0.5, -0.5,  0.5], normal: [1.0, 0.0, 0.0], color: [0.4, 0.35, 0.7], ao: 1.0 },
            TerrainVertex { position: [ 0.5,  0.5,  0.5], normal: [1.0, 0.0, 0.0], color: [0.4, 0.35, 0.7], ao: 1.0 },
            TerrainVertex { position: [ 0.5,  0.5, -0.5], normal: [1.0, 0.0, 0.0], color: [0.4, 0.35, 0.7], ao: 1.0 },
            TerrainVertex { position: [ 0.5, -0.5, -0.5], normal: [1.0, 0.0, 0.0], color: [0.4, 0.35, 0.7], ao: 1.0 },
            // Left face (X-), gold
            TerrainVertex { position: [-0.5, -0.5, -0.5], normal: [-1.0, 0.0, 0.0], color: [0.75, 0.65, 0.3], ao: 1.0 },
            TerrainVertex { position: [-0.5,  0.5, -0.5], normal: [-1.0, 0.0, 0.0], color: [0.75, 0.65, 0.3], ao: 1.0 },
            TerrainVertex { position: [-0.5,  0.5,  0.5], normal: [-1.0, 0.0, 0.0], color: [0.75, 0.65, 0.3], ao: 1.0 },
            TerrainVertex { position: [-0.5, -0.5,  0.5], normal: [-1.0, 0.0, 0.0], color: [0.75, 0.65, 0.3], ao: 1.0 },
        ];

        #[rustfmt::skip]
        let indices: Vec<u32> = vec![
             0,  1,  2,   0,  2,  3, // top
             4,  5,  6,   4,  6,  7, // bottom
             8,  9, 10,   8, 10, 11, // front
            12, 13, 14,  12, 14, 15, // back
            16, 17, 18,  16, 18, 19, // right
            20, 21, 22,  20, 22, 23, // left
        ];

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("debug_cube_vertex_buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("debug_cube_index_buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        }
    }
}