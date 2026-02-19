use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::{Chunk, ChunkNeighbors, ChunkSnapshot, CHUNK_SIZE, CHUNK_WORLD_SIZE, VOXEL_SCALE};
use crate::world::voxel::{Voxel, MAT_AIR};
use std::collections::HashMap;

use super::qef::QefSolver;
use super::MaterialConfig;

/// Maxmimum sharpness bias applied to QEF constraint normals.
/// Higher values distort vertex placement on slopes. The full
/// material sharpness is still applied to the display normal
/// for visual edge sharpening.
const QEF_MAX_SHARPNESS: f32 = 0.3;

/// Map from (cx, cy, cz) local cell coords to the TerrainVertex for boundary cells.
pub type BoundaryVertexMap = HashMap<(u8, u8, u8), TerrainVertex>;

/// Intermediate result from Phase 1 (vertex generation).
pub struct CellVertexData {
    pub vertices: Vec<TerrainVertex>,
    pub vertex_indices: Vec<u32>,  // cell_count^3 entries
    pub boundary_map: BoundaryVertexMap,
}

/// Result of meshing a single chunk.
pub struct ChunkMeshData {
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
}

/// Boundary vertex maps from the +X, +Y, +Z (and diagonal) neighbors.
/// Indexed as dx + dy*2 + dz*4, where dx/dy/dz are 0 or 1.
/// Index 0 is unused (self). Indices 1..=7 are the 7 positive-direction neighbors.
pub struct NeighborBoundaries<'a> {
    pub maps: [Option<&'a BoundaryVertexMap>; 8],
}

impl<'a> NeighborBoundaries<'a> {
    pub fn empty() -> Self {
        Self { maps: [None; 8] }
    }
}

/// Owned version of NeighborBoundaries for sending to worker threads.
pub struct OwnedNeighborBoundaries {
    pub maps: [Option<BoundaryVertexMap>; 8],
}

impl OwnedNeighborBoundaries {
    pub fn empty() -> Self {
        Self {
            maps: [None, None, None, None, None, None, None, None],
        }
    }

