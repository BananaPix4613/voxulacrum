//! Runtime visibility / enclosure detection (design §11: underground-visibility
//! note). A bounded 6-connected flood through air from the player's head cell
//! yields the cells the player can see through and an *undergroundness* factor
//! `u` in [0,1] - 0 in open air, 1 in a sealed underground space. This volume is
//! player-anchored, transient, and single-writer (recomputed from one origin on
//! movement, never stored per chunk), so later reuse for fog (§11) and audio
//! occlusion (§12) is safe where the walkability mask was not.
//!
//! `overhead_coverage` is retained as a cheap pre-filter: when little sits
//! overhead the player is plainly outdoors and the flood is skipped.

use std::collections::{HashMap, HashSet, VecDeque};

use bevy_ecs::prelude::Resource;
use glam::IVec3;

/// Latest visibility result, refreshed by `room_detection_system`.
#[derive(Resource, Default)]
pub struct RoomState {
    /// Undergroundness `u` in [0,1] from the flood: 0 open air, 1 sealed.
    pub undergroundness: f32,
    /// `u` cleared the threshold — the mask renders the space as a cutaway.
    pub enclosed: bool,
    /// Head cell of the last flood; recompute-on-move throttle + window center.
    pub last_origin: Option<IVec3>,
    /// Reached air cells (cell -> flood cost). The mask marks exactly these; the
    /// shader classifies each face by the air cell it fronts (note v2 §3).
    pub visible: HashMap<IVec3, u32>,
    /// The flood changed and the mask texture needs re-uploading.
    pub dirty: bool,
    /// World-space radius of the visible volume (max distance from head to a visible
    /// cell); the shader projects this to the on-screen volume-circle size each frame.
    pub volume_radius: f32,
}

/// 6-connected neighbor offsets for the visibility flood.
const NEIGHBORS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

/// Fraction of the *open* columns in a `radius` grid around `head` that have a
/// solid voxel within `height` cells above head level. Kept as a cheap pre-filter:
/// when this is near zero the player is plainly outdoors and the flood is skipped.
/// `is_solid(cell)` is true for solid voxels. Returns 0 when there are no open columns.
pub fn overhead_coverage(
    head: IVec3,
    radius: i32,
    height: i32,
    is_solid: impl Fn(IVec3) -> bool,
) -> f32 {
    let mut covered = 0u32;
    let mut open_cols = 0u32;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let base = IVec3::new(head.x + dx, head.y, head.z + dz);
            // Only count columns the player's space opens into (open at head level).
            if is_solid(base) {
                continue;
            }
            open_cols += 1;
            for dy in 1..=height {
                if is_solid(IVec3::new(base.x, head.y + dy, base.z)) {
                    covered += 1;
                    break;
                }
            }
        }
    }
    if open_cols == 0 {
        0.0
    } else {
        covered as f32 / open_cols as f32
    }
}

/// The visibility flood's output.
pub struct VisibilityVolume {
    /// Reached air cells mapped to their flood cost (6-connected steps from origin).
    pub visible: HashMap<IVec3, u32>,
    /// Undergroundness `u` in [0,1]: the share of the flood frontier that is not
    /// sky-lit. Walls and ceilings count as enclosure; openings to the sky do not.
    pub undergroundness: f32,
}

