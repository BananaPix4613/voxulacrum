//! Fluid flow simulation (design doc §7): mass-conserving cellular automata over
//! a chunk's active fluid cells. Fixed-point `u16` mass (deterministic across
//! machines/thread orderings). Intra-chunk in this substep - out-of-chunk
//! neighbors are treated as solid; cross-chunk flow arrives in a later substep.
//!
//! This substep covers gravity (down flow); horizontal equalization and settling
//! follow. `simulate_chunk` is not yet wired to a scheduler (that's a later
//! substep), hence the module-level allow.

#![allow(dead_code)] // wired to the fixed-rate scheduler in a later substep

use std::collections::{HashMap, HashSet};

use voxel_core::LocalPos;

use super::chunk::CHUNK_SIZE;
use super::fluid_gen::{fluid_capacity, FULL_MASS};
use super::layers::{FluidCell, FluidFillMode, FluidId, FluidLayer};
use super::storage::ChunkStorage;

/// Mass of one completely full cell.
const MAX_MASS: u16 = FULL_MASS;
/// Cells at or below this mass are treated as empty and removed, so infinitesimal
/// residue can't accumulate (the design's "small loss threshold").
const MIN_MASS: u16 = 64;

/// Read-only fluid mass + capacity at any local position, including positions in
/// neighbor chunks (outside `0..CHUNK_SIZE`). Edge cells sample across the
/// boundary through this; the neighbor lookups are wired in Substep 6b.
pub trait FluidSample {
    fn mass(&self, x: i32, y: i32, z: i32) -> u16;
    /// Mass this position can hold before it's full: `0` for a full cube (blocks
    /// flow entirely), half of a full cell for a slab (only its empty half holds
    /// fluid), full for an empty voxel. See [`fluid_capacity`].
    fn capacity(&self, x: i32, y: i32, z: i32) -> u16;
    /// Whether the cell is ticking in its chunk. A boundary transfer fires only
    /// when both edge cells are active, keeping the two sides symmetric.
    fn active(&self, x: i32, y: i32, z: i32) -> bool;
}

fn in_chunk(x: i32, y: i32, z: i32) -> bool {
    let d = CHUNK_SIZE as i32;
    (0..d).contains(&x) && (0..d).contains(&y) && (0..d).contains(&z)
}

/// Sampler over a single chunk: out-of-chunk positions read as solid, so there's
/// no flow through boundaries (the pre-cross-chunk behavior).
pub struct SelfSample<'a> {
    fluids: &'a FluidLayer,
    storage: &'a ChunkStorage,
    submerged: bool,
}

impl<'a> SelfSample<'a> {
    pub fn new(fluids: &'a FluidLayer, storage: &'a ChunkStorage) -> Self {
        Self {
            fluids,
            storage,
            submerged: matches!(fluids.fill_mode, FluidFillMode::Submerged(_)),
        }
    }
}

impl FluidSample for SelfSample<'_> {
    fn mass(&self, x: i32, y: i32, z: i32) -> u16 {
        if !in_chunk(x, y, z) {
            return 0;
        }
        let p = LocalPos::new_unchecked(x as u8, y as u8, z as u8);
        if let Some(c) = self.fluids.cells.get(&p) {
            c.mass
        } else if self.submerged {
            fluid_capacity(self.storage.voxel(p.to_index()))
        } else {
            0
        }
    }

    fn capacity(&self, x: i32, y: i32, z: i32) -> u16 {
        if !in_chunk(x, y, z) {
            return 0; // no neighbor here -> no capacity (no flow through)
        }
        fluid_capacity(self.storage.voxel(LocalPos::new_unchecked(x as u8, y as u8, z as u8).to_index()))
    }

    fn active(&self, x: i32, y: i32, z: i32) -> bool {
        in_chunk(x, y, z) && self.fluids.active.contains(&LocalPos::new_unchecked(x as u8, y as u8, z as u8))
    }
}

/// Sampler that reads neighbor chunks for out-of-chunk positions. `faces` holds
/// the six face-neighbors in order `[-x, +x, -y, +y, -z, +z]` (`None` = unloaded,
/// read as air, so water may flow out into ungenerated space and be lost).
pub struct NeighborSample<'a> {
    fluids: &'a FluidLayer,
    storage: &'a ChunkStorage,
    submerged: bool,
    faces: [Option<(&'a FluidLayer, &'a ChunkStorage)>; 6],
}

