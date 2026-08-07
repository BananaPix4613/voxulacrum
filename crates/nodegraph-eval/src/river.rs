//! River networks (design §5, "River networks").
//!
//! A coarse world grid, one hash-jittered **node** per cell. Each node samples
//! an **elevation potential** - a dedicated low-frequency field, deliberately
//! *not* the generated surface - and links to whichever of its eight
//! neighboring nodes has the lowest potential. The links form a forest, and
//! **acyclicity is structural**: every link strictly decreases potential, so a
//! cycle would require a node lower than itself.
//!
//! **The bed is the network's, not the terrain's.** Carving into the generated
//! surface would need a pointwise surface query per column, which is the cost
//! the potential field exists to avoid - a structure source's density roll gates
//! its probes, and a river has no such gate. Instead, the floor is interpolated
//! along a segment from its endpoint elevations. The consequence, stated so it
//! is not discovered: **a river cannot follow terrain it did not shape.**
//!
//! **Resolved per chunk, sampled per column.** Working out one cell's segment
//! costs nine potential samples; doing that per column would be ~81 per column.
//! A chunk touches at most a small block of cells, so the network is resolved
//! once and each column then tests distance against the few segments near it.
//! That resolution holds no cross-chunk state - every chunk resolves the cells
//! it touches identically, because the network is a pure function of world
//! position and seed.

use fastnoise_lite::NoiseType;
use glam::Vec2;
use nodegraph_ir::{NoiseParams, RiverParams};

use crate::context::EvalContext;
use crate::eval::configured_noise;
use crate::field::CHUNK_DIM;
use crate::scatter::SplitMix64;

const N: f32 = CHUNK_DIM as f32;

/// Elevation a potential maps to.
fn elevation(p: &RiverParams, potential: f32) -> f32 {
    let t = (potential * 0.5 + 0.5).clamp(0.0, 1.0);
    p.bed_low + (p.bed_high - p.bed_low) * t
}

/// Half-width at a potential. Lower potential is further downstream, so wider:
/// monotone, and derived without the unbounded upstream traversal a Strahler
/// order would need.
fn width(p: &RiverParams, potential: f32) -> f32 {
    let t = (potential * 0.5 + 0.5).clamp(0.0, 1.0);
    p.max_width + (p.min_width - p.max_width) * t
}

/// One link from a node to its downstream neighbor.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Segment {
    a: Vec2,
    b: Vec2,
    a_elev: f32,
    b_elev: f32,
    a_width: f32,
    b_width: f32,
}

/// What a river does to one column.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RiverColumn {
    /// World-Y below which the channel does not cut. Density above is removed.
    pub floor_y: f32,
    /// World-Y of the channel's water surface.
    pub water_y: f32,
}

/// The segments reaching one chunk, resolved once.
pub struct RiverNetwork {
    segments: Vec<Segment>,
}

/// Node position and potential for one cell. Pure is the cell coordinates, which
/// is what makes two chunks resolve a shared cell identically.
fn node(ctx: &EvalContext, p: &RiverParams, noise: &fastnoise_lite::FastNoiseLite, cx: i64, cz: i64) -> (Vec2, f32) {
    let mut rng = SplitMix64::new(ctx.world_cell_seed(p.seed, cx, cz));
    let jx = rng.next_f32();
    let jz = rng.next_f32();
    let pos = Vec2::new(
        (cx as f32 + jx) * p.cell_size,
        (cz as f32 + jz) * p.cell_size,
    );
    (pos, noise.get_noise_2d(pos.x, pos.y))
}

