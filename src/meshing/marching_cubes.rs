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

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::rendering::pipelines::TerrainVertex;
use crate::world::chunk::{ChunkSnapshot, CHUNK_SIZE, CHUNK_WORLD_SIZE, VOXEL_SCALE};
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
// Greedy face merging — data structures
// ============================================================================

/// Canonical edge between two quantized vertex positions.
/// Used for adjacency detection between coplanar triangles.
#[derive(Hash, Eq, PartialEq, Clone, Copy)]
struct PositionEdge {
    a: u64,
    b: u64,
}

/// Key for grouping coplanar, same-material faces.
#[derive(Hash, Eq, PartialEq, Ord, PartialOrd, Clone)]
struct PlaneGroupKey {
    normal_index: u8,
    material_id: u32,
    plane_distance_quantized: i32,
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
fn canonical_edge_id(cx: i32, cy: i32, cz: i32, edge_num: usize) -> EdgeId {
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
pub(crate) static ALLOWED_NORMALS: [[f32; 3]; 42] = [
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
    snap.get_density(x, y, z) as f32
}

/// Sample material ID from snapshot at chunk-local voxel coordinates.
fn snap_material(snap: &ChunkSnapshot, x: i32, y: i32, z: i32) -> u16 {
    let cs = CHUNK_SIZE as i32;
    if x < -2 || x > cs + 1 || y < -2 || y > cs + 1 || z < -2 || z > cs + 1 {
        return MAT_AIR;
    }
    snap.get_material(x, y, z)
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
                if dy < 0 {
                    continue; // Skip terrain body below the surface — it never blocks skylight
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

                if snap.get_density(vx, vy, vz) > 0 {
                    weighted_solid += weight;
                }
            }
        }
    }

    let occlusion = weighted_solid / total_weight;
    (1.0 - occlusion * 0.5).clamp(0.5, 1.0)
}

// ============================================================================
// Greedy face merging — helpers
// ============================================================================

/// Pack a vertex position into a u64 key for edge hashing and deduplication.
///
/// With VOXEL_SCALE=0.5 and quarter-voxel snapping, vertex coordinates are
/// multiples of 0.125. Multiplying by 8192 (power of 2) and rounding gives
/// integer keys with no collisions. Each axis gets 21 bits, supporting
/// positions from -128 to +128 world units - sufficient for the 64x32x64
/// world (8x4x8 chunks x 16 world units each).
fn pack_position_key(pos: [f32; 3]) -> u64 {
    const SCALE: f32 = 8192.0;
    let x = ((pos[0] * SCALE).round() as i32 as u64) & 0x1FFFFF;
    let y = ((pos[1] * SCALE).round() as i32 as u64) & 0x1FFFFF;
    let z = ((pos[2] * SCALE).round() as i32 as u64) & 0x1FFFFF;
    x | (y << 21) | (z << 42)
}

/// Create a canonical (order-independent) edge from two position keys.
fn make_edge(key_a: u64, key_b: u64) -> PositionEdge {
    if key_a <= key_b {
        PositionEdge { a: key_a, b: key_b }
    } else {
        PositionEdge { a: key_b, b: key_a }
    }
}

/// Find which ALLOWED_NORMALS index a snapped normal corresponds to.
/// Returns 0 as fallback (should not happen with properly snapped normals).
pub(crate) fn find_normal_index(normal: [f32; 3]) -> u8 {
    let mut best = 0u8;
    let mut best_dot = dot3(normal, ALLOWED_NORMALS[0]);
    for (i, &candidate) in ALLOWED_NORMALS.iter().enumerate().skip(1) {
        let d = dot3(normal, candidate);
        if d > best_dot {
            best = i as u8;
            best_dot = d;
        }
    }
    best
}

/// Quantize the plane distance for coplanarity grouping.
/// Two faces are considered coplanar if their plane distances fall in the
/// same quantization bin (width = flat_threshold_error).
fn quantize_plane_distance(position: [f32; 3], normal: [f32; 3], threshold: f32) -> i32 {
    let d = dot3(position, normal);
    (d / threshold).round() as i32
}