impl<'a> NeighborSample<'a> {
    pub fn new(
        fluids: &'a FluidLayer,
        storage: &'a ChunkStorage,
        faces: [Option<(&'a FluidLayer, &'a ChunkStorage)>; 6],
    ) -> Self {
        Self { fluids, storage, submerged: matches!(fluids.fill_mode, FluidFillMode::Submerged(_)), faces }
    }

    /// Resolve a position to (this chunk or a face-neighbor, wrapped local coords).
    /// Returns `None` for positions out on more than one axis (no diagonal face).
    fn resolve(x: i32, y: i32, z: i32) -> Option<(Option<usize>, u8, u8, u8)> {
        let d = CHUNK_SIZE as i32;
        let mut face = None;
        let mut oob = 0;
        let axis = |v: i32, lo: usize, hi: usize, face: &mut Option<usize>, oob: &mut i32| -> i32 {
            if v < 0 { *face = Some(lo); *oob += 1; v + d }
            else if v >= d { *face = Some(hi); *oob += 1; v - d }
            else { v }
        };
        let lx = axis(x, 0, 1, &mut face, &mut oob);
        let ly = axis(y, 2, 3, &mut face, &mut oob);
        let lz = axis(z, 4, 5, &mut face, &mut oob);
        match oob {
            0 => Some((None, x as u8, y as u8, z as u8)),
            1 => Some((face, lx as u8, ly as u8, lz as u8)),
            _ => None,
        }
    }

    fn chunk(&self, target: Option<usize>) -> Option<(&FluidLayer, &ChunkStorage)> {
        match target {
            None => Some((self.fluids, self.storage)),
            Some(i) => self.faces[i],
        }
    }
}

impl FluidSample for NeighborSample<'_> {
    fn mass(&self, x: i32, y: i32, z: i32) -> u16 {
        let Some((t, lx, ly, lz)) = Self::resolve(x, y, z) else { return 0 };
        let Some((fl, st)) = self.chunk(t) else { return 0 }; // unloaded = air
        let p = LocalPos::new_unchecked(lx, ly, lz);
        if let Some(c) = fl.cells.get(&p) {
            c.mass
        } else if matches!(fl.fill_mode, FluidFillMode::Submerged(_)) {
            fluid_capacity(st.voxel(p.to_index()))
        } else {
            let _ = self.submerged;
            0
        }
    }

    fn capacity(&self, x: i32, y: i32, z: i32) -> u16 {
        let Some((t, lx, ly, lz)) = Self::resolve(x, y, z) else { return 0 };
        let Some((_, st)) = self.chunk(t) else { return FULL_MASS }; // unloaded = air, unrestricted (flow out, lost)
        fluid_capacity(st.voxel(LocalPos::new_unchecked(lx, ly, lz).to_index()))
    }

    fn active(&self, x: i32, y: i32, z: i32) -> bool {
        let Some((t, lx, ly, lz)) = Self::resolve(x, y, z) else { return false };
        let Some((fl, _)) = self.chunk(t) else { return false };
        fl.active.contains(&LocalPos::new_unchecked(lx, ly, lz))
    }
}

/// One chunk's planned tick: the new mass for each cell that changed.
pub struct ChunkPlan {
    changes: Vec<(LocalPos, u16)>,
}

impl ChunkPlan {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The cells this plan changes, as `(position, new mass)`.
    pub fn changed_cells(&self) -> &[(LocalPos, u16)] {
        &self.changes
    }
}

