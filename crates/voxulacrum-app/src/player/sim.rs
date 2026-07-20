//! Pure player simulation (Substep 4). No ECS, no renderer, no `World` type - only
//! `glam` and the [`WorldView`] seam - so this module moves to the server-core
//! crate unchanged at Phase 10. `player_step` is the fixed-timestep function the
//! networking proposal specifies both client and server run.
//!
//! Substep 4 scope: state + the pure-function shape + a determinism test. It reads
//! the world for ground contact and turns to face the move direction, but does not
//! move or collide - that is Substep 5, which fills the physics body of `player_step`.

use glam::{IVec3, Vec2, Vec3};

/// Fixed sim tick duration. `PlayerClock` ticks at this rate; `player_step`
/// integrates against it, so the sim is framerate-independent and deterministic.
pub const TICK_DT: f32 = 1.0 / 30.0;

const HALF_WIDTH: f32 = 0.3; // AABB is 0.6 wide/deep,
const HEIGHT: f32 = 1.5;     // 1.5 tall, feet at `position`.
const GRAVITY: f32 = 24.0;
const MOVE_SPEED: f32 = 4.5;
const TERMINAL_VELOCITY: f32 = -50.0;
const EPS: f32 = 1e-4;
const STEP_HEIGHT: f32 = 1.0; // auto-step at most one cube (slabs land partway)

/// Read-only world access threaded into the pure step. The app's `World` (and,
/// later, the server's) implements it; the step never touches ECS or renderer state.
/// Geometry is the sole authority: standability is decided by the collision
/// resolution itself (rise, move, snap down, fit), not by a precomputed mask.
pub trait WorldView {
    /// The solid vertical interval `[y0, y1]` a voxel occupies, or `None` if empty.
    fn solid_interval(&self, voxel: IVec3) -> Option<(f32, f32)>;
}

/// The player's simulation state - the whole of what `player_step` transforms.
/// `Copy`/`PartialEq` so determinism tests can compare whole traces by value
/// (deterministic runs produce bit-identical floats, so `==` is byte-identical).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PlayerState {
    /// Feet position (bottom-center of the AABB), world-space.
    pub position: Vec3,
    pub velocity: Vec3,
    /// Yaw in radians, in the world XZ plane.
    pub facing: f32,
    pub on_ground: bool,
    /// Reserved status bits (0 for now).
    pub flags: u8,
}

/// One tick of player input. Movement is already world-space (the screen-relative
/// `InputState::move_vector` is rotated by the camera basis before it lands here),
/// so `player_step` is camera-agnostic - the shape the server can replay directly.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct InputFrame {
    /// World-space horizontal move intent: `x` = +X, `y` = +Z, magnitude 0..=1.
    pub move_world: Vec2,
    /// Reserved for Substep 5 (jump).
    pub jump: bool,
}

/// Advance the player by one fixed tick. Pure: reads `state`, `input`, and the
/// read-only `world`; returns the next state. Substep 5a: gravity + per-axis,
/// slab-aware AABB collision. Auto-step is Substep 5b.
pub fn player_step(state: &PlayerState, input: &InputFrame, world: &impl WorldView) -> PlayerState {
    let mut next = *state;

    // Face the move direction when moving.
    if input.move_world.length_squared() > 1e-6 {
        next.facing = input.move_world.y.atan2(input.move_world.x);
    }

    // Horizontal velocity is set directly from input (immediate character control);
    // vertical integrates gravity.
    next.velocity.x = input.move_world.x * MOVE_SPEED;
    next.velocity.z = input.move_world.y * MOVE_SPEED;
    next.velocity.y = (state.velocity.y - GRAVITY * TICK_DT).max(TERMINAL_VELOCITY);

    let disp = next.velocity * TICK_DT;
    let (new_pos, on_ground) = resolve_collision(state.position, disp, world);
    next.position = new_pos;
    next.on_ground = on_ground;
    if on_ground && next.velocity.y < 0.0 {
        next.velocity.y = 0.0; // stop accumulating downward speed while resting
    }
    next
}