/// Compute two orthonormal basis vectors for the plane perpendicular to `normal`.
/// Used to project 3D polygon vertices into 2D for ear clipping.
fn compute_plane_basis(normal: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let hint = if normal[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = normalize3(cross3(normal, hint));
    let v = cross3(normal, u);
    (u, v)
}

/// Project a 3D position onto a 2D plane defined by basis vectors.
#[inline]
fn project_2d(pos: [f32; 3], u: [f32; 3], v: [f32; 3]) -> [f32; 2] {
    [dot3(pos, u), dot3(pos, v)]
}

/// 2D cross product (z-component of the 3D cross product of 2D vectors).
#[inline]
fn cross_2d(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

/// 2D vector subtraction.
#[inline]
fn sub_2d(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

/// Test whether point `p` lies inside triangle (a, b, c) in 2D.
/// Uses barycentric sign tests. Returns true for interior and edge points.
fn point_in_triangle_2d(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let d1 = cross_2d(sub_2d(b, a), sub_2d(p, a));
    let d2 = cross_2d(sub_2d(c, b), sub_2d(p, b));
    let d3 = cross_2d(sub_2d(a, c), sub_2d(p, c));

    let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);

    !(has_neg && has_pos)
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
fn sample_corners(snap: &ChunkSnapshot, cx: i32, cy: i32, cz: i32) -> [f32; 8] {
    [
        classify_density(snap_density(snap, cx,     cy,     cz    )),
        classify_density(snap_density(snap, cx + 1, cy,     cz    )),
        classify_density(snap_density(snap, cx + 1, cy + 1, cz    )),
        classify_density(snap_density(snap, cx,     cy + 1, cz    )),
        classify_density(snap_density(snap, cx,     cy,     cz + 1)),
        classify_density(snap_density(snap, cx + 1, cy,     cz + 1)),
        classify_density(snap_density(snap, cx + 1, cy + 1, cz + 1)),
        classify_density(snap_density(snap, cx,     cy + 1, cz + 1)),
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
    cx: i32, cy: i32, cz: i32,
    corner: usize,
    chunk_offset: [f32; 3],
) -> [f32; 3] {
    let off = CORNER_OFFSETS[corner];
    [
        (cx + off[0] as i32) as f32 * VOXEL_SCALE + chunk_offset[0],
        (cy + off[1] as i32) as f32 * VOXEL_SCALE + chunk_offset[1],
        (cz + off[2] as i32) as f32 * VOXEL_SCALE + chunk_offset[2],
    ]
}

/// Get chunk-local voxel coordinates for a cell corner (for material lookup).
fn corner_local_coords(cx: i32, cy: i32, cz: i32, corner: usize) -> (i32, i32, i32) {
    let off = CORNER_OFFSETS[corner];
    (
        cx + off[0] as i32,
        cy + off[1] as i32,
        cz + off[2] as i32,
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
    cx: i32, cy: i32, cz: i32,
    corners: &[f32; 8],
) -> u16 {
    // Cell center in chunk-local voxel coordinates (integer, rounded)
    let center_x = cx;
    let center_z = cz;

    // Start from the top of the cell (cy + 1) and trace upward to find the
    // first solid voxel when coming down from above — i.e. the surface voxel.
    let cs = CHUNK_SIZE as i32;
    let max_y = cs + 1; // snapshot upper bound

    // Scan upward from cell center to find the surface: the first air voxel
    // tells us that the voxel just below it is the surface voxel.
    // Limit the search to a few voxels so cave ceilings deep underground
    // don't get overwritten with surface materials.
    let start_y = cy;
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
    cx: i32, cy: i32, cz: i32,
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
/// normal, and compute per-vertex ambient occlusion, and assign uniform per-face
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

        // Per-vertex AO — same world position always produces the same AO value,
        // so deduplication in greedy_merge stays consistent.
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
// Greedy face merging — adjacency and connected components
// ============================================================================

/// Build edge adjacency within a coplanar group and find connected components
/// via flood fill.
///
/// Returns:
/// - Edge-to-triangles map (for boundary extraction)
/// - List of connected components (each is a Vec of triangle indices)
fn find_connected_components(
    tri_list: &[usize],
    vertices: &[TerrainVertex],
    indices: &[u32],
) -> (HashMap<PositionEdge, Vec<usize>>, Vec<Vec<usize>>) {
    // Build edge -> triangles map
    let mut edge_to_tris: HashMap<PositionEdge, Vec<usize>> = HashMap::new();

    for &tri_idx in tri_list {
        let base = tri_idx * 3;
        let vi = [
            indices[base] as usize,
            indices[base + 1] as usize,
            indices[base + 2] as usize,
        ];
        for &(i, j) in &[(0, 1), (1, 2), (2, 0)] {
            let edge = make_edge(
                pack_position_key(vertices[vi[i]].position),
                pack_position_key(vertices[vi[j]].position),
            );
            edge_to_tris.entry(edge).or_default().push(tri_idx);
        }
    }

    // Build adjacency list: triangles connected by shared edges
    let tri_set: HashSet<usize> = tri_list.iter().copied().collect();
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();

    for tris in edge_to_tris.values() {
        if tris.len() == 2 && tri_set.contains(&tris[0]) && tri_set.contains(&tris[1]) {
            adjacency.entry(tris[0]).or_default().push(tris[1]);
            adjacency.entry(tris[1]).or_default().push(tris[0]);
        }
    }

    // Flood fill to find connected components
    let mut visited: HashSet<usize> = HashSet::with_capacity(tri_list.len());
    let mut components: Vec<Vec<usize>> = Vec::new();

    for &tri_idx in tri_list {
        if visited.contains(&tri_idx) {
            continue;
        }

        let mut component = Vec::new();
        let mut stack = vec![tri_idx];

        while let Some(t) = stack.pop() {
            if !visited.insert(t) {
                continue;
            }
            component.push(t);
            if let Some(neighbors) = adjacency.get(&t) {
                for &n in neighbors {
                    if !visited.contains(&n) {
                        stack.push(n);
                    }
                }
            }
        }

        components.push(component);
    }

    (edge_to_tris, components)
}

// ============================================================================
// Greedy face merging — boundary extraction
// ============================================================================

/// Extract the ordered boundary polygon of a connected component of coplanar
/// triangles.
///
/// A boundary half-edge is a directed edge (a->b) whose canonical undirected
/// edge belongs to exactly one triangle in the component. The boundary forms
/// a closed loop. Returns `None` if the boundary is not a single simple loop
/// (e.g., multiple disconnected loops indicating a hole, or malformed data).
///
/// Returns vertex buffer indices (not position keys) so that per-vertex AO
/// values are preserved in the output.
fn extract_boundary(
    component: &[usize],
    vertices: &[TerrainVertex],
    indices: &[u32],
    edge_to_tris: &HashMap<PositionEdge, Vec<usize>>,
) -> Option<Vec<u32>> {
    let component_set: HashSet<usize> = component.iter().copied().collect();

    // Collect boundary directed half-edges: from_pos_key -> (to_pos_key, from_vertex_index).
    // A half-edge a->b is boundary if the canonical edge {a,b} has exactly 1 triangle
    // in this component.
    let mut half_edges: BTreeMap<u64, (u64, u32)> = BTreeMap::new();

    for &tri_idx in component {
        let base = tri_idx * 3;
        let vi = [
            indices[base] as usize,
            indices[base + 1] as usize,
            indices[base + 2] as usize,
        ];

        for &(i, j) in &[(0, 1), (1, 2), (2, 0)] {
            let key_a = pack_position_key(vertices[vi[i]].position);
            let key_b = pack_position_key(vertices[vi[j]].position);
            let canonical = make_edge(key_a, key_b);

            let count_in_component = edge_to_tris
                .get(&canonical)
                .map_or(0, |tris| tris.iter().filter(|t| component_set.contains(t)).count());

            if count_in_component == 1 {
                half_edges.insert(key_a, (key_b, vi[i] as u32));
            }
        }
    }

    if half_edges.is_empty() {
        return None;
    }

    // Walk the boundary loop
    let &start_key = half_edges.keys().next()?;
    let mut boundary = Vec::with_capacity(half_edges.len());
    let mut current_key = start_key;

    loop {
        let &(next_key, vertex_idx) = half_edges.get(&current_key)?;
        boundary.push(vertex_idx);
        current_key = next_key;

        if current_key == start_key {
            break;
        }

        // Guard against infinite loops from malformed data
        if boundary.len() > half_edges.len() {
            return None;
        }
    }

    // Must form a valid polygon
    if boundary.len() < 3 {
        return None;
    }

    // Verify we consumed all boundary edges (single loop, no holes)
    if boundary.len() != half_edges.len() {
        return None;
    }

    Some(boundary)
}

// ============================================================================
// Greedy face merging — collinear vertex removal
// ============================================================================

/// Remove collinear vertices from a boundary polygon. Three consecutive
/// vertices where the middle one lies on the line between the other two
/// can be removed without changing the polygon shape. This reduces the
/// final triangle count after ear clipping.
fn remove_collinear_vertices(boundary: &mut Vec<u32>, vertices: &[TerrainVertex]) {
    if boundary.len() <= 3 {
        return;
    }

    let mut i = 0;
    let mut iterations = 0;
    let max_iterations = boundary.len() * 2; // Safety bound

    while boundary.len() > 3 && iterations < max_iterations {
        iterations += 1;
        let n = boundary.len();
        let prev = boundary[(i + n - 1) % n] as usize;
        let curr = boundary[i % n] as usize;
        let next = boundary[(i + 1) % n] as usize;

        let edge1 = sub3(vertices[curr].position, vertices[prev].position);
        let edge2 = sub3(vertices[next].position, vertices[curr].position);
        let cross = cross3(edge1, edge2);

        if length_sq3(cross) < 1e-10 {
            boundary.remove(i % boundary.len());
            // After removal, the current index now points to what was `next`.
            // Recheck from the previous vertex since it may now be collinear
            // with its new neighbors.
            if i > 0 {
                i -= 1;
            }
        } else {
            i += 1;
        }

        if i >= boundary.len() {
            break;
        }
    }
}

// ============================================================================
// Greedy face merging — ear clipping triangulation
// ============================================================================

/// Triangulate a planar polygon using the ear clipping algorithm.
///
/// The polygon is defined by `boundary` (ordered vertex indices, CCW winding
/// when viewed from the direction of `normal`). All vertices are coplanar.
///
/// Returns a list of triangle index triples referencing the original vertex
/// buffer.
fn ear_clip_triangulate(
    boundary: &[u32],
    vertices: &[TerrainVertex],
    normal: [f32; 3],
) -> Vec<[u32; 3]> {
    let n = boundary.len();
    if n < 3 {
        return vec![];
    }
    if n == 3 {
        return vec![[boundary[0], boundary[1], boundary[2]]];
    }
    if n == 4 {
        // Quad fast-path: AO-optimal diagonal.
        // Place the diagonal through the vertex pair with LARGER AO contrast —
        // the off-diagonal pair (which produces the visible seam) will then have
        // the SMALLER contrast, minimizing the brightness difference between the
        // two triangles.
        //   diagonal 0-2: off-diagonal pair is (1,3), seam ∝ |ao1-ao3|
        //   diagonal 1-3: off-diagonal pair is (0,2), seam ∝ |ao0-ao2|
        let ao0 = vertices[boundary[0] as usize].ao;
        let ao1 = vertices[boundary[1] as usize].ao;
        let ao2 = vertices[boundary[2] as usize].ao;
        let ao3 = vertices[boundary[3] as usize].ao;
        if (ao0 - ao2).abs() >= (ao1 - ao3).abs() {
            // diagonal 0-2 has larger contrast → use it; off-diagonal (1,3) is smaller
            return vec![
                [boundary[0], boundary[1], boundary[2]],
                [boundary[0], boundary[2], boundary[3]],
            ];
        } else {
            // diagonal 1-3 has larger contrast → use it; off-diagonal (0,2) is smaller
            return vec![
                [boundary[0], boundary[1], boundary[3]],
                [boundary[1], boundary[2], boundary[3]],
            ];
        }
    }

    // General ear clipping
    let (u_axis, v_axis) = compute_plane_basis(normal);

    // Project all boundary vertices to 2D
    let pts_2d: Vec<[f32; 2]> = boundary
        .iter()
        .map(|&vi| project_2d(vertices[vi as usize].position, u_axis, v_axis))
        .collect();

    let mut remaining: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity(n - 2);

    let mut safety = 0;
    let max_safety = n * n; // 0(n^2) worst case

    while remaining.len() > 3 && safety < max_safety {
        safety += 1;
        let rn = remaining.len();
        let mut found_ear = false;

        for idx in 0..rn {
            let pi = remaining[(idx + rn - 1) % rn];
            let ci = remaining[idx];
            let ni = remaining[(idx + 1) % rn];

            let p0 = pts_2d[pi];
            let p1 = pts_2d[ci];
            let p2 = pts_2d[ni];

            // Check convexity (positive cross product = CCW = convex ear)
            if cross_2d(sub_2d(p1, p0), sub_2d(p2, p1)) <= 1e-10 {
                continue; // Reflex or degenerate vertex
            }

            // Check that no other remaining vertex falls inside this triangle
            let mut is_ear = true;
            for (j, &ri) in remaining.iter().enumerate() {
                if j == (idx + rn - 1) % rn || j == idx || j == (idx + 1) % rn {
                    continue;
                }
                if point_in_triangle_2d(pts_2d[ri], p0, p1, p2) {
                    is_ear = false;
                    break;
                }
            }

            if is_ear {
                triangles.push([boundary[pi], boundary[ci], boundary[ni]]);
                remaining.remove(idx);
                found_ear = true;
                break;
            }
        }

        if !found_ear {
            // Degenerate polygon - cannot find an ear. This should not happen
            // with well-formed MC output. Fall back: emit nothing and let the
            // caller use original triangles.
            return vec![];
        }
    }

    // Last triangle
    if remaining.len() == 3 {
        triangles.push([
            boundary[remaining[0]],
            boundary[remaining[1]],
            boundary[remaining[2]],
        ]);
    }

    triangles
}

// ============================================================================
// Greedy face merging — top-level orchestrator
// ============================================================================

/// Merge coplanar adjacent triangles into larger polygons and re-triangulate.
///
/// Groups flat-shaded triangles by (snapped normal, material, plane distance),
/// finds connected components of adjacent triangles within each group, extracts
/// boundary polygons, removes collinear vertices, and re-triangulate via ear
/// clipping.
///
/// Falls back to emitting original triangles for any group/component where
/// merging fails (non-simple boundary, holes, degenerate geometry).
///
/// AO is preserved: boundary vertices retain their original per-vertex AO values
/// computed during flatten_mesh. Interior vertex AO is interpolated by the GPU.
fn greedy_merge(
    flat_vertices: &[TerrainVertex],
    flat_indices: &[u32],
    flat_threshold_error: f32,
) -> (Vec<TerrainVertex>, Vec<u32>) {
    let tri_count = flat_indices.len() / 3;
    if tri_count == 0 {
        return (Vec::new(), Vec::new());
    }

    // Stage 1: Group triangles by coplanar plane + material
    let mut plane_groups: BTreeMap<PlaneGroupKey, Vec<usize>> = BTreeMap::new();

    for tri_idx in 0..tri_count {
        let vi0 = flat_indices[tri_idx * 3] as usize;
        let normal = flat_vertices[vi0].normal;
        let material_id = flat_vertices[vi0].material_id;
        let normal_index = find_normal_index(normal);
        let plane_dist = quantize_plane_distance(
            flat_vertices[vi0].position,
            normal,
            flat_threshold_error,
        );

        let key = PlaneGroupKey {
            normal_index,
            material_id,
            plane_distance_quantized: plane_dist,
        };

        plane_groups.entry(key).or_default().push(tri_idx);
    }

    let mut out_vertices: Vec<TerrainVertex> = Vec::with_capacity(flat_vertices.len() / 2);
    let mut out_indices: Vec<u32> = Vec::with_capacity(flat_indices.len() / 2);

    // Per-group vertex dedup map. Within a single PlaneGroupKey, all vertices
    // share the same normal and material, so dedup by position alone is safe.
    // We clear and reuse this map per group to prevent cross-group collisions
    // that would incorrectly merge vertices with different materials or normals.
    let mut vertex_map: HashMap<u64, u32> = HashMap::new();

    for (key, tri_list) in &plane_groups {
        // Clear the dedup map for each group — this is the critical fix that
        // prevents vertices from different materials/normals from being merged.
        vertex_map.clear();

        // Single triangle — emit directly, no merging overhead
        if tri_list.len() == 1 {
            let base = tri_list[0] * 3;
            for offset in 0..3 {
                let vi = flat_indices[base + offset] as usize;
                let v = &flat_vertices[vi];
                let pos_key = pack_position_key(v.position);
                let idx = vertex_map.entry(pos_key).or_insert_with(|| {
                    let i = out_vertices.len() as u32;
                    out_vertices.push(*v);
                    i
                });
                out_indices.push(*idx);
            }
            continue;
        }

        // Stage 2: Build adjacency and find connected components
        let (edge_to_tris, components) =
            find_connected_components(tri_list, flat_vertices, flat_indices);

        let normal = ALLOWED_NORMALS[key.normal_index as usize];

        for component in &components {
            if component.len() == 1 {
                let base = component[0] * 3;
                for offset in 0..3 {
                    let vi = flat_indices[base + offset] as usize;
                    let v = &flat_vertices[vi];
                    let pos_key = pack_position_key(v.position);
                    let idx = vertex_map.entry(pos_key).or_insert_with(|| {
                        let i = out_vertices.len() as u32;
                        out_vertices.push(*v);
                        i
                    });
                    out_indices.push(*idx);
                }
                continue;
            }

            // Extract boundary polygon
            let boundary_opt = extract_boundary(component, flat_vertices, flat_indices, &edge_to_tris)
                .and_then(|mut boundary| {
                    remove_collinear_vertices(&mut boundary, flat_vertices);
                    if boundary.len() < 3 { return None; }
                    Some(boundary)
                });

            match boundary_opt {
                Some(boundary) => {
                    let nb = boundary.len();

                    // Compute centroid position and AO.
                    let mut cx = 0.0f32; let mut cy = 0.0f32; let mut cz = 0.0f32;
                    let mut ao_sum = 0.0f32;
                    for &vi in &boundary {
                        let v = &flat_vertices[vi as usize];
                        cx += v.position[0]; cy += v.position[1]; cz += v.position[2];
                        ao_sum += v.ao;
                    }
                    let inv = 1.0 / nb as f32;
                    let centroid_pos = [cx * inv, cy * inv, cz * inv];
                    let centroid_ao  = ao_sum * inv;

                    // Check that the centroid lies inside the polygon (i.e., every fan
                    // triangle has the same winding as the polygon's normal). Fails for
                    // concave polygons where the centroid is outside.
                    let fan_valid = (0..nb).all(|i| {
                        let pi = flat_vertices[boundary[i] as usize].position;
                        let ni = flat_vertices[boundary[(i + 1) % nb] as usize].position;
                        let ea = [pi[0] - centroid_pos[0], pi[1] - centroid_pos[1], pi[2] - centroid_pos[2]];
                        let eb = [ni[0] - centroid_pos[0], ni[1] - centroid_pos[1], ni[2] - centroid_pos[2]];
                        let cross = [
                            ea[1] * eb[2] - ea[2] * eb[1],
                            ea[2] * eb[0] - ea[0] * eb[2],
                            ea[0] * eb[1] - ea[1] * eb[0],
                        ];
                        cross[0] * normal[0] + cross[1] * normal[1] + cross[2] * normal[2] > 0.0
                    });

                    if fan_valid {
                        // Fan-from-centroid: all fan triangles share the centroid vertex,
                        // so adjacent triangles agree on every shared vertex's AO → no seam.
                        // Boundary vertices keep their original per-vertex AO → gradients preserved.
                        let first_v = &flat_vertices[boundary[0] as usize];
                        let centroid_v = TerrainVertex {
                            position: centroid_pos,
                            ao: centroid_ao,
                            ..*first_v  // normal, color, material_id, cell_flags from any boundary vert
                        };
                        let centroid_idx = out_vertices.len() as u32;
                        out_vertices.push(centroid_v);

                        for i in 0..nb {
                            let vi0 = boundary[i] as usize;
                            let vi1 = boundary[(i + 1) % nb] as usize;

                            out_indices.push(centroid_idx);

                            for vi in [vi0, vi1] {
                                let v = &flat_vertices[vi];
                                let pos_key = pack_position_key(v.position);
                                let idx = *vertex_map.entry(pos_key).or_insert_with(|| {
                                    let j = out_vertices.len() as u32;
                                    out_vertices.push(*v);
                                    j
                                });
                                out_indices.push(idx);
                            }
                        }
                    } else {
                        // Concave polygon: fall back to ear-clipping (original behaviour).
                        let triangles = ear_clip_triangulate(&boundary, flat_vertices, normal);
                        let tris_to_emit: &[_] = &triangles;
                        if tris_to_emit.is_empty() {
                            // ear-clip also failed — emit original triangles
                            for &tri_idx in component {
                                let base = tri_idx * 3;
                                for offset in 0..3 {
                                    let vi = flat_indices[base + offset] as usize;
                                    let v = &flat_vertices[vi];
                                    let pos_key = pack_position_key(v.position);
                                    let idx = *vertex_map.entry(pos_key).or_insert_with(|| {
                                        let i = out_vertices.len() as u32;
                                        out_vertices.push(*v);
                                        i
                                    });
                                    out_indices.push(idx);
                                }
                            }
                        } else {
                            for tri in tris_to_emit {
                                for &vi in tri {
                                    let v = &flat_vertices[vi as usize];
                                    let pos_key = pack_position_key(v.position);
                                    let idx = *vertex_map.entry(pos_key).or_insert_with(|| {
                                        let i = out_vertices.len() as u32;
                                        out_vertices.push(*v);
                                        i
                                    });
                                    out_indices.push(idx);
                                }
                            }
                        }
                    }
                }
                None => {
                    // Fallback: emit original triangles unmodified
                    for &tri_idx in component {
                        let base = tri_idx * 3;
                        for offset in 0..3 {
                            let vi = flat_indices[base + offset] as usize;
                            let v = &flat_vertices[vi];
                            let pos_key = pack_position_key(v.position);
                            let idx = *vertex_map.entry(pos_key).or_insert_with(|| {
                                let i = out_vertices.len() as u32;
                                out_vertices.push(*v);
                                i
                            });
                            out_indices.push(idx);
                        }
                    }
                }
            }
        }
    }

    (out_vertices, out_indices)
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

    // Extend iteration to cell -1 on axes where this chunk is at the negative
    // world border. Normally cell -1 is covered by the neighbor chunk's cell 31,
    // but at the world edge there is no neighbor.
    let start_x = if snap.border_min[0] { -1i32 } else { 0 };
    let start_y = if snap.border_min[1] { -1i32 } else { 0 };
    let start_z = if snap.border_min[2] { -1i32 } else { 0 };

    for cz in start_z..(CHUNK_SIZE as i32) {
        for cy in start_y..(CHUNK_SIZE as i32) {
            for cx in start_x..(CHUNK_SIZE as i32) {
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

    // Greedy face merging: reduce triangle count by merging coplanar faces
    let (final_vertices, final_indices) = if materials.greedy_merge_enabled {
        greedy_merge(&flat_vertices, &flat_indices, materials.flat_threshold_error)
    } else {
        (flat_vertices, flat_indices)
    };

    // Build CellVertexData matching pipeline contract.
    let stride = CHUNK_SIZE + 1;
    let total_cells = stride * stride * stride;

    CellVertexData {
        vertices: final_vertices,
        vertex_indices: vec![u32::MAX; total_cells],
        boundary_map: BoundaryVertexMap::new(),
        cell_classes: vec![CellClass::Empty; total_cells],
        greedy_indices: final_indices,
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
    min_chunk_y: i32,
    max_chunk_y: i32,
) -> ChunkMeshData {
    let snapshot = ChunkSnapshot::extract(chunk, neighbors, min_chunk_y, max_chunk_y);
    let mut cell_data = generate_cell_vertices_from_snapshot(&snapshot, materials);
    let nb = NeighborBoundaries::empty();
    let indices = generate_faces_from_snapshot(&mut cell_data, &snapshot, &nb);
    ChunkMeshData {
        vertices: cell_data.vertices,
        indices,
    }
}