/// Phase 1 (read-only): compute this chunk's new cell masses from the snapshot,
/// reading neighbors via `sample`. Mutates nothing, so all chunks can plan from a
/// consistent pre-tick snapshot.
pub fn plan_chunk(fluids: &FluidLayer, sample: &impl FluidSample) -> ChunkPlan {
    if fluids.active.is_empty() {
        return ChunkPlan { changes: Vec::new() };
    }
    let mut active: Vec<LocalPos> = fluids.active.iter().copied().collect();
    active.sort_unstable();

    // Propose transfers from the snapshot. A transfer endpoint is `None` when it
    // lives in a neighbor chunk (that chunk applies its own side). Boundary
    // transfers fire only when the neighbor cell is also active, so both sides
    // compute the identical transfer. Priority 0 = gravity, 1 = horizontal.
    type Flow = (u8, Option<LocalPos>, Option<LocalPos>, u16);
    let mut flows: Vec<Flow> = Vec::new();
    let lp = |x: i32, y: i32, z: i32| LocalPos::new_unchecked(x as u8, y as u8, z as u8);
    for &pos in &active {
        let (x, y, z) = (pos.x as i32, pos.y as i32, pos.z as i32);
        let m = sample.mass(x, y, z);

        // Down outflow (this cell falls into the cell below).
        let below_capacity = sample.capacity(x, y - 1, z);
        if m > 0 && below_capacity > 0 {
            let flow = m.min(below_capacity - sample.mass(x, y - 1, z));
            if flow > 0 {
                if in_chunk(x, y - 1, z) {
                    flows.push((0, Some(pos), Some(lp(x, y - 1, z)), flow));
                } else if sample.active(x, y - 1, z) {
                    flows.push((0, Some(pos), None, flow));
                }
            }
        }
        // Down inflow from a neighbor chunk above (in-chunk above is handled by
        // that cell's own down flow).
        if !in_chunk(x, y + 1, z) && sample.capacity(x, y + 1, z) > 0 && sample.active(x, y + 1, z) {
            let flow = sample.mass(x, y + 1, z).min(sample.capacity(x, y, z) - m);
            if flow > 0 {
                flows.push((0, None, Some(pos), flow));
            }
        }
        // Horizontal.
        for (dx, dz) in [(-1i32, 0), (1, 0), (0, -1), (0, 1)] {
            if sample.capacity(x + dx, y, z + dz) == 0 {
                continue;
            }
            let nm = sample.mass(x + dx, y, z + dz);
            if in_chunk(x + dx, y, z + dz) {
                if m > nm {
                    let flow = (m - nm) / 4;
                    if flow > 0 {
                        flows.push((1, Some(pos), Some(lp(x + dx, y, z + dz)), flow));
                    }
                }
            } else if sample.active(x + dx, y, z + dz) {
                if m > nm {
                    let flow = (m - nm) / 4;
                    if flow > 0 {
                        flows.push((1, Some(pos), None, flow));
                    }
                } else if nm > m {
                    let flow = (nm - m) / 4;
                    if flow > 0 {
                        flows.push((1, None, Some(pos), flow));
                    }
                }
            }
        }
    }

    // Apply live-capped in a deterministic order (gravity first). Only in-chunk
    // endpoints are tracked/mutated; `None` endpoints belong to the neighbor.
    flows.sort_by_key(|&(p, s, d, _)| (p, s, d));
    let mut live: HashMap<LocalPos, u16> = HashMap::new();
    let seed = |p: Option<LocalPos>, live: &mut HashMap<LocalPos, u16>| {
        if let Some(p) = p {
            live.entry(p).or_insert_with(|| sample.mass(p.x as i32, p.y as i32, p.z as i32));
        }
    };
    for &(_, s, d, _) in &flows {
        seed(s, &mut live);
        seed(d, &mut live);
    }
    for (_, s, d, amt) in flows {
        match (s, d) {
            (Some(s), Some(d)) => {
                let cap_d = sample.capacity(d.x as i32, d.y as i32, d.z as i32);
                let a = amt.min(live[&s]).min(cap_d - live[&d]);
                if a > 0 {
                    *live.get_mut(&s).unwrap() -= a;
                    *live.get_mut(&d).unwrap() += a;
                }
            }
            (Some(s), None) => {
                let a = amt.min(live[&s]);
                *live.get_mut(&s).unwrap() -= a;
            }
            (None, Some(d)) => {
                let cap_d = sample.capacity(d.x as i32, d.y as i32, d.z as i32);
                let a = amt.min(cap_d - live[&d]);
                *live.get_mut(&d).unwrap() += a;
            }
            (None, None) => {}
        }
    }

    let mut changes = Vec::new();
    for (pos, new) in live {
        if new != sample.mass(pos.x as i32, pos.y as i32, pos.z as i32) {
            changes.push((pos, new));
        }
    }
    ChunkPlan { changes }
}

