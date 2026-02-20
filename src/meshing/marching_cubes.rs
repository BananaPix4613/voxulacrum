//! Constrained Marching Cubes mesher.
//!
//! Replaces the dual contouring system with a marching-cubes-based mesher that
//! produces flat-shaded geometry with constrained vertex positions (snapped to
//! quarter-voxel grid) and snapped face normals from a discrete set of ~42
//! allowed directions.
//!
//! Key properties:
//! - Topological correctness of standard MC (caves, overhangs, arbitrary shapes)
//! - Trivial chunk stitching (deterministic snap from identical boundary densities)
//! - Flat faces at controlled angles for clean cel-shaded light bands
//! - No QEF, no SVD, no numerical instability

use std::collections::HashMap;

use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::{ChunkMesh, ChunkSnapshot, CHUNK_SIZE, CHUNK_WORLD_SIZE, VOXEL_SCALE};
use crate::world::voxel::MAT_AIR;

use super::mc_tables::{EDGE_TABLE, TRI_TABLE};
use super::MaterialConfig;

// ============================================================================
// Public types (must match what mod.rs imports)
// ============================================================================

/// Map from (cx, cy, cz) local cell coords to the TerrainVertex for boundary cells.
/// Unused by MC (boundary stitching is automatic) but required by pipeline contract.
pub type BoundaryVertexMap = HashMap<(u8, u8, u8), TerrainVertex>;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum CellClass {
    #[default]
    Empty,
    Flat { axis: u8, positive: bool },
    Feature,
}

