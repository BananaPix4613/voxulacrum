//! Fluid generation (design doc §7, generation stage 9).
//!
//! Populates a chunk's [`FluidLayer`] at generation time. This substep covers
//! ocean fill from the global `sea_level`; biome-authored water bodies layer on
//! top in a later substep. Generated fluid is not persisted - it re-derives
//! deterministically on load - so this must be a pure function of its inputs.

use std::collections::{HashMap, HashSet};

use nodegraph_eval::{ColumnField, NO_POND};
use voxel_core::{LocalPos, ShapeId, Voxel};

use super::chunk::CHUNK_SIZE;
use super::layers::{FluidCell, FluidFillMode, FluidId, FluidLayer};
use super::storage::ChunkStorage;

/// Fixed-point mass of one completely full fluid cell (design doc §7: 65535).
pub const FULL_MASS: u16 = u16::MAX;

/// Fluid capacity of a half-height slab: only its empty half can hold fluid, so
/// it holds half of what a fully empty voxel holds.
pub const SLAB_MASS: u16 = FULL_MASS / 2;

/// Fluid capacity of `voxel`: the mass its empty portion can hold before it's
/// full. A full cube has none (fully occupied by material); a half-height slab
/// holds [`SLAB_MASS`] in its empty half; a fully empty voxel (AIR) holds [`FULL_MASS`].
pub fn fluid_capacity(voxel: Voxel) -> u16 {
    match voxel.shape {
        ShapeId::Empty => FULL_MASS,
        ShapeId::SlabBottom | ShapeId::SlabTop => SLAB_MASS,
        ShapeId::Cube => 0,
    }
}

/// Initialize a chunk's ocean fluid from the global `sea_level` (a world-Y
/// plane). `chunk_y` is the chunk's Y coordinate; a voxel at local y sits at
/// world Y `chunk_y * CHUNK_SIZE + y`. Every voxel at or below `sea_level` with
/// fluid capacity ([`fluid_capacity`]) becomes settled water at that capacity:
/// full for an empty voxel, half for a slab (filling its empty half so the
/// surface lines up flush with neighboring full-depth water instead of leaving
/// a dry seam at slab steps near the shoreline).
///
/// - Entirely above sea level -> [`FluidFillMode::Empty`], no cells.
/// - Entirely at/below sea level -> [`FluidFillMode::Submerged`] (O(1); the
///   renderer treats every voxel as filled to its fluid capacity, no per-cell
///   storage).
/// - Straddling sea level -> explicit settled cells for its below-sea voxels.
pub fn ocean_fill(storage: &ChunkStorage, chunk_y: i32, sea_level: i32) -> FluidLayer {
    let dim = CHUNK_SIZE as i32;
    let base_y = chunk_y * dim;
    let top_y = base_y + dim - 1;

    // Entirely above sea level: dry.
    if base_y > sea_level {
        return FluidLayer::default();
    }

    // Entirely at/below sea level: every empty voxel is water (fast path).
    if top_y <= sea_level {
        return FluidLayer {
            fill_mode: FluidFillMode::Submerged(FluidId::WATER),
            cells: HashMap::new(),
            active: HashSet::new(),
            stable_ticks: HashMap::new(),
        };
    }

    // Straddling: settle explicit cells for empty voxels up to sea level.
    // base_y <= sea_level < top_y here, so top_local is in 0..CHUNK_SIZE-1.
    let top_local = (sea_level - base_y) as usize;
    let mut cells = HashMap::new();
    for z in 0..CHUNK_SIZE {
        for y in 0..=top_local {
            for x in 0..CHUNK_SIZE {
                let idx = x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE;
                let capacity = fluid_capacity(storage.voxel(idx));
                if capacity > 0 {
                    cells.insert(
                        LocalPos::new_unchecked(x as u8, y as u8, z as u8),
                        FluidCell {
                            fluid_id: FluidId::WATER,
                            mass: capacity,
                            flags: FluidCell::FLAG_SETTLED,
                        },
                    );
                }
            }
        }
    }
    FluidLayer { fill_mode: FluidFillMode::Empty, cells, active: HashSet::new(), stable_ticks: HashMap::new() }
}

