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

    // Material: use the solid side's material
    let solid_corner = if da > 0.0 { ci_a } else { ci_b };
    let (mx, my, mz) = corner_local_coords(cx, cy, cz, solid_corner);
    let mat = snap_material(snap, mx, my, mz);

    let color = if (mat as usize) < materials.colors.len() {
        materials.colors[mat as usize]
    } else {
        [0.5, 0.5, 0.5]
    };

    TerrainVertex {
        position,
        normal: [0.0, 1.0, 0.0], // placeholder - set during flatten_mesh
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
/// normal. THis produces flat-shaded geometry where every triangle has a single
/// uniform normal from the allowed set.
fn flatten_mesh(
    shared_vertices: &[TerrainVertex],
    shared_indices: &[u32],
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

        // Skip degenerate triangles (zero-area from coincidient snapped vertices)
        if length_sq3(raw_cross) < 1e-12 {
            continue;
        }

        let raw_normal = normalize3(raw_cross);
        let snapped = snap_normal(raw_normal);

        let base = vertices.len() as u32;
        vertices.push(TerrainVertex { normal: snapped, ..v0 });
        vertices.push(TerrainVertex { normal: snapped, ..v1 });
        vertices.push(TerrainVertex { normal: snapped, ..v2 });

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

                    i += 3;
                }
            }
        }
    }

    // Flatten for flat shading with snapped normals
    let (flat_vertices, flat_indices) = flatten_mesh(&shared_vertices, &shared_indices);

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