/// Phase 2: apply a plan, run the settling state machine, and return whether
/// anything changed (so the caller can rebuild the water mesh).
pub fn commit_chunk(fluids: &mut FluidLayer, plan: ChunkPlan) -> bool {
    if plan.changes.is_empty() {
        let active_now: Vec<LocalPos> = fluids.active.iter().copied().collect();
        for pos in active_now {
            fluids.note_stable(pos);
        }
        return false;
    }
    let changed_set: HashSet<LocalPos> = plan.changes.iter().map(|&(p, _)| p).collect();

    for &(pos, new) in &plan.changes {
        if new <= MIN_MASS {
            fluids.cells.remove(&pos);
            fluids.active.remove(&pos);
            fluids.stable_ticks.remove(&pos);
        } else {
            fluids
                .cells
                .entry(pos)
                .or_insert(FluidCell { fluid_id: FluidId::WATER, mass: 0, flags: 0 })
                .mass = new;
            fluids.activate(pos);
        }
    }

    for &(pos, _) in &plan.changes {
        for (dx, dy, dz) in [
            (-1i32, 0, 0), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1),
        ] {
            let (nx, ny, nz) = (pos.x as i32 + dx, pos.y as i32 + dy, pos.z as i32 + dz);
            if !in_chunk(nx, ny, nz) {
                continue;
            }
            let np = LocalPos::new_unchecked(nx as u8, ny as u8, nz as u8);
            if fluids.cells.contains_key(&np) {
                fluids.activate(np);
            }
        }
    }

    let active_now: Vec<LocalPos> = fluids.active.iter().copied().collect();
    for pos in active_now {
        if changed_set.contains(&pos) {
            fluids.note_changed(pos);
        } else {
            fluids.note_stable(pos);
        }
    }
    true
}

