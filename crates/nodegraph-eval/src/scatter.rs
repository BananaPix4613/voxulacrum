//! Deterministic point scatter for prop placement.
//!
//! Points live in chunk-local continuous XZ space. To support props that
//! straddle a chunk border, scatter covers a margin band `[-M, N+M)²`
//! ([`PROP_MARGIN`]); placement later clips to `[0, N)`. `JitteredGrid`
//! seeds from world-absolute cells (seam-correct); `PoissonDisk` seeds
//! per-chunk (not seam-continuous - acceptable for Phase 10).

use nodegraph_ir::{JitteredGridParams, PoissonDiskParams};

use crate::context::EvalContext;
use crate::field::CHUNK_DIM;

/// Margin (voxels) beyond `[0, N)` that scatter covers, so a prop footprint
/// crossing a border is still emitted (and clipped) by the owning chunk.
pub const PROP_MARGIN: f32 = 12.0;

const N: f32 = CHUNK_DIM as f32;

/// A scattered point in chunk-local continuous XZ space. `lx`/`lz` may be
/// negative or ≥ N (margin band). `surface_y` is set by a scanner; `None`
/// until then. `seed` is a per-point splitmix stream seed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScatterPoint {
    /// Chunk-local X (continuous; may be outside `[0, N)`).
    pub lx: f32,
    /// Chunk-local Z (continuous; may be outside `[0, N)`).
    pub lz: f32,
    /// Surface Y, set by a scanner (`FindFlat`).
    pub surface_y: Option<i32>,
    /// Per-point seed for downstream stochastic placement.
    pub seed: u64,
}

/// Minimal SplitMix64 PRNG, Deterministic, dependency-free.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self { Self { state: seed } }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// Uniform `f32` in `[0, 1)`.
    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
}

/// World-cell-seeded jittered grid over this chunk's margin band.
pub(crate) fn jittered_grid(ctx: &EvalContext, p: &JitteredGridParams) -> Vec<ScatterPoint> {
    let mut out = Vec::new();
    let cell = p.cell_size.max(0.5);
    let jitter = p.jitter.clamp(0.0, 1.0);

    let base_x = ctx.chunk.x as f32 * N;
    let base_z = ctx.chunk.z as f32 * N;
    let (wx_lo, wx_hi) = (base_x - PROP_MARGIN, base_x + N + PROP_MARGIN);
    let (wz_lo, wz_hi) = (base_z - PROP_MARGIN, base_z + N + PROP_MARGIN);

    let cx_lo = (wx_lo / cell).floor() as i64;
    let cx_hi = (wx_hi / cell).floor() as i64;
    let cz_lo = (wz_lo / cell).floor() as i64;
    let cz_hi = (wz_hi / cell).floor() as i64;

    for cz in cz_lo..=cz_hi {
        for cx in cx_lo..=cx_hi {
            let mut rng = SplitMix64::new(ctx.world_cell_seed(p.seed, cx, cz));
            if rng.next_f32() >= p.density { continue; }
            let jx = (rng.next_f32() - 0.5) * jitter + 0.5;
            let jz = (rng.next_f32() - 0.5) * jitter + 0.5;
            let wx = (cx as f32 + jx) * cell;
            let wz = (cz as f32 + jz) * cell;
            let (lx, lz) = (wx - base_x, wz - base_z);
            if lx < -PROP_MARGIN || lx >= N + PROP_MARGIN
                || lz < -PROP_MARGIN || lz >= N + PROP_MARGIN { continue; }
            out.push(ScatterPoint { lx, lz, surface_y: None, seed: rng.next_u64() });
        }
    }
    out
}