    pub fn as_ref(&self) -> NeighborBoundaries {
        NeighborBoundaries {
            maps: [
                self.maps[0].as_ref(),
                self.maps[1].as_ref(),
                self.maps[2].as_ref(),
                self.maps[3].as_ref(),
                self.maps[4].as_ref(),
                self.maps[5].as_ref(),
                self.maps[6].as_ref(),
                self.maps[7].as_ref(),
            ],
        }
    }
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

/// Get ambient occlusion density, resolving across chunk boundaries.
fn sample_ao_density(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> f32 {
    let cs = CHUNK_SIZE as i32;
    let cx = x.clamp(-cs, 2 * cs - 1);
    let cy = y.clamp(-cs, 2 * cs - 1);
    let cz = z.clamp(-cs, 2 * cs - 1);
    match resolve_voxel(chunk, neighbors, cx, cy, cz) {
        Some(v) => v.density as f32,
        None => 0.0,  // Missing neighbor: treat as air (no artificial occlusion)
    }
}

/// Compute density gradient via central differences at chunk-local integer coords.
fn density_gradient(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> [f32; 3] {
    let gx = (sample_density(chunk, neighbors, x + 1, y, z) - sample_density(chunk, neighbors, x - 1, y, z)) * 0.5;
    let gy = (sample_density(chunk, neighbors, x, y + 1, z) - sample_density(chunk, neighbors, x, y - 1, z)) * 0.5;
    let gz = (sample_density(chunk, neighbors, x, y, z + 1) - sample_density(chunk, neighbors, x, y, z - 1)) * 0.5;
    [-gx, -gy, -gz]  // Negate: gradient points INTO surface, normals should point OUT
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
/// Uses distance-weighted 5x5x5 sampling for smoother results.
fn compute_ao(chunk: &Chunk, neighbors: &ChunkNeighbors, pos: [f32; 3], chunk_offset: [f32; 3]) -> f32 {
    // Convert world position to voxel-local indices
    let lx = (pos[0] - chunk_offset[0]) / VOXEL_SCALE;
    let ly = (pos[1] - chunk_offset[1]) / VOXEL_SCALE;
    let lz = (pos[2] - chunk_offset[2]) / VOXEL_SCALE;

    let cx = lx.round() as i32;
    let cy = ly.round() as i32;
    let cz = lz.round() as i32;

    let radius: i32 = 2;
    let mut weighted_solid = 0.0_f32;
    let mut total_weight = 0.0_f32;

    for dz in -radius..=radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let dist_sq = (dx * dx + dy * dy + dz * dz) as f32;
                let weight = 1.0 / dist_sq;
                total_weight += weight;

                if sample_ao_density(chunk, neighbors, cx + dx, cy + dy, cz + dz) > 0.0 {
                    weighted_solid += weight;
                }
            }
        }
    }

    let occlusion = weighted_solid / total_weight;
    (1.0 - occlusion * 0.5).clamp(0.5, 1.0)
}

// ============================================================================
// Snapshot-based sampling functions (for threaded meshing)
// ============================================================================

fn snap_density(snap: &ChunkSnapshot, x: i32, y: i32, z: i32) -> f32 {
    let cs = CHUNK_SIZE as i32;
    if x < -2 || x > cs + 1 || y < -2 || y > cs + 1 || z < -2 || z > cs + 1 {
        return -1.0;
    }
    snap.get_voxel(x, y, z).density as f32
}

fn snap_material(snap: &ChunkSnapshot, x: i32, y: i32, z: i32) -> u16 {
    let cs = CHUNK_SIZE as i32;
    if x < -2 || x > cs + 1 || y < -2 || y > cs + 1 || z < -2 || z > cs + 1 {
        return MAT_AIR;
    }
    snap.get_voxel(x, y, z).material
}

fn snap_ao_density(snap: &ChunkSnapshot, x: i32, y: i32, z: i32) -> f32 {
    let cs = CHUNK_SIZE as i32;
    let cx = x.clamp(-2, cs + 1);
    let cy = y.clamp(-2, cs + 1);
    let cz = z.clamp(-2, cs + 1);
    snap.get_voxel(cx, cy, cz).density as f32
}

/// Classify density for sign-change detection. Treats exact zero as
/// slightly negative (air) to avoid ambiguous crossings at chunk
/// boundaries where missing neighbors default to density=0.
#[inline]
fn classify_density(d: f32) -> f32 {
    if d == 0.0 { -0.5 } else { d }
}

fn snap_gradient(snap: &ChunkSnapshot, x: i32, y: i32, z: i32) -> [f32; 3] {
    let gx = (snap_density(snap, x + 1, y, z) - snap_density(snap, x - 1, y, z)) * 0.5;
    let gy = (snap_density(snap, x, y + 1, z) - snap_density(snap, x, y - 1, z)) * 0.5;
    let gz = (snap_density(snap, x, y, z + 1) - snap_density(snap, x, y, z - 1)) * 0.5;
    [-gx, -gy, -gz]
}

fn snap_compute_ao(snap: &ChunkSnapshot, pos: [f32; 3], chunk_offset: [f32; 3]) -> f32 {
    let lx = (pos[0] - chunk_offset[0]) / VOXEL_SCALE;
    let ly = (pos[1] - chunk_offset[1]) / VOXEL_SCALE;
    let lz = (pos[2] - chunk_offset[2]) / VOXEL_SCALE;

    let cx = lx.round() as i32;
    let cy = ly.round() as i32;
    let cz = lz.round() as i32;

    let radius: i32 = 2;
    let mut weighted_solid = 0.0_f32;
    let mut total_weight = 0.0_f32;

    for dz in -radius..=radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let dist_sq = (dx * dx + dy * dy + dz * dz) as f32;
                let weight = 1.0 / dist_sq;
                total_weight += weight;

                if snap_ao_density(snap, cx + dx, cy + dy, cz + dz) > 0.0 {
                    weighted_solid += weight;
                }
            }
        }
    }

    let occlusion = weighted_solid / total_weight;
    (1.0 - occlusion * 0.5).clamp(0.5, 1.0)
}

