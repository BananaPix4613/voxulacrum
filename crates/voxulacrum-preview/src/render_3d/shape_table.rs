//! Per-(ShapeId, Rotation) triangle table for the voxel mesher.
//!
//! Base-orientation triangles for the 4 distinct base shapes (Cube,
//! SlopeN, OuterCornerNW, InnerCornerSE) are defined as `const` arrays
//! below. Rotated variants (SlopeE/S/W, the other 3 outer corners, the
//! other 3 inner corners) are generated on first access by applying a
//! 90°-CW-around-Y rotation transform to each base triangle's position,
//! normal, and conditional-render direction.
//!
//! ## Y-axis rotation convention
//!
//! Per `docs/voxel_shape_atlas_spec.md` §5.1, 90° CW around Y (looking
//! down) maps the unit-cube coordinate `(x, z)` to `(1 - z, x)`. The Y
//! component is invariant. The same transform is applied to normals
//! (without the translate-back) and to neighbor directions:
//! `+X → +Z → -X → -Z → +X` (the horizontal-direction cycle).
//!
//! ## Shape canonicalization
//!
//! Each of the 14 ShapeId variants maps to a (base shape, number of CW
//! quarter-turns) pair via [`canonical`]. Cube and Empty are their own
//! base; slopes derive from SlopeN; outer corners from OuterCornerNW;
//! inner corners from InnerCornerSE. The voxel's own `rotation` field
//! composes on top (added modulo 4).

use std::sync::OnceLock;

use voxel_core::{Rotation, ShapeId};

// ---------------------------------------------------------------------------
// Cube corners (atlas spec §4 labeling).
// ---------------------------------------------------------------------------

const A: [f32; 3] = [0.0, 0.0, 0.0]; // (-X, -Y, -Z)
const B: [f32; 3] = [1.0, 0.0, 0.0]; // (+X, -Y, -Z)
const C: [f32; 3] = [1.0, 1.0, 0.0]; // (+X, +Y, -Z)
const D: [f32; 3] = [0.0, 1.0, 0.0]; // (-X, +Y, -Z)
const E: [f32; 3] = [0.0, 0.0, 1.0]; // (-X, -Y, +Z)
const F: [f32; 3] = [1.0, 0.0, 1.0]; // (+X, -Y, +Z)
const G: [f32; 3] = [1.0, 1.0, 1.0]; // (+X, +Y, +Z)
const H: [f32; 3] = [0.0, 1.0, 1.0]; // (-X, +Y, +Z)

// ---------------------------------------------------------------------------
// Public types.
// ---------------------------------------------------------------------------

/// One of the 6 axis-aligned neighbor directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NeighborDir {
    PosX, NegX, PosY, NegY, PosZ, NegZ,
}

impl NeighborDir {
    /// Voxel-index offset toward this neighbor.
    pub const fn offset(self) -> [i32; 3] {
        match self {
            Self::PosX => [ 1,  0,  0],
            Self::NegX => [-1,  0,  0],
            Self::PosY => [ 0,  1,  0],
            Self::NegY => [ 0, -1,  0],
            Self::PosZ => [ 0,  0,  1],
            Self::NegZ => [ 0,  0, -1],
        }
    }
}

/// When this triangle should be emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceCondition {
    /// Always drawn (slope surfaces, cut faces).
    Always,
    /// Drawn only when the neighbor in `dir` is air.
    WhenAirAt(NeighborDir),
}

/// One triangle in cube-local space (verts in [0,1]³, CCW from outside).
#[derive(Clone, Copy, Debug)]
pub struct TriangleSpec {
    pub verts:     [[f32; 3]; 3],
    pub normal:    [f32; 3],
    pub condition: FaceCondition,
}

// ---------------------------------------------------------------------------
// Base-orientation triangle tables (4 distinct shapes).
// ---------------------------------------------------------------------------