/// Resolve `disp` from `pos` against solid voxels, one axis at a time (Y, then
/// horizontal). A grounded horizontal move blocked by a single-cube obstacle
/// triggers a geometric auto-step. Returns the new feet position and grounded state.
fn resolve_collision(pos: Vec3, disp: Vec3, world: &impl WorldView) -> (Vec3, bool) {
    let (mut min, mut max) = feet_aabb(pos);

    // Vertical.
    let dy = clamp_axis(&min, &max, 1, disp.y, world);
    min[1] += dy;
    max[1] += dy;
    let mut on_ground = disp.y < 0.0 && dy > disp.y + EPS;

    // Horizontal, with a geometric auto-step when a grounded move is blocked.
    // Only `min` feeds the returned feet position; the max bound is implied by
    // the fixed AABB size.
    let (h_min, _, blocked) = move_horizontal(&min, &max, disp.x, disp.z, world);
    if blocked && on_ground {
        if let Some((s_min, _)) = try_auto_step(&min, &max, disp.x, disp.z, world) {
            min = s_min;
            on_ground = true; // now standing on the step surface
        } else {
            min = h_min;
        }
    } else {
        min = h_min;
    }

    (Vec3::new(min[0] + HALF_WIDTH, min[1], min[2] + HALF_WIDTH), on_ground)
}

/// Move the AABB horizontally (X then Z). Returns the new bounds and whether
/// either axis was clamped short of its desired delta (blocked).
fn move_horizontal(
    min: &[f32; 3],
    max: &[f32; 3],
    dx_want: f32,
    dz_want: f32,
    world: &impl WorldView,
) -> ([f32; 3], [f32; 3], bool) {
    let mut min = *min;
    let mut max = *max;
    let mut blocked = false;

    let dx = clamp_axis(&min, &max, 0, dx_want, world);
    if (dx - dx_want).abs() > EPS {
        blocked = true;
    }
    min[0] += dx;
    max[0] += dx;

    let dz = clamp_axis(&min, &max, 2, dz_want, world);
    if (dz - dz_want).abs() > EPS {
        blocked = true;
    }
    min[2] += dz;
    max[2] += dz;

    (min, max, blocked)
}

/// Attempt a single-cube (or slab) step-up: rise up to `STEP_HEIGHT`, redo the
/// horizontal move at the raised height, then snap back down onto the step
/// surface. Purely geometric: the rise is bounded by real ceilings, the move by
/// real walls, and the snap-down must land on real support within the step - so
/// "standable" is decided by the same collision the rest of the sim uses.
/// Returns the stepped AABB, or `None` (the caller keeps the blocked result).
fn try_auto_step(
    min: &[f32; 3],
    max: &[f32; 3],
    dx: f32,
    dz: f32,
    world: &impl WorldView,
) -> Option<([f32; 3], [f32; 3])> {
    // Rise (bounded by any ceiling).
    let up = clamp_axis(min, max, 1, STEP_HEIGHT, world);
    if up < EPS {
        return None; // no vertical clearance to step at all
    }
    let mut min = *min;
    let mut max = *max;
    min[1] += up;
    max[1] += up;

    // Horizontal at the raised height. Still blocked -> taller than one cube.
    let (mut min2, mut max2, blocked) = move_horizontal(&min, &max, dx, dz, world);
    if blocked {
        return None;
    }

    // Snap down onto the step surface, no further than we rose (a bigger drop
    // would be walking off a ledge, not stepping up).
    let down = clamp_axis(&min2, &max2, 1, -(up + EPS), world);
    if down <= -(up + EPS) + EPS {
        return None; // nothing to land on within a step
    }
    min2[1] += down;
    max2[1] += down;

    Some((min2, max2))
}

/// AABB `[min, max]` for a player whose feet are at `pos`.
fn feet_aabb(pos: Vec3) -> ([f32; 3], [f32; 3]) {
    (
        [pos.x - HALF_WIDTH, pos.y, pos.z - HALF_WIDTH],
        [pos.x + HALF_WIDTH, pos.y + HEIGHT, pos.z + HALF_WIDTH],
    )
}

