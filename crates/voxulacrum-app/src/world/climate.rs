//! Cloud/vapor density simulation (climate redesign phase 3): a coarse macro-
//! grid cellular automaton, independent of the voxel/chunk grid. Each cell
//! covers a large patch of world space (`CELL_SIZE`) and holds a density
//! scalar advected by wind and relaxed toward a per-cell condensation target
//! (Substep 3b: humidity vs. a biome's condensation threshold). Precipitation
//! and rendering wiring follow in later substeps.
//!
//! Deliberately kept separate from `world::fluid_sim` - no cross-feedback
//! between weather and fluid simulation (fluid sim is already expensive).
//!
//! Not yet wired to the frame schedule (later substep), hence the module-level
//! allow.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use glam::Vec2;
use nodegraph_eval::WorldEvaluator;

/// World-space size of one macro-grid cell, in world units. Deliberately
/// coarse and independent of the voxel/chunk grid - the simulation only
/// needs to know roughly which biome a broad region counts as and whether
/// it's near water (see `regional_condensation_target`), not fine-grained
/// terrain data, and biome identity itself already blends over ~320m in this
/// engine (`nodegraph_eval::world_eval::FADE_RADIUS`). A coarser cell means
/// fewer active cells and fewer regional lookups per tick - that's the actual
/// cost lever, not the render-time detail noise (see `cloud_shadow`'s
/// `DETAIL_FLOOR` blending), which is free to be much finer.
pub const CELL_SIZE: f32 = 512.0;

/// Cells at or below this density are treated as empty and dropped, so
/// infinitesimal residue can't accumulate forever (mirrors `fluid_sim`'s
/// `MIN_MASS` treatment).
const MIN_DENSITY: f32 = 0.01;

/// A cell stops ticking after this many consecutive stable ticks (no
/// significant change), mirroring `FluidLayer::SETTLE_AFTER_TICKS`.
const SETTLE_AFTER_TICKS: u16 = 8;

/// Integer macro-grid cell coordinate (world position / `CELL_SIZE`, floored).
pub type CellPos = (i32, i32);

/// One simulated cloud/vapor layer's macro-grid state.
#[derive(Clone, Default)]
pub struct ClimateGrid {
    /// Density at each non-empty cell, `0.0..=1.0`.
    pub density: HashMap<CellPos, f32>,
    /// Cells currently ticking.
    pub active: HashSet<CellPos>,
    /// Consecutive stable ticks per active cell (mirrors `FluidLayer`).
    stable_ticks: HashMap<CellPos, u16>,
}

impl ClimateGrid {
    /// Wake a cell: it joins the ticking set and its stability counter resets.
    pub fn activate(&mut self, pos: CellPos) {
        self.active.insert(pos);
        self.stable_ticks.remove(&pos);
    }

    fn settle(&mut self, pos: CellPos) {
        self.active.remove(&pos);
        self.stable_ticks.remove(&pos);
    }

    /// Record a tick where `pos` didn't change; settles it once
    /// `SETTLE_AFTER_TICKS` consecutive stable ticks have passed.
    fn note_stable(&mut self, pos: CellPos) {
        let n = self.stable_ticks.entry(pos).or_insert(0);
        *n += 1;
        if *n >= SETTLE_AFTER_TICKS {
            self.settle(pos);
        }
    }

    /// Record a tick where `pos` changed, resetting its stability counter.
    fn note_changed(&mut self, pos: CellPos) {
        self.stable_ticks.insert(pos, 0);
    }

    pub fn density_at(&self, pos: CellPos) -> f32 {
        self.density.get(&pos).copied().unwrap_or(0.0)
    }
}

/// Splits a 2D travel vector (in cells) into the two grid-axis neighbor
/// offsets it points between, with normalized weights (`wx + wz == 1`, or
/// both `0.0` if `travel` is ~zero). E.g. wind angled 30 degrees off +X
/// splits mostly onto the `(+1, 0)` neighbor and partly onto `(0, +1)` or
/// `(0, -1)`. A directional donor-cell scheme - cheap, and adequate for a
/// grid this coarse (a full semi-Lagrangian solve would be overkill here).
fn axis_split(travel: Vec2) -> (i32, i32, f32, f32) {
    let ax = travel.x.abs();
    let az = travel.y.abs();
    let total = ax + az;
    if total <= f32::EPSILON {
        return (0, 0, 0.0, 0.0);
    }
    let x_offset = if travel.x >= 0.0 { 1 } else { -1 };
    let z_offset = if travel.y >= 0.0 { 1 } else { -1 };
    (x_offset, z_offset, ax / total, az / total)
}

/// The local density a cell relaxes toward, given how much moisture is
/// locally available (`humidity`, `0.0..=1.0`) versus how readily this
/// biome's air condenses it into visible cloud (`threshold`, `0.0..=1.0` -
/// lower means clouds form more easily, e.g. over oceans/wetlands). Below the
/// threshold, cloud dissipates (target `0.0`); at or above it, cloud builds
/// toward the available humidity.
pub fn condensation_target(humidity: f32, threshold: f32) -> f32 {
    if humidity >= threshold {
        humidity.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Biome-param names this module reads for cloud condensation. A biome
/// without either entry falls back to the caller-supplied defaults, so
/// existing world manifests need no changes to keep working.
pub const CLOUD_HUMIDITY_PARAM: &str = "cloud_humidity";
pub const CLOUD_CONDENSATION_PARAM: &str = "cloud_condensation";

/// Coherent (correlated, not independent-per-cell) pseudo-noise in
/// `0.0..=1.0`, for regional humidity variation at a scale coarser than a
/// single macro cell. Unlike a per-cell hash (which can only ever vary at
/// exactly the grid's own resolution, and was rejected earlier for producing
/// speckle instead of shape), this interpolates between hashed lattice
/// points so neighboring queries correlate - giving genuine multi-cell
/// "here's a humid region, there's a dry one" structure.
fn coherent_noise(x: f32, y: f32) -> f32 {
    fn hash(ix: i32, iy: i32) -> f32 {
        let mut h: u32 = (ix as u32).wrapping_mul(0x9E37_79B1) ^ (iy as u32).wrapping_mul(0x85EB_CA77);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        h = h.wrapping_mul(0x297A_2D39);
        h ^= h >> 15;
        (h as f32) / (u32::MAX as f32)
    }
    fn smooth(t: f32) -> f32 {
        t * t * (3.0 - 2.0 * t)
    }

    let x0 = x.floor();
    let y0 = y.floor();
    let tx = smooth(x - x0);
    let ty = smooth(y - y0);
    let (ix0, iy0) = (x0 as i32, y0 as i32);
    let a = hash(ix0, iy0);
    let b = hash(ix0 + 1, iy0);
    let c = hash(ix0, iy0 + 1);
    let d = hash(ix0 + 1, iy0 + 1);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * ty
}

/// How many macro cells are "climate region" spans for `ambient_humidity`'s
/// regional variation - large enough to read as genuine clear/cloudy
/// regions, not per-cell speckle.
const HUMIDITY_REGION_CELLS: f32 = 1.5;

/// The humidity used for a macro cell when no biome declares `cloud_humidity`
/// - the layer's flat default, scaled by a coherent regional variation (so
/// unauthored terrain still has real clear/cloudy regions instead of one
/// flat value everywhere), then boosted when the cell's representative chunk
/// has water present (a cheap proxy for "near a lake/ocean/river," reusing
/// data that's already loaded for a resident chunk rather than requiring any
/// new persisted state or fine-grained hydrology).
fn ambient_humidity(pos: CellPos, default_humidity: f32, has_water: bool, water_humidity_boost: f32) -> f32 {
    let regional = coherent_noise(
        pos.0 as f32 / HUMIDITY_REGION_CELLS,
        pos.1 as f32 / HUMIDITY_REGION_CELLS,
    );
    let base = default_humidity * (0.3 + 0.7 * regional);
    if has_water {
        (base + water_humidity_boost).min(1.0)
    } else {
        base
    }
}

/// `condensation_target`, sourced from a biome's `BiomeParams` sidecar
/// (falling back to `ambient_humidity`/`default_condensation` for biomes that
/// don't declare either param). `biome_id` and `has_water` describe the ONE
/// representative chunk a caller sampled for this macro cell - a rough
/// estimate, not a precise aggregate, by design: the simulation only needs to
/// know roughly which biome a broad region counts as and whether it's near
/// water, not exact per-voxel data.
pub fn regional_condensation_target(
    world_eval: &WorldEvaluator,
    biome_id: u16,
    pos: CellPos,
    has_water: bool,
    default_humidity: f32,
    default_condensation: f32,
    water_humidity_boost: f32,
) -> f32 {
    let humidity = world_eval
        .biome_param(biome_id, CLOUD_HUMIDITY_PARAM)
        .unwrap_or_else(|| ambient_humidity(pos, default_humidity, has_water, water_humidity_boost));
    let threshold = world_eval
        .biome_param(biome_id, CLOUD_CONDENSATION_PARAM)
        .unwrap_or(default_condensation);
    condensation_target(humidity, threshold)
}

/// Phase 1 (read-only): compute each touched cell's new density from the
/// current snapshot - advects a fraction of each active cell's density
/// downwind, then relaxes every touched cell toward `condensation_target`'s
/// per-cell result. Two-phase (plan/commit) so, like `fluid_sim::plan_chunk`,
/// results don't depend on hashmap iteration order. Only returns cells whose
/// density actually moved.
fn plan(
    grid: &ClimateGrid,
    wind: Vec2,
    dt: f32,
    decay_rate: f32,
    mut condensation_target: impl FnMut(CellPos) -> f32,
) -> Vec<(CellPos, f32)> {
    if grid.active.is_empty() {
        return Vec::new();
    }

    let travel = wind * dt / CELL_SIZE;
    let (x_offset, z_offset, wx, wz) = axis_split(travel);
    let outflow_fraction = travel.length().min(1.0);

    let mut touched: HashSet<CellPos> = HashSet::new();
    for &pos in &grid.active {
        touched.insert(pos);
        // Only touch a neighbor along an axis the wind actually has weight
        // on - `axis_split` always returns *some* direction for each axis
        // even when its weight is exactly 0.0 (e.g. pure +x wind still
        // picks a z_offset sign), and touching a zero-weight neighbor still
        // runs it through the relaxation step below, spuriously activating
        // cells the wind never actually reaches.
        if wx > 0.0 {
            touched.insert((pos.0 + x_offset, pos.1));
        }
        if wz > 0.0 {
            touched.insert((pos.0, pos.1 + z_offset));
        }
    }

    let mut live: HashMap<CellPos, f32> =
        touched.iter().map(|&p| (p, grid.density_at(p))).collect();

    for &pos in &grid.active {
        let d = grid.density_at(pos);
        if d <= 0.0 {
            continue;
        }
        let outflow = d * outflow_fraction;
        *live.get_mut(&pos).unwrap() -= outflow;
        // Mirror the same wx/wz gating used when building `touched` above -
        // a neighbor only exists in `live` when its axis actually has
        // nonzero weight, so writing to it unconditionally here panics on
        // the zero-weight axis (e.g. a pure +x wind has no z-neighbor
        // entry at all).
        if wx > 0.0 {
            *live.get_mut(&(pos.0 + x_offset, pos.1)).unwrap() += outflow * wx;
        }
        if wz > 0.0 {
            *live.get_mut(&(pos.0, pos.1 + z_offset)).unwrap() += outflow * wz;
        }
    }

    for (&pos, d) in live.iter_mut() {
        let target = condensation_target(pos);
        *d = (*d + (target - *d) * decay_rate * dt).clamp(0.0, 1.0);
    }

    live.into_iter()
        .filter(|&(pos, d)| (d - grid.density_at(pos)).abs() > 1e-4)
        .collect()
}

/// Phase 2: apply a plan, waking any cell that received meaningful inflow and
/// settling ones that go quiet. Returns whether anything changed (mirrors
/// `fluid_sim::commit_chunk`'s return value; useful once this feeds a
/// dirty-texture-upload check in substep 3c).
fn commit(grid: &mut ClimateGrid, changes: Vec<(CellPos, f32)>) -> bool {
    if changes.is_empty() {
        let active_now: Vec<CellPos> = grid.active.iter().copied().collect();
        for pos in active_now {
            grid.note_stable(pos);
        }
        return false;
    }
    let changed_set: HashSet<CellPos> = changes.iter().map(|&(p, _)| p).collect();

    for (pos, new_density) in changes {
        if new_density <= MIN_DENSITY {
            grid.density.remove(&pos);
        } else {
            grid.density.insert(pos, new_density);
        }
        grid.activate(pos);
    }

    let active_now: Vec<CellPos> = grid.active.iter().copied().collect();
    for pos in active_now {
        if changed_set.contains(&pos) {
            grid.note_changed(pos);
        } else {
            grid.note_stable(pos);
        }
    }
    true
}

/// Run one tick (plan then commit) for a single layer's grid.
pub fn simulate(
    grid: &mut ClimateGrid,
    wind: Vec2,
    dt: f32,
    decay_rate: f32,
    condensation_target: impl FnMut(CellPos) -> f32,
) -> bool {
    let changes = plan(grid, wind, dt, decay_rate, condensation_target);
    commit(grid, changes)
}

/// Bilinearly sample the grid's density at an arbitrary world position (as
/// opposed to `density_at`, which reads a single cell exactly). Used to
/// rasterize a smooth texture from the otherwise blocky per-cell field.
pub fn sample_density_bilinear(grid: &ClimateGrid, world_pos: Vec2) -> f32 {
    let cell_pos = world_pos / CELL_SIZE;
    let x0 = cell_pos.x.floor();
    let z0 = cell_pos.y.floor();
    let tx = cell_pos.x - x0;
    let tz = cell_pos.y - z0;
    let (x0, z0) = (x0 as i32, z0 as i32);

    let a = grid.density_at((x0, z0));
    let b = grid.density_at((x0 + 1, z0));
    let c = grid.density_at((x0, z0 + 1));
    let d = grid.density_at((x0 + 1, z0 + 1));
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * tz
}

/// Ensure every macro-cell within `radius_cells` of `center` (world position)
/// is active, so `simulate` will process it. Cheap: touches only the handful
/// of cells actually visible in a render window, not the whole (conceptually
/// unbounded) grid. Pair with `prune_outside_window` (same `center`/
/// `radius_cells`) every tick - waking alone doesn't bound the active set,
/// since a cell the camera has moved away from only settles out naturally
/// once its density decays to stability, which can take arbitrarily long
/// depending on wind speed.
pub fn wake_window(grid: &mut ClimateGrid, center: Vec2, radius_cells: i32) {
    let center_cell = (
        (center.x / CELL_SIZE).floor() as i32,
        (center.y / CELL_SIZE).floor() as i32,
    );
    for cz in -radius_cells..=radius_cells {
        for cx in -radius_cells..=radius_cells {
            grid.activate((center_cell.0 + cx, center_cell.1 + cz));
        }
    }
}

/// Wake every macro-cell within `radius_cells` of `center`, exactly like
/// `wake_window`, but additionally seed any cell that has no density entry
/// yet directly at `target_for(pos)` instead of leaving it to start from 0.0
/// and relax upward over time. Without this, panning the camera faster than
/// a cell can converge means newly-entered cells are almost always caught
/// mid-transition rather than showing their true, stable value - the target
/// is a cheap, deterministic function of position, so there's no reason a
/// freshly-activated cell has to start wrong and catch up.
pub fn seed_new_cells(
    grid: &mut ClimateGrid,
    center: Vec2,
    radius_cells: i32,
    mut target_for: impl FnMut(CellPos) -> f32,
) {
    let center_cell = (
        (center.x / CELL_SIZE).floor() as i32,
        (center.y / CELL_SIZE).floor() as i32,
    );
    for cz in -radius_cells..=radius_cells {
        for cx in -radius_cells..=radius_cells {
            let pos = (center_cell.0 + cx, center_cell.1 + cz);
            if !grid.density.contains_key(&pos) {
                let target = target_for(pos);
                if target > MIN_DENSITY {
                    grid.density.insert(pos, target);
                }
            }
            grid.activate(pos);
        }
    }
}

/// Stop ticking cells outside the window, but KEEP their density. The field
/// must be a function of world position, not camera history - panning away
/// and back must show the same clouds. (Dropping density here was the
/// "panning reseeds the world" bug.)
pub fn settle_outside_window(grid: &mut ClimateGrid, center: Vec2, radius_cells: i32) {
    let center_cell = (
        (center.x / CELL_SIZE).floor() as i32,
        (center.y / CELL_SIZE).floor() as i32,
    );
    let in_range = |&(cx, cz): &CellPos| {
        (cx - center_cell.0).abs() <= radius_cells && (cz - center_cell.1).abs() <= radius_cells
    };
    grid.active.retain(in_range);
    grid.stable_ticks.retain(|pos, _| in_range(pos));
}

/// Memory bound only: drop density far beyond anywhere the camera has
/// recently looked. Cells this far out re-seed deterministically on return.
pub fn evict_density_beyond(grid: &mut ClimateGrid, center: Vec2, radius_cells: i32) {
    let center_cell = (
        (center.x / CELL_SIZE).floor() as i32,
        (center.y / CELL_SIZE).floor() as i32,
    );
    grid.density.retain(|&(cx, cz), _| {
        (cx - center_cell.0).abs() <= radius_cells && (cz - center_cell.1).abs() <= radius_cells
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_when_no_active_cells() {
        let mut grid = ClimateGrid::default();
        assert!(!simulate(&mut grid, Vec2::new(1.0, 0.0), 1.0, 0.1, |_| 0.0));
    }

    #[test]
    fn density_advects_downwind() {
        let mut grid = ClimateGrid::default();
        grid.density.insert((0, 0), 1.0);
        grid.activate((0, 0));

        // Strong +X wind, no decay (target == start density everywhere, so
        // relaxation alone can't explain any change) - density should move
        // off-source.
        for _ in 0..20 {
            simulate(&mut grid, Vec2::new(CELL_SIZE, 0.0), 0.1, 0.0, |_| 1.0);
        }
        assert!(grid.density_at((0, 0)) < 1.0, "source cell density decreased");
        assert!(
            grid.density_at((1, 0)) > 0.0 || grid.density_at((2, 0)) > 0.0,
            "density moved downwind"
        );
    }

    #[test]
    fn decays_toward_target_with_no_wind() {
        let mut grid = ClimateGrid::default();
        grid.density.insert((0, 0), 1.0);
        grid.activate((0, 0));

        for _ in 0..200 {
            simulate(&mut grid, Vec2::ZERO, 0.1, 0.5, |_| 0.2);
        }
        assert!((grid.density_at((0, 0)) - 0.2).abs() < 0.01, "relaxed to target");
    }

    #[test]
    fn grows_toward_condensation_target_when_humid() {
        let mut grid = ClimateGrid::default();
        grid.density.insert((0, 0), 0.0);
        grid.activate((0, 0));

        // Humidity (0.8) clears the threshold (0.3) everywhere -> cloud builds.
        for _ in 0..200 {
            simulate(&mut grid, Vec2::ZERO, 0.1, 0.5, |pos| {
                condensation_target(0.8, 0.3) * if pos == (0, 0) { 1.0 } else { 0.0 }
            });
        }
        assert!(grid.density_at((0, 0)) > 0.7, "cloud grew toward the humid target");
    }

    #[test]
    fn condensation_target_below_threshold_dissipates() {
        assert_eq!(condensation_target(0.2, 0.5), 0.0, "humidity below threshold -> no cloud");
        assert_eq!(condensation_target(0.6, 0.5), 0.6, "humidity above threshold -> builds to it");
    }

    #[test]
    fn flow_is_deterministic() {
        let mut a = ClimateGrid::default();
        a.density.insert((0, 0), 1.0);
        a.activate((0, 0));
        let mut b = a.clone();

        for _ in 0..30 {
            simulate(&mut a, Vec2::new(50.0, 30.0), 0.1, 0.2, |_| 0.3);
            simulate(&mut b, Vec2::new(50.0, 30.0), 0.1, 0.2, |_| 0.3);
        }
        assert_eq!(a.density.len(), b.density.len());
        for (pos, d) in &a.density {
            assert!((d - b.density[pos]).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn quiet_cell_settles_out_of_the_active_set() {
        let mut grid = ClimateGrid::default();
        grid.density.insert((0, 0), 0.3);
        grid.activate((0, 0));

        // Wind-free, target equal to the starting density: nothing to do, so
        // it should settle after SETTLE_AFTER_TICKS stable ticks.
        for _ in 0..SETTLE_AFTER_TICKS {
            simulate(&mut grid, Vec2::ZERO, 0.1, 0.5, |_| 0.3);
        }
        assert!(grid.active.is_empty(), "quiet cell should settle");
    }

    #[test]
    fn bilinear_sample_interpolates_between_cells() {
        let mut grid = ClimateGrid::default();
        grid.density.insert((0, 0), 0.0);
        grid.density.insert((1, 0), 1.0);
        let midpoint = Vec2::new(CELL_SIZE * 0.5, 0.0);
        let sampled = sample_density_bilinear(&grid, midpoint);
        assert!((sampled - 0.5).abs() < 1e-4, "midpoint between a 0 and a 1 cell reads ~0.5, got {sampled}");
    }

    #[test]
    fn wake_window_activates_the_covered_cells() {
        let mut grid = ClimateGrid::default();
        wake_window(&mut grid, Vec2::ZERO, 1);
        for cz in -1..=1 {
            for cx in -1..=1 {
                assert!(grid.active.contains(&(cx, cz)), "cell ({cx},{cz}) should be active");
            }
        }
        assert!(!grid.active.contains(&(2, 0)), "cell outside the radius should not be woken");
    }

    #[test]
    fn ambient_humidity_boosts_near_water() {
        let pos = (5, 5);
        let without_water = ambient_humidity(pos, 0.5, false, 0.2);
        let with_water = ambient_humidity(pos, 0.5, true, 0.2);
        assert!(
            (with_water - without_water - 0.2).abs() < 1e-5,
            "water should add exactly the boost, before clamping"
        );
        assert!(ambient_humidity(pos, 1.0, true, 0.5) <= 1.0, "clamped at 1.0");
    }

    #[test]
    fn settle_outside_window_keeps_density() {
        let mut grid = ClimateGrid::default();
        grid.density.insert((10, 10), 0.5);
        grid.activate((10, 10));
        settle_outside_window(&mut grid, Vec2::ZERO, 1);
        assert!(!grid.active.contains(&(10, 10)), "far cell stops ticking");
        assert!(grid.density.contains_key(&(10, 10)), "...but keeps its density");
    }

    /// THE regression test for the original complaint: panning away and back
    /// must show exactly the field you left.
    #[test]
    fn pan_and_return_preserves_density() {
        let mut grid = ClimateGrid::default();
        seed_new_cells(&mut grid, Vec2::ZERO, 2, |_| 0.6);
        for _ in 0..10 {
            simulate(&mut grid, Vec2::new(30.0, 0.0), 0.1, 0.2, |_| 0.6);
        }
        let before = grid.density.clone();

        // Away phase: settle + evict EVERY tick, exactly as the real
        // CloudShadowState::tick loop does. Settling once and then ticking
        // lets the relaxation-driven activation front (every touched cell
        // climbs toward the target at ~1 cell/tick) escape the observed
        // window and march arbitrarily far - per-tick settling is what
        // bounds it to one leading-edge ring in production, so the test
        // must impose the same rhythm.
        let away = Vec2::new(8.0 * CELL_SIZE, 0.0);
        for _ in 0..10 {
            settle_outside_window(&mut grid, away, 2);
            evict_density_beyond(&mut grid, away, 12);
            simulate(&mut grid, Vec2::new(30.0, 0.0), 0.1, 0.2, |_| 0.6);
        }

        // Return: seeding must not touch surviving cells.
        seed_new_cells(&mut grid, Vec2::ZERO, 2, |_| 0.6);

        let away_cell = (8, 0);
        for (pos, d) in &before {
            // The away window (radius 2) plus one leading-edge ring is
            // OBSERVED during the away phase - those cells may evolve, same
            // as production. Everything else must be frozen exactly.
            let observed =
                (pos.0 - away_cell.0).abs() <= 3 && (pos.1 - away_cell.1).abs() <= 3;
            if observed {
                continue;
            }
            let now = grid.density_at(*pos);
            assert!(
                (now - *d).abs() < 1e-4,
                "cell {pos:?} changed while unobserved: {d} -> {now}"
            );
        }
    }

    /// The complement: a cell inside the CURRENT window is observed - it may
    /// keep simulating. Only settled cells are frozen.
    #[test]
    fn observed_cells_keep_simulating() {
        let mut grid = ClimateGrid::default();
        grid.density.insert((0, 0), 0.1);
        grid.activate((0, 0));
        settle_outside_window(&mut grid, Vec2::ZERO, 2); // (0,0) is in-window: stays active
        simulate(&mut grid, Vec2::ZERO, 0.1, 0.5, |_| 0.6);
        assert!(grid.density_at((0, 0)) > 0.1, "observed cell relaxes toward target");
    }

    #[test]
    fn seed_new_cells_starts_new_cells_at_their_target() {
        let mut grid = ClimateGrid::default();
        seed_new_cells(&mut grid, Vec2::ZERO, 1, |_| 0.6);
        assert!((grid.density_at((0, 0)) - 0.6).abs() < 1e-6, "new cell seeded at target, not 0");
        assert!(grid.active.contains(&(0, 0)));
    }

    #[test]
    fn seed_new_cells_does_not_overwrite_existing_density() {
        let mut grid = ClimateGrid::default();
        grid.density.insert((0, 0), 0.2);
        seed_new_cells(&mut grid, Vec2::ZERO, 1, |_| 0.9);
        assert!(
            (grid.density_at((0, 0)) - 0.2).abs() < 1e-6,
            "existing cell's density is untouched, only new cells are seeded"
        );
    }
}
