//! Slope refinement: post-process a cube-only `ChunkBuffer<Voxel>` to
//! reclassify surface voxels as slopes / outer corners / inner corners.
//!
//! Implements the priority cascade from `docs/voxel_shape_atlas_spec.md`
//! §6.2, restricted to the 14 ShapeIds voxel-core declares (no
//! VerticalWedge / AntiTetra / MicroWedge / compound shapes - those need a
//! ShapeId expansion).
//!
//! ## Coordinate convention
//!
//! - `+X = east`,  `-X = west`
//! - `+Y = up`,    `-Y = down`
//! - `+Z = south`, `-Z = north`
//!
//! ## Shape orientation convention (matches voxel-core/src/shape.rs)
//!
//! - `SlopeN`        - high edge at `-Z` (north); air-facing direction is `+Z` (south).
//! - `OuterCornerNE` - high corner at `(-Z, +X)`; the two air faces are `+Z` and `-X`.
//! - `InnerCornerNE` - concave bite out of the top-`(-Z, +X)` diagonal edge above.

use voxel_core::{ChunkBuffer, Rotation, ShapeId, Voxel};

const N: usize = 32;

// Face-direction bit indices (matches atlas spec §6.1).
const F_POS_X: u8 = 0;
const F_NEG_X: u8 = 1;
const F_POS_Y: u8 = 2;
const F_NEG_Y: u8 = 3;
const F_POS_Z: u8 = 4;
const F_NEG_Z: u8 = 5;

// Top-edge bit indices: the four `+Y` diagonal edges relevant to inner-corner
// classification. Bit `i` set iff that diagonal neighbor is solid.
const E_POS_X_POS_Z: u8 = 0; // (+X, +Y, +Z) - south-east above
const E_POS_X_NEG_Z: u8 = 1; // (+X, +Y, -Z) - north-east above
const E_NEG_X_POS_Z: u8 = 2; // (-X, +Y, +Z) - south-west above
const E_NEG_X_NEG_Z: u8 = 3; // (-X, +Y, -Z) - north-west above

/// Reclassify every surface `Cube` voxel in `input` into a slope / corner
/// shape based on its 6-face neighbor pattern (with edge checks for corner
/// cases). Non-Cube voxels pass through unchanged.
pub(crate) fn refine_chunk(input: &ChunkBuffer<Voxel, 32>) -> ChunkBuffer<Voxel, 32> {
    let mut out: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);

    for z in 0..N {
        for y in 0..N {
            for x in 0..N {
                let v = input.get(x, y, z);
                if v.shape != ShapeId::Cube {
                    out.set(x, y, z, v);
                    continue;
                }

                let face = face_mask(input, x, y, z);
                let air_above = (face & (1 << F_POS_Y)) == 0;
                let air_below = (face & (1 << F_NEG_Y)) == 0;

                let (shape, rotation) = classify(input, x, y, z, face, air_above, air_below);

                out.set(x, y, z, Voxel { shape, rotation, material: v.material, flags: v.flags });
            }
        }
    }

    out.try_collapse();
    log_shape_counts(&out);
    out
}