/// Full cube — 12 triangles, all conditional. Same winding convention as
/// the Phase-8 mesher (`u × v = outward_normal`).
const CUBE_TRIS: &[TriangleSpec] = &[
    // +X
    TriangleSpec { verts: [B, C, G], normal: [ 1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosX) },
    TriangleSpec { verts: [B, G, F], normal: [ 1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosX) },
    // -X
    TriangleSpec { verts: [A, E, H], normal: [-1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegX) },
    TriangleSpec { verts: [A, H, D], normal: [-1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegX) },
    // +Y
    TriangleSpec { verts: [D, H, G], normal: [ 0.0,  1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosY) },
    TriangleSpec { verts: [D, G, C], normal: [ 0.0,  1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosY) },
    // -Y
    TriangleSpec { verts: [A, B, F], normal: [ 0.0, -1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegY) },
    TriangleSpec { verts: [A, F, E], normal: [ 0.0, -1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegY) },
    // +Z
    TriangleSpec { verts: [E, F, G], normal: [ 0.0,  0.0,  1.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosZ) },
    TriangleSpec { verts: [E, G, H], normal: [ 0.0,  0.0,  1.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosZ) },
    // -Z
    TriangleSpec { verts: [A, D, C], normal: [ 0.0,  0.0, -1.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegZ) },
    TriangleSpec { verts: [A, C, B], normal: [ 0.0,  0.0, -1.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegZ) },
];

/// `SlopeN` — high edge along `(A→D)` at `-Z`, low edge along `(E→F)` at
/// `+Z`. Slope surface is the quad `D-C-F-E` (atlas §4 shape 1).
const SLOPE_N_NORMAL: [f32; 3] = [0.0, 0.7071068, 0.7071068];
const SLOPE_N_TRIS: &[TriangleSpec] = &[
    // Slope quad — ALWAYS. Triangulate the quad D-E-F-C (CCW from outside —
    // i.e., viewed from the air side which sits in +Y/+Z) so each triangle's
    // (v1-v0) × (v2-v0) equals SLOPE_N_NORMAL = (0, +√2/2, +√2/2).
    TriangleSpec { verts: [D, E, F], normal: SLOPE_N_NORMAL, condition: FaceCondition::Always },
    TriangleSpec { verts: [D, F, C], normal: SLOPE_N_NORMAL, condition: FaceCondition::Always },
    // -Z full quad (front)
    TriangleSpec { verts: [A, D, C], normal: [ 0.0,  0.0, -1.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegZ) },
    TriangleSpec { verts: [A, C, B], normal: [ 0.0,  0.0, -1.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegZ) },
    // -X half-face (west) triangle
    TriangleSpec { verts: [A, E, D], normal: [-1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegX) },
    // +X half-face (east) triangle
    TriangleSpec { verts: [B, C, F], normal: [ 1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosX) },
    // -Y full quad (bottom)
    TriangleSpec { verts: [A, B, F], normal: [ 0.0, -1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegY) },
    TriangleSpec { verts: [A, F, E], normal: [ 0.0, -1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegY) },
];

/// `OuterCornerNW` — high corner at `D = (-X, +Y, -Z)`. Slopes down toward
/// `+X` and `+Z` (atlas §4 shape 2). The two slope-triangle normals are
/// `(+0.707, +0.707, 0)` (toward `+X`) and `(0, +0.707, +0.707)` (toward `+Z`).
const OUTER_CORNER_NW_SLOPE_EAST_N: [f32; 3] = [0.7071068, 0.7071068, 0.0];
const OUTER_CORNER_NW_SLOPE_SOUTH_N: [f32; 3] = [0.0, 0.7071068, 0.7071068];
const OUTER_CORNER_NW_TRIS: &[TriangleSpec] = &[
    // Slope toward +X (face replaces +X and partially +Y) — ALWAYS
    TriangleSpec { verts: [D, F, B], normal: OUTER_CORNER_NW_SLOPE_EAST_N, condition: FaceCondition::Always },
    // Slope toward +Z (face replaces +Z and partially +Y) — ALWAYS
    TriangleSpec { verts: [D, E, F], normal: OUTER_CORNER_NW_SLOPE_SOUTH_N, condition: FaceCondition::Always },
    // -Z half-face triangle
    TriangleSpec { verts: [A, D, B], normal: [ 0.0,  0.0, -1.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegZ) },
    // -X half-face triangle
    TriangleSpec { verts: [A, E, D], normal: [-1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegX) },
    // -Y full quad (bottom)
    TriangleSpec { verts: [A, B, F], normal: [ 0.0, -1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegY) },
    TriangleSpec { verts: [A, F, E], normal: [ 0.0, -1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegY) },
];

/// `InnerCornerSE` — cube with the corner tetrahedron at `G = (+X, +Y, +Z)`
/// removed; cut face is triangle `C-H-F` (atlas §4 shape 3). The cut face's
/// outward normal is `(1, 1, 1) / √3`.
const INNER_CORNER_SE_CUT_N: [f32; 3] = [0.5773503, 0.5773503, 0.5773503];
const INNER_CORNER_SE_TRIS: &[TriangleSpec] = &[
    // Cut face — ALWAYS
    TriangleSpec { verts: [C, H, F], normal: INNER_CORNER_SE_CUT_N, condition: FaceCondition::Always },
    // +Y partial (single triangle — the surviving slice of the top)
    TriangleSpec { verts: [D, H, C], normal: [ 0.0,  1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosY) },
    // +X partial triangle
    TriangleSpec { verts: [B, C, F], normal: [ 1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosX) },
    // +Z partial triangle
    TriangleSpec { verts: [E, F, H], normal: [ 0.0,  0.0,  1.0], condition: FaceCondition::WhenAirAt(NeighborDir::PosZ) },
    // -Z full quad
    TriangleSpec { verts: [A, D, C], normal: [ 0.0,  0.0, -1.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegZ) },
    TriangleSpec { verts: [A, C, B], normal: [ 0.0,  0.0, -1.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegZ) },
    // -X full quad
    TriangleSpec { verts: [A, E, H], normal: [-1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegX) },
    TriangleSpec { verts: [A, H, D], normal: [-1.0,  0.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegX) },
    // -Y full quad
    TriangleSpec { verts: [A, B, F], normal: [ 0.0, -1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegY) },
    TriangleSpec { verts: [A, F, E], normal: [ 0.0, -1.0,  0.0], condition: FaceCondition::WhenAirAt(NeighborDir::NegY) },
];

// ---------------------------------------------------------------------------
// Canonicalization: each ShapeId → (base shape, CW quarter-turns around Y,
// vertical flip). Ceiling shapes are vertical mirrors of their floor
// counterparts and reuse the same base tables.
// ---------------------------------------------------------------------------

fn canonical(shape: ShapeId) -> (ShapeId, u8, bool) {
    use ShapeId::*;
    match shape {
        Empty                     => (Empty,           0, false),
        Cube                      => (Cube,            0, false),
        // Slopes derive from SlopeN: -Z → +X → +Z → -X (CW from above).
        SlopeN                    => (SlopeN,          0, false),
        SlopeE                    => (SlopeN,          1, false),
        SlopeS                    => (SlopeN,          2, false),
        SlopeW                    => (SlopeN,          3, false),
        // Outer corners derive from OuterCornerNW: NW → NE → SE → SW.
        OuterCornerNW             => (OuterCornerNW,   0, false),
        OuterCornerNE             => (OuterCornerNW,   1, false),
        OuterCornerSE             => (OuterCornerNW,   2, false),
        OuterCornerSW             => (OuterCornerNW,   3, false),
        // Inner corners derive from InnerCornerSE: SE → SW → NW → NE.
        InnerCornerSE             => (InnerCornerSE,   0, false),
        InnerCornerSW             => (InnerCornerSE,   1, false),
        InnerCornerNW             => (InnerCornerSE,   2, false),
        InnerCornerNE             => (InnerCornerSE,   3, false),
        // Ceiling slopes: vertical mirrors of their floor counterparts.
        CeilingSlopeN             => (SlopeN,          0, true),
        CeilingSlopeE             => (SlopeN,          1, true),
        CeilingSlopeS             => (SlopeN,          2, true),
        CeilingSlopeW             => (SlopeN,          3, true),
        // Ceiling outer corners.
        CeilingOuterCornerNW      => (OuterCornerNW,   0, true),
        CeilingOuterCornerNE      => (OuterCornerNW,   1, true),
        CeilingOuterCornerSE      => (OuterCornerNW,   2, true),
        CeilingOuterCornerSW      => (OuterCornerNW,   3, true),
    }
}

fn base_table(base: ShapeId) -> &'static [TriangleSpec] {
    match base {
        ShapeId::Empty         => &[],
        ShapeId::Cube          => CUBE_TRIS,
        ShapeId::SlopeN        => SLOPE_N_TRIS,
        ShapeId::OuterCornerNW => OUTER_CORNER_NW_TRIS,
        ShapeId::InnerCornerSE => INNER_CORNER_SE_TRIS,
        other => panic!("base_table called with non-base shape {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Rotation transforms (atlas spec §5.1).
// ---------------------------------------------------------------------------

fn rotate_pos_cw90(p: [f32; 3], turns: u8) -> [f32; 3] {
    // Center at (0.5, 0.5, 0.5); rotate around Y; translate back.
    let mut x = p[0] - 0.5;
    let mut z = p[2] - 0.5;
    for _ in 0..(turns % 4) {
        let (nx, nz) = (-z, x); // (x, z) → (-z, x) in centred space
        x = nx;
        z = nz;
    }
    [x + 0.5, p[1], z + 0.5]
}

fn rotate_normal_cw90(n: [f32; 3], turns: u8) -> [f32; 3] {
    let mut x = n[0];
    let mut z = n[2];
    for _ in 0..(turns % 4) {
        let (nx, nz) = (-z, x);
        x = nx;
        z = nz;
    }
    [x, n[1], z]
}

fn rotate_neighbor_cw90(d: NeighborDir, turns: u8) -> NeighborDir {
    use NeighborDir::*;
    let mut d = d;
    for _ in 0..(turns % 4) {
        d = match d {
            PosX => PosZ,
            PosZ => NegX,
            NegX => NegZ,
            NegZ => PosX,
            PosY => PosY,
            NegY => NegY,
        };
    }
    d
}

fn rotate_triangle(t: &TriangleSpec, turns: u8) -> TriangleSpec {
    TriangleSpec {
        verts: [
            rotate_pos_cw90(t.verts[0], turns),
            rotate_pos_cw90(t.verts[1], turns),
            rotate_pos_cw90(t.verts[2], turns),
        ],
        normal: rotate_normal_cw90(t.normal, turns),
        condition: match t.condition {
            FaceCondition::Always => FaceCondition::Always,
            FaceCondition::WhenAirAt(d) => FaceCondition::WhenAirAt(rotate_neighbor_cw90(d, turns)),
        },
    }
}

// ---------------------------------------------------------------------------
// Vertical flip transform (used to derive ceiling shapes from their floor
// counterparts). Reflecting `y → 1 - y` inverts triangle orientation, so the
// winding must be reversed to keep CCW-from-outside.
// ---------------------------------------------------------------------------

fn flip_pos_y(p: [f32; 3]) -> [f32; 3] {
    [p[0], 1.0 - p[1], p[2]]
}

fn flip_normal_y(n: [f32; 3]) -> [f32; 3] {
    [n[0], -n[1], n[2]]
}

fn flip_neighbor_y(d: NeighborDir) -> NeighborDir {
    match d {
        NeighborDir::PosY => NeighborDir::NegY,
        NeighborDir::NegY => NeighborDir::PosY,
        other => other,
    }
}

fn vertical_flip_triangle(t: &TriangleSpec) -> TriangleSpec {
    TriangleSpec {
        // Reverse winding (swap verts[1] and verts[2]) to compensate for
        // the reflection's orientation inversion.
        verts: [
            flip_pos_y(t.verts[0]),
            flip_pos_y(t.verts[2]),
            flip_pos_y(t.verts[1]),
        ],
        normal: flip_normal_y(t.normal),
        condition: match t.condition {
            FaceCondition::Always => FaceCondition::Always,
            FaceCondition::WhenAirAt(d) => FaceCondition::WhenAirAt(flip_neighbor_y(d)),
        },
    }
}

// ---------------------------------------------------------------------------
// Expanded per-ShapeId cache. 22 entries (14 floor + 8 ceiling); built once
// on first access.
// ---------------------------------------------------------------------------

const SHAPE_COUNT: usize = (ShapeId::MAX_DISCRIMINANT as usize) + 1;

static SHAPE_CACHE: OnceLock<[Vec<TriangleSpec>; SHAPE_COUNT]> = OnceLock::new();

fn shape_cache() -> &'static [Vec<TriangleSpec>; SHAPE_COUNT] {
    SHAPE_CACHE.get_or_init(|| {
        std::array::from_fn(|idx| {
            let shape = ShapeId::from_raw(idx as u8)
                .expect("ShapeId discriminants 0..=MAX_DISCRIMINANT are all valid");
            let (base, turns, flip) = canonical(shape);
            base_table(base)
                .iter()
                .map(|t| {
                    let rotated = rotate_triangle(t, turns);
                    if flip { vertical_flip_triangle(&rotated) } else { rotated }
                })
                .collect()
        })
    })
}

/// Triangle list for a voxel of `(shape, voxel_rotation)`, in cube-local
/// coordinates. The voxel's own `Rotation` field composes on top of the
/// shape's intrinsic orientation.
pub fn shape_geometry(shape: ShapeId, voxel_rotation: Rotation) -> Vec<TriangleSpec> {
    let extra_turns = voxel_rotation_turns(voxel_rotation);
    let base = &shape_cache()[shape as usize];
    if extra_turns == 0 {
        return base.clone();
    }
    base.iter().map(|t| rotate_triangle(t, extra_turns)).collect()
}

// ---------------------------------------------------------------------------
// Face-coverage predicate. Used by the mesher to decide whether a neighbor
// shape's face fully blocks a cube-aligned face on this voxel — i.e., the
// cube-aligned face can be culled.
//
// "Full coverage" means the neighbor's geometry in that direction is a
// 1×1 axis-aligned quad coincident with the cube boundary. Slope surfaces,
// cut faces, and half-face triangles do NOT count as full coverage — the
// neighbor through them would still see the cube face peeking out.
// ---------------------------------------------------------------------------

/// True iff `shape` (rotated by `rotation`) presents a full 1×1 quad on its
/// face in direction `dir`, such that a cube neighbor in that direction can
/// safely cull its opposing face.
pub fn covers_face(shape: ShapeId, rotation: Rotation, dir: NeighborDir) -> bool {
    let (base, intrinsic, flip) = canonical(shape);
    let total = (intrinsic + voxel_rotation_turns(rotation)) % 4;
    // Geometry was: base → rotate(total) → flip(if). To map a world `dir`
    // back to the base orientation, un-flip first, then un-rotate.
    let after_unflip = if flip { flip_neighbor_y(dir) } else { dir };
    let base_dir = rotate_neighbor_cw90(after_unflip, (4 - total) % 4);
    base_covers(base, base_dir)
}

fn base_covers(base: ShapeId, dir: NeighborDir) -> bool {
    use NeighborDir::*;
    use ShapeId::*;
    match base {
        Empty => false,
        Cube => true,
        // SlopeN: full quad on -Z (front wall) and -Y (bottom). All others
        // are slope surface or half-face triangles.
        SlopeN => matches!(dir, NegZ | NegY),
        // OuterCornerNW: only -Y is a full quad. -Z and -X are half-face
        // triangles; +Y/+Z/+X are slope surfaces.
        OuterCornerNW => matches!(dir, NegY),
        // InnerCornerSE: full quads on -Z, -X, -Y (the three "interior"
        // sides of the cube-minus-corner-tetra). +Y/+X/+Z are partial.
        InnerCornerSE => matches!(dir, NegZ | NegX | NegY),
        other => panic!("base_covers called with non-base shape {other:?}"),
    }
}

fn voxel_rotation_turns(rotation: Rotation) -> u8 {
    match rotation {
        Rotation::None  => 0,
        Rotation::Cw90  => 1,
        Rotation::Cw180 => 2,
        Rotation::Cw270 => 3,
    }
}