/// Layer biome-authored ponds on top of the ocean fill. `levels` is the per-
/// column pond water-surface (world-Y) from the biome `FluidOutput` terminals
/// ([`NO_POND`] where a column has no pond). For each pond column, empty voxels
/// strictly above `sea_level` up to the pond surface become settled water.
///
/// Ponds live above sea level; at or below it the ocean already fills, so those
/// cells are skipped, and fully-submerged chunks get no ponds at all.
pub fn apply_biome_ponds(
    fluids: &mut FluidLayer,
    storage: &ChunkStorage,
    chunk_y: i32,
    sea_level: i32,
    levels: &ColumnField,
) {
    if matches!(fluids.fill_mode, FluidFillMode::Submerged(_)) {
        return; // entirely below sea level; ponds add nothing
    }
    let base_y = chunk_y * CHUNK_SIZE as i32;
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let level = levels.get(x, z);
            if level <= NO_POND {
                continue;
            }
            // In the sea-straddling chunk, skip open-ocean columns (empty at the
            // sea plane): only columns where terrain reaches sea level are land
            // that can hold a pond, so ponds never overlap and hide the sea
            // surface. Chunks fully above sea have no sea plane here, so no guard.
            let sea_local = sea_level - base_y;
            if (0..CHUNK_SIZE as i32).contains(&sea_local) {
                let sidx =
                    x + sea_local as usize * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE;
                if !storage.voxel(sidx).is_solid() {
                    continue;
                }
            }
            let surface = level.floor() as i32; // world-Y water surface
            for y in 0..CHUNK_SIZE {
                let world_y = base_y + y as i32;
                if world_y > surface {
                    break; // above the pond surface (y ascends)
                }
                if world_y <= sea_level {
                    continue; // ocean's domain
                }
                let idx = x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE;
                let capacity = fluid_capacity(storage.voxel(idx));
                if capacity > 0 {
                    fluids.cells.insert(
                        LocalPos::new_unchecked(x as u8, y as u8, z as u8),
                        FluidCell {
                            fluid_id: FluidId::WATER,
                            mass: capacity,
                            flags: FluidCell::FLAG_SETTLED,
                        },
                    );
                }
            }
        }
    }
}

/// True when the air cell directly above surface voxel `(x, sy, z)` holds water,
/// so foliage anchored so that surface would render submerged (design doc §5:
/// fluid initialization, stage 9, precedes foliage, stage 10). In a `Submerged`
/// chunk every empty voxel is water, so any surface foliage is submerged. When the
/// surface sits at the chunk's ceiling (`sy + 1` out of range), the neighbor
/// chunk's water is unknown at single-chunk generation time, so the column is
/// treated as dry - a rare edge; surfaces sit well below the ceiling in practice.
pub fn foliage_submerged(fluids: &FluidLayer, x: usize, sy: usize, z: usize) -> bool {
    match fluids.fill_mode {
        FluidFillMode::Submerged(_) => true,
        FluidFillMode::Empty => {
            let above = sy + 1;
            if above >= CHUNK_SIZE {
                return false;
            }
            let lp = LocalPos::new_unchecked(x as u8, above as u8, z as u8);
            fluids.cells.get(&lp).map_or(false, |c| c.mass > 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn above_sea_level_is_dry() {
        let s = ChunkStorage::new_air();
        let f = ocean_fill(&s, 1, 10); // chunk base y = 32 > 10
        assert_eq!(f.fill_mode, FluidFillMode::Empty);
        assert!(f.cells.is_empty());
    }

    #[test]
    fn fully_submerged_uses_fast_path() {
        let s = ChunkStorage::new_air();
        let f = ocean_fill(&s, -1, 10); // world y -32..-1, all <= 10
        assert_eq!(f.fill_mode, FluidFillMode::Submerged(FluidId::WATER));
        assert!(f.cells.is_empty()); // fast path stores no cells
    }

    #[test]
    fn straddle_settles_below_sea_cells() {
        let s = ChunkStorage::new_air();
        let f = ocean_fill(&s, 0, 5); // world y 0..31, sea at 5
        assert_eq!(f.fill_mode, FluidFillMode::Empty);
        // y in 0..=5 (6 layers) * 32 * 32 empty voxels become water.
        assert_eq!(f.cells.len(), 6 * CHUNK_SIZE * CHUNK_SIZE);
        let cell = f.cells.get(&LocalPos::new_unchecked(0, 5, 0)).unwrap();
        assert_eq!(cell.mass, FULL_MASS);
        assert_eq!(cell.fluid_id, FluidId::WATER);
        assert_eq!(cell.flags, FluidCell::FLAG_SETTLED);
        // A voxel just above sea level gets no water cell.
        assert!(f.cells.get(&LocalPos::new_unchecked(0, 6, 0)).is_none());
    }

    #[test]
    fn solid_voxels_below_sea_get_no_cell() {
        let mut s = ChunkStorage::new_air();
        s.set_voxel(0, voxel_core::Voxel::cube(voxel_core::MaterialId(1)));
        let f = ocean_fill(&s, 0, 5);
        assert!(f.cells.get(&LocalPos::new_unchecked(0, 0, 0)).is_none());
        assert!(f.cells.get(&LocalPos::new_unchecked(1, 0, 0)).is_some());
    }

    #[test]
    fn ocean_fill_is_deterministic() {
        let s = ChunkStorage::new_air();
        assert_eq!(ocean_fill(&s, 0, 5), ocean_fill(&s, 0, 5));
    }

    #[test]
    fn ponds_fill_above_sea_in_masked_columns() {
        use voxel_core::{MaterialId, Voxel};
        let mut s = ChunkStorage::new_air();
        // Land in the pond column: terrain reaches the sea plane (y=24), so the
        // column can hold a pond.
        s.set_voxel(5 + 24 * CHUNK_SIZE + 5 * CHUNK_SIZE * CHUNK_SIZE, Voxel::cube(MaterialId(1)));

        let mut levels = ColumnField::filled(NO_POND);
        levels.set(5, 5, 30.0); // land pond column (surface at world-Y 30)
        levels.set(0, 0, 30.0); // open-ocean column (no terrain at sea)

        let mut fluids = ocean_fill(&s, 0, 24); // straddle chunk, sea level 24
        apply_biome_ponds(&mut fluids, &s, 0, 24, &levels);

        // Land pond: settled water above sea (25) up to the surface (30).
        assert!(fluids.cells.contains_key(&LocalPos::new_unchecked(5, 25, 5)));
        assert!(fluids.cells.contains_key(&LocalPos::new_unchecked(5, 30, 5)));
        assert!(!fluids.cells.contains_key(&LocalPos::new_unchecked(5, 31, 5))); // above surface
        // Open-ocean column: skipped, so the sea surface stays visible.
        assert!(!fluids.cells.contains_key(&LocalPos::new_unchecked(0, 25, 0)));

        // Deterministic.
        let mut fluids2 = ocean_fill(&s, 0, 24);
        apply_biome_ponds(&mut fluids2, &s, 0, 24, &levels);
        assert_eq!(fluids, fluids2);
    }

    #[test]
    fn foliage_submerged_detects_water_above_surface() {
        // Empty fill, water at (2,6,2): a surface at y=5 is submerged; y=7 is dry.
        let mut f = FluidLayer::default();
        f.cells.insert(
            LocalPos::new_unchecked(2, 6, 2),
            FluidCell { fluid_id: FluidId::WATER, mass: FULL_MASS, flags: FluidCell::FLAG_SETTLED },
        );
        assert!(foliage_submerged(&f, 2, 5, 2), "water directly above the surface");
        assert!(!foliage_submerged(&f, 2, 7, 2), "surface above the water line");
        // Submerged fast path: everything is submerged.
        let s = FluidLayer { fill_mode: FluidFillMode::Submerged(FluidId::WATER), ..Default::default() };
        assert!(foliage_submerged(&s, 10, 10, 10));
    }
}
