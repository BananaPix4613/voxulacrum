use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::{Chunk, ChunkNeighbors, CHUNK_SIZE};
use crate::world::voxel::{Voxel, MATERIAL_TABLE, MAT_AIR};

use super::qef::QefSolver;

/// Result of meshing a single chunk.
pub struct ChunkMeshData {
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
}

/// Resolve a voxel reference at potentially out-of-bounds coordinates.
/// Returns None if coordinate requires an edge/corner neighbor or neighbor is missing.
fn resolve_voxel<'a>(
    chunk: &'a Chunk,
    neighbors: &'a ChunkNeighbors<'a>,
    x: i32, y: i32, z: i32,
) -> Option<&'a Voxel> {
    let cs = CHUNK_SIZE as i32;
    let (dx, lx) = if x < 0 { (-1, (x + cs) as usize) } else if x >= cs { (1, (x - cs) as usize) } else { (0, x as usize) };
    let (dy, ly) = if y < 0 { (-1, (y + cs) as usize) } else if y >= cs { (1, (y - cs) as usize) } else { (0, y as usize) };
    let (dz, lz) = if z < 0 { (-1, (z + cs) as usize) } else if z >= cs { (1, (z - cs) as usize) } else { (0, z as usize) };

    if dx == 0 && dy == 0 && dz == 0 {
        return Some(chunk.get_voxel(lx, ly, lz));
    }

    let neighbor = neighbors.get(dx, dy, dz)?;
    Some(neighbor.get_voxel(lx, ly, lz))
}