/// Generate cell vertices from a ChunkSnapshot. Phase 1 worker function.
/// Produces identical output to generate_cell_vertices() but operates on
/// owned snapshot data instead of borrowed chunk + neighbors.
pub fn generate_cell_vertices_from_snapshot(snap: &ChunkSnapshot, materials: &MaterialConfig) -> CellVertexData {
    let chunk_offset = [
        snap.position.x as f32 * CHUNK_WORLD_SIZE,
        snap.position.y as f32 * CHUNK_WORLD_SIZE,
        snap.position.z as f32 * CHUNK_WORLD_SIZE,
    ];

    let stride = CHUNK_SIZE + 1;
    let total_cells = stride * stride * stride;
    let mut vertex_indices = vec![u32::MAX; total_cells];
    let mut vertices: Vec<TerrainVertex> = Vec::new();
    let mut boundary_map = BoundaryVertexMap::new();

    for cz in 0..CHUNK_SIZE {
        for cy in 0..CHUNK_SIZE {
            for cx in 0..CHUNK_SIZE {
                let cell_idx = cx + cy * stride + cz * stride * stride;

                let ix = cx as i32;
                let iy = cy as i32;
                let iz = cz as i32;

                let corners = [
                    classify_density(snap_density(snap, ix, iy, iz)),
                    classify_density(snap_density(snap, ix + 1, iy, iz)),
                    classify_density(snap_density(snap, ix, iy + 1, iz)),
                    classify_density(snap_density(snap, ix + 1, iy + 1, iz)),
                    classify_density(snap_density(snap, ix, iy, iz + 1)),
                    classify_density(snap_density(snap, ix + 1, iy, iz + 1)),
                    classify_density(snap_density(snap, ix, iy + 1, iz + 1)),
                    classify_density(snap_density(snap, ix + 1, iy + 1, iz + 1)),
                ];

                #[rustfmt::skip]
                let edges: [(usize, usize, [i32; 3], [i32; 3]); 12] = [
                    (0, 1, [ix, iy, iz],         [ix + 1, iy, iz]),
                    (2, 3, [ix, iy + 1, iz],     [ix + 1, iy + 1, iz]),
                    (4, 5, [ix, iy, iz + 1],     [ix + 1, iy, iz + 1]),
                    (6, 7, [ix, iy + 1, iz + 1], [ix + 1, iy + 1, iz + 1]),
                    (0, 2, [ix, iy, iz],         [ix, iy + 1, iz]),
                    (1, 3, [ix + 1, iy, iz],     [ix + 1, iy + 1, iz]),
                    (4, 6, [ix, iy, iz + 1],     [ix, iy + 1, iz + 1]),
                    (5, 7, [ix + 1, iy, iz + 1], [ix + 1, iy + 1, iz + 1]),
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

                    if (da > 0.0) == (db > 0.0) {
                        continue;
                    }

                    has_crossing = true;

                    let t = if (da - db).abs() < 1e-6 {
                        0.5
                    } else {
                        da / (da - db)
                    };
                    let intersection = [
                        pa[0] as f32 * VOXEL_SCALE + (pb[0] - pa[0]) as f32 * VOXEL_SCALE * t + chunk_offset[0],
                        pa[1] as f32 * VOXEL_SCALE + (pb[1] - pa[1]) as f32 * VOXEL_SCALE * t + chunk_offset[1],
                        pa[2] as f32 * VOXEL_SCALE + (pb[2] - pa[2]) as f32 * VOXEL_SCALE * t + chunk_offset[2],
                    ];

                    let grad_x = (pa[0] as f32 + (pb[0] - pa[0]) as f32 * t).round() as i32;
                    let grad_y = (pa[1] as f32 + (pb[1] - pa[1]) as f32 * t).round() as i32;
                    let grad_z = (pa[2] as f32 + (pb[2] - pa[2]) as f32 * t).round() as i32;
                    let raw_normal = normalize(snap_gradient(snap, grad_x, grad_y, grad_z));

                    let solid_pos = if da > 0.0 { pa } else { pb };
                    let mat = snap_material(snap, solid_pos[0], solid_pos[1], solid_pos[2]);
                    let solid_d = if da > 0.0 { da } else { db };
                    if mat != MAT_AIR && solid_d > max_solid_density {
                        max_solid_density = solid_d;
                        dominant_material = mat;
                    }

                    let sharpness = if (mat as usize) < materials.sharpness.len() {
                        materials.sharpness[mat as usize]
                    } else {
                        0.5
                    };
                    let axis_normal = snap_to_nearest_axis(raw_normal);

                    // QEF constraint normal: cap sharpness to avoid distorting vertex placement
                    let qef_sharpness = sharpness.min(QEF_MAX_SHARPNESS);
                    let qef_normal = normalize(lerp3(raw_normal, axis_normal, qef_sharpness));

                    // Display normal: full sharpness for visual edge sharpening
                    let display_normal = normalize(lerp3(raw_normal, axis_normal, sharpness));

                    qef.add_plane(intersection, qef_normal);

                    normal_sum[0] += display_normal[0];
                    normal_sum[1] += display_normal[1];
                    normal_sum[2] += display_normal[2];
                }

                if !has_crossing {
                    continue;
                }

                let cell_min = [
                    cx as f32 * VOXEL_SCALE + chunk_offset[0],
                    cy as f32 * VOXEL_SCALE + chunk_offset[1],
                    cz as f32 * VOXEL_SCALE + chunk_offset[2],
                ];
                let cell_max = [
                    cell_min[0] + VOXEL_SCALE,
                    cell_min[1] + VOXEL_SCALE,
                    cell_min[2] + VOXEL_SCALE,
                ];

                let (position, _error) = qef.solve(cell_min, cell_max);

                let avg_normal = normalize(normal_sum);

                let base_color = if (dominant_material as usize) < materials.colors.len() {
                    materials.colors[dominant_material as usize]
                } else {
                    [0.5, 0.5, 0.5]
                };
                let color = base_color;

                let ao = snap_compute_ao(snap, position, chunk_offset);

                let vert_idx = vertices.len() as u32;
                vertex_indices[cell_idx] = vert_idx;
                let vertex = TerrainVertex {
                    position,
                    normal: avg_normal,
                    color,
                    ao,
                    material_id: dominant_material as u32,
                    _pad_vert: [0; 3],
                };

                if cx == 0 || cy == 0 || cz == 0 {
                    boundary_map.insert((cx as u8, cy as u8, cz as u8), vertex);
                }

                vertices.push(vertex);
            }
        }
    }

    CellVertexData { vertices, vertex_indices, boundary_map }
}