impl RiverNetwork {
    /// Resolve every segment that can reach `ctx.chunk`'s column window.
    pub fn resolve(ctx: EvalContext, p: &RiverParams) -> Self {
        let noise = configured_noise(
            ctx.noise_seed(p.seed),
            &NoiseParams { frequency: p.potential_frequency, ..NoiseParams::default() },
            NoiseType::OpenSimplex2,
        );
        // The band covers the chunk plus the widest channel, plus one cell so a
        // segment whose *endpoints* are both outside still get considered.
        let margin = p.max_width + p.cell_size;
        let base_x = ctx.chunk.x as f32 * N;
        let base_z = ctx.chunk.z as f32 * N;
        let cx_lo = ((base_x - margin) / p.cell_size).floor() as i64;
        let cx_hi = ((base_x + N + margin) / p.cell_size).floor() as i64;
        let cz_lo = ((base_z - margin) / p.cell_size).floor() as i64;
        let cz_hi = ((base_z + N + margin) / p.cell_size).floor() as i64;

        let mut segments = Vec::new();
        for cz in cz_lo..=cz_hi {
            for cx in cx_lo..=cx_hi {
                let (a, a_pot) = node(&ctx, p, &noise, cx, cz);
                // Steepest descent among the eight neighbors. A node with no
                // lower neighbor is a terminus and emits nothing.
                let mut best: Option<(Vec2, f32)> = None;
                for dz in -1..=1i64 {
                    for dx in -1..=1i64 {
                        if dx == 0 && dz == 0 {
                            continue;
                        }
                        let (b, b_pot) = node(&ctx, p, &noise, cx + dx, cz + dz);
                        if b_pot < a_pot && best.map_or(true, |(_, p0)| b_pot < p0) {
                            best = Some((b, b_pot));
                        }
                    }
                }
                if let Some((b, b_pot)) = best {
                    segments.push(Segment {
                        a,
                        b,
                        a_elev: elevation(p, a_pot),
                        b_elev: elevation(p, b_pot),
                        a_width: width(p, a_pot),
                        b_width: width(p, b_pot),
                    });
                }
            }
        }
        Self { segments }
    }

    /// The river's effect on one world column, or `None` if it is outside every
    /// channel.
    pub fn sample(&self, p: &RiverParams, wx: f32, wz: f32) -> Option<RiverColumn> {
        let c = Vec2::new(wx, wz);
        let mut best: Option<(f32, f32, f32)> = None; // (distance, bank_elev, width)
        for s in &self.segments {
            let ab = s.b - s.a;
            let len2 = ab.length_squared().max(1e-6);
            let t = ((c - s.a).dot(ab) / len2).clamp(0.0, 1.0);
            let closest = s.a + ab * t;
            let dist = (c - closest).length();
            let width = s.a_width + (s.b_width - s.a_width) * t;
            if dist > width {
                continue;
            }
            let elev = s.a_elev + (s.b_elev - s.a_elev) * t;
            if best.map_or(true, |(d, _, _)| dist < d) {
                best = Some((dist, elev, width));
            }
        }
        let (dist, bank, width) = best?;
        // Floor rises from the bed at the centerline to the bank at the edge.
        let across = (dist / width.max(1e-6)).clamp(0.0, 1.0);
        Some(RiverColumn {
            floor_y: bank - p.depth * (1.0 - across * across),
            water_y: bank - p.depth * 0.25,
        })
    }
    
    /// Density after the river cuts. Above the channel floor the column
    /// becomes air; elsewhere the terrain passes through untouched.
    pub fn carve(&self, p: &RiverParams, wx: f32, wy: f32, wz: f32, density: f32) -> f32 {
        match self.sample(p, wx, wz) {
            Some(col) if wy > col.floor_y => density.min(-1.0),
            _ => density,
        }
    }