/// Sample density, resolving across chunk boundaries via neighbors.
fn sample_density(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> f32 {
    match resolve_voxel(chunk, neighbors, x, y, z) {
        Some(v) => v.density as f32,
        None => -1.0,
    }
}

/// Get material ID, resolving across chunk boundaries.
fn sample_material(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> u16 {
    match resolve_voxel(chunk, neighbors, x, y, z) {
        Some(v) => v.material,
        None => MAT_AIR,
    }
}

/// Compute density gradient via central differences at chunk-local integer coords.
fn density_gradient(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> [f32; 3] {
    let gx = (sample_density(chunk, neighbors, x + 1, y, z) - sample_density(chunk, neighbors, x - 1, y, z)) * 0.5;
    let gy = (sample_density(chunk, neighbors, x, y + 1, z) - sample_density(chunk, neighbors, x, y - 1, z)) * 0.5;
    let gz = (sample_density(chunk, neighbors, x, y, z + 1) - sample_density(chunk, neighbors, x, y, z - 1)) * 0.5;
    [gx, gy, gz]
}

/// Normalize a 3D vector. Returns zero vector if length is near zero.
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-8 {
        return [0.0, 1.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

/// Snap a normal to its nearest axis direction.
fn snap_to_nearest_axis(n: [f32; 3]) -> [f32; 3] {
    let ax = n[0].abs();
    let ay = n[1].abs();
    let az = n[2].abs();
    if ax >= ay && ax >= az {
        [n[0].signum(), 0.0, 0.0]
    } else if ay >= az {
        [0.0, n[1].signum(), 0.0]
    } else {
        [0.0, 0.0, n[2].signum()]
    }
}

/// Lerp between two 3D vectors.
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Compute ambient occlusion for a position by sampling nearby solid voxels.
fn compute_ao(chunk: &Chunk, neighbors: &ChunkNeighbors, pos: [f32; 3], chunk_offset: [f32; 3]) -> f32 {
    // Convert world position to chunk-local
    let lx = pos[0] - chunk_offset[0];
    let ly = pos[1] - chunk_offset[1];
    let lz = pos[2] - chunk_offset[2];

    // Sample a 3x3x3 neighborhood around the position
    let cx = lx.round() as i32;
    let cy = ly.round() as i32;
    let cz = lz.round() as i32;

    let mut solid_count = 0;

    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let sx = cx + dx;
                let sy = cy + dy;
                let sz = cz + dz;

                if sample_density(chunk, neighbors, sx, sy, sz) > 0.0 {
                    solid_count += 1;
                }
            }
        }
    }

    // AO: more nearby solid = more occlusion
    let occlusion = solid_count as f32 / 26.0;
    (1.0 - occlusion * 0.7).clamp(0.3, 1.0)
}

/// Generate mesh data for a single chunk using dual contouring.
pub fn mesh_chunk(chunk: &Chunk, neighbors: &ChunkNeighbors) -> ChunkMeshData {
    let chunk_offset = [
        chunk.position.x as f32 * CHUNK_SIZE as f32,
        chunk.position.y as f32 * CHUNK_SIZE as f32,
        chunk.position.z as f32 * CHUNK_SIZE as f32,
    ];

    // Phase 1: For each cell [0..CHUNK_SIZE)^3, detect sign-changing edges
    // and solve QEF to place a vertex.
    // Cell (cx, cy, cz) spans voxels [cx..cx+1, cy..cy+1, cz..cz+1].
    let cell_count = CHUNK_SIZE + 1;
    let total_cells = cell_count * cell_count * cell_count;

    // vertex_indices[cell_index] = index into vertices vec, or u32::MAX if no vertex
    let mut vertex_indices = vec![u32::MAX; total_cells];
    let mut vertices: Vec<TerrainVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // For each cell, check all 12 edges for sign changes and solve QEF.
    for cz in 0..cell_count {
        for cy in 0..cell_count {
            for cx in 0..cell_count {
                let cell_idx = cx + cy * cell_count + cz * cell_count * cell_count;

                // Sample the 8 corners of this cell
                let ix = cx as i32;
                let iy = cy as i32;
                let iz = cz as i32;

                let corners = [
                    sample_density(chunk, neighbors, ix, iy, iz),
                    sample_density(chunk, neighbors, ix + 1, iy, iz),
                    sample_density(chunk, neighbors, ix, iy + 1, iz),
                    sample_density(chunk, neighbors, ix + 1, iy + 1, iz),
                    sample_density(chunk, neighbors, ix, iy, iz + 1),
                    sample_density(chunk, neighbors, ix + 1, iy, iz + 1),
                    sample_density(chunk, neighbors, ix, iy + 1, iz + 1),
                    sample_density(chunk, neighbors, ix + 1, iy + 1, iz + 1),
                ];

                // Check all 12 edges for sign changes
                #[rustfmt::skip]
                let edges: [(usize, usize, [i32; 3], [i32; 3]); 12] = [
                    // X-aligned edges
                    (0, 1, [ix, iy, iz],         [ix + 1, iy, iz]),
                    (2, 3, [ix, iy + 1, iz],     [ix + 1, iy + 1, iz]),
                    (4, 5, [ix, iy, iz + 1],     [ix + 1, iy, iz + 1]),
                    (6, 7, [ix, iy + 1, iz + 1], [ix + 1, iy + 1, iz + 1]),
                    // Y-aligned edges
                    (0, 2, [ix, iy, iz],         [ix, iy + 1, iz]),
                    (1, 3, [ix + 1, iy, iz],     [ix + 1, iy + 1, iz]),
                    (4, 6, [ix, iy, iz + 1],     [ix, iy + 1, iz + 1]),
                    (5, 7, [ix + 1, iy, iz + 1], [ix + 1, iy + 1, iz + 1]),
                    // Z-aligned edges
                    (0, 4, [ix, iy, iz],         [ix, iy, iz + 1]),
                    (1, 5, [ix + 1, iy, iz],     [ix + 1, iy, iz + 1]),
                    (2, 6, [ix, iy + 1, iz],     [ix, iy + 1, iz + 1]),
                    (3, 7, [ix + 1, iy + 1, iz], [ix + 1, iy + 1, iz + 1]),
                ];

                let mut qef = QefSolver::new();
                let mut normal_sum = [0.0_f32; 3];
                let mut dominant_material = MAT_AIR;
                let mut max_solid_density = 0.0_f32;
                let mut has_crossing = false;

                for &(ci_a, ci_b, ref pa, ref pb) in &edges {
                    let da = corners[ci_a];
                    let db = corners[ci_b];

                    // Sign change: one positive (solid), one non-positive (air)
                    if (da > 0.0) == (db > 0.0) {
                        continue;
                    }

                    has_crossing = true;

                    // Intersection point via linear interpolation
                    let t = if (da - db).abs() < 1e-6 {
                        0.5
                    } else {
                        da / (da - db)
                    };
                    let intersection = [
                        pa[0] as f32 + (pb[0] - pa[0]) as f32 * t + chunk_offset[0],
                        pa[1] as f32 + (pb[1] - pa[1]) as f32 * t + chunk_offset[1],
                        pa[2] as f32 + (pb[2] - pa[2]) as f32 * t + chunk_offset[2],
                    ];

                    // Gradient at intersection (use nearest integer coordinate)
                    let grad_x = (pa[0] as f32 + (pb[0] - pa[0]) as f32 * t).round() as i32;
                    let grad_y = (pa[1] as f32 + (pb[1] - pa[1]) as f32 * t).round() as i32;
                    let grad_z = (pa[2] as f32 + (pb[2] - pa[2]) as f32 * t).round() as i32;
                    let raw_normal = normalize(density_gradient(chunk, neighbors, grad_x, grad_y, grad_z));

                    // Get dominant material (from the solid side)
                    let solid_pos = if da > 0.0 { pa } else { pb };
                    let mat = sample_material(chunk, neighbors, solid_pos[0], solid_pos[1], solid_pos[2]);
                    let solid_d = if da > 0.0 { da } else { db };
                    if mat != MAT_AIR && solid_d > max_solid_density {
                        max_solid_density = solid_d;
                        dominant_material = mat;
                    }

                    // Apply material sharpness bias
                    let sharpness = if (mat as usize) < MATERIAL_TABLE.len() {
                        MATERIAL_TABLE[mat as usize].sharpness
                    } else {
                        0.5
                    };
                    let axis_normal = snap_to_nearest_axis(raw_normal);
                    let biased_normal = normalize(lerp3(raw_normal, axis_normal, sharpness));

                    qef.add_plane(intersection, biased_normal);

                    normal_sum[0] += biased_normal[0];
                    normal_sum[1] += biased_normal[1];
                    normal_sum[2] += biased_normal[2];
                }

                if !has_crossing {
                    continue;
                }

                // Solve QEF
                let cell_min = [
                    cx as f32 + chunk_offset[0],
                    cy as f32 + chunk_offset[1],
                    cz as f32 + chunk_offset[2],
                ];
                let cell_max = [
                    cell_min[0] + 1.0,
                    cell_min[1] + 1.0,
                    cell_min[2] + 1.0,
                ];

                let (position, _error) = qef.solve(cell_min, cell_max);

                // Average normal
                let avg_normal = normalize(normal_sum);

                // Material color
                let color = if (dominant_material as usize) < MATERIAL_TABLE.len() {
                    MATERIAL_TABLE[dominant_material as usize].color
                } else {
                    [0.5, 0.5, 0.5]
                };

                // AO
                let ao = compute_ao(chunk, neighbors, position, chunk_offset);

                let vert_idx = vertices.len() as u32;
                vertex_indices[cell_idx] = vert_idx;
                vertices.push(TerrainVertex {
                    position,
                    normal: avg_normal,
                    color,
                    ao,
                });
            }
        }
    }

    // Phase 2: Generate faces (quads split into 2 triangles).
    // For each edge that has a sign change, connect the 4 cells sharing that edge.
    // X-edges are shared by cells at (x, cy, cz) where cy in {y-1, y}, cz in {z-1, z}
    // Y-edges are shared by cells at (cx, y, cz) where cx in {x-1, x}, cz in {z-1, z}
    // Z-edges are shared by cells at (cx, cy, z) where cx in {x-1, x}, cy in {y-1, y}
    for z in 0..=CHUNK_SIZE as i32 {
        for y in 0..=CHUNK_SIZE as i32 {
            for x in 0..=CHUNK_SIZE as i32 {
                let ix = x;
                let iy = y;
                let iz = z;
                let d0 = sample_density(chunk, neighbors, ix, iy, iz);

                // X-edge: between (x,y,z) and (x+1,y,z)
                if x + 1 <= CHUNK_SIZE as i32 {
                    let d1 = sample_density(chunk, neighbors, ix + 1, iy, iz);
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            (x as usize, (y - 1) as usize, (z - 1) as usize),
                            (x as usize, y as usize, (z - 1) as usize),
                            (x as usize, y as usize, z as usize),
                            (x as usize, (y - 1) as usize, z as usize),
                        ];
                        emit_quad(
                            &cells,
                            cell_count,
                            &vertex_indices,
                            &mut indices,
                            d0 > 0.0,
                        );
                    }
                }

                // Y-edge: between (x,y,z) and (x,y+1,z)
                if y + 1 <= CHUNK_SIZE as i32 {
                    let d1 = sample_density(chunk, neighbors, ix, iy + 1, iz);
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            ((x - 1) as usize, y as usize, (z - 1) as usize),
                            ((x - 1) as usize, y as usize, z as usize),
                            (x as usize, y as usize, z as usize),
                            (x as usize, y as usize, (z - 1) as usize),
                        ];
                        emit_quad(
                            &cells,
                            cell_count,
                            &vertex_indices,
                            &mut indices,
                            d0 > 0.0,
                        );
                    }
                }

                // Z-edge: between (x,y,z) and (x,y,z+1)
                if z + 1 <= CHUNK_SIZE as i32 {
                    let d1 = sample_density(chunk, neighbors, ix, iy, iz + 1);
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            ((x - 1) as usize, (y - 1) as usize, z as usize),
                            (x as usize, (y - 1) as usize, z as usize),
                            (x as usize, y as usize, z as usize),
                            ((x - 1) as usize, y as usize, z as usize),
                        ];
                        emit_quad(
                            &cells,
                            cell_count,
                            &vertex_indices,
                            &mut indices,
                            d0 > 0.0,
                        );
                    }
                }
            }
        }
    }

    ChunkMeshData { vertices, indices }
}

