pub mod voxel;
pub mod chunk;
pub mod generation;

use chunk::{Chunk, ChunkMesh, ChunkNeighbors, CHUNK_VOLUME};
use generation::{TerrainGenerator, WORLD_CHUNKS_X, WORLD_CHUNKS_Y, WORLD_CHUNKS_Z};
use voxel::{MATERIAL_COUNT, MATERIAL_TABLE};
use wgpu::util::DeviceExt;

use crate::meshing::dual_contouring;

pub struct World {
    pub chunks: Vec<Chunk>,
    pub chunks_x: usize,
    pub chunks_y: usize,
    pub chunks_z: usize,
    pub generator: TerrainGenerator,
}

use crate::params::TerrainGenParams;

impl World {
    pub fn generate(params: &TerrainGenParams) -> Self {
        let generator = TerrainGenerator::new(params);
        let chunks = generator.generate_world(params);

        Self {
            chunks,
            chunks_x: WORLD_CHUNKS_X,
            chunks_y: WORLD_CHUNKS_Y,
            chunks_z: WORLD_CHUNKS_Z,
            generator,
        }
    }

    /// Mesh all chunks and upload to GPU.
    pub fn mesh_all_chunks(&mut self, device: &wgpu::Device) {
        use crate::meshing::dual_contouring::{
            generate_cell_vertices, generate_faces,
            BoundaryVertexMap, CellVertexData, NeighborBoundaries,
        };

        let mut total_vertices: u64 = 0;
        let mut total_indices: u64 = 0;
        let mut meshed_count: u32 = 0;

        // Pass 1: Generate cell vertices for all chunks (immutable borrow of self)
        let mut cell_data_vec: Vec<CellVertexData> = (0..self.chunks.len())
            .map(|i| {
                let chunk = &self.chunks[i];
                let cx = chunk.position.x as usize;
                let cy = chunk.position.y as usize;
                let cz = chunk.position.z as usize;
                let neighbors = self.build_neighbors(cx, cy, cz);
                generate_cell_vertices(chunk, &neighbors)
            })
            .collect();

        // Extract boundary maps for cross-referencing
        let boundary_maps: Vec<BoundaryVertexMap> = cell_data_vec.iter()
            .map(|cd| cd.boundary_map.clone())
            .collect();

        // Pass 2a: Generate faces with stitched boundaries
        let mut face_indices: Vec<Vec<u32>> = Vec::with_capacity(self.chunks.len());
        for i in 0..self.chunks.len() {
            let chunk = &self.chunks[i];
            let cx = chunk.position.x as usize;
            let cy = chunk.position.y as usize;
            let cz = chunk.position.z as usize;
            let neighbors = self.build_neighbors(cx, cy, cz);

            let mut nb = NeighborBoundaries::empty();
            for dz in 0u8..=1 {
                for dy in 0u8..=1 {
                    for dx in 0u8..=1 {
                        if dx == 0 && dy == 0 && dz == 0 { continue; }
                        let nx = cx + dx as usize;
                        let ny = cy + dy as usize;
                        let nz = cz + dz as usize;
                        if nx < self.chunks_x && ny < self.chunks_y && nz < self.chunks_z {
                            let ni = nx + ny * self.chunks_x + nz * self.chunks_x * self.chunks_y;
                            let map_idx = dx as usize + (dy as usize) * 2 + (dz as usize) * 4;
                            nb.maps[map_idx] = Some(&boundary_maps[ni]);
                        }
                    }
                }
            }

            let indices = generate_faces(&mut cell_data_vec[i], chunk, &neighbors, &nb);
            face_indices.push(indices);
        }

        // Pass 2b: Upload to GPU (mutable borrow of self.chunks)
        for (i, indices) in face_indices.into_iter().enumerate() {
            let cell_data = &cell_data_vec[i];

            if cell_data.vertices.is_empty() || indices.is_empty() {
                self.chunks[i].mesh = None;
                self.chunks[i].mesh_dirty = false;
                continue;
            }

            total_vertices += cell_data.vertices.len() as u64;
            total_indices += indices.len() as u64;
            meshed_count += 1;

            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk_vertex_buffer"),
                contents: bytemuck::cast_slice(&cell_data.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk_index_buffer"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });

            self.chunks[i].mesh = Some(ChunkMesh {
                vertex_buffer,
                index_buffer,
                index_count: indices.len() as u32,
            });
            self.chunks[i].mesh_dirty = false;
        }

        log::info!(
            "Meshed {} chunks: {} vertices, {} indices ({} triangles)",
            meshed_count, total_vertices, total_indices, total_indices / 3
        );
    }

    fn build_neighbors(&self, cx: usize, cy: usize, cz: usize) -> ChunkNeighbors {
        let mut neighbors = ChunkNeighbors::empty();
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 && dz == 0 { continue; }
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    let nz = cz as i32 + dz;
                    if nx >= 0 && (nx as usize) < self.chunks_x
                        && ny >= 0 && (ny as usize) < self.chunks_y
                        && nz >= 0 && (nz as usize) < self.chunks_z
                    {
                        neighbors.set(dx, dy, dz, self.get_chunk(nx as usize, ny as usize, nz as usize));
                    }
                }
            }
        }
        neighbors
    }

    /// Get a chunk by its chunk-space coordinates, or None if out of bounds.
    pub fn get_chunk(&self, cx: usize, cy: usize, cz: usize) -> Option<&Chunk> {
        if cx >= self.chunks_x || cy >= self.chunks_y || cz >= self.chunks_z {
            return None;
        }
        let index = cx + cy * self.chunks_x + cz * self.chunks_x * self.chunks_y;
        self.chunks.get(index)
    }

    /// Get a mutable chunk by its chunk-space coordinates.
    pub fn get_chunk_mut(&mut self, cx: usize, cy: usize, cz: usize) -> Option<&mut Chunk> {
        if cx >= self.chunks_x || cy >= self.chunks_y || cz >= self.chunks_z {
            return None;
        }
        let index = cx + cy * self.chunks_x + cz * self.chunks_x * self.chunks_y;
        self.chunks.get_mut(index)
    }

    /// Print debug statistics about the generated world.
    pub fn print_debug_stats(&self) {
        let mut total_solid: u64 = 0;
        let mut total_air: u64 = 0;
        let mut material_counts = [0u64; MATERIAL_COUNT];
        let mut min_density: i8 = i8::MAX;
        let mut max_density: i8 = i8::MIN;
        let mut flora_count: u64 = 0;

        for chunk in &self.chunks {
            for voxel in chunk.voxels.iter() {
                if voxel.density > 0 {
                    total_solid += 1;
                } else {
                    total_air += 1;
                }
                let mat = voxel.material as usize;
                if mat < MATERIAL_COUNT {
                    material_counts[mat] += 1;
                }
                if voxel.density < min_density {
                    min_density = voxel.density;
                }
                if voxel.density > max_density {
                    max_density = voxel.density;
                }
                if voxel.flora_id != 0 {
                    flora_count += 1;
                }
            }
        }

        let total = total_solid + total_air;
        log::info!("=== World Generation Stats ===");
        log::info!("Chunks: {}", self.chunks.len());
        log::info!(
            "Total voxels: {} ({} solid, {} air)",
            total,
            total_solid,
            total_air
        );
        log::info!(
            "Solid: {:.1}%, Air: {:.1}%",
            total_solid as f64 / total as f64 * 100.0,
            total_air as f64 / total as f64 * 100.0
        );
        log::info!("Density range: {} to {}", min_density, max_density);
        log::info!("Flora voxels: {}", flora_count);
        log::info!("--- Material distribution ---");
        for (id, count) in material_counts.iter().enumerate() {
            if *count > 0 {
                log::info!(
                    "  [{}] {}: {} ({:.1}%)",
                    id,
                    MATERIAL_TABLE[id].name,
                    count,
                    *count as f64 / total as f64 * 100.0
                );
            }
        }
        log::info!(
            "Voxel memory: ~{} MB",
            self.chunks.len() * CHUNK_VOLUME * 12 / (1024 * 1024)
        );
        log::info!("==============================")
    }
}