/// Diagnostic: tally each ShapeId in the refined buffer and emit a single
/// `info!` line. Useful for verifying which shape families are firing in a
/// given graph (e.g., whether inner corners ever match in the biome).
fn log_shape_counts(buf: &ChunkBuffer<Voxel, 32>) {
    if !log::log_enabled!(log::Level::Info) { return; }
    let mut counts = [0u32; (ShapeId::MAX_DISCRIMINANT as usize) + 1];
    for z in 0..N { for y in 0..N { for x in 0..N {
        let s = buf.get(x, y, z).shape;
        counts[s as usize] += 1;
    }}}
    let mut nonzero: Vec<(ShapeId, u32)> = (0..=ShapeId::MAX_DISCRIMINANT)
        .filter_map(|i| ShapeId::from_raw(i).map(|s| (s, counts[i as usize])))
        .filter(|&(_, n)| n > 0)
        .collect();
    nonzero.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    log::info!(
        "slope_refine: {}",
        nonzero.iter()
            .map(|(s, n)| format!("{:?}={}", s, n))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// Bitmask of which of the 6 face-neighbors are solid. OOB = air.
fn face_mask(buf: &ChunkBuffer<Voxel, 32>, x: usize, y: usize, z: usize) -> u8 {
    let mut m = 0u8;
    if is_solid(buf, x as i32 + 1, y as i32,     z as i32    ) { m |= 1 << F_POS_X; }
    if is_solid(buf, x as i32 - 1, y as i32,     z as i32    ) { m |= 1 << F_NEG_X; }
    if is_solid(buf, x as i32,     y as i32 + 1, z as i32    ) { m |= 1 << F_POS_Y; }
    if is_solid(buf, x as i32,     y as i32 - 1, z as i32    ) { m |= 1 << F_NEG_Y; }
    if is_solid(buf, x as i32,     y as i32,     z as i32 + 1) { m |= 1 << F_POS_Z; }
    if is_solid(buf, x as i32,     y as i32,     z as i32 - 1) { m |= 1 << F_NEG_Z; }
    m
}

/// Bitmask of the 4 `+Y` diagonal-edge neighbors' solidity. Only consulted
/// for inner-corner classification. OOB = air.
fn top_edge_mask(buf: &ChunkBuffer<Voxel, 32>, x: usize, y: usize, z: usize) -> u8 {
    let mut m = 0u8;
    if is_solid(buf, x as i32 + 1, y as i32 + 1, z as i32 + 1) { m |= 1 << E_POS_X_POS_Z; }
    if is_solid(buf, x as i32 + 1, y as i32 + 1, z as i32 - 1) { m |= 1 << E_POS_X_NEG_Z; }
    if is_solid(buf, x as i32 - 1, y as i32 + 1, z as i32 + 1) { m |= 1 << E_NEG_X_POS_Z; }
    if is_solid(buf, x as i32 - 1, y as i32 + 1, z as i32 - 1) { m |= 1 << E_NEG_X_NEG_Z; }
    m
}

#[inline]
fn is_solid(buf: &ChunkBuffer<Voxel, 32>, x: i32, y: i32, z: i32) -> bool {
    if !(0..N as i32).contains(&x)
        || !(0..N as i32).contains(&y)
        || !(0..N as i32).contains(&z)
    {
        return false;
    }
    buf.get(x as usize, y as usize, z as usize).is_solid()
}

/// Priority cascade: outer corner → inner corner → slope → cube fallback.
fn classify(
    buf: &ChunkBuffer<Voxel, 32>,
    x: usize, y: usize, z: usize,
    face: u8, air_above: bool, air_below: bool,
) -> (ShapeId, Rotation) {
    if let Some(s) = try_outer_corner(face, air_above, air_below) {
        return (s, Rotation::None);
    }
    if let Some(s) = try_inner_corner(face, air_above, top_edge_mask(buf, x, y, z)) {
        return (s, Rotation::None);
    }
    if let Some(s) = try_slope(buf, x, y, z, face, air_above, air_below) {
        return (s, Rotation::None);
    }
    if let Some(s) = try_ceiling_outer_corner(face, air_above, air_below) {
        return (s, Rotation::None);
    }
    if let Some(s) = try_ceiling_slope(buf, x, y, z, face, air_above, air_below) {
        return (s, Rotation::None);
    }
    (ShapeId::Cube, Rotation::None)
}

/// Outer corner: convex hilltop - air above, solid below, and air on
/// exactly two ADJACENT horizontal faces (with solid on the opposing two).
/// The high corner is the one diagonally opposite the air faces.
fn try_outer_corner(face: u8, air_above: bool, air_below: bool) -> Option<ShapeId> {
    if !air_above || air_below { return None; }
    let air_e = (face & (1 << F_POS_X)) == 0;
    let air_w = (face & (1 << F_NEG_X)) == 0;
    let air_s = (face & (1 << F_POS_Z)) == 0;
    let air_n = (face & (1 << F_NEG_Z)) == 0;
    match (air_n, air_e, air_s, air_w) {
        (false, false, true,  true ) => Some(ShapeId::OuterCornerNE), // air S+W → high NE
        (false, true,  true,  false) => Some(ShapeId::OuterCornerNW), // air S+E → high NW
        (true,  false, false, true ) => Some(ShapeId::OuterCornerSE), // air N+W → high SE
        (true,  true,  false, false) => Some(ShapeId::OuterCornerSW), // air N+E → high SW
        _ => None,
    }
}

/// Inner corner: concave valley fill - air above, ALL 4 horizontal faces
/// solid, and exactly one of the 4 top-diagonal edge neighbors is air
/// (the concave "bite").
fn try_inner_corner(face: u8, air_above: bool, top_edges: u8) -> Option<ShapeId> {
    if !air_above { return None; }
    let horiz_mask = (1 << F_POS_X) | (1 << F_NEG_X) | (1 << F_POS_Z) | (1 << F_NEG_Z);
    if face & horiz_mask != horiz_mask { return None; }

    let mut air_count = 0u8;
    let mut which: Option<u8> = None;
    for bit in [E_POS_X_POS_Z, E_POS_X_NEG_Z, E_NEG_X_POS_Z, E_NEG_X_NEG_Z] {
        if (top_edges >> bit) & 1 == 0 {
            air_count += 1;
            which = Some(bit);
        }
    }
    if air_count != 1 { return None; }
    match which? {
        E_POS_X_POS_Z => Some(ShapeId::InnerCornerSE),
        E_POS_X_NEG_Z => Some(ShapeId::InnerCornerNE),
        E_NEG_X_POS_Z => Some(ShapeId::InnerCornerSW),
        E_NEG_X_NEG_Z => Some(ShapeId::InnerCornerNW),
        _ => None,
    }
}

/// Slope (45° ramp): air above, solid below, exactly one horizontal air,
/// the opposite horizontal solid. Additional rejection: the cell one Y
/// below the air-side face must also be solid — rejects isolated ledges
/// per atlas §6.3.
fn try_slope(
    buf: &ChunkBuffer<Voxel, 32>,
    x: usize, y: usize, z: usize,
    face: u8, air_above: bool, air_below: bool,
) -> Option<ShapeId> {
    if !air_above || air_below { return None; }

    let air_e = (face & (1 << F_POS_X)) == 0;
    let air_w = (face & (1 << F_NEG_X)) == 0;
    let air_s = (face & (1 << F_POS_Z)) == 0;
    let air_n = (face & (1 << F_NEG_Z)) == 0;

    if air_s && !air_n && !air_w && !air_e
        && is_solid(buf, x as i32, y as i32 - 1, z as i32 + 1)
    {
        return Some(ShapeId::SlopeN); // high edge at -Z, slope faces +Z
    }
    if air_n && !air_s && !air_w && !air_e
        && is_solid(buf, x as i32, y as i32 - 1, z as i32 - 1)
    {
        return Some(ShapeId::SlopeS);
    }
    if air_w && !air_e && !air_n && !air_s
        && is_solid(buf, x as i32 - 1, y as i32 - 1, z as i32)
    {
        return Some(ShapeId::SlopeE);
    }
    if air_e && !air_w && !air_n && !air_s
        && is_solid(buf, x as i32 + 1, y as i32 - 1, z as i32)
    {
        return Some(ShapeId::SlopeW);
    }
    None
}

/// Ceiling outer corner (cave roof, convex pendant): solid above, air below,
/// air on exactly two ADJACENT horizontal faces. Mirror of [`try_outer_corner`].
fn try_ceiling_outer_corner(face: u8, air_above: bool, air_below: bool) -> Option<ShapeId> {
    if air_above || !air_below { return None; }
    let air_e = (face & (1 << F_POS_X)) == 0;
    let air_w = (face & (1 << F_NEG_X)) == 0;
    let air_s = (face & (1 << F_POS_Z)) == 0;
    let air_n = (face & (1 << F_NEG_Z)) == 0;
    match (air_n, air_e, air_s, air_w) {
        (false, false, true,  true ) => Some(ShapeId::CeilingOuterCornerNE),
        (false, true,  true,  false) => Some(ShapeId::CeilingOuterCornerNW),
        (true,  false, false, true ) => Some(ShapeId::CeilingOuterCornerSE),
        (true,  true,  false, false) => Some(ShapeId::CeilingOuterCornerSW),
        _ => None,
    }
}

/// Ceiling slope (cave-roof ramp): solid above, air below, exactly one
/// horizontal air with the opposite solid. The cell one Y *above* the
/// air-side face must also be solid (rejects isolated 1-voxel ceiling
/// ledges — mirror of [`try_slope`]'s rejection rule).
fn try_ceiling_slope(
    buf: &ChunkBuffer<Voxel, 32>,
    x: usize, y: usize, z: usize,
    face: u8, air_above: bool, air_below: bool,
) -> Option<ShapeId> {
    if air_above || !air_below { return None; }

    let air_e = (face & (1 << F_POS_X)) == 0;
    let air_w = (face & (1 << F_NEG_X)) == 0;
    let air_s = (face & (1 << F_POS_Z)) == 0;
    let air_n = (face & (1 << F_NEG_Z)) == 0;

    if air_s && !air_n && !air_w && !air_e
        && is_solid(buf, x as i32, y as i32 + 1, z as i32 + 1)
    {
        return Some(ShapeId::CeilingSlopeN);
    }
    if air_n && !air_s && !air_w && !air_e
        && is_solid(buf, x as i32, y as i32 + 1, z as i32 - 1)
    {
        return Some(ShapeId::CeilingSlopeS);
    }
    if air_w && !air_e && !air_n && !air_s
        && is_solid(buf, x as i32 - 1, y as i32 + 1, z as i32)
    {
        return Some(ShapeId::CeilingSlopeE);
    }
    if air_e && !air_w && !air_n && !air_s
        && is_solid(buf, x as i32 + 1, y as i32 + 1, z as i32)
    {
        return Some(ShapeId::CeilingSlopeW);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::{MaterialId, ShapeId, Voxel};

    fn cube_at(buf: &mut ChunkBuffer<Voxel, 32>, x: usize, y: usize, z: usize) {
        buf.set(x, y, z, Voxel::cube(MaterialId(1)));
    }

    #[test]
    fn empty_passes_through() {
        let buf: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        let out = refine_chunk(&buf);
        for z in 0..N { for y in 0..N { for x in 0..N {
            assert_eq!(out.get(x, y, z), Voxel::EMPTY);
        }}}
    }

    #[test]
    fn interior_cube_stays_cube() {
        // A 3×3×3 block of cubes; the center cube has 6 solid neighbors.
        let mut buf: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        for dz in 0..3 { for dy in 0..3 { for dx in 0..3 {
            cube_at(&mut buf, 5 + dx, 5 + dy, 5 + dz);
        }}}
        let out = refine_chunk(&buf);
        let v = out.get(6, 6, 6);
        assert_eq!(v.shape, ShapeId::Cube);
    }

    #[test]
    fn isolated_floating_cube_stays_cube() {
        // No solid below → slope rejection rule keeps it a Cube.
        let mut buf: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        cube_at(&mut buf, 10, 10, 10);
        let out = refine_chunk(&buf);
        assert_eq!(out.get(10, 10, 10).shape, ShapeId::Cube);
    }

    #[test]
    fn north_facing_slope() {
        // Build the minimal fixture so the candidate at (10, 5, 5) satisfies
        // every clause of `try_slope`'s SlopeN branch:
        //   +Z (10, 5, 6) air, -Z (10, 5, 4) solid → high edge at -Z (north)
        //   +X (11, 5, 5) solid, -X ( 9, 5, 5) solid → no outer-corner match
        //   +Y (10, 6, 5) air, -Y (10, 4, 5) solid  → not a floating ledge
        //   "below the air side" (10, 4, 6) solid    → not a single-voxel ledge
        let mut buf: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        cube_at(&mut buf, 10, 5, 5); // candidate
        cube_at(&mut buf, 10, 5, 4); // -Z
        cube_at(&mut buf, 11, 5, 5); // +X
        cube_at(&mut buf,  9, 5, 5); // -X
        cube_at(&mut buf, 10, 4, 5); // -Y
        cube_at(&mut buf, 10, 4, 6); // below the air-south side

        let out = refine_chunk(&buf);
        assert_eq!(
            out.get(10, 5, 5).shape,
            ShapeId::SlopeN,
            "candidate cell should classify as SlopeN",
        );
    }

    #[test]
    fn outer_corner_at_peak() {
        // 2×2 base of cubes with one corner extended up by one cell.
        // The top cell has air S + air E + solid N + solid W + solid below
        // → OuterCornerNW (high corner at NW = the present cell's corner).
        // To match the convention, place the peak with N+W as the solid
        // neighbors:
        let mut buf: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        // 2×2 floor at y = 4 covering (x = 10..12, z = 4..6).
        for z in 4..6 { for x in 10..12 { cube_at(&mut buf, x, 4, z); } }
        // Walls: north (z = 4) and west (x = 10) at y = 5.
        cube_at(&mut buf, 10, 5, 4); // -Z neighbor of candidate
        cube_at(&mut buf, 11, 5, 4); // candidate's -Z + extra to keep N solid
        cube_at(&mut buf, 10, 5, 5); // -X neighbor of candidate
        // Candidate at (11, 5, 5): -Z solid (11,5,4), -X solid (10,5,5),
        // +Z air (11,5,6), +X air (12,5,5), +Y air, -Y solid (11,4,5).
        cube_at(&mut buf, 11, 5, 5);
        let out = refine_chunk(&buf);
        assert_eq!(out.get(11, 5, 5).shape, ShapeId::OuterCornerNW);
    }

    #[test]
    fn inner_corner_in_concave_bend() {
        // L-shaped wall at y = 5: cells north and east of the candidate
        // are solid, and the +Y-diagonal at (-X, +Y, -Z) is air (the bend
        // opens NW). All 4 horizontal neighbors solid → InnerCornerNW.
        let mut buf: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        // Floor.
        for z in 4..7 { for x in 9..12 { cube_at(&mut buf, x, 4, z); } }
        // Candidate.
        cube_at(&mut buf, 10, 5, 5);
        // All 4 horizontal neighbors solid at y = 5.
        cube_at(&mut buf, 11, 5, 5); // +X
        cube_at(&mut buf,  9, 5, 5); // -X
        cube_at(&mut buf, 10, 5, 6); // +Z
        cube_at(&mut buf, 10, 5, 4); // -Z
        // Three of the four +Y diagonals solid; the NW one (-X,+Y,-Z) is air.
        cube_at(&mut buf, 11, 6, 6); // E_POS_X_POS_Z
        cube_at(&mut buf, 11, 6, 4); // E_POS_X_NEG_Z
        cube_at(&mut buf,  9, 6, 6); // E_NEG_X_POS_Z
        // skip (9, 6, 4) — this is the air diagonal → InnerCornerNW
        let out = refine_chunk(&buf);
        assert_eq!(out.get(10, 5, 5).shape, ShapeId::InnerCornerNW);
    }
}