    /// Segment count, for tests and diagnostics.
    pub fn len(&self) -> usize {
        self.segments.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;

    fn params() -> RiverParams {
        // Cells small relative to a chunk so the border strip reliably contains
        // channel columns. At 48/10 the strip caught none at this seed, and the
        // test's guard correctly refused to pass on a comparison of `None`s.
        // Measured across cell sizes 24/48/96, widths 6/12/20 and eight seeds:
        // 24/12 yields 118-256 channel columns on the seam at every seed tried,
        // 48/12 yields between 0 and 1.
        RiverParams { cell_size: 24.0, max_width: 12.0, min_width: 4.0, ..RiverParams::default() }
    }

    #[test]
    fn every_link_flows_downhill_so_the_network_cannot_cycle() {
        // Acyclicity is structural rather than checked: a cycle would require a
        // node strictly lower than itself. This asserts the property the
        // argument rests on.
        let net = RiverNetwork::resolve(EvalContext::new(7, IVec3::new(2, 0, -1)), &params());
        assert!(net.len() > 0, "the test window must contain segments");
        for s in &net.segments {
            assert!(
                s.b_elev <= s.a_elev,
                "a link ran uphill: {} -> {}",
                s.a_elev,
                s.b_elev,
            );
        }
    }

    #[test]
    fn resolution_is_a_pure_function_of_seed_and_position() {
        let ctx = EvalContext::new(3, IVec3::new(-4, 0, 6));
        let a = RiverNetwork::resolve(ctx, &params());
        let b = RiverNetwork::resolve(ctx, &params());
        assert_eq!(a.segments, b.segments);

        let other = RiverNetwork::resolve(EvalContext::new(4, IVec3::new(-4, 0, 6)), &params());
        assert_ne!(a.segments, other.segments, "a different seed must move the network");
    }

    #[test]
    fn two_chunks_agree_about_a_column_on_their_shared_border() {
        // The river counterpart of the structure seam test. A column is owned by
        // exactly one chunk, so the property is not "both stamp it" but "both
        // *would compute the same thing* for it" - which is what makes the carve
        // continuous across the border rather than stepping at x = 32.
        let p = params();
        let left = RiverNetwork::resolve(EvalContext::new(11, IVec3::new(0, 0, 0)), &p);
        let right = RiverNetwork::resolve(EvalContext::new(11, IVec3::new(1, 0, 0)), &p);

        let mut compared = 0usize;
        let mut in_channel = 0usize;
        for wz in 0..32 {
            for wx in 28..36 {
                let (x, z) = (wx as f32 + 0.5, wz as f32 + 0.5);
                let a = left.sample(&p, x, z);
                let b = right.sample(&p, x, z);
                assert_eq!(a, b, "chunks disagree about the column at ({x}, {z})");
                compared += 1;
                if a.is_some() {
                    in_channel += 1;
                }
            }
        }
        assert!(compared > 0);
        assert!(
            in_channel > 0,
            "no sampled column fell in a channel, so this compared only `None`s",
        );
    }

    #[test]
    fn a_column_far_from_every_channel_is_untouched() {
        let p = RiverParams { max_width: 2.0, ..params() };
        let net = RiverNetwork::resolve(EvalContext::new(5, IVec3::ZERO), &p);
        let dry = (0..32)
            .flat_map(|z| (0..32).map(move |x| (x as f32, z as f32)))
            .filter(|(x, z)| net.sample(&p, *x, *z).is_none())
            .count();
        assert!(dry > 0, "a 2-voxel channel cannot cover an entire chunk");
    }

    #[test]
    fn the_channel_is_deepest_at_its_centre() {
        let p = params();
        let net = RiverNetwork::resolve(EvalContext::new(11, IVec3::ZERO), &p);
        let s = net.segments.first().copied().expect("a segment");
        let mid = (s.a + s.b) * 0.5;
        let centre = net.sample(&p, mid.x, mid.y).expect("the centreline is in its own channel");
        let perp = (s.b - s.a).perp().normalize_or_zero();
        let edge_pt = mid + perp * (s.a_width * 0.9);
        if let Some(edge) = net.sample(&p, edge_pt.x, edge_pt.y) {
            assert!(
                centre.floor_y < edge.floor_y,
                "the floor must rise from centreline ({}) to bank ({})",
                centre.floor_y,
                edge.floor_y,
            );
        }
    }
}