/// Intermediate result from Phase 1 (vertex generation).
pub struct CellVertexData {
    pub vertices: Vec<TerrainVertex>,
    pub vertex_indices: Vec<u32>,
    pub boundary_map: BoundaryVertexMap,
    pub cell_classes: Vec<CellClass>,
    pub greedy_indices: Vec<u32>,
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

/// Owned version of NeighborBoundaries for sending across threads.
pub struct OwnedNeighborBoundaries {
    pub maps: [Option<BoundaryVertexMap>; 8],
}

impl OwnedNeighborBoundaries {
    pub fn empty() -> Self {
        Self {
            maps: [
                None, None, None, None,
                None, None, None, None,
            ],
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

// ============================================================================
// MC corner and edge conventions (Paul Bourke standard)
// ============================================================================

/// Corner offsets within a unit cell: corner index -> (dx, dy, dz).
///
/// ```text
/// Corner 0: (0, 0, 0)    Corner 4: (0, 0, 1)
/// Corner 1: (1, 0, 0)    Corner 5: (1, 0, 1)
/// Corner 2: (1, 1, 0)    Corner 6: (1, 1, 1)
/// Corner 3: (0, 1, 0)    Corner 7: (0, 1, 1)
/// ```
const CORNER_OFFSETS: [[usize; 3]; 8] = [
    [0, 0, 0], // 0
    [1, 0, 0], // 1
    [1, 1, 0], // 2
    [0, 1, 0], // 3
    [0, 0, 1], // 4
    [1, 0, 1], // 5
    [1, 1, 1], // 6
    [0, 1, 1], // 7
];

/// Which two corners each of the 12 edges connects.
const EDGE_CORNERS: [(usize, usize); 12] = [
    (0, 1), // edge 0
    (1, 2), // edge 1
    (2, 3), // edge 2
    (3, 0), // edge 3
    (4, 5), // edge 4
    (5, 6), // edge 5
    (6, 7), // edge 6
    (7, 4), // edge 7
    (0, 4), // edge 8
    (1, 5), // edge 9
    (2, 6), // edge 10
    (3, 7), // edge 11
];

// ============================================================================
// Edge vertex cache key
// ============================================================================

/// Canonical edge identifier. Each edge is uniquely represented by the grid
/// coordinates of its lower-coordinate endpoint and the axis it runs along.
/// Two adjacent cells sharing an edge always produce the same EdgeId.
#[derive(Hash, Eq, PartialEq, Clone, Copy)]
struct EdgeId {
    x: u16,
    y: u16,
    z: u16,
    axis: u8, // 0=X, 1=Y, 2=Z
}

/// Map MC edge number within cell (cx, cy, cz) to a canonical EdgeId.
///
/// The canonical form uses the minimum-coordinate endpoint of the edge and the
/// axis direction, so shared edges between adjacent cells always hash identically.
fn canonical_edge_id(cx: usize, cy: usize, cz: usize, edge_num: usize) -> EdgeId {
    match edge_num {
        0  => EdgeId { x: cx as u16,       y: cy as u16,       z: cz as u16,       axis: 0 },
        1  => EdgeId { x: (cx + 1) as u16, y: cy as u16,       z: cz as u16,       axis: 1 },
        2  => EdgeId { x: cx as u16,       y: (cy + 1) as u16, z: cz as u16,       axis: 0 },
        3  => EdgeId { x: cx as u16,       y: cy as u16,       z: cz as u16,       axis: 1 },
        4  => EdgeId { x: cx as u16,       y: cy as u16,       z: (cz + 1) as u16, axis: 0 },
        5  => EdgeId { x: (cx + 1) as u16, y: cy as u16,       z: (cz + 1) as u16, axis: 1 },
        6  => EdgeId { x: cx as u16,       y: (cy + 1) as u16, z: (cz + 1) as u16, axis: 0 },
        7  => EdgeId { x: cx as u16,       y: cy as u16,       z: (cz + 1) as u16, axis: 1 },
        8  => EdgeId { x: cx as u16,       y: cy as u16,       z: cz as u16,       axis: 2 },
        9  => EdgeId { x: (cx + 1) as u16, y: cy as u16,       z: cz as u16,       axis: 2 },
        10 => EdgeId { x: (cx + 1) as u16, y: (cy + 1) as u16, z: cz as u16,       axis: 2 },
        11 => EdgeId { x: cx as u16,       y: (cy + 1) as u16, z: cz as u16,       axis: 2 },
        _  => unreachable!(),
    }
}

// ============================================================================
// Allowed normal directions
// ============================================================================

const R: f32 = 0.7071067811865476;  // 1/sqrt(2)
const S: f32 = 0.5773502691896258;  // 1/sqrt(3)
const G: f32 = 0.24253562503633297; // 0.25/sqrt(0.25^2 + 1^2)
const H: f32 = 0.9701425001453319;  // 1.0/sqrt(0.25^2 + 1^2)
const D: f32 = 0.23570226039551587; // 0.25/sqrt(2*0.25^2 + 1^2)
const K: f32 = 0.9428090415820634;  // 1.0/sqrt(2*0.25^2 + 1^2)

/// The complete set of allowed face normal directions (~42 normals).
/// Every face normal produced by the mesher is snapped to the closest entry.
static ALLOWED_NORMALS: [[f32; 3]; 42] = [
    // 6 axis-aligned (cardinal walls + flat top/bottom)
    [ 1.0,  0.0,  0.0], [-1.0,  0.0,  0.0],
    [ 0.0,  1.0,  0.0], [ 0.0, -1.0,  0.0],
    [ 0.0,  0.0,  1.0], [ 0.0,  0.0, -1.0],

    // 12 edge normals (45-degree two-axis combinations)
    [ R,  R,  0.0], [ R, -R,  0.0], [-R,  R,  0.0], [-R, -R,  0.0],
    [ R,  0.0,  R], [ R,  0.0, -R], [-R,  0.0,  R], [-R,  0.0, -R],
    [ 0.0,  R,  R], [ 0.0,  R, -R], [ 0.0, -R,  R], [ 0.0, -R, -R],

    // 8 corner normals (steep diagonal slopes)
    [ S,  S,  S], [ S,  S, -S], [ S, -S,  S], [ S, -S, -S],
    [-S,  S,  S], [-S,  S, -S], [-S, -S,  S], [-S, -S, -S],

    // 8 gentle slopes up-facing (~14 degrees from vertical)
    [ G,  H,  0.0], [-G,  H,  0.0], [ 0.0,  H,  G], [ 0.0,  H, -G],
    [ D,  K,  D],   [ D,  K, -D],   [-D,  K,  D],   [-D,  K, -D],

    // 8 gentle slopes down-facing
    [ G, -H,  0.0], [-G, -H,  0.0], [ 0.0, -H,  G], [ 0.0, -H, -G],
    [ D, -K,  D],   [ D, -K, -D],   [-D, -K,  D],   [-D, -K, -D],
];

/// Snap a raw face normal to the nearest allowed direction.
fn snap_normal(raw: [f32; 3]) -> [f32; 3] {
    let mut best = ALLOWED_NORMALS[0];
    let mut best_dot = dot3(raw, best);

    for &candidate in &ALLOWED_NORMALS[1..] {
        let d = dot3(raw, candidate);
        if d > best_dot {
            best = candidate;
            best_dot = d;
        }
    }

    best
}

// ============================================================================
// Density sampling helpers
// ============================================================================

/// Sample density from snapshot at chunk-local voxel coordinates.
/// Returns -1.0 for out-of-range coordinates.
fn snap_density(snap: &ChunkSnapshot, x: i32, y: i32, z: i32) -> f32 {
    let cs = CHUNK_SIZE as i32;
    if x < -2 || x > cs + 1 || y < -2 || y > cs + 1 || z < -2 || z > cs + 1 {
        return -1.0;
    }
    snap.get_voxel(x, y, z).density as f32
}

/// Sample material ID from snapshot at chunk-local voxel coordinates.
fn snap_material(snap: &ChunkSnapshot, x: i32, y: i32, z: i32) -> u16 {
    let cs = CHUNK_SIZE as i32;
    if x < -2 || x > cs + 1 || y < -2 || y > cs + 1 || z < -2 || z > cs + 1 {
        return MAT_AIR;
    }
    snap.get_voxel(x, y, z).material
}

/// Classify density for sign-change detection. Treats exact zero as
/// slightly negative (air) to avoid ambiguous crossings at chunk
/// boundaries where missing neighbors default to density=0.
#[inline]
fn classify_density(d: f32) -> f32 {
    if d == 0.0 { -0.5 } else { d }
}

// ============================================================================
// Ambient occlusion
// ============================================================================

/// Compute ambient occlusion for a vertex by sampling nearby voxel occupancy.
///
/// Uses a distance-weighted 5x5x5 sampling kernel centered on the vertex
/// position (converted to voxel-local coordinates). Voxels that are solid
/// contribute occlusion inversely proportional to their squared distance.
///
/// Returns a value in [0.5, 1.0] where 1.0 = fully lit, 0.5 = maximally occluded.
fn compute_ao(snap: &ChunkSnapshot, position: [f32; 3], chunk_offset: [f32; 3]) -> f32 {
    // Convert world position to chunk-local voxel coordinates
    let lx = (position[0] - chunk_offset[0]) / VOXEL_SCALE;
    let ly = (position[1] - chunk_offset[1]) / VOXEL_SCALE;
    let lz = (position[2] - chunk_offset[2]) / VOXEL_SCALE;

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

                let sx = cx + dx;
                let sy = cy + dy;
                let sz = cz + dz;

                // Clamp to snapshot bounds to avoid out-of-range access
                let cs = CHUNK_SIZE as i32;
                let vx = sx.clamp(-2, cs + 1);
                let vy = sy.clamp(-2, cs + 1);
                let vz = sz.clamp(-2, cs + 1);

                if snap.get_voxel(vx, vy, vz).density > 0 {
                    weighted_solid += weight;
                }
            }
        }
    }