/// Generate faces from snapshot + cell data + neighbor boundaries.
/// Phase 2 worker function. Produces identical output to generate_faces().
pub fn generate_faces_from_snapshot(
    cell_data: &mut CellVertexData,
    snap: &ChunkSnapshot,
    neighbor_boundaries: &NeighborBoundaries,
) -> Vec<u32> {
    let stride = CHUNK_SIZE + 1;

    // Import boundary vertices from +X, +Y, +Z (and diagonal) neighbors.
    for (map_idx, map_opt) in neighbor_boundaries.maps.iter().enumerate() {
        let Some(bmap) = map_opt else { continue };
        if map_idx == 0 { continue; }

        let dx = map_idx & 1;
        let dy = (map_idx >> 1) & 1;
        let dz = (map_idx >> 2) & 1;

        for (&(bcx, bcy, bcz), vertex) in bmap.iter() {
            let matches = (dx == 0 || bcx == 0)
                && (dy == 0 || bcy == 0)
                && (dz == 0 || bcz == 0);
            if !matches { continue; }

            let local_cx = if dx == 1 { CHUNK_SIZE } else { bcx as usize };
            let local_cy = if dy == 1 { CHUNK_SIZE } else { bcy as usize };
            let local_cz = if dz == 1 { CHUNK_SIZE } else { bcz as usize };

            let cell_idx = local_cx + local_cy * stride + local_cz * stride * stride;

            if cell_data.vertex_indices[cell_idx] == u32::MAX {
                let vert_idx = cell_data.vertices.len() as u32;
                cell_data.vertex_indices[cell_idx] = vert_idx;
                cell_data.vertices.push(*vertex);
            }
        }
    }

    let mut indices: Vec<u32> = Vec::new();

    for z in 0..=CHUNK_SIZE as i32 {
        for y in 0..=CHUNK_SIZE as i32 {
            for x in 0..=CHUNK_SIZE as i32 {
                let d0 = classify_density(snap_density(snap, x, y, z));

                // X-edge
                if x + 1 <= CHUNK_SIZE as i32 {
                    let d1 = classify_density(snap_density(snap, x + 1, y, z));
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            (x as usize, (y - 1) as usize, (z - 1) as usize),
                            (x as usize, y as usize, (z - 1) as usize),
                            (x as usize, y as usize, z as usize),
                            (x as usize, (y - 1) as usize, z as usize),
                        ];
                        emit_quad(&cells, stride, &cell_data.vertex_indices, &mut indices, d0 > 0.0);
                    }
                }

                // Y-edge
                if y + 1 <= CHUNK_SIZE as i32 {
                    let d1 = classify_density(snap_density(snap, x, y + 1, z));
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            ((x - 1) as usize, y as usize, (z - 1) as usize),
                            ((x - 1) as usize, y as usize, z as usize),
                            (x as usize, y as usize, z as usize),
                            (x as usize, y as usize, (z - 1) as usize),
                        ];
                        emit_quad(&cells, stride, &cell_data.vertex_indices, &mut indices, d0 > 0.0);
                    }
                }

                // Z-edge
                if z + 1 <= CHUNK_SIZE as i32 {
                    let d1 = classify_density(snap_density(snap, x, y, z + 1));
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            ((x - 1) as usize, (y - 1) as usize, z as usize),
                            (x as usize, (y - 1) as usize, z as usize),
                            (x as usize, y as usize, z as usize),
                            ((x - 1) as usize, y as usize, z as usize),
                        ];
                        emit_quad(&cells, stride, &cell_data.vertex_indices, &mut indices, d0 > 0.0);
                    }
                }
            }
        }
    }

    indices
}