/// Emit a quad connecting 4 cells that share a sign-changing edge.
/// `cells` contains 4 (cx, cy, cz) tuples in the cell grid.
/// `solid_first` indicates whether the first endpoint of the edge is solid,
/// which determines winding order.
fn emit_quad(
    cells: &[(usize, usize, usize); 4],
    cell_count: usize,
    vertex_indices: &[u32],
    indices: &mut Vec<u32>,
    solid_first: bool,
) {
    // Validate all cells are in bounds and have vertices
    let mut vis = [0u32; 4];
    for (i, &(cx, cy, cz)) in cells.iter().enumerate() {
        if cx >= cell_count || cy >= cell_count || cz >= cell_count {
            return;
        }
        let idx = cx + cy * cell_count + cz * cell_count * cell_count;
        let vi = vertex_indices[idx];
        if vi == u32::MAX {
            return;
        }
        vis[i] = vi;
    }

    // Emit two triangles for the quad.
    // Vertices are in ring order [0,1,2,3]. Split along diagonal 0-2.
    // Triangles: 0-1-2 and 2-3-0
    // Winding depends on which side of the edge is solid.
    if solid_first {
        indices.push(vis[0]);
        indices.push(vis[1]);
        indices.push(vis[2]);

        indices.push(vis[2]);
        indices.push(vis[3]);
        indices.push(vis[0]);
    } else {
        // Reversed winding
        indices.push(vis[2]);
        indices.push(vis[1]);
        indices.push(vis[0]);

        indices.push(vis[0]);
        indices.push(vis[3]);
        indices.push(vis[2]);
    }
}