/// Largest movement along `axis` (0=X, 1=Y, 2=Z) from the AABB that doesn't
/// penetrate solid geometry. Same sign as `delta`, magnitude <= |delta|. Slab
/// intervals give each voxel a sub-cell box, so a half-slab only blocks the AABB
/// range it actually occupies.
fn clamp_axis(min: &[f32; 3], max: &[f32; 3], axis: usize, delta: f32, world: &impl WorldView) -> f32 {
    if delta.abs() < EPS {
        return 0.0;
    }
    // Broadphase: the AABB swept by `delta` along `axis`.
    let mut smin = *min;
    let mut smax = *max;
    if delta > 0.0 {
        smax[axis] += delta;
    } else {
        smin[axis] += delta;
    }
    let lo = [smin[0].floor() as i32, smin[1].floor() as i32, smin[2].floor() as i32];
    let hi = [
        (smax[0] - EPS).floor() as i32,
        (smax[1] - EPS).floor() as i32,
        (smax[2] - EPS).floor() as i32,
    ];

    let mut allowed = delta;
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                let Some((iy0, iy1)) = world.solid_interval(IVec3::new(x, y, z)) else {
                    continue;
                };
                let bmin = [x as f32, y as f32 + iy0, z as f32];
                let bmax = [(x + 1) as f32, y as f32 + iy1, (z + 1) as f32];

                // Overlap on the two non-axis dims, at the current (unmoved) position.
                let mut lateral = true;
                for a in 0..3 {
                    if a == axis {
                        continue;
                    }
                    if max[a] <= bmin[a] + EPS || min[a] >= bmax[a] - EPS {
                        lateral = false;
                        break;
                    }
                }
                if !lateral {
                    continue;
                }

                if delta > 0.0 {
                    let gap = bmin[axis] - max[axis];
                    if gap >= -EPS {
                        allowed = allowed.min(gap.max(0.0));
                    }
                } else {
                    let gap = bmax[axis] - min[axis];
                    if gap <= EPS {
                        allowed = allowed.max(gap.min(0.0));
                    }
                }
            }
        }
    }
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk_x() -> InputFrame {
        InputFrame { move_world: Vec2::new(1.0, 0.0), jump: false }
    }

    /// Solid floor at y < 0.
    struct FlatWorld;
    impl WorldView for FlatWorld {
        fn solid_interval(&self, v: IVec3) -> Option<(f32, f32)> {
            if v.y < 0 { Some((0.0, 1.0)) } else { None }
        }
    }

    #[test]
    fn deterministic_over_input_sequence() {
        let world = FlatWorld;
        let start = PlayerState { position: Vec3::new(0.5, 5.0, 0.5), ..Default::default() };
        let inputs = [walk_x(); 50];
        let run = || {
            let mut s = start;
            let mut trace = Vec::new();
            for inp in &inputs {
                s = player_step(&s, inp, &world);
                trace.push(s);
            }
            trace
        };
        assert_eq!(run(), run(), "same state+inputs+world must be byte-identical");
    }

    #[test]
    fn falls_and_rests_on_floor() {
        let world = FlatWorld;
        let mut s = PlayerState { position: Vec3::new(0.5, 5.0, 0.5), ..Default::default() };
        for _ in 0..300 {
            s = player_step(&s, &InputFrame::default(), &world);
        }
        assert!((s.position.y - 0.0).abs() < 1e-2, "feet should rest at y=0, got {}", s.position.y);
        assert!(s.on_ground);
        assert!(s.velocity.y.abs() < 1e-2);
    }

    #[test]
    fn stops_at_two_cube_wall() {
        struct WallWorld;
        impl WorldView for WallWorld {
            fn solid_interval(&self, v: IVec3) -> Option<(f32, f32)> {
                if v.y < 0 || (v.x >= 2 && (0..2).contains(&v.y)) { Some((0.0, 1.0)) } else { None }
            }
        }
        let world = WallWorld;
        let mut s = PlayerState { position: Vec3::new(0.5, 0.0, 0.5), on_ground: true, ..Default::default() };
        for _ in 0..120 {
            s = player_step(&s, &walk_x(), &world);
        }
        // A 2-cube wall is not a single-cube step: the raised horizontal move is
        // still blocked, so no auto-step - the player stops.
        assert!(s.position.x <= 1.7 + 1e-2, "should stop before the wall, got {}", s.position.x);
        assert!(s.position.y < 0.5, "must not step a 2-cube wall, y={}", s.position.y);
    }

    #[test]
    fn walks_off_ledge_and_falls() {
        struct LedgeWorld;
        impl WorldView for LedgeWorld {
            fn solid_interval(&self, v: IVec3) -> Option<(f32, f32)> {
                if v.y == -1 && v.x < 2 { Some((0.0, 1.0)) } else { None }
            }
        }
        let world = LedgeWorld;
        let mut s = PlayerState { position: Vec3::new(0.5, 0.0, 0.5), on_ground: true, ..Default::default() };
        for _ in 0..120 {
            s = player_step(&s, &walk_x(), &world);
        }
        assert!(s.position.x > 2.0, "walked off the ledge");
        assert!(s.position.y < -0.5, "fell after leaving the floor");
        assert!(!s.on_ground);
    }

    /// Ground rises by one cube for x >= 2.
    struct StepWorld;
    impl WorldView for StepWorld {
        fn solid_interval(&self, v: IVec3) -> Option<(f32, f32)> {
            if v.y < 0 { Some((0.0, 1.0)) } else if v.x >= 2 && v.y == 0 { Some((0.0, 1.0)) } else { None }
        }
    }

    #[test]
    fn steps_up_one_cube() {
        let world = StepWorld;
        let mut s = PlayerState { position: Vec3::new(0.5, 0.0, 0.5), on_ground: true, ..Default::default() };
        for _ in 0..120 {
            s = player_step(&s, &walk_x(), &world);
        }
        assert!(s.position.x > 2.5, "walked onto the higher ground, x={}", s.position.x);
        assert!((s.position.y - 1.0).abs() < 0.1, "stepped up to y=1, got {}", s.position.y);
        assert!(s.on_ground);
    }

    #[test]
    fn steps_up_a_half_slab() {
        struct SlabStep;
        impl WorldView for SlabStep {
            fn solid_interval(&self, v: IVec3) -> Option<(f32, f32)> {
                if v.y < 0 { Some((0.0, 1.0)) } else if v.x >= 2 && v.y == 0 { Some((0.0, 0.5)) } else { None }
            }
        }
        let world = SlabStep;
        let mut s = PlayerState { position: Vec3::new(0.5, 0.0, 0.5), on_ground: true, ..Default::default() };
        for _ in 0..120 {
            s = player_step(&s, &walk_x(), &world);
        }
        assert!(s.position.x > 2.5);
        assert!((s.position.y - 0.5).abs() < 0.1, "stepped onto the slab top y=0.5, got {}", s.position.y);
    }

    #[test]
    fn steps_onto_slab_top_surface() {
        // A SlabTop's top face is at the cell top (a full 1.0 rise) - geometrically
        // standable, so auto-step takes it. (The old mask gate refused this because
        // SlabTop was never "walkable"; landing on it by falling still worked, so
        // movement rules differed by approach direction. Geometry is now the sole
        // authority and both approaches agree.)
        struct SlabTopStep;
        impl WorldView for SlabTopStep {
            fn solid_interval(&self, v: IVec3) -> Option<(f32, f32)> {
                if v.y < 0 { Some((0.0, 1.0)) } else if v.x >= 2 && v.y == 0 { Some((0.5, 1.0)) } else { None }
            }
        }
        let world = SlabTopStep;
        let mut s = PlayerState { position: Vec3::new(0.5, 0.0, 0.5), on_ground: true, ..Default::default() };
        for _ in 0..120 {
            s = player_step(&s, &walk_x(), &world);
        }
        assert!(s.position.x > 2.5, "walked onto the slab-top surface, x={}", s.position.x);
        assert!((s.position.y - 1.0).abs() < 0.1, "stood on the cell-top face y=1.0, got {}", s.position.y);
    }

    #[test]
    fn autostep_is_deterministic() {
        let world = StepWorld;
        let start = PlayerState { position: Vec3::new(0.5, 0.0, 0.5), on_ground: true, ..Default::default() };
        let run = || {
            let mut s = start;
            let mut trace = Vec::new();
            for _ in 0..60 {
                s = player_step(&s, &walk_x(), &world);
                trace.push(s);
            }
            trace
        };
        assert_eq!(run(), run());
    }
}