/// Generate mesh data for a single chunk using dual contouring.
pub fn mesh_chunk(chunk: &Chunk, neighbors: &ChunkNeighbors, materials: &MaterialConfig) -> ChunkMeshData {
    let mut cell_data = generate_cell_vertices(chunk, neighbors, materials);
    let nb = NeighborBoundaries::empty();
    let indices = generate_faces(&mut cell_data, chunk, neighbors, &nb);
    ChunkMeshData { vertices: cell_data.vertices, indices }
}

pub fn generate_cell_vertices(chunk: &Chunk, neighbors: &ChunkNeighbors, materials: &MaterialConfig) -> CellVertexData {
    let chunk_offset = [
        chunk.position.x as f32 * CHUNK_WORLD_SIZE,
        chunk.position.y as f32 * CHUNK_WORLD_SIZE,
        chunk.position.z as f32 * CHUNK_WORLD_SIZE,
    ];

    // Use stride CHUNK_SIZE+1 so indices 0..CHUNK_SIZE are for owned cells,
    // and index CHUNK_SIZE is reserved for imported boundary vertices.
    let stride = CHUNK_SIZE + 1;
    let total_cells = stride * stride * stride;
    let mut vertex_indices = vec![u32::MAX; total_cells];
    let mut vertices: Vec<TerrainVertex> = Vec::new();
    let mut boundary_map = BoundaryVertexMap::new();

    for cz in 0..CHUNK_SIZE {
        for cy in 0..CHUNK_SIZE {
            for cx in 0..CHUNK_SIZE {
                let cell_idx = cx + cy * stride + cz * stride * stride;

                // Sample the 8 corners of this cell
                let ix = cx as i32;
                let iy = cy as i32;
                let iz = cz as i32;

                let corners = [
                    classify_density(sample_density(chunk, neighbors, ix, iy, iz)),
                    classify_density(sample_density(chunk, neighbors, ix + 1, iy, iz)),
                    classify_density(sample_density(chunk, neighbors, ix, iy + 1, iz)),
                    classify_density(sample_density(chunk, neighbors, ix + 1, iy + 1, iz)),
                    classify_density(sample_density(chunk, neighbors, ix, iy, iz + 1)),
                    classify_density(sample_density(chunk, neighbors, ix + 1, iy, iz + 1)),
                    classify_density(sample_density(chunk, neighbors, ix, iy + 1, iz + 1)),
                    classify_density(sample_density(chunk, neighbors, ix + 1, iy + 1, iz + 1)),
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

                    // Intersection point via linear interpolation (in world space)
                    let t = if (da - db).abs() < 1e-6 {
                        0.5
                    } else {
                        da / (da - db)
                    };
                    let intersection = [
                        pa[0] as f32 * VOXEL_SCALE + (pb[0] - pa[0]) as f32 * VOXEL_SCALE * t + chunk_offset[0],
                        pa[1] as f32 * VOXEL_SCALE + (pb[1] - pa[1]) as f32 * VOXEL_SCALE * t + chunk_offset[1],
                        pa[2] as f32 * VOXEL_SCALE + (pb[2] - pa[2]) as f32 * VOXEL_SCALE * t + chunk_offset[2],
                    ];

                    // Gradient at intersection (use nearest voxel index)
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
                    let sharpness = if (mat as usize) < materials.sharpness.len() {
                        materials.sharpness[mat as usize]
                    } else {
                        0.5
                    };
                    let axis_normal = snap_to_nearest_axis(raw_normal);

                    // QEF constraint normal: cap sharpness to avoid distorting vertex placement
                    let qef_sharpness = sharpness.min(QEF_MAX_SHARPNESS);
                    let qef_normal = normalize(lerp3(raw_normal, axis_normal, qef_sharpness));

                    // Display normal: full sharpness for visual edge sharpening
                    let display_normal = normalize(lerp3(raw_normal, axis_normal, sharpness));

                    qef.add_plane(intersection, qef_normal);

                    normal_sum[0] += display_normal[0];
                    normal_sum[1] += display_normal[1];
                    normal_sum[2] += display_normal[2];
                }

                if !has_crossing {
                    continue;
                }

                // Solve QEF - cell bounds in world space
                let cell_min = [
                    cx as f32 * VOXEL_SCALE + chunk_offset[0],
                    cy as f32 * VOXEL_SCALE + chunk_offset[1],
                    cz as f32 * VOXEL_SCALE + chunk_offset[2],
                ];
                let cell_max = [
                    cell_min[0] + VOXEL_SCALE,
                    cell_min[1] + VOXEL_SCALE,
                    cell_min[2] + VOXEL_SCALE,
                ];

                let (position, _error) = qef.solve(cell_min, cell_max);

                // Average normal
                let avg_normal = normalize(normal_sum);

                // Material color with per-vertex variation
                let base_color = if (dominant_material as usize) < materials.colors.len() {
                    materials.colors[dominant_material as usize]
                } else {
                    [0.5, 0.5, 0.5]
                };
                let color = base_color;

                // AO
                let ao = compute_ao(chunk, neighbors, position, chunk_offset);

                let vert_idx = vertices.len() as u32;
                vertex_indices[cell_idx] = vert_idx;
                let vertex = TerrainVertex {
                    position,
                    normal: avg_normal,
                    color,
                    ao,
                    material_id: dominant_material as u32,
                    _pad_vert: [0; 3],
                };

                // Record boundary vertices (cells at the low face of the chunk)
                if cx == 0 || cy == 0 || cz == 0 {
                    boundary_map.insert((cx as u8, cy as u8, cz as u8), vertex);
                }

                vertices.push(vertex);
            }
        }
    }

    CellVertexData { vertices, vertex_indices, boundary_map }
}