/// Bridson Poisson-disk sampling over this chunk's margin band `[-M, N+M)²`,
/// seeded by `base`. Returns sample positions in chunk-local continuous XZ.
/// Shared by the `PoissonDisk` node (which attaches per-point seeds) and the
/// `PoissonPlacement` library kernel, so the two never drift.
pub fn poisson_placement(base: u64, radius: f32, k: u32) -> Vec<(f32, f32)> {
    let radius = radius.max(0.5);
    let k = k.max(1);
    let lo = -PROP_MARGIN;
    let span = N + 2.0 * PROP_MARGIN;
    let cell = radius / std::f32::consts::SQRT_2;
    let grid_dim = (span / cell).ceil() as usize + 1;
    let mut grid = vec![usize::MAX; grid_dim * grid_dim];
    let mut samples: Vec<(f32, f32)> = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut rng = SplitMix64::new(base);

    let grid_idx = |x: f32, z: f32| -> usize {
        let gx = (((x - lo) / cell) as usize).min(grid_dim - 1);
        let gz = (((z - lo) / cell) as usize).min(grid_dim - 1);
        gz * grid_dim + gx
    };

    let x0 = lo + rng.next_f32() * span;
    let z0 = lo + rng.next_f32() * span;
    samples.push((x0, z0));
    grid[grid_idx(x0, z0)] = 0;
    active.push(0);

    while !active.is_empty() {
        let ai = ((rng.next_f32() * active.len() as f32) as usize).min(active.len() - 1);
        let (px, pz) = samples[active[ai]];
        let mut found = false;
        for _ in 0..k {
            let ang = rng.next_f32() * std::f32::consts::TAU;
            let dist = radius * (1.0 + rng.next_f32());
            let nx = px + ang.cos() * dist;
            let nz = pz + ang.sin() * dist;
            if nx < lo || nx >= lo + span || nz < lo || nz >= lo + span { continue; }
            let gx = ((nx - lo) / cell) as isize;
            let gz = ((nz - lo) / cell) as isize;
            let mut ok = true;
            'outer: for dz in -2..=2 {
                for dx in -2..=2 {
                    let (cxi, czi) = (gx + dx, gz + dz);
                    if cxi < 0 || czi < 0
                        || cxi as usize >= grid_dim || czi as usize >= grid_dim { continue; }
                    let s = grid[czi as usize * grid_dim + cxi as usize];
                    if s != usize::MAX {
                        let (sx, sz) = samples[s];
                        let d2 = (sx - nx) * (sx - nx) + (sz - nz) * (sz - nz);
                        if d2 < radius * radius { ok = false; break 'outer; }
                    }
                }
            }
            if ok {
                let idx = samples.len();
                samples.push((nx, nz));
                grid[grid_idx(nx, nz)] = idx;
                active.push(idx);
                found = true;
                break;
            }
        }
        if !found { active.swap_remove(ai); }
    }

    samples
}

/// Per-chunk Bridson Poisson-disk scatter over the margin band, with per-point
/// seeds attached. Delegates the point layout to [`poisson_placement`].
pub(crate) fn poisson_disk(ctx: &EvalContext, p: &PoissonDiskParams) -> Vec<ScatterPoint> {
    let base = ctx.scatter_seed(p.seed);
    poisson_placement(base, p.radius, p.k)
        .into_iter()
        .enumerate()
        .map(|(i, (lx, lz))| {
            let mut pr = SplitMix64::new(base ^ (i as u64).wrapping_mul(0x9E3779B97F4A7C15));
            ScatterPoint { lx, lz, surface_y: None, seed: pr.next_u64() }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;

    fn ctx(chunk: IVec3) -> EvalContext { EvalContext::new(42, chunk) }

    #[test]
    fn jittered_grid_is_deterministic() {
        let p = JitteredGridParams::default();
        let a = jittered_grid(&ctx(IVec3::ZERO), &p);
        let b = jittered_grid(&ctx(IVec3::ZERO), &p);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn jittered_grid_is_seam_consistent() {
        // A world cell in the overlap of chunk (0,0)'s +X margin and chunk
        // (1,0)'s -X margin must produce the identical WORLD position from
        // both chunks. Compare by converting back to world coords.
        let p = JitteredGridParams { seed: 7, cell_size: 6.0, jitter: 0.6, density: 1.0 };
        let c0 = jittered_grid(&ctx(IVec3::new(0, 0, 0)), &p);
        let c1 = jittered_grid(&ctx(IVec3::new(1, 0, 0)), &p);
        let world0: Vec<(i64, i64)> = c0.iter()
            .map(|pt| (((pt.lx + 0.0 * N) * 16.0) as i64, (pt.lz * 16.0) as i64))
            .collect();
        let world1: Vec<(i64, i64)> = c1.iter()
            .map(|pt| (((pt.lx + 1.0 * N) * 16.0) as i64, (pt.lz * 16.0) as i64))
            .collect();
        // Every point chunk 1 sees in its -X margin (lx < 0) must appear in
        // chunk 0's +X margin (lx ≥ N), at the same world position.
        for (wx, wz) in &world1 {
            if *wx < (N * 16.0) as i64 { // in the shared band near the border
                assert!(world0.contains(&(*wx, *wz)),
                        "seam point {wx},{wz} missing from neighbor chunk");
            }
        }
    }

    #[test]
    fn poisson_respects_min_distance() {
        let p = PoissonDiskParams { seed: 3, radius: 4.0, k: 30 };
        let pts = poisson_disk(&ctx(IVec3::ZERO), &p);
        assert!(pts.len() > 4);
        for i in 0..pts.len() {
            for j in (i + 1)..pts.len() {
                let dx = pts[i].lx - pts[j].lx;
                let dz = pts[i].lz - pts[j].lz;
                assert!((dx * dx + dz * dz).sqrt() >= 4.0 - 1e-3,
                        "two points closer than radius");
            }
        }
    }

    #[test]
    fn poisson_placement_is_deterministic() {
        let a = poisson_placement(12345, 4.0, 30);
        let b = poisson_placement(12345, 4.0, 30);
        assert_eq!(a, b);
        assert!(a.len() > 4);
    }
}