    let occlusion = weighted_solid / total_weight;
    (1.0 - occlusion * 0.5).clamp(0.5, 1.0)
}

// ============================================================================
// Vector math helpers
// ============================================================================

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn length_sq3(v: [f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = length_sq3(v).sqrt();
    if len < 1e-10 {
        return [0.0, 1.0, 0.0]; // degenerate fallback
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

// ============================================================================
// Core MC functions
// ============================================================================

/// Snap raw interpolation parameter to the nearest quarter-voxel position.
/// Allowed values: {0.0, 0.25, 0.5, 0.75, 1.0}
#[inline]
fn snap_t(raw_t: f32) -> f32 {
    let snapped = (raw_t * 4.0).round() / 4.0;
    snapped.clamp(0.0, 1.0)
}

/// Compute the standard MC interpolation parameter for the zero-crossing
/// between two density values on a cell edge.
#[inline]
fn compute_raw_t(density_a: f32, density_b: f32) -> f32 {
    let denom = density_a - density_b;
    if denom.abs() < 1e-6 {
        0.5
    } else {
        density_a / denom
    }
}

/// Sample the 8 corner densities for a cell at (cx, cy, cz).
fn sample_corners(snap: &ChunkSnapshot, cx: usize, cy: usize, cz: usize) -> [f32; 8] {
    let x = cx as i32;
    let y = cy as i32;
    let z = cz as i32;
    [
        classify_density(snap_density(snap, x,     y,     z    )),
        classify_density(snap_density(snap, x + 1, y,     z    )),
        classify_density(snap_density(snap, x + 1, y + 1, z    )),
        classify_density(snap_density(snap, x,     y + 1, z    )),
        classify_density(snap_density(snap, x,     y,     z + 1)),
        classify_density(snap_density(snap, x + 1, y,     z + 1)),
        classify_density(snap_density(snap, x + 1, y + 1, z + 1)),
        classify_density(snap_density(snap, x,     y + 1, z + 1)),
    ]
}

/// Compute the MC case index (0-255) from 8 corner density signs.
/// Bit i is set when corner i is solid (density > 0).
fn compute_case_index(corners: &[f32; 8]) -> usize {
    let mut index = 0usize;
    for i in 0..8 {
        if corners[i] > 0.0 {
            index |= 1 << i;
        }
    }
    index
}

/// Compute the world-space position of a cell corner.
fn corner_world_position(
    cx: usize, cy: usize, cz: usize,
    corner: usize,
    chunk_offset: [f32; 3],
) -> [f32; 3] {
    let off = CORNER_OFFSETS[corner];
    [
        (cx + off[0]) as f32 * VOXEL_SCALE + chunk_offset[0],
        (cy + off[1]) as f32 * VOXEL_SCALE + chunk_offset[1],
        (cz + off[2]) as f32 * VOXEL_SCALE + chunk_offset[2],
    ]
}

/// Get chunk-local voxel coordinates for a cell corner (for material lookup).
fn corner_local_coords(cx: usize, cy: usize, cz: usize, corner: usize) -> (i32, i32, i32) {
    let off = CORNER_OFFSETS[corner];
    (
        (cx + off[0]) as i32,
        (cy + off[1]) as i32,
        (cz + off[2]) as i32,
    )
}

/// Determine the material for an entire MC cell by tracing upward from the cell
/// center to find the nearest surface voxel (first solid voxel from above).
///
/// On slopes, the MC cells that produce the visible surface geometry sit deeper
/// in the terrain than the grass layer. Their own corners are all well below the
/// surface, so sampling corners directly would always pick soil/rock. By tracing
/// upward we find the actual surface material (e.g. grass) that should be visible.
///
/// Falls back to the lowest-density solid corner if the upward trace fails.
fn determine_cell_material(
    snap: &ChunkSnapshot,
    cx: usize, cy: usize, cz: usize,
    corners: &[f32; 8],
) -> u16 {
    // Cell center in chunk-local voxel coordinates (integer, rounded)
    let center_x = cx as i32;
    let center_z = cz as i32;

    // Start from the top of the cell (cy + 1) and trace upward to find the
    // first solid voxel when coming down from above — i.e. the surface voxel.
    let cs = CHUNK_SIZE as i32;
    let max_y = cs + 1; // snapshot upper bound

    // Scan upward from cell center to find the surface: the first air voxel
    // tells us that the voxel just below it is the surface voxel.
    // Limit the search to a few voxels so cave ceilings deep underground
    // don't get overwritten with surface materials.
    let start_y = cy as i32;
    let max_trace = (start_y + 4).min(max_y); // at most 4 voxels above cell
    let mut surface_mat = MAT_AIR;

    for y in start_y..=max_trace {
        let density = snap_density(snap, center_x, y, center_z);
        if density <= 0.0 {
            // Found air — the voxel at y-1 is the surface voxel
            let surface_y = y - 1;
            if surface_y >= -2 {
                surface_mat = snap_material(snap, center_x, surface_y, center_z);
            }
            break;
        }
    }

    // If we found a valid surface material, use it
    if surface_mat != MAT_AIR {
        return surface_mat;
    }

    // Fallback: lowest-density solid corner (original approach)
    let mut best_mat = MAT_AIR;
    let mut best_density = f32::MAX;

    for i in 0..8 {
        if corners[i] > 0.0 && corners[i] < best_density {
            let (mx, my, mz) = corner_local_coords(cx, cy, cz, i);
            let mat = snap_material(snap, mx, my, mz);
            if mat != MAT_AIR {
                best_density = corners[i];
                best_mat = mat;
            }
        }
    }

    // Fallback: if no solid non-air corner found, try any solid corner
    if best_mat == MAT_AIR {
        for i in 0..8 {
            if corners[i] > 0.0 {
                let (mx, my, mz) = corner_local_coords(cx, cy, cz, i);
                let mat = snap_material(snap, mx, my, mz);
                if mat != MAT_AIR {
                    return mat;
                }
            }
        }
    }

    best_mat
}

/// Compute the edge vertex for a given MC edge within a cell.
fn compute_edge_vertex(
    snap: &ChunkSnapshot,
    materials: &MaterialConfig,
    cx: usize, cy: usize, cz: usize,
    edge_num: usize,
    corners: &[f32; 8],
    chunk_offset: [f32; 3],
) -> TerrainVertex {
    let (ci_a, ci_b) = EDGE_CORNERS[edge_num];
    let da = corners[ci_a];
    let db = corners[ci_b];

    let raw_t = compute_raw_t(da, db);
    let t = snap_t(raw_t);

    let pos_a = corner_world_position(cx, cy, cz, ci_a, chunk_offset);
    let pos_b = corner_world_position(cx, cy, cz, ci_b, chunk_offset);
    let position = lerp3(pos_a, pos_b, t);

    // Material: sample at vertex position offset into the solid interior,
    // then round to nearest voxel grid point. Rounding (not flooring) ensures
    // that all vertices near the same voxel sample the same material,
    // eliminating sub-voxel jitter at material boundaries.
    let solid_pos = if da > 0.0 { pos_a } else { pos_b };
    let toward_solid = normalize3(sub3(solid_pos, position));
    let sample_pos = [
        position[0] + toward_solid[0] * VOXEL_SCALE * 0.5,
        position[1] + toward_solid[1] * VOXEL_SCALE * 0.5,
        position[2] + toward_solid[2] * VOXEL_SCALE * 0.5,
    ];
    // Round to nearest voxel center (not floor) for grid-aligned material lookup
    let vx = ((sample_pos[0] - chunk_offset[0]) / VOXEL_SCALE).round() as i32;
    let vy = ((sample_pos[1] - chunk_offset[1]) / VOXEL_SCALE).round() as i32;
    let vz = ((sample_pos[2] - chunk_offset[2]) / VOXEL_SCALE).round() as i32;
    let mut mat = snap_material(snap, vx, vy, vz);

    // Fallback: if we sample air, use the solid corner directly
    if mat == MAT_AIR {
        let solid_corner = if da > 0.0 { ci_a } else { ci_b };
        let (mx, my, mz) = corner_local_coords(cx, cy, cz, solid_corner);
        mat = snap_material(snap, mx, my, mz);
    }

    let color = if (mat as usize) < materials.colors.len() {
        materials.colors[mat as usize]
    } else {
        [0.5, 0.5, 0.5]
    };

    TerrainVertex {
        position,
        normal: [0.0, 1.0, 0.0],
        color,
        ao: 1.0,
        material_id: mat as u32,
        cell_flags: 0,
        _pad_vert: [0; 2],
    }
}

// ============================================================================
// Flat shading pass
// ============================================================================

/// Duplicate shared vertices per-triangle so each face gets its own snapped
/// normal, and compute per-vertex ambient occlusion, and assign uniform per-frace
/// material via majority vote.
fn flatten_mesh(
    shared_vertices: &[TerrainVertex],
    shared_indices: &[u32],
    tri_materials: &[u32],
    snap: &ChunkSnapshot,
    chunk_offset: [f32; 3],
    materials: &MaterialConfig,
) -> (Vec<TerrainVertex>, Vec<u32>) {
    let tri_count = shared_indices.len() / 3;
    let mut vertices = Vec::with_capacity(tri_count * 3);
    let mut indices = Vec::with_capacity(tri_count * 3);

    for tri in 0..tri_count {
        let i0 = shared_indices[tri * 3] as usize;
        let i1 = shared_indices[tri * 3 + 1] as usize;
        let i2 = shared_indices[tri * 3 + 2] as usize;

        let v0 = shared_vertices[i0];
        let v1 = shared_vertices[i1];
        let v2 = shared_vertices[i2];

        // Compute face normal from triangle edges
        let e1 = sub3(v1.position, v0.position);
        let e2 = sub3(v2.position, v0.position);
        let raw_cross = cross3(e1, e2);

        // Skip degenerate triangles
        if length_sq3(raw_cross) < 1e-12 {
            continue;
        }

        let raw_normal = normalize3(raw_cross);
        let snapped = snap_normal(raw_normal);

        // Per-vertex AO
        let ao0 = compute_ao(snap, v0.position, chunk_offset);
        let ao1 = compute_ao(snap, v1.position, chunk_offset);
        let ao2 = compute_ao(snap, v2.position, chunk_offset);

        // Per-face material: use the cell's material (all triangles from
        // the same MC cell share one material, eliminating zigzag artifacts)
        let face_mat = tri_materials[tri];
        let face_color = if (face_mat as usize) < materials.colors.len() {
            materials.colors[face_mat as usize]
        } else {
            [0.5, 0.5, 0.5]
        };

        let base = vertices.len() as u32;
        vertices.push(TerrainVertex {
            normal: snapped, ao: ao0, color: face_color, material_id: face_mat, ..v0
        });
        vertices.push(TerrainVertex {
            normal: snapped, ao: ao1, color: face_color, material_id: face_mat, ..v1
        });
        vertices.push(TerrainVertex {
            normal: snapped, ao: ao2, color: face_color, material_id: face_mat, ..v2
        });

        indices.push(base);
        indices.push(base + 1);
        indices.push(base + 2);
    }

    (vertices, indices)
}

// ============================================================================
// Public API — Phase 1 and Phase 2 entry points
// ============================================================================

/// Phase 1: Generate all mesh data from a ChunkSnapshot.
///
/// Performs the complete MC algorithm: corner sampling, case computation, edge
/// vertex generation with snap_t, triangle generation via TRI_TABLE, and
/// flatten_mesh for flat shading with snapped normals.
///
/// The resulting CellVertexData contains the final flat-shaded vertices and
/// triangle indices (stored in `greedy_indices`). Phase 2 simply extracts them.
pub fn generate_cell_vertices_from_snapshot(
    snap: &ChunkSnapshot,
    materials: &MaterialConfig,
) -> CellVertexData {
    let chunk_offset = [
        snap.position.x as f32 * CHUNK_WORLD_SIZE,
        snap.position.y as f32 * CHUNK_WORLD_SIZE,
        snap.position.z as f32 * CHUNK_WORLD_SIZE,
    ];

    let mut shared_vertices: Vec<TerrainVertex> = Vec::new();
    let mut shared_indices: Vec<u32> = Vec::new();
    let mut tri_materials: Vec<u32> = Vec::new();
    let mut edge_cache: HashMap<EdgeId, u32> =
        HashMap::with_capacity(CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE * 3);

    for cz in 0..CHUNK_SIZE {
        for cy in 0..CHUNK_SIZE {
            for cx in 0..CHUNK_SIZE {
                let corners = sample_corners(snap, cx, cy, cz);
                let case_index = compute_case_index(&corners);

                let edge_bits = EDGE_TABLE[case_index];
                if edge_bits == 0 {
                    continue; // entirely inside or outside
                }

                // Determine cell material once for all triangles in this cell
                let cell_material = determine_cell_material(snap, cx, cy, cz, &corners);

                // Compute edge vertices for all active edges
                for edge_num in 0..12u32 {
                    if edge_bits & (1 << edge_num) == 0 {
                        continue;
                    }
                    let eid = canonical_edge_id(cx, cy, cz, edge_num as usize);
                    if edge_cache.contains_key(&eid) {
                        continue; // already computed by adjacent cell
                    }
                    let vertex = compute_edge_vertex(
                        snap, materials, cx, cy, cz,
                        edge_num as usize, &corners, chunk_offset,
                    );
                    let idx = shared_vertices.len() as u32;
                    edge_cache.insert(eid, idx);
                    shared_vertices.push(vertex);
                }

                // Generate triangles from TRI_TABLE
                let tri_entry = &TRI_TABLE[case_index];
                let mut i = 0;
                while i < 16 && tri_entry[i] != -1 {
                    let e0 = tri_entry[i] as usize;
                    let e1 = tri_entry[i + 1] as usize;
                    let e2 = tri_entry[i + 2] as usize;

                    let vi0 = edge_cache[&canonical_edge_id(cx, cy, cz, e0)];
                    let vi1 = edge_cache[&canonical_edge_id(cx, cy, cz, e1)];
                    let vi2 = edge_cache[&canonical_edge_id(cx, cy, cz, e2)];

                    shared_indices.push(vi0);
                    shared_indices.push(vi2);
                    shared_indices.push(vi1);

                    tri_materials.push(cell_material as u32);

                    i += 3;
                }
            }
        }
    }

    // Flatten for flat shading with snapped normals
    let (flat_vertices, flat_indices) = flatten_mesh(
        &shared_vertices, &shared_indices, &tri_materials,
        snap, chunk_offset, materials,
    );

    // Build CellVertexData matching pipeline contract.
    // MC doesn't use vertex_indices, cell_classes, or boundary_map - these exist
    // solely to satisfy the Phase 2 interface in mod.rs.
    let stride = CHUNK_SIZE + 1;
    let total_cells = stride * stride * stride;

    CellVertexData {
        vertices: flat_vertices,
        vertex_indices: vec![u32::MAX; total_cells],
        boundary_map: BoundaryVertexMap::new(),
        cell_classes: vec![CellClass::Empty; total_cells],
        greedy_indices: flat_indices,
    }
}

/// Phase 2: Extract pre-computed indices.
///
/// MC produces complete geometry in Phase 1. Phase 2 simply extracts the
/// triangle indices that were stored in `greedy_indices`.
pub fn generate_faces_from_snapshot(
    cell_data: &mut CellVertexData,
    _snap: &ChunkSnapshot,
    _neighbor_boundaries: &NeighborBoundaries,
) -> Vec<u32> {
    std::mem::take(&mut cell_data.greedy_indices)
}

/// Generate mesh data for a single chunk (single-threaded convenience method).
pub fn mesh_chunk(
    chunk: &crate::world::chunk::Chunk,
    neighbors: &crate::world::chunk::ChunkNeighbors,
    materials: &MaterialConfig,
) -> ChunkMeshData {
    let snapshot = ChunkSnapshot::extract(chunk, neighbors);
    let mut cell_data = generate_cell_vertices_from_snapshot(&snapshot, materials);
    let nb = NeighborBoundaries::empty();
    let indices = generate_faces_from_snapshot(&mut cell_data, &snapshot, &nb);
    ChunkMeshData {
        vertices: cell_data.vertices,
        indices,
    }
}