pub fn generate_faces(
    cell_data: &mut CellVertexData,
    chunk: &Chunk,
    neighbors: &ChunkNeighbors,
    neighbor_boundaries: &NeighborBoundaries,
) -> Vec<u32> {
    let stride = CHUNK_SIZE + 1;

    // Import boundary vertices from +X, +Y, +Z (and diagonal) neighbors.
    for (map_idx, map_opt) in neighbor_boundaries.maps.iter().enumerate() {
        let Some(bmap) = map_opt else { continue };
        if map_idx == 0 { continue; }

        let dx = map_idx & 1;
        let dy = (map_idx >> 1) & 1;
        let dz = (map_idx >> 2) & 1;

        for (&(bcx, bcy, bcz), vertex) in bmap.iter() {
            let matches = (dx == 0 || bcx == 0)
                && (dy == 0 || bcy == 0)
                && (dz == 0 || bcz == 0);
            if !matches { continue; }

            let local_cx = if dx == 1 { CHUNK_SIZE } else { bcx as usize };
            let local_cy = if dy == 1 { CHUNK_SIZE } else { bcy as usize };
            let local_cz = if dz == 1 { CHUNK_SIZE } else { bcz as usize };

            let cell_idx = local_cx + local_cy * stride + local_cz * stride * stride;

            if cell_data.vertex_indices[cell_idx] == u32::MAX {
                let vert_idx = cell_data.vertices.len() as u32;
                cell_data.vertex_indices[cell_idx] = vert_idx;
                cell_data.vertices.push(*vertex);
            }
        }
    }

    let mut indices: Vec<u32> = Vec::new();

    for z in 0..=CHUNK_SIZE as i32 {
        for y in 0..=CHUNK_SIZE as i32 {
            for x in 0..=CHUNK_SIZE as i32 {
                let d0 = classify_density(sample_density(chunk, neighbors, x, y, z));

                // X-edge
                if x + 1 <= CHUNK_SIZE as i32 {
                    let d1 = classify_density(sample_density(chunk, neighbors, x + 1, y, z));
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            (x as usize, (y - 1) as usize, (z - 1) as usize),
                            (x as usize, y as usize, (z - 1) as usize),
                            (x as usize, y as usize, z as usize),
                            (x as usize, (y - 1) as usize, z as usize),
                        ];
                        emit_quad(&cells, stride, &cell_data.vertex_indices, &mut indices, d0 > 0.0);
                    }
                }

                // Y-edge
                if y + 1 <= CHUNK_SIZE as i32 {
                    let d1 = classify_density(sample_density(chunk, neighbors, x, y + 1, z));
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            ((x - 1) as usize, y as usize, (z - 1) as usize),
                            ((x - 1) as usize, y as usize, z as usize),
                            (x as usize, y as usize, z as usize),
                            (x as usize, y as usize, (z - 1) as usize),
                        ];
                        emit_quad(&cells, stride, &cell_data.vertex_indices, &mut indices, d0 > 0.0);
                    }
                }

                // Z-edge
                if z + 1 <= CHUNK_SIZE as i32 {
                    let d1 = classify_density(sample_density(chunk, neighbors, x, y, z + 1));
                    if (d0 > 0.0) != (d1 > 0.0) {
                        let cells = [
                            ((x - 1) as usize, (y - 1) as usize, z as usize),
                            (x as usize, (y - 1) as usize, z as usize),
                            (x as usize, y as usize, z as usize),
                            ((x - 1) as usize, y as usize, z as usize),
                        ];
                        emit_quad(&cells, stride, &cell_data.vertex_indices, &mut indices, d0 > 0.0);
                    }
                }
            }
        }
    }

    indices
}

/// Emit a quad connecting 4 cells that share a sign-changing edge.
fn emit_quad(
    cells: &[(usize, usize, usize); 4],
    stride: usize,
    vertex_indices: &[u32],
    indices: &mut Vec<u32>,
    solid_first: bool,
) {
    let mut vis = [0u32; 4];
    for (i, &(cx, cy, cz)) in cells.iter().enumerate() {
        if cx >= stride || cy >= stride || cz >= stride {
            return;
        }
        let idx = cx + cy * stride + cz * stride * stride;
        let vi = vertex_indices[idx];
        if vi == u32::MAX {
            return;
        }
        vis[i] = vi;
    }

    // Skip degenerate quads where two or more cells share a vertex index.
    // This happens when QEF clamping collapses neighboring cell vertices
    // to the same position, producing zero-area triangles.
    if vis[0] == vis[1] || vis[0] == vis[2] || vis[0] == vis[3]
        || vis[1] == vis[2] || vis[1] == vis[3]
        || vis[2] == vis[3]
    {
        return;
    }

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