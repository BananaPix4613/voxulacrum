# Voxel Shape Atlas Meshing System — Implementation Specification

## Purpose

This document is a complete implementation specification for replacing a Marching Cubes (MC) voxel meshing system with a **per-voxel shape atlas** system in a Rust isometric voxel game engine. The agent implementing this should treat this document as the authoritative reference. Read it fully before beginning implementation.

---

## Table of Contents

1. [Design Goals and Constraints](#1-design-goals-and-constraints)
2. [Terminology and Coordinate System](#2-terminology-and-coordinate-system)
3. [Voxel Data Format](#3-voxel-data-format)
4. [The Shape Catalog](#4-the-shape-catalog)
5. [Shape Rotation System](#5-shape-rotation-system)
6. [Shape Classification Algorithm](#6-shape-classification-algorithm)
7. [Meshing Pipeline](#7-meshing-pipeline)
8. [Chunk Border Strategy](#8-chunk-border-strategy)
9. [LOD System](#9-lod-system)
10. [Implementation Phases](#10-implementation-phases)

---

## 1. Design Goals and Constraints

### Hard Constraints

- **Vertex constraint**: Every vertex in every shape must lie on one of the 8 corners of the unit voxel cube. Coordinates are restricted to `{0, 1}` on each axis. No midpoints (`0.5`), no interpolation. This eliminates peaks, valleys, ridges, and half-slabs from the shape catalog.
- **Per-voxel shapes**: Each solid voxel independently selects exactly one shape from the catalog based on its local neighbor configuration. The shape is self-contained within the voxel's unit cube.
- **Neighbor-agnostic meshing**: A chunk must be meshable with at most a 1-voxel-thick border from face-adjacent chunks. It must NOT require data from all 26 neighbors. The system should support a fallback mode where chunks mesh with zero neighbor data (assuming air at borders) and re-mesh only the outermost slice when a neighbor arrives.

### Design Goals (prioritized)

1. **Individual voxel identifiability**: A player must be able to visually distinguish where one voxel ends and another begins. Mining a single voxel must produce a clear, predictable geometric change.
2. **Fast chunk loading**: The meshing pipeline must support rapid chunk loading during fast travel. Per-voxel shape lookup from a table is faster than MC's per-cell isosurface extraction.
3. **Isometric/orthographic visual clarity**: Shapes must produce geometry that reads well in orthographic projection. Large flat faces with sharp predictable edges. The 45° diagonals in the catalog align with isometric projection axes.
4. **Interesting geometry**: The world should not look like plain Minecraft cubes. Slopes, diagonal walls, and compound shapes create terrain variety while remaining geometrically structured.
5. **LOD support**: The same shape catalog and classification pipeline must work at multiple resolutions for a bird's-eye map view.

---

## 2. Terminology and Coordinate System

### Voxel-Local Coordinates

Each voxel occupies a unit cube from `(0, 0, 0)` to `(1, 1, 1)` in its local space. The 8 corners are labeled:

```
    D -------- C          Y (up)
   /|         /|          |
  H -------- G |          |
  | |        | |          +---- X (east)
  | A -------| B         /
  |/         |/          Z (south/depth)
  E -------- F
```

| Label | Coordinates | Position Description |
|-------|-------------|---------------------|
| A     | (0, 0, 0)   | Bottom-front-left (origin) |
| B     | (1, 0, 0)   | Bottom-front-right |
| C     | (1, 1, 0)   | Top-front-right |
| D     | (0, 1, 0)   | Top-front-left |
| E     | (0, 0, 1)   | Bottom-back-left |
| F     | (1, 0, 1)   | Bottom-back-right |
| G     | (1, 1, 1)   | Top-back-right |
| H     | (0, 1, 1)   | Top-back-left |

**Axes**: X = east, Y = up, Z = south (into screen in isometric view). The isometric camera looks from the direction of `(-X, +Y, -Z)` toward `(+X, -Y, +Z)`, meaning the viewer sees the **top** (Y+), **front-right** (Z=0), and **front-left** (X=0) faces of a cube.

### Neighbor Directions

Face neighbors are referenced by the axis and direction:

| Index | Direction | Axis | Offset     |
|-------|-----------|------|------------|
| 0     | East      | +X   | (1, 0, 0)  |
| 1     | West      | -X   | (-1, 0, 0) |
| 2     | Up        | +Y   | (0, 1, 0)  |
| 3     | Down      | -Y   | (0, -1, 0) |
| 4     | South     | +Z   | (0, 0, 1)  |
| 5     | North     | -Z   | (0, 0, -1) |

Edge neighbors (12 total) are pairs of face directions. Corner neighbors (8 total) are triples. These use the same offset convention.

---

## 3. Voxel Data Format

### Current Format (to be replaced)

```rust
// OLD — continuous density field
struct ChunkStorage {
    density: Box<[i8; CHUNK_VOLUME]>,       // -128..127, positive = solid
    material_id: PalettedBitArray,           // palette-compressed material IDs
}
```

### New Format

```rust
/// A single voxel's data. Material ID of 0 (MAT_AIR) means the voxel is empty/air.
/// A non-zero material ID means the voxel is solid.
/// The shape is NOT stored — it is computed at mesh time from the neighbor configuration.

const CHUNK_SIZE: usize = 32;
const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE; // 32,768

/// Compact per-voxel storage. Occupancy is derived: solid = (material_id != MAT_AIR).
struct ChunkStorage {
    /// Palette-compressed material IDs. MAT_AIR (0) = empty, anything else = solid.
    /// Same PalettedBitArray as the existing system — no changes needed here.
    material_id: PalettedBitArray,
}

/// Uniform variant for homogeneous chunks (all air, all solid stone, etc.)
enum ChunkStorageVariant {
    /// Every voxel has the same material. If material == MAT_AIR, the chunk is fully empty.
    Uniform(u16),
    /// Heterogeneous chunk with per-voxel materials.
    Populated(ChunkStorage),
}
```

**Key change**: The `density: i8` field is eliminated entirely. Occupancy is now binary, derived from `material_id != MAT_AIR`. This halves the per-voxel storage (no separate density array) and simplifies the pipeline.

**Why no stored shape ID**: The shape for each voxel is a pure function of its neighbor configuration. Storing it would waste memory and create a cache invalidation problem (editing a voxel would require updating the stored shapes of all its neighbors). Computing it on the fly during meshing is cheap — it's a table lookup indexed by a bitmask.

### Indexing Convention

Flat array indexing within a chunk:

```rust
#[inline]
fn voxel_index(x: usize, y: usize, z: usize) -> usize {
    // Y-major for vertical slice iteration, then Z, then X
    // This layout optimizes for column-wise access patterns common in terrain generation
    y * CHUNK_SIZE * CHUNK_SIZE + z * CHUNK_SIZE + x
}

#[inline]
fn is_solid(storage: &ChunkStorageVariant, x: usize, y: usize, z: usize) -> bool {
    match storage {
        ChunkStorageVariant::Uniform(mat) => *mat != MAT_AIR,
        ChunkStorageVariant::Populated(s) => s.material_id.get(voxel_index(x, y, z)) != MAT_AIR,
    }
}
```

---

## 4. The Shape Catalog

Every shape in the catalog is defined by its vertex subset (which of the 8 cube corners are included in the solid volume) and its face list (triangulated for GPU upload). All shapes are defined in their **base orientation** and then rotated to produce all variants (see Section 5).

### 4.1 Shape Families Overview

| ID Range | Family              | Base Shapes | With Rotations | Volume   | Description |
|----------|---------------------|-------------|----------------|----------|-------------|
| 0        | Full Block          | 1           | 1              | 1.0      | Standard cube |
| 1–8      | Slope               | 1           | 8              | 0.5      | Wedge ramp, 45° terrain transition |
| 9–16     | Outer Corner        | 1           | 8              | ~0.33    | Convex 2-slope intersection |
| 17–24    | Inner Corner Fill   | 1           | 8              | ~0.67    | Large concave fill, complement of outer corner |
| 25–28    | Vertical Wedge      | 1           | 4              | 0.5      | Diagonal vertical wall (45° to grid axes) |
| 29–36    | Anti-Tetrahedron    | 1           | 8              | ~0.83    | Cube with one corner chamfered (subtle bevel) |
| 37–44    | Micro Wedge         | 1           | 8              | ~0.17    | Small tetrahedral corner fill |
| 45–52    | Slope + Vert. Wedge | 1           | 8              | ~0.25    | Compound: slope intersected with vertical diagonal |

**Total: 53 shape variants** (including the full block at index 0).

### 4.2 Shape Definitions — Base Orientations

Each shape is defined by:
- **Vertices**: Which cube corners are part of the solid
- **Faces**: Ordered vertex lists with outward-facing winding (counter-clockwise when viewed from outside). All faces are either triangles or quads. Quads should be split into two triangles for GPU upload.
- **Exposed faces**: Which faces are always rendered (slope surfaces, diagonal cuts). Other faces (cube faces shared with solid neighbors) are only rendered when the neighbor is air.

**IMPORTANT**: In the face vertex lists below, vertices are ordered counter-clockwise as seen from outside the shape (outward normal via right-hand rule). This winding is critical for backface culling.

---

#### Shape 0: Full Block

The standard cube. Every surface voxel defaults to this unless a neighbor pattern triggers a more specific shape.

```
Vertices: A B C D E F G H (all 8)

Faces (rendered only when the adjacent neighbor is air):
  +Y (top):    [D, H, G, C]    — render when neighbor at +Y is air
  -Y (bottom): [A, B, F, E]    — render when neighbor at -Y is air
  +X (east):   [B, C, G, F]    — render when neighbor at +X is air
  -X (west):   [A, E, H, D]    — render when neighbor at -X is air
  +Z (south):  [E, F, G, H]    — render when neighbor at +Z is air
  -Z (north):  [A, D, C, B]    — render when neighbor at -Z is air
```

---

#### Shape 1 (base): Slope

A wedge/ramp. In base orientation: the top edge at Z=0 (front) is at full height, the bottom edge at Z=1 (back) is at ground level. The slope surface faces upward-and-backward.

```
Solid corners: A B C D E F (6 vertices)
Removed corners: G H (the top-back edge is gone)

           D -------- C
          /          /
  (removed H/G)    /
        /   /      /
       /   A ---- B
      /   /      /
     E - - - - F

Faces:
  slope:       [D, C, F, E]    — ALWAYS rendered (the diagonal surface)
  -Z (front):  [A, D, C, B]    — full quad, render when -Z neighbor is air
  -X (west):   [A, E, D]       — triangle (half face), render when -X neighbor is air
  +X (east):   [B, C, F]       — triangle (half face), render when +X neighbor is air
  -Y (bottom): [A, B, F, E]    — full quad, render when -Y neighbor is air

Slope face normal (base orientation): (0, 1, -1) normalized = (0, 0.707, -0.707)
The slope normal points upward and toward -Z.
```

**Half-face matching rule**: When a slope's triangular half-face is adjacent to a full block's full quad face, the full block renders its complete face. The slope's triangle is a geometric subset — no gap occurs. This is why the system tiles seamlessly without explicit stitching.

---

#### Shape 2 (base): Outer Corner

The convex hilltop corner. Created by intersecting two slope planes. In base orientation: the single high corner is at D = (0, 1, 0). The shape slopes down toward both +X and +Z.

```
Solid corners: A B D E F (5 vertices)
Removed corners: C G H

The shape is a square-based pyramid with base ABFE at Y=0 and apex D at Y=1,
but D is at corner (0,1,0), not centered — it's a "leaning pyramid."

         D
        /|\
       / | \
      /  |  \
     /   |   \
    A----+----B
    |  (base) |
    E---------F

Faces:
  slope_xz:    [D, B, F]       — triangle, ALWAYS rendered (slope toward +X)
  slope_yz:    [D, F, E]       — triangle, ALWAYS rendered (slope toward +Z)
  -Z (front):  [A, D, B]       — triangle (half face), render when -Z neighbor is air
  -X (west):   [A, E, D]       — triangle (half face), render when -X neighbor is air
  -Y (bottom): [A, B, F, E]    — full quad, render when -Y neighbor is air

Note: This shape has NO +X face and NO +Z face — those directions are covered by the slope surfaces.
```

**Cutting planes that produce this shape**:
- Plane 1 (slope down +X): `x + y = 1` — passes through D(0,1,0), H(0,1,1), F(1,0,1), B(1,0,0)
- Plane 2 (slope down +Z): `y + z = 1` — passes through D(0,1,0), C(1,1,0), F(1,0,1), E(0,0,1)
- The shape is the intersection: `{x + y ≤ 1} ∩ {y + z ≤ 1}` within the unit cube.

---

#### Shape 3 (base): Inner Corner Fill

The concave fill piece. This is a full cube with the corner tetrahedron at G = (1, 1, 1) removed. It fills the concave gap where two slopes meet at the inside of a valley bend.

```
Solid corners: A B C D E F H (7 vertices)
Removed corners: G

The shape is a cube with a triangular notch cut from the top-back-right corner.

    D -------- C
   /|         /
  H -------  /   (G removed, cut face C-F-H)
  | |      |/
  | A -----|- B
  |/       |/
  E -------F

Faces:
  cut_face:    [C, F, H]       — triangle, ALWAYS rendered (the diagonal cut surface)
  +Y (top):    [D, H, C]       — triangle (partial top), render when +Y neighbor is air
  +X (east):   [B, C, F]       — triangle (partial east), render when +X neighbor is air
  +Z (south):  [E, F, H]       — triangle (partial south), render when +Z neighbor is air
  -Z (north):  [A, D, C, B]    — full quad, render when -Z neighbor is air
  -X (west):   [A, E, H, D]    — full quad, render when -X neighbor is air
  -Y (bottom): [A, B, F, E]    — full quad, render when -Y neighbor is air
```

**Cutting plane**: Triangle through C(1,1,0), F(1,0,1), H(0,1,1) — removes the tetrahedron G-C-F-H (volume ≈ 1/6 of cube). The shape retains ≈ 5/6 volume.

**Distinction from anti-tetrahedron**: The inner corner fill IS a type of anti-tetrahedron cut (specifically the cube minus a corner tetra). However, the inner corner fill is classified differently because its *neighbor trigger* is different — it activates at concave terrain bends, not at arbitrary wall intersections.

---

#### Shape 4 (base): Vertical Wedge

A triangular prism created by a vertical diagonal cut through the cube. In base orientation: the cutting plane goes from the bottom-front-left edge (A–D) to the top-back-right edge (F–G)... actually, the cutting plane is vertical and passes through corners on a 45° diagonal in the XZ plane.

**Base orientation**: The cut plane passes through A(0,0,0), C(1,1,0), G(1,1,1), E(0,0,1) — the plane `x = y` but expressed through four cube corners. This is a vertical plane. We keep the side where `x ≥ y` (containing B and F).

```
Solid corners: A B C E F G (6 vertices — the "right" half)
Removed corners: D H (the "left" half above the diagonal)

Side view (looking along Z-axis):

  D---C        becomes:      /C
  |   |                     / |
  |   |                    /  |
  A---B                   A---B

The shape is a triangular prism extruded along Z.

Faces:
  diagonal:    [A, C, G, E]    — quad, ALWAYS rendered (the 45° vertical wall face)
  +X (east):   [B, C, G, F]    — full quad, render when +X neighbor is air
  -Z (north):  [A, B, C]       — triangle (half face), render when -Z neighbor is air
  +Z (south):  [E, F, G]       — triangle (half face), render when +Z neighbor is air
  -Y (bottom): [A, B, F, E]    — full quad, render when -Y neighbor is air

Note: The diagonal face replaces both the -X and +Y faces.
The surface normal of the diagonal face points toward (-X, +Y, 0) = (-0.707, 0.707, 0).
```

**Why this shape matters**: MC cannot produce true vertical diagonal walls because MC interpolates density gradients, which always produces surfaces perpendicular to the gradient. A density field cannot represent a vertical 45° plane through a cell. This shape is unique to the atlas system and creates cliff faces that align with isometric projection axes — they look intentional and structured rather than aliased.

**The other vertical diagonal** (cut plane through B, D, F, H — the plane `x + y = 1`): This is generated by the rotation system (a 90° rotation of the base vertical wedge). See Section 5.

---

#### Shape 5 (base): Anti-Tetrahedron (Corner Chamfer)

A full cube with one small corner tetrahedron beveled off. In base orientation: the chamfer is at corner A = (0, 0, 0).

```
Solid corners: B C D E F G H (7 vertices)
Removed corner: A

The tetrahedron removed is A-B-D-E (the three face-neighbors of A).

    D -------- C
   /|         /|
  H -------- G |
  | |        | |
  |  --------|-- B
  |/    ↗    |/
  E -------- F
      (A removed — tiny triangular bevel at bottom-front-left)

Faces:
  chamfer:     [B, D, E]       — triangle, ALWAYS rendered (the bevel surface)
  -Z (north):  [B, C, D]       — triangle (partial), render when -Z neighbor is air
  -X (west):   [D, H, E]       — triangle (partial), render when -X neighbor is air
  -Y (bottom): [B, F, E]       — triangle (partial), render when -Y neighbor is air
  +Y (top):    [D, C, G, H]    — full quad, render when +Y neighbor is air
  +X (east):   [B, C, G, F]    — full quad, render when +X neighbor is air
  +Z (south):  [E, F, G, H]    — full quad, render when +Z neighbor is air

Chamfer normal: the outward normal of triangle B(1,0,0)-D(0,1,0)-E(0,0,1) points toward (-1,-1,-1) normalized.
```

**Usage**: The chamfer is subtle — it removes only ~1/6 of the cube. It serves as a geometric detail at wall corners and intersections, catching light differently from flat surfaces. Use material properties to control which materials get chamfered (e.g., stone yes, dirt no).

---

#### Shape 6 (base): Micro Wedge (Corner Tetrahedron)

The small tetrahedral corner piece. In base orientation: the tetrahedron at corner D = (0, 1, 0).

```
Solid corners: D A C H (4 vertices)
This is the tetrahedron formed by corner D and its 3 face-adjacent neighbors.

         D
        /|\
       / | \
      /  |  \
     A   |   C
      \  |  /
       \ | /
        \|/
         H

Wait — let's be precise. D's three face-neighbors on the cube are:
  - A = (0, 0, 0)  — neighbor along -Y
  - C = (1, 1, 0)  — neighbor along +X
  - H = (0, 1, 1)  — neighbor along +Z

Faces:
  face_xy:     [D, A, C]       — triangle, render when appropriate (on the Z=0 plane)
  face_xz:     [D, C, H]       — triangle, render when appropriate (on the X+ / Y+ diagonal)
  face_yz:     [D, H, A]       — triangle, render when appropriate (on the X=0 plane)
  base:        [A, H, C]       — triangle, ALWAYS rendered (the internal diagonal face)

Note: "base" here is the triangle that separates this tetrahedron from the rest of the cube space.
Its normal points inward (toward the cube center).
```

**Usage**: This is a very small piece (~1/6 cube volume). It fills the triangular gap at corners where three slopes converge — the fine detail complement to the larger outer corner. In practice, this shape may appear rarely in natural terrain but becomes important for player-built structures with beveled edges.

---

#### Shape 7 (base): Slope + Vertical Wedge Compound

The intersection of a terrain slope with a vertical diagonal cut. This creates a tetrahedron at a specific corner where a ramp meets a diagonal cliff. In base orientation:

```
Cutting planes:
  - Slope (y + z ≤ 1): high at Z=0, low at Z=1
  - Vertical wedge (x ≤ y): keep the "left/upper" side of the diagonal wall

Surviving corners (both conditions satisfied):
  A(0,0,0): y+z=0 ≤ 1 ✓, x=0 ≤ y=0 ✓ (boundary)  →  included
  C(1,1,0): y+z=1 ≤ 1 ✓, x=1 ≤ y=1 ✓ (boundary)  →  included
  D(0,1,0): y+z=1 ≤ 1 ✓, x=0 ≤ y=1 ✓              →  included
  E(0,0,1): y+z=1 ≤ 1 ✓, x=0 ≤ y=0 ✓ (boundary)  →  included
  All others violate at least one condition.

Solid corners: A C D E (4 vertices) — tetrahedron

Faces:
  slope_face:  [D, C, E]       — triangle, ALWAYS rendered (the slope surface portion)
  vert_face:   [A, C, D]       — triangle, ALWAYS rendered (the vertical diagonal portion)
  -X (west):   [A, D, E]       — triangle, render when -X neighbor is air
  base:        [A, E, C]       — triangle, render when appropriate (bottom/back face)
```

**Usage**: This shape appears at the junction where a terrain ramp meets a 45° cliff face. It's a compound cut that neither MC nor standard autotiling systems produce. It creates distinctive angular geometry at geological transitions.

---

### 4.3 Face Triangulation for GPU Upload

All quad faces must be split into two triangles for the GPU vertex buffer. Use a consistent diagonal split:

```rust
// For a quad face [V0, V1, V2, V3] (in CCW order):
// Triangle 1: [V0, V1, V2]
// Triangle 2: [V0, V2, V3]
```

Each vertex in the GPU buffer should contain:

```rust
#[repr(C)]
struct MeshVertex {
    position: [f32; 3],   // World-space position (voxel_pos * VOXEL_SCALE + local_vertex_pos * VOXEL_SCALE)
    normal: [f32; 3],     // Face normal (for lighting)
    material_id: u16,     // Material for texture/color lookup
    ao: u8,               // Ambient occlusion value (0-255), computed per-vertex
    _padding: u8,
}
```

---

## 5. Shape Rotation System

All shapes are defined in a single base orientation. Rotation variants are generated by applying coordinate transforms to the vertex positions. This avoids storing redundant geometry and ensures consistency.

### 5.1 Rotation Transforms

There are 4 Y-axis rotations and an optional vertical flip, giving up to 8 variants per shape:

```rust
/// Rotates voxel-local coordinates (each component is 0 or 1) around the Y axis.
/// rot: 0 = 0°, 1 = 90° CW, 2 = 180°, 3 = 270° CW (when viewed from above)
fn rotate_y(x: u8, y: u8, z: u8, rot: u8) -> (u8, u8, u8) {
    match rot {
        0 => (x, y, z),
        1 => (1 - z, y, x),       // 90° CW: (x,z) → (1-z, x)
        2 => (1 - x, y, 1 - z),   // 180°:   (x,z) → (1-x, 1-z)
        3 => (z, y, 1 - x),       // 270° CW: (x,z) → (z, 1-x)
        _ => unreachable!()
    }
}

/// Flips vertically (for ceiling/inverted variants).
fn flip_y(x: u8, y: u8, z: u8) -> (u8, u8, u8) {
    (x, 1 - y, z)
}
```

### 5.2 Variant Generation

```rust
/// A shape variant is identified by: base_shape_id + rotation + flip
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct ShapeVariant {
    base: BaseShape,     // Which of the 8 base shapes
    rotation: u8,        // 0–3 (Y-axis rotation)
    flipped: bool,       // Vertical flip for ceiling variants
}

/// Compact shape ID for storage/lookup. Maps to 0..52.
type ShapeId = u8;

/// Convert a ShapeVariant to its compact ID.
impl ShapeVariant {
    fn to_id(&self) -> ShapeId {
        // Full block: 0
        // Slopes: 1..=8 (4 rotations × 2 flip states)
        // Outer corners: 9..=16
        // Inner corners: 17..=24
        // Vertical wedges: 25..=28 (4 variants, no flip — flip of a vertical wedge is a different rotation)
        // Anti-tetra: 29..=36
        // Micro wedge: 37..=44
        // Compound slope+vert: 45..=52
        // See the lookup table in Section 6 for the exact mapping.
        todo!()
    }
}
```

### 5.3 Rotating Face Normals

When rotating a shape, the face normals must be rotated by the same transform. The neighbor-direction checks for conditional face rendering must also be rotated:

```rust
/// Rotate a face-neighbor direction index (0..5) by the shape's rotation.
/// This maps "which face of the BASE shape" to "which face of the ROTATED shape".
fn rotate_face_dir(dir: u8, rot: u8) -> u8 {
    // +Y (2) and -Y (3) are unchanged by Y-axis rotation
    // +X (0), -X (1), +Z (4), -Z (5) rotate among themselves
    match dir {
        2 | 3 => dir,  // Y directions unchanged
        _ => {
            // Map horizontal directions through rotation
            let horizontal_dirs = [0, 4, 1, 5]; // +X, +Z, -X, -Z in CW order
            let idx = horizontal_dirs.iter().position(|&d| d == dir).unwrap();
            horizontal_dirs[(idx + rot as usize) % 4]
        }
    }
}
```

### 5.4 Generating the Shape Mesh Table

At startup (or compile time via `const`), generate a lookup table mapping each `ShapeId` to its pre-computed mesh data:

```rust
struct ShapeMeshData {
    /// Triangulated faces. Each face has a condition for when it should be rendered.
    faces: Vec<ShapeFace>,
}

struct ShapeFace {
    /// 3 vertices per triangle (already triangulated from quads).
    triangles: Vec<[VertexPos; 3]>,
    /// When to render this face:
    /// - `Always`: slope surfaces, cut faces — always in the mesh
    /// - `WhenAirAt(dir)`: cube faces — only rendered when the neighbor in `dir` is air
    condition: FaceCondition,
    /// Outward normal of this face (pre-rotated).
    normal: [f32; 3],
}

enum FaceCondition {
    Always,
    WhenAirAt(u8), // face direction index 0..5
}

/// Pre-computed at startup. Index by ShapeId (0..52).
static SHAPE_MESH_TABLE: [ShapeMeshData; 53] = /* generated */;
```

---

## 6. Shape Classification Algorithm

### 6.1 Input

For each solid surface voxel (a voxel that is solid and has at least one air face-neighbor), compute:

```rust
/// Bitmask of which face neighbors are solid. Bit i = 1 means neighbor in direction i is solid.
/// Directions: 0=+X, 1=-X, 2=+Y, 3=-Y, 4=+Z, 5=-Z
type FaceMask = u8; // bits 0..5 used

/// Bitmask of which edge neighbors are solid. 12 bits for 12 edge directions.
/// Edge directions are pairs of face directions:
///   0: +X+Y, 1: +X-Y, 2: -X+Y, 3: -X-Y
///   4: +X+Z, 5: +X-Z, 6: -X+Z, 7: -X-Z
///   8: +Y+Z, 9: +Y-Z, 10: -Y+Z, 11: -Y-Z
type EdgeMask = u16; // bits 0..11 used

/// Bitmask of which corner neighbors are solid. 8 bits for 8 corners.
///   0: +X+Y+Z, 1: +X+Y-Z, 2: +X-Y+Z, 3: +X-Y-Z
///   4: -X+Y+Z, 5: -X+Y-Z, 6: -X-Y+Z, 7: -X-Y-Z
type CornerMask = u8;
```

### 6.2 Classification Rules (Conservative — Prefer Full Blocks)

The classification is a priority cascade. The first matching rule wins. Rules are ordered from most-specific to least-specific (compound shapes first, then simple shapes, then full block as default).

```rust
fn classify_shape(face: FaceMask, edge: EdgeMask, corner: CornerMask, material: u16) -> ShapeId {
    // Material-based opt-out: some materials never get diagonals
    if !material_allows_diagonals(material) {
        return SHAPE_FULL_BLOCK; // 0
    }

    let air_above = (face & (1 << 2)) == 0;  // +Y neighbor is air
    let air_below = (face & (1 << 3)) == 0;  // -Y neighbor is air

    // ========== COMPOUND SHAPES (highest priority, most specific) ==========

    // Slope + Vertical Wedge: requires both a slope pattern AND a vertical diagonal pattern.
    // These appear at geological transitions where a ramp meets a cliff at 45°.
    // Check for this BEFORE simple slopes to avoid mis-classifying.
    if let Some(id) = try_classify_compound_slope_vert(face, edge) {
        return id;
    }

    // ========== CORNER SHAPES ==========

    // Outer Corner: air above + air on two ADJACENT horizontal faces + solid below
    // + the two horizontal faces opposite the air ones are solid
    // + the edge neighbor between the two solid horizontal faces is solid (confirms a true corner)
    if let Some(id) = try_classify_outer_corner(face, edge, air_above, air_below) {
        return id;
    }

    // Inner Corner Fill: all horizontal face-neighbors are solid, air above,
    // but ONE edge-diagonal above is air (the concave gap)
    if let Some(id) = try_classify_inner_corner(face, edge, air_above, air_below) {
        return id;
    }

    // Anti-Tetrahedron (corner chamfer): check for corner-neighbor air patterns
    // at wall intersections. Only for materials with chamfer enabled.
    if material_allows_chamfer(material) {
        if let Some(id) = try_classify_anti_tetra(face, edge, corner) {
            return id;
        }
    }

    // ========== SLOPE SHAPES ==========

    // Slope: air above + air on exactly one horizontal face + solid on opposite face + solid below
    // ADDITIONAL CONSTRAINT (conservative): The voxel BELOW the air-side face must be solid
    // (confirms a staircase/ramp pattern rather than an isolated ledge).
    if let Some(id) = try_classify_slope(face, edge, air_above, air_below) {
        return id;
    }

    // ========== VERTICAL WEDGE ==========

    // Vertical Wedge: detects diagonal staircase patterns in the horizontal plane.
    // Both horizontal face neighbors on one diagonal are air, both on the other are solid.
    if let Some(id) = try_classify_vertical_wedge(face, edge) {
        return id;
    }

    // ========== MICRO WEDGE ==========

    // Micro Wedge: very specific corner fill. Rarely triggered in natural terrain.
    // Three face-neighbors are air, forming a corner tetrahedron of solid material.
    if let Some(id) = try_classify_micro_wedge(face, edge) {
        return id;
    }

    // ========== DEFAULT ==========

    SHAPE_FULL_BLOCK // 0
}
```

### 6.3 Detailed Classification Sub-Routines

#### Slope Detection

```rust
fn try_classify_slope(face: FaceMask, edge: EdgeMask, air_above: bool, air_below: bool) -> Option<ShapeId> {
    if !air_above { return None; }  // Must have air above
    if air_below { return None; }   // Must have solid below (otherwise it's a floating ledge)

    // Check each cardinal direction for the slope pattern:
    // air on one side, solid on the opposite side
    for rot in 0..4u8 {
        let (air_dir, solid_dir) = match rot {
            0 => (4, 5),  // Air at +Z, solid at -Z → slope faces +Z (south)
            1 => (0, 1),  // Air at +X, solid at -X → slope faces +X (east)
            2 => (5, 4),  // Air at -Z, solid at +Z → slope faces -Z (north)
            3 => (1, 0),  // Air at -X, solid at +X → slope faces -X (west)
            _ => unreachable!()
        };

        let air_side = (face & (1 << air_dir)) == 0;
        let solid_side = (face & (1 << solid_dir)) != 0;

        if air_side && solid_side {
            // CONSERVATIVE CHECK: verify the "staircase" pattern.
            // The voxel diagonally below the air side should be solid.
            // This is the edge neighbor at (air_dir + down).
            // This check prevents isolated ledges from becoming slopes.
            let edge_idx = edge_index_for_pair(air_dir, 3 /*-Y*/);
            let staircase = (edge & (1 << edge_idx)) != 0;

            if staircase {
                // This is a valid slope. Return the shape ID for this rotation.
                return Some(SHAPE_SLOPE_BASE + rot * 2); // *2 because flipped variants exist
            }
        }
    }

    // Also check for inverted slopes (air below, solid above, ceiling ramps)
    // Same logic but with air_below and flip flag set.
    // ... (mirror of above with flip_y)

    None
}
```

#### Outer Corner Detection

```rust
fn try_classify_outer_corner(face: FaceMask, edge: EdgeMask, air_above: bool, air_below: bool) -> Option<ShapeId> {
    if !air_above { return None; }
    if air_below { return None; }

    // Check each pair of adjacent horizontal air faces
    for rot in 0..4u8 {
        let (air_dir_1, air_dir_2, solid_dir_1, solid_dir_2) = match rot {
            0 => (0, 4, 1, 5),  // Air +X and +Z, solid -X and -Z → corner at -X,-Z (front-left top = D)
            1 => (4, 1, 5, 0),  // Air +Z and -X, solid -Z and +X → corner at +X,-Z
            2 => (1, 5, 0, 4),  // Air -X and -Z, solid +X and +Z → corner at +X,+Z
            3 => (5, 0, 4, 1),  // Air -Z and +X, solid +Z and -X → corner at -X,+Z
            _ => unreachable!()
        };

        let air_1 = (face & (1 << air_dir_1)) == 0;
        let air_2 = (face & (1 << air_dir_2)) == 0;
        let solid_1 = (face & (1 << solid_dir_1)) != 0;
        let solid_2 = (face & (1 << solid_dir_2)) != 0;

        if air_1 && air_2 && solid_1 && solid_2 {
            return Some(SHAPE_OUTER_CORNER_BASE + rot * 2);
        }
    }

    None
}
```

#### Inner Corner Detection

```rust
fn try_classify_inner_corner(face: FaceMask, edge: EdgeMask, air_above: bool, air_below: bool) -> Option<ShapeId> {
    if !air_above { return None; }

    // All 4 horizontal face-neighbors must be solid (we're on a flat surface)
    let all_horiz_solid = (face & 0b110011) == 0b110011; // bits 0,1,4,5 all set
    if !all_horiz_solid { return None; }

    // Exactly one diagonal-up edge neighbor should be air (the concave gap)
    // Edge neighbors at +Y are: +X+Y(0), -X+Y(2), +Y+Z(8), +Y-Z(9)
    for rot in 0..4u8 {
        let edge_idx = match rot {
            0 => 0,   // +X+Y edge air → inner corner fills the +X,+Y gap → cut at far top corner
            1 => 8,   // +Y+Z edge air
            2 => 2,   // -X+Y edge air
            3 => 9,   // +Y-Z edge air
            _ => unreachable!()
        };

        let edge_air = (edge & (1 << edge_idx)) == 0;
        if edge_air {
            return Some(SHAPE_INNER_CORNER_BASE + rot * 2);
        }
    }

    None
}
```

#### Vertical Wedge Detection

```rust
fn try_classify_vertical_wedge(face: FaceMask, edge: EdgeMask) -> Option<ShapeId> {
    // Vertical wedge: a diagonal wall in the XZ plane.
    // Pattern: two diagonal horizontal neighbors are air, the other two are solid.
    // Diagonal 1 (NE-SW): +X and -Z vs -X and +Z
    // Diagonal 2 (NW-SE): -X and -Z vs +X and +Z

    // Check diagonal 1: air at +X and -Z, solid at -X and +Z
    let d1_a = (face & (1 << 0)) == 0 && (face & (1 << 5)) == 0; // air +X, air -Z
    let d1_s = (face & (1 << 1)) != 0 && (face & (1 << 4)) != 0; // solid -X, solid +Z
    if d1_a && d1_s {
        return Some(SHAPE_VERT_WEDGE_BASE + 0);
    }

    // Check opposite side of diagonal 1
    let d1_b = (face & (1 << 1)) == 0 && (face & (1 << 4)) == 0;
    let d1_bs = (face & (1 << 0)) != 0 && (face & (1 << 5)) != 0;
    if d1_b && d1_bs {
        return Some(SHAPE_VERT_WEDGE_BASE + 1);
    }

    // Check diagonal 2: air at -X and -Z, solid at +X and +Z
    let d2_a = (face & (1 << 1)) == 0 && (face & (1 << 5)) == 0;
    let d2_s = (face & (1 << 0)) != 0 && (face & (1 << 4)) != 0;
    if d2_a && d2_s {
        return Some(SHAPE_VERT_WEDGE_BASE + 2);
    }

    // Check opposite side of diagonal 2
    let d2_b = (face & (1 << 0)) == 0 && (face & (1 << 4)) == 0;
    let d2_bs = (face & (1 << 1)) != 0 && (face & (1 << 5)) != 0;
    if d2_b && d2_bs {
        return Some(SHAPE_VERT_WEDGE_BASE + 3);
    }

    None
}
```

### 6.4 Lookup Table Approach (Recommended for Performance)

Instead of the cascade of `if` checks, pre-compute a lookup table at compile time:

```rust
/// The primary classification table. Indexed by:
///   face_mask (6 bits = 64 entries) × edge_mask_reduced (relevant bits only)
/// Returns a ShapeId.
///
/// For the fast path, use only the face_mask (64 entries) to get a "shape family",
/// then refine with edge neighbors only when the family requires it.
const SHAPE_LUT_FACE: [ShapeFamily; 64] = /* pre-computed */;

enum ShapeFamily {
    FullBlock,
    MaybeSlope { air_dir: u8 },
    MaybeOuterCorner { rot: u8 },
    MaybeInnerCorner,
    MaybeVerticalWedge { variant: u8 },
    // etc.
}
```

This two-level approach (fast face-mask lookup → refined edge-mask check) avoids iterating through all rules for every voxel. The face-mask LUT has only 64 entries and fits in a cache line.

---

## 7. Meshing Pipeline

### 7.1 Overview

```
ChunkStorageVariant
        ↓
  For each solid voxel with at least one air face-neighbor:
        ↓
  Compute face_mask, edge_mask from neighbors
        ↓
  classify_shape() → ShapeId
        ↓
  Look up SHAPE_MESH_TABLE[shape_id] → face list
        ↓
  For each face:
    - Check FaceCondition (Always or WhenAirAt)
    - If rendering: transform vertices to world space, compute AO, append to mesh buffer
        ↓
  Upload mesh buffer to GPU
```

### 7.2 Mesh Generation (Per-Chunk)

```rust
fn mesh_chunk(
    storage: &ChunkStorageVariant,
    border: &ChunkBorder,  // 1-voxel border from face-adjacent neighbors (see Section 8)
    chunk_world_pos: Vec3,
) -> ChunkMesh {
    let mut vertices: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                // Skip air voxels
                let mat = get_material(storage, x, y, z);
                if mat == MAT_AIR { continue; }

                // Compute neighbor masks
                let face_mask = compute_face_mask(storage, border, x, y, z);

                // Skip fully interior voxels (all 6 face-neighbors are solid → not visible)
                if face_mask == 0b111111 { continue; }

                let edge_mask = compute_edge_mask(storage, border, x, y, z);
                let corner_mask = compute_corner_mask(storage, border, x, y, z);

                // Classify shape
                let shape_id = classify_shape(face_mask, edge_mask, corner_mask, mat);

                // Get mesh data for this shape
                let shape_mesh = &SHAPE_MESH_TABLE[shape_id as usize];

                // Emit faces
                for face in &shape_mesh.faces {
                    // Check face render condition
                    let should_render = match face.condition {
                        FaceCondition::Always => true,
                        FaceCondition::WhenAirAt(dir) => (face_mask & (1 << dir)) == 0,
                    };

                    if !should_render { continue; }

                    // Compute ambient occlusion per vertex
                    // (see Section 7.3)

                    // Transform vertices to world space and append
                    let base_idx = vertices.len() as u32;
                    for tri in &face.triangles {
                        for &local_pos in tri {
                            let world_pos = [
                                chunk_world_pos.x + (x as f32 + local_pos[0] as f32) * VOXEL_SCALE,
                                chunk_world_pos.y + (y as f32 + local_pos[1] as f32) * VOXEL_SCALE,
                                chunk_world_pos.z + (z as f32 + local_pos[2] as f32) * VOXEL_SCALE,
                            ];

                            vertices.push(MeshVertex {
                                position: world_pos,
                                normal: face.normal,
                                material_id: mat,
                                ao: compute_ao_at_vertex(storage, border, x, y, z, local_pos, face.normal),
                                _padding: 0,
                            });
                        }
                    }
                }
            }
        }
    }

    ChunkMesh { vertices, indices }
}
```

### 7.3 Ambient Occlusion

For each vertex of each rendered face, compute AO by sampling the 2×2×2 cube of voxels surrounding that vertex position:

```rust
fn compute_ao_at_vertex(
    storage: &ChunkStorageVariant,
    border: &ChunkBorder,
    vx: usize, vy: usize, vz: usize,  // voxel position in chunk
    local_pos: [u8; 3],                 // vertex position within voxel (0 or 1 each axis)
    normal: [f32; 3],                   // face normal (to bias sampling direction)
) -> u8 {
    // The vertex is at chunk position (vx + local_pos.x, vy + local_pos.y, vz + local_pos.z).
    // Sample the 4 voxels sharing this vertex that are on the OUTSIDE of the face (in normal direction).
    // Standard voxel AO formula:
    //   side1, side2 = two axis-aligned neighbors in the plane perpendicular to normal
    //   corner = the diagonal neighbor
    //   if side1 && side2: ao = 0 (fully occluded)
    //   else: ao = 3 - (side1 + side2 + corner)
    //   Map to [0, 255]: ao * 85

    // This is the same algorithm as standard Minecraft-style AO.
    // Adapt for the specific vertex position and normal direction.
    // Implementation is vertex-position-dependent — see reference implementation for details.

    // For diagonal faces (slopes, wedge cuts), the AO computation uses
    // the face normal to determine which neighbors to sample.
    // Sample the 4 neighbors in the hemisphere of the normal direction.

    todo!("Implement standard voxel AO")
}
```

### 7.4 Normal Snapping (Optional, Recommended)

To maintain the cel-shaded aesthetic from the existing system, snap computed normals to a discrete set. With the shape atlas, normals are already well-defined per face, but snapping ensures consistent lighting bands:

```rust
const DISCRETE_NORMALS: &[[f32; 3]] = &[
    // 6 axis-aligned
    [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0], [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0], [0.0, 0.0, -1.0],
    // 12 edge diagonals (normalized)
    [0.707, 0.707, 0.0], [0.707, -0.707, 0.0], /* ... etc ... */
    // 8 corner diagonals (normalized)
    [0.577, 0.577, 0.577], /* ... etc ... */
    // 4 slope normals specific to the shape catalog
    [0.0, 0.707, 0.707], [0.0, 0.707, -0.707],
    [0.707, 0.707, 0.0], [-0.707, 0.707, 0.0],
];

fn snap_normal(n: [f32; 3]) -> [f32; 3] {
    DISCRETE_NORMALS.iter()
        .max_by(|a, b| {
            let dot_a = a[0]*n[0] + a[1]*n[1] + a[2]*n[2];
            let dot_b = b[0]*n[0] + b[1]*n[1] + b[2]*n[2];
            dot_a.partial_cmp(&dot_b).unwrap()
        })
        .copied()
        .unwrap()
}
```

---

## 8. Chunk Border Strategy

### 8.1 Border Data Structure

Each chunk caches a 1-voxel-thick border from each of its 6 face-adjacent neighbors. This border is the outermost slice of the neighbor chunk.

```rust
/// The border data from one face-adjacent chunk.
/// Contains the material IDs of the 32×32 voxel slice nearest to the shared face.
struct FaceBorder {
    /// 32×32 material IDs. MAT_AIR (0) = empty.
    /// Indexed as [row * CHUNK_SIZE + col] where row/col are the two axes
    /// perpendicular to the face normal.
    materials: Box<[u16; CHUNK_SIZE * CHUNK_SIZE]>,
}

/// All 6 face borders for a chunk.
struct ChunkBorder {
    /// Index by face direction (0=+X, 1=-X, 2=+Y, 3=-Y, 4=+Z, 5=-Z).
    /// None = neighbor not yet loaded (treat as air).
    faces: [Option<FaceBorder>; 6],
}
```

### 8.2 Border Synchronization

When a chunk finishes loading/generation:

1. Extract its 6 border slices (one per face).
2. Push each border slice to the corresponding face-adjacent chunk (if loaded).
3. The receiving chunk stores the border and marks its outermost voxel slice as dirty for re-meshing.

```rust
fn on_chunk_loaded(world: &mut World, chunk_pos: IVec3) {
    let chunk = world.get_chunk(chunk_pos);

    for dir in 0..6 {
        let neighbor_pos = chunk_pos + FACE_OFFSETS[dir];
        let opposite_dir = dir ^ 1; // +X(0) <-> -X(1), +Y(2) <-> -Y(3), +Z(4) <-> -Z(5)

        // Extract the border slice from this chunk facing the neighbor
        let border = extract_face_border(chunk, dir);

        // Push to neighbor (if loaded)
        if let Some(neighbor) = world.get_chunk_mut(neighbor_pos) {
            neighbor.border.faces[opposite_dir] = Some(border);
            neighbor.mark_border_dirty(opposite_dir); // Only re-mesh the outermost slice
        }
    }
}
```

### 8.3 Fallback: Zero-Neighbor Meshing

When a chunk meshes before any neighbors are loaded:

```rust
fn is_solid_at(storage: &ChunkStorageVariant, border: &ChunkBorder, x: i32, y: i32, z: i32) -> bool {
    // Inside chunk bounds: use storage directly
    if x >= 0 && x < CHUNK_SIZE as i32 && y >= 0 && y < CHUNK_SIZE as i32 && z >= 0 && z < CHUNK_SIZE as i32 {
        return is_solid(storage, x as usize, y as usize, z as usize);
    }

    // Outside chunk bounds: check border data
    let (dir, row, col) = classify_border_position(x, y, z);
    match &border.faces[dir] {
        Some(face_border) => face_border.materials[row * CHUNK_SIZE + col] != MAT_AIR,
        None => false, // No neighbor data → assume air (conservative default)
    }
}
```

**Behavior**: With `None` borders, boundary voxels will be classified as if surrounded by air on the unloaded side. This may assign them incorrect shapes (e.g., a slope where a full block should be). When the neighbor loads and pushes its border, only the affected 32×32×1 slice needs re-meshing — roughly 1/32 of the chunk.

### 8.4 Memory Overhead

Border data per chunk: 6 faces × 32 × 32 × 2 bytes (u16 material) = **12,288 bytes ≈ 12 KB**.

Compare to current system: `SNAP_PAD=3` padded volume = 38³ - 32³ ≈ 22,000 voxels × 1 byte (density) = **22 KB**, plus material Arc references for all 27 neighbors.

The new system uses less border memory, requires data from only 6 neighbors instead of 26, and the border data is a simple flat array rather than Arc-referenced chunk storages.

---

## 9. LOD System

### 9.1 LOD Levels

| LOD | Merge Factor | Voxels Per Chunk Edge | Voxels Per Chunk | Scale |
|-----|-------------|----------------------|------------------|-------|
| 0   | 1×1×1       | 32                   | 32,768           | 1×    |
| 1   | 2×2×2       | 16                   | 4,096            | 2×    |
| 2   | 4×4×4       | 8                    | 512              | 4×    |
| 3   | 8×8×8       | 4                    | 64               | 8×    |
| 4   | 16×16×16    | 2                    | 8                | 16×   |

### 9.2 LOD Generation

At each LOD level, merge `N×N×N` voxels into a single supervoxel:

```rust
fn generate_lod(storage: &ChunkStorageVariant, lod_level: u32) -> ChunkStorageVariant {
    let merge = 1 << lod_level; // 2, 4, 8, 16
    let lod_size = CHUNK_SIZE / merge;

    let mut lod_materials = vec![MAT_AIR; lod_size * lod_size * lod_size];

    for ly in 0..lod_size {
        for lz in 0..lod_size {
            for lx in 0..lod_size {
                // Count solid voxels and find most common material in the NxNxN block
                let mut counts: HashMap<u16, u32> = HashMap::new();
                let mut total_solid = 0u32;

                for dy in 0..merge {
                    for dz in 0..merge {
                        for dx in 0..merge {
                            let mat = get_material(storage, lx*merge+dx, ly*merge+dy, lz*merge+dz);
                            if mat != MAT_AIR {
                                *counts.entry(mat).or_insert(0) += 1;
                                total_solid += 1;
                            }
                        }
                    }
                }

                // Majority rule: solid if ≥50% of source voxels are solid
                let threshold = (merge * merge * merge) / 2;
                if total_solid >= threshold as u32 {
                    // Use most common material
                    let best_mat = counts.into_iter().max_by_key(|&(_, c)| c).unwrap().0;
                    lod_materials[ly * lod_size * lod_size + lz * lod_size + lx] = best_mat;
                }
            }
        }
    }

    // Wrap in ChunkStorageVariant::Populated (or Uniform if all same)
    // ...
    todo!()
}
```

### 9.3 LOD Meshing

The exact same `mesh_chunk()` pipeline runs on LOD data. The only difference is the voxel scale is multiplied by the merge factor:

```rust
let lod_voxel_scale = VOXEL_SCALE * (1 << lod_level) as f32;
```

This means the same shape catalog produces geometry at 2×, 4×, 8× scale — slopes and corners scale proportionally.

### 9.4 LOD Transition Skirts

At the boundary between two chunks at different LOD levels, geometry may not align perfectly (a LOD 0 slope edge meets a LOD 1 full block edge). Hide this with a **skirt**: extend the lower edge of each LOD chunk downward by one LOD-scale voxel height. This creates a thin strip of extra geometry that covers the seam.

```rust
fn add_lod_skirt(mesh: &mut ChunkMesh, chunk_world_pos: Vec3, lod_level: u32) {
    let skirt_height = VOXEL_SCALE * (1 << lod_level) as f32;

    // For each edge vertex on the chunk boundary:
    // Add a vertical quad extending downward by skirt_height.
    // This overlaps with the adjacent chunk's geometry, hiding any seam.
    todo!()
}
```

---

## 10. Implementation Phases

### Phase 1: Data Format Migration

1. **Remove the density field** from `ChunkStorage`. Replace with material-only storage.
2. **Update terrain generation** to write material IDs directly (solid/air) instead of density values.
3. **Update `is_solid()` checks** throughout the codebase to use `material != MAT_AIR` instead of `density > 0`.
4. **Verify** that existing chunk loading, saving, and generation still works with the new format.

### Phase 2: Shape Catalog and Mesh Table

1. **Define the `BaseShape` enum** and all vertex/face data for the 8 base shapes.
2. **Implement the rotation system** (Section 5) to generate all 53 variants.
3. **Build `SHAPE_MESH_TABLE`** at compile time or startup.
4. **Write unit tests** verifying:
   - Each shape's face normals point outward
   - All vertices are at `{0, 1}` coordinates only
   - Rotation transforms preserve vertex validity
   - Face winding is consistent (CCW from outside)

### Phase 3: Shape Classification

1. **Implement `compute_face_mask`, `compute_edge_mask`, `compute_corner_mask`**.
2. **Implement `classify_shape()`** with the conservative rules from Section 6.
3. **Write integration tests** with known voxel configurations and expected shape outputs:
   - Flat ground surface → all full blocks
   - Single-step staircase → slope at the step
   - Two-step L-shaped staircase → outer corner at the bend
   - Flat surface with one diagonal air pocket above → inner corner fill
   - Diagonal cliff (voxels stepping in both X and Z) → vertical wedges

### Phase 4: Meshing Pipeline

1. **Implement `mesh_chunk()`** (Section 7.2) replacing the existing MC pipeline.
2. **Implement per-vertex AO** (Section 7.3).
3. **Implement normal snapping** (Section 7.4) for cel-shaded lighting consistency.
4. **Integrate with existing GPU upload path** — the vertex format should match the existing shader expectations (position, normal, material, AO). Adjust shaders if needed.

### Phase 5: Chunk Border System

1. **Implement `ChunkBorder` struct** and `FaceBorder` extraction.
2. **Implement border push/receive** in the chunk loading pipeline.
3. **Implement border-dirty re-meshing** (only the outermost slice).
4. **Remove the old `ChunkSnapshot` / `ChunkNeighbors` system** — the 27-neighbor padded volume is no longer needed.
5. **Remove `SNAP_PAD`** and all related padding logic.

### Phase 6: LOD Integration

1. **Implement `generate_lod()`** (Section 9.2).
2. **Wire LOD meshing** into the existing chunk management system.
3. **Implement LOD skirts** (Section 9.4) for seamless transitions.
4. **Connect to camera distance** — select LOD level based on chunk distance from camera.

### Phase 7: Tuning and Polish

1. **Adjust classification rules** based on visual results. The conservative settings may need per-material or per-biome overrides.
2. **Add `material_allows_diagonals()` and `material_allows_chamfer()`** to material definitions.
3. **Profile meshing performance** and optimize hot paths (the face_mask LUT should be the inner-loop bottleneck, which is very fast).
4. **Remove all remaining MC code** (edge tables, tri tables, MC pipeline, greedy merging).

---

## Appendix A: Shape ID Constants

```rust
const SHAPE_FULL_BLOCK: ShapeId = 0;

// Slopes: 4 rotations × 2 (normal + flipped) = 8
const SHAPE_SLOPE_BASE: ShapeId = 1;       // 1..=8

// Outer Corners: 4 rotations × 2 = 8
const SHAPE_OUTER_CORNER_BASE: ShapeId = 9; // 9..=16

// Inner Corner Fills: 4 rotations × 2 = 8
const SHAPE_INNER_CORNER_BASE: ShapeId = 17; // 17..=24

// Vertical Wedges: 2 diagonal orientations × 2 sides = 4
const SHAPE_VERT_WEDGE_BASE: ShapeId = 25; // 25..=28

// Anti-Tetrahedra (corner chamfers): 8 corners
const SHAPE_ANTI_TETRA_BASE: ShapeId = 29; // 29..=36

// Micro Wedges (corner tetrahedra): 8 corners
const SHAPE_MICRO_WEDGE_BASE: ShapeId = 37; // 37..=44

// Compound Slope + Vertical Wedge: 4 slope dirs × 2 vert dirs = 8
const SHAPE_COMPOUND_SV_BASE: ShapeId = 45; // 45..=52

const SHAPE_COUNT: usize = 53;
```

## Appendix B: Vertex Label to Coordinate Mapping

For use in face definitions. Copy this into the implementation as a constant:

```rust
const CUBE_CORNERS: [[u8; 3]; 8] = [
    [0, 0, 0], // A = 0
    [1, 0, 0], // B = 1
    [1, 1, 0], // C = 2
    [0, 1, 0], // D = 3
    [0, 0, 1], // E = 4
    [1, 0, 1], // F = 5
    [1, 1, 1], // G = 6
    [0, 1, 1], // H = 7
];
```

## Appendix C: Edge Neighbor Index Mapping

```rust
/// Maps a pair of face directions to the edge-neighbor index.
/// face_dir_1 and face_dir_2 are from 0..5.
/// Returns the edge-mask bit index (0..11).
fn edge_index_for_pair(dir1: u8, dir2: u8) -> u8 {
    // Sorted pair → index mapping:
    // (+X,+Y)=0  (+X,-Y)=1  (-X,+Y)=2  (-X,-Y)=3
    // (+X,+Z)=4  (+X,-Z)=5  (-X,+Z)=6  (-X,-Z)=7
    // (+Y,+Z)=8  (+Y,-Z)=9  (-Y,+Z)=10 (-Y,-Z)=11
    match (dir1.min(dir2), dir1.max(dir2)) {
        (0, 2) => 0,  // +X, +Y
        (0, 3) => 1,  // +X, -Y
        (1, 2) => 2,  // -X, +Y
        (1, 3) => 3,  // -X, -Y
        (0, 4) => 4,  // +X, +Z
        (0, 5) => 5,  // +X, -Z
        (1, 4) => 6,  // -X, +Z
        (1, 5) => 7,  // -X, -Z
        (2, 4) => 8,  // +Y, +Z
        (2, 5) => 9,  // +Y, -Z
        (3, 4) => 10, // -Y, +Z
        (3, 5) => 11, // -Y, -Z
        _ => panic!("Invalid edge pair: {} {}", dir1, dir2),
    }
}
```

---

**End of specification.** The implementing agent should read this document completely before beginning work, implement phases in order, and write tests at each phase boundary before proceeding to the next.