/// Run one intra-chunk tick (plan against a self-sampler, then commit).
/// Convenience wrapper; the scheduler drives `plan_chunk`/`commit_chunk` directly
/// for the cross-chunk global two-phase.
pub fn simulate_chunk(fluids: &mut FluidLayer, storage: &ChunkStorage) -> bool {
    let plan = plan_chunk(fluids, &SelfSample::new(fluids, storage));
    commit_chunk(fluids, plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::{MaterialId, Voxel};

    fn water(mass: u16) -> FluidCell {
        FluidCell { fluid_id: FluidId::WATER, mass, flags: 0 }
    }

    fn set_solid(s: &mut ChunkStorage, x: u8, y: u8, z: u8) {
        s.set_voxel(LocalPos::new_unchecked(x, y, z).to_index(), Voxel::cube(MaterialId(1)));
    }

    fn total_mass(f: &FluidLayer) -> u32 {
        f.cells.values().map(|c| c.mass as u32).sum()
    }

    #[test]
    fn water_falls_down_a_walled_shaft() {
        // Corner column (0,*,0): two sides are chunk edges (solid); wall the other
        // two so water falls straight instead of spreading.
        let mut s = ChunkStorage::new_air();
        set_solid(&mut s, 0, 0, 0); // floor
        for y in 1..=5u8 {
            set_solid(&mut s, 1, y, 0);
            set_solid(&mut s, 0, y, 1);
        }
        let top = LocalPos::new_unchecked(0, 5, 0);
        let mut f = FluidLayer::default();
        f.cells.insert(top, water(MAX_MASS));
        f.activate(top);

        for _ in 0..10 {
            simulate_chunk(&mut f, &s);
        }
        assert_eq!(f.cells.get(&LocalPos::new_unchecked(0, 1, 0)).map(|c| c.mass), Some(MAX_MASS));
        assert_eq!(total_mass(&f), MAX_MASS as u32); // conserved exactly
    }

    #[test]
    fn water_equalizes_horizontally() {
        // Sealed 2-cell basin at y=1: floor below, walls around, connected only
        // to each other, so flow is purely horizontal a <-> b.
        let mut s = ChunkStorage::new_air();
        for &(x, y, z) in &[
            (5, 0, 5), (6, 0, 5), // floor
            (4, 1, 5), (5, 1, 4), (5, 1, 6), // walls around a
            (7, 1, 5), (6, 1, 4), (6, 1, 6), // walls around b
        ] {
            set_solid(&mut s, x, y, z);
        }
        let a = LocalPos::new_unchecked(5, 1, 5);
        let b = LocalPos::new_unchecked(6, 1, 5);
        let mut f = FluidLayer::default();
        f.cells.insert(a, water(MAX_MASS));
        f.activate(a);

        for _ in 0..60 {
            simulate_chunk(&mut f, &s);
        }
        assert_eq!(total_mass(&f), MAX_MASS as u32); // conserved
        let ma = f.cells.get(&a).map(|c| c.mass).unwrap_or(0) as i32;
        let mb = f.cells.get(&b).map(|c| c.mass).unwrap_or(0) as i32;
        assert!((ma - mb).abs() <= 4, "levels off: a={ma}, b={mb}");
    }

    #[test]
    fn flow_is_deterministic() {
        let mut s = ChunkStorage::new_air();
        set_solid(&mut s, 16, 0, 16); // a floor cell to pool on
        let src = LocalPos::new_unchecked(16, 8, 16);
        let mut a = FluidLayer::default();
        a.cells.insert(src, water(MAX_MASS));
        a.activate(src);
        let mut b = a.clone();

        for _ in 0..12 {
            simulate_chunk(&mut a, &s);
            simulate_chunk(&mut b, &s);
        }
        assert_eq!(a.cells, b.cells);
    }

    #[test]
    fn idle_when_no_active_cells() {
        let s = ChunkStorage::new_air();
        let mut f = FluidLayer::default();
        assert!(!simulate_chunk(&mut f, &s));
    }

    #[test]
    fn settled_pool_leaves_the_active_set() {
        // Water in the sealed 2-cell basin: once it levels and holds steady for
        // the settle threshold, no cells remain active (the pool goes quiet).
        let mut s = ChunkStorage::new_air();
        for &(x, y, z) in &[
            (5, 0, 5), (6, 0, 5),
            (4, 1, 5), (5, 1, 4), (5, 1, 6),
            (7, 1, 5), (6, 1, 4), (6, 1, 6),
        ] {
            set_solid(&mut s, x, y, z);
        }
        let a = LocalPos::new_unchecked(5, 1, 5);
        let mut f = FluidLayer::default();
        f.cells.insert(a, water(MAX_MASS));
        f.activate(a);

        for _ in 0..100 {
            simulate_chunk(&mut f, &s);
        }
        assert!(f.active.is_empty(), "pool should go quiet, active={:?}", f.active);
        assert_eq!(total_mass(&f), MAX_MASS as u32); // still conserved
    }

    #[test]
    fn water_crosses_a_chunk_boundary_symmetrically() {
        // A sealed 2-cell basin split across the x-boundary: A's cell (31,1,5)
        // and B's cell (0,1,5). Water in A should level into B across the seam.
        let mut sa = ChunkStorage::new_air();
        let mut sb = ChunkStorage::new_air();
        // Floors + walls so flow is purely A(31,1,5) <-> B(0,1,5).
        set_solid(&mut sa, 31, 0, 5);
        set_solid(&mut sa, 31, 1, 4);
        set_solid(&mut sa, 31, 1, 6);
        set_solid(&mut sa, 30, 1, 5);
        set_solid(&mut sb, 0, 0, 5);
        set_solid(&mut sb, 0, 1, 4);
        set_solid(&mut sb, 0, 1, 6);
        set_solid(&mut sb, 1, 1, 5);

        let ca = LocalPos::new_unchecked(31, 1, 5);
        let cb = LocalPos::new_unchecked(0, 1, 5);
        let mut fa = FluidLayer::default();
        let mut fb = FluidLayer::default();
        fa.cells.insert(ca, water(MAX_MASS));
        fa.activate(ca);
        fb.activate(cb); // woken empty edge (the scheduler's job in Substep 7)

        for _ in 0..80 {
            // Global two-phase: plan both from the shared snapshot, then commit.
            let plan_a = plan_chunk(&fa, &NeighborSample::new(&fa, &sa, [None, Some((&fb, &sb)), None, None, None, None]));
            let plan_b = plan_chunk(&fb, &NeighborSample::new(&fb, &sb, [Some((&fa, &sa)), None, None, None, None, None]));
            commit_chunk(&mut fa, plan_a);
            commit_chunk(&mut fb, plan_b);
            // Keep both edges awake for the duration (scheduler stand-in).
            fa.activate(ca);
            fb.activate(cb);
        }

        let total = fa.cells.values().map(|c| c.mass as u32).sum::<u32>()
            + fb.cells.values().map(|c| c.mass as u32).sum::<u32>();
        assert_eq!(total, MAX_MASS as u32, "mass conserved across the boundary");
        let ma = fa.cells.get(&ca).map(|c| c.mass).unwrap_or(0) as i32;
        let mb = fb.cells.get(&cb).map(|c| c.mass).unwrap_or(0) as i32;
        assert!((ma - mb).abs() <= 8, "leveled across the seam: a={ma}, b={mb}");
    }
}