/// Bounded 6-connected flood through air from `origin`, capped at `bucket` steps.
///
/// `is_air(c)` is true for cells the flood passes through (empty or slab cells).
/// `sky_lit(c)` is true when no solid voxel sits above `c` in its column. The
/// frontier is every non-visible cell adjacent to the visible set - solid walls
/// plus air cells just past the budget - and `undergroundness` is the fraction of
/// that frontier which is not sky-lit.
pub fn flood_visibility(
    origin: IVec3,
    budget: u32,
    is_air: impl Fn(IVec3) -> bool,
    sky_lit: impl Fn(IVec3) -> bool,
) -> VisibilityVolume {
    let mut visible: HashMap<IVec3, u32> = HashMap::new();
    let mut frontier: HashSet<IVec3> = HashSet::new();

    if !is_air(origin) {
        // Head embedded in solid: treat as fully enclosed.
        return VisibilityVolume { visible, undergroundness: 1.0 };
    }

    let mut queue: VecDeque<IVec3> = VecDeque::new();
    visible.insert(origin, 0);
    queue.push_back(origin);

    while let Some(c) = queue.pop_front() {
        let cost = visible[&c];
        for d in NEIGHBORS {
            let n = c + d;
            if visible.contains_key(&n) {
                continue;
            }
            if is_air(n) && cost + 1 <= budget {
                visible.insert(n, cost + 1);
                queue.push_back(n);
            } else {
                // A solid wall, or air just past the budget: a frontier cell.
                frontier.insert(n);
            }
        }
    }

    let undergroundness = if frontier.is_empty() {
        0.0
    } else {
        // A frontier cell is an *opening* only if it is air open to the sky. Solid
        // walls always enclose — even in thin/holey terrain where rock has sky
        // above it — so a cave reads as enclosed regardless of nearby surface holes.
        let openings = frontier.iter().filter(|&&c| is_air(c) && sky_lit(c)).count();
        (frontier.len() - openings) as f32 / frontier.len() as f32
    };

    VisibilityVolume { visible, undergroundness }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_sky_not_a_room() {
        // Solid only below y=0; nothing above → open columns have no ceiling.
        let cov = overhead_coverage(IVec3::new(0, 2, 0), 4, 12, |c| c.y < 0);
        assert!(cov < 0.05, "open sky → ~0 coverage, got {cov}");
    }

    #[test]
    fn ceiling_is_a_room() {
        // A ceiling at y=6, head at y=2 → every open column is covered.
        let cov = overhead_coverage(IVec3::new(0, 2, 0), 4, 12, |c| c.y == 6);
        assert!(cov > 0.99, "full ceiling → ~1.0 coverage, got {cov}");
    }

    #[test]
    fn open_topped_pit_not_a_room() {
        // Walls solid for |x|>2 or |z|>2 (a 5-wide open pit). Only the open interior
        // counts; it has no ceiling → low coverage. (Walls excluded.)
        let is_solid = |c: IVec3| c.x.abs() > 2 || c.z.abs() > 2;
        let cov = overhead_coverage(IVec3::new(0, 2, 0), 5, 12, is_solid);
        assert!(cov < 0.1, "an open-topped pit is not a room, got {cov}");
    }

    #[test]
    fn flood_open_space_is_not_underground() {
        // Air in every direction, all open to sky → the whole frontier is openings.
        let is_air = |_c: IVec3| true;
        let sky_lit = |_c: IVec3| true;
        let vol = flood_visibility(IVec3::new(0, 0, 0), 8, is_air, sky_lit);
        assert!(vol.undergroundness < 0.05, "open space u≈0, got {}", vol.undergroundness);
    }

    #[test]
    fn flood_solid_walls_stay_enclosed_even_under_sky() {
        // A sealed pocket where every cell reports sky-lit (as in thin/holey terrain
        // whose rock has air above it). Solid walls must still count as enclosure.
        let is_air = |c: IVec3| c.x.abs() <= 2 && c.y.abs() <= 2 && c.z.abs() <= 2;
        let sky_lit = |_c: IVec3| true; // pathological: everything "sky-lit"
        let vol = flood_visibility(IVec3::new(0, 0, 0), 12, is_air, sky_lit);
        assert!(vol.undergroundness > 0.99, "solid walls enclose regardless of sky, got {}", vol.undergroundness);
    }

    #[test]
    fn flood_sealed_chamber_is_underground() {
        // A 5^3 air pocket in deep rock: nothing outside the pocket is sky-lit.
        let is_air = |c: IVec3| c.x.abs() <= 2 && c.y.abs() <= 2 && c.z.abs() <= 2;
        let sky_lit = |_c: IVec3| false;
        let vol = flood_visibility(IVec3::new(0, 0, 0), 12, is_air, sky_lit);
        assert_eq!(vol.visible.len(), 125, "the whole pocket is visible");
        assert!(vol.undergroundness > 0.99, "sealed chamber u≈1, got {}", vol.undergroundness);
    }

    #[test]
    fn flood_skylight_hole_stays_underground() {
        // The sealed pocket plus a 1-wide vertical shaft to the sky at (0,*,0).
        let is_air = |c: IVec3| {
            let pocket = c.x.abs() <= 2 && c.y.abs() <= 2 && c.z.abs() <= 2;
            let shaft = c.x == 0 && c.z == 0 && c.y >= 2; // open shaft rising out the top
            pocket || shaft
        };
        // Only the shaft column is open to the sky.
        let sky_lit = |c: IVec3| c.x == 0 && c.z == 0 && c.y >= 2;
        let vol = flood_visibility(IVec3::new(0, 0, 0), 20, is_air, sky_lit);
        assert!(vol.undergroundness > 0.8, "a skylight doesn't disqualify enclosure, got {}", vol.undergroundness);
    }

    #[test]
    fn flood_is_deterministic() {
        let is_air = |c: IVec3| c.x.abs() <= 3 && c.y.abs() <= 3 && c.z.abs() <= 3;
        let sky_lit = |_c: IVec3| false;
        let a = flood_visibility(IVec3::new(0, 0, 0), 10, is_air, sky_lit);
        let b = flood_visibility(IVec3::new(0, 0, 0), 10, is_air, sky_lit);
        assert_eq!(a.visible.len(), b.visible.len());
        assert_eq!(a.undergroundness, b.undergroundness);
    }
}
