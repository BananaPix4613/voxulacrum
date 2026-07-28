//! Per-column biome-border analysis and the `BiomeBorderFade` kernel.
//!
//! At a biome boundary, terrain should blend rather than cut sharply. This
//! module computes the two per-column quantities that drive the blend, plus the
//! kernel that turns them into a weight:
//!
//! - **border distance**: world-column distance to the nearest column of a
//!   different biome, capped at a fade radius.
//! - **nearest neighbor biome**: the differing biome found at that distance
//!   (the column's own biome when none lies within the radius).
//!
//! The scan uses [`ColumnEvaluator::sample_column`], which is chunk-independent,
//! so a column's border analysis is identical no matter which chunk computes it,
//! and the scan reaches correctly into neighbor chunks. Nothing consumes this
//! yet - with one biome there are no borders; it activates once the world has
//! multiple biomes.

use glam::IVec3;
use nodegraph_ir::NodeId;

use crate::column::{ColumnField, IdColumn};
use crate::column_eval::{ColumnEvaluator, ColumnSample};
use crate::error::{EvalError, EvalResult};
use crate::field::CHUNK_DIM;

/// Per-column biome-border metadata for one chunk: the distance to the nearest
/// differing biome (capped at the fade radius) and which biome that is.
pub struct BorderAnalysis {
    /// Distance, in world columns, to the nearest differing biome - capped at
    /// the fade radius when none is closer.
    pub distance: ColumnField,
    /// The nearest differing biome per column, or the column's own biome when
    /// none lies within the radius.
    pub neighbor: IdColumn,
}

/// Compute per-column biome-border metadata for `chunk` by scanning each
/// column's `radius`-neighborhood (which may reach into adjacent chunks) for the
/// nearest column of a different biome.
///
/// `biome_node` is the id-producing terminal of the graph `eval` wraps (a
/// `ZoneOutput`, or a `WorldOutput` for zone borders). Columns are framed by
/// absolute world coordinates from `chunk`; since [`ColumnEvaluator::sample_column`]
/// is chunk-independent, the evaluator's own chunk is irrelevant to the result.
pub fn analyze_biome_borders(
    eval: &ColumnEvaluator,
    biome_node: NodeId,
    chunk: IVec3,
    radius: i32,
) -> EvalResult<BorderAnalysis> {
    let r = radius.max(0);
    let dim = CHUNK_DIM as i32;
    let side = (dim + 2 * r) as usize;
    let base_x = chunk.x * dim - r;
    let base_z = chunk.z * dim - r;
    
    // Sample the biome id for every column of the radius-extended footprint once,
    // so the neighborhood scan below is just grid reads (instead of re-sampling,
    // and re-noising, per neighbor pair).
    let mut grid = vec![0u16; side * side];
    for ez in 0..side {
        for ex in 0..side {
            let wx = base_x + ex as i32;
            let wz = base_z + ez as i32;
            grid[ez * side + ex] = sample_id(eval, biome_node, wx, wz)?;
        }
    }
    let at = |ex: i32, ez: i32| grid[ez as usize * side + ex as usize];

    let mut distance = ColumnField::filled(r as f32);
    let mut neighbor = IdColumn::zeroed();
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let (gx, gz) = (x as i32 + r, z as i32 + r);
            let own = at(gx, gz);
            neighbor.set(x, z, own);

            // Nearest differing biome within the radius (squared Euclidean).
            let mut best_d2 = (r * r) as f32;
            for dz in -r..=r {
                for dx in -r..=r {
                    let d2 = (dx * dx + dz * dz) as f32;
                    if d2 == 0.0 || d2 >= best_d2 {
                        continue;
                    }
                    let other = at(gx + dx, gz + dz);
                    if other != own {
                        best_d2 = d2;
                        neighbor.set(x, z, other);
                    }
                }
            }
            distance.set(x, z, best_d2.sqrt());
        }
    }
    Ok(BorderAnalysis { distance, neighbor })
}

/// Sample a column's discrete biome id, erroring if the node is not id-typed.
fn sample_id(eval: &ColumnEvaluator, node: NodeId, wx: i32, wz: i32) -> EvalResult<u16> {
    match eval.sample_column(node, wx, wz)? {
        ColumnSample::Id(b) => Ok(b),
        ColumnSample::Surface(_) => Err(EvalError::WrongInputType {
            node,
            expected: "id",
            got: "surface",
        }),
    }
}

/// `BiomeBorderFade` kernel: the weight of a column's *own* biome given its
/// border `distance` and the fade `radius`. `0.5` at a border (even blend with
/// the neighbor) ramping linearly to `1.0` at or beyond the radius (pure own
/// biome). A non-positive radius disables fading (always `1.0`).
pub fn biome_border_fade(distance: f32, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 1.0;
    }
    (0.5 + 0.5 * (distance / radius)).clamp(0.5, 1.0)
}

/// Blend a column's own density with its neighbor biome's density using a fade
/// `weight` (see [`biome_border_fade`]): `weight` toward `own`, the remainder
/// toward `neighbor`.
pub fn blend_density(own: f32, neighbor: f32, weight: f32) -> f32 {
    let w = weight.clamp(0.0, 1.0);
    own * w + neighbor * (1.0 - w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::EvalContext;
    use nodegraph_ir::{Graph, NodeKind, NoiseParams, PinRef, ZoneOutputParams};

    /// ZoneGraph (SurfaceNoise -> ZoneOutput) plus its terminal node id.
    fn zone_graph(biome_bands: Vec<f32>) -> (Graph, NodeId) {
        let mut g = Graph::new();
        let noise = g.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let out = g.add_node(NodeKind::ZoneOutput(ZoneOutputParams { biome_bands, ..Default::default() }));
        g.connect(PinRef::new(noise, 0), PinRef::new(out, 0)).unwrap();
        (g, out)
    }

    #[test]
    fn fade_kernel_endpoints() {
        assert_eq!(biome_border_fade(0.0, 8.0), 0.5);  // at a border: even blend
        assert_eq!(biome_border_fade(8.0, 8.0), 1.0);  // at radius: pure own
        assert_eq!(biome_border_fade(20.0, 8.0), 1.0); // beyond radius: clamped
        assert_eq!(biome_border_fade(4.0, 8.0), 0.75); // midway
        assert_eq!(biome_border_fade(3.0, 0.0), 1.0);  // disabled
    }

    #[test]
    fn blend_density_lerps() {
        assert_eq!(blend_density(10.0, 20.0, 1.0), 10.0); // pure own
        assert_eq!(blend_density(10.0, 20.0, 0.5), 15.0); // even
        assert_eq!(blend_density(10.0, 20.0, 0.0), 20.0); // pure neighbor
    }

    #[test]
    fn single_biome_has_no_borders() {
        // No bands ⇒ biome 0 everywhere ⇒ every column sits at the capped radius
        // with itself as its "neighbor".
        let (g, out) = zone_graph(vec![]);
        let eval = ColumnEvaluator::new(&g, EvalContext::new(1, IVec3::ZERO));
        let radius = 6;
        let b = analyze_biome_borders(&eval, out, IVec3::ZERO, radius).unwrap();
        assert!(b.distance.data().iter().all(|&d| d == radius as f32));
        assert!(b.neighbor.data().iter().all(|&n| n == 0));
    }

    #[test]
    fn border_analysis_is_deterministic_and_bounded() {
        // A single band over noise yields a (likely) two-biome chunk.
        let (g, out) = zone_graph(vec![0.0]);
        let chunk = IVec3::new(2, 0, -1);
        let run = || {
            let eval = ColumnEvaluator::new(&g, EvalContext::new(9, chunk));
            analyze_biome_borders(&eval, out, chunk, 8).unwrap()
        };
        let (a, b) = (run(), run());
        assert_eq!(a.distance.data(), b.distance.data());
        assert_eq!(a.neighbor.data(), b.neighbor.data());
        assert!(a.distance.data().iter().all(|&d| (0.0..=8.0).contains(&d)));
        assert!(a.neighbor.data().iter().all(|&n| n <= 1));
    }

    #[test]
    fn border_scan_crosses_chunk_boundary_consistently() {
        // The edge column (x = 31) must be framed by absolute world coords and
        // scan into the adjacent chunk. Validate against an independent
        // brute-force scan (no early-skip).
        let (g, out) = zone_graph(vec![0.0]);
        let chunk = IVec3::new(0, 0, 0);
        let eval = ColumnEvaluator::new(&g, EvalContext::new(4, chunk));
        let radius = 5;
        let analysis = analyze_biome_borders(&eval, out, chunk, radius).unwrap();

        let (cx, cz) = (CHUNK_DIM - 1, 0usize);
        let wx = chunk.x * CHUNK_DIM as i32 + cx as i32;
        let wz = chunk.z * CHUNK_DIM as i32 + cz as i32;
        let id_at = |x: i32, z: i32| match eval.sample_column(out, x, z).unwrap() {
            ColumnSample::Id(b) => b,
            ColumnSample::Surface(_) => unreachable!(),
        };
        let own = id_at(wx, wz);
        let mut best_d2 = (radius * radius) as f32;
        let mut nbr = own;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let d2 = (dx * dx + dz * dz) as f32;
                let other = id_at(wx + dx, wz + dz);
                if other != own && d2 < best_d2 {
                    best_d2 = d2;
                    nbr = other;
                }
            }
        }
        assert_eq!(analysis.distance.get(cx, cz), best_d2.sqrt());
        assert_eq!(analysis.neighbor.get(cx, cz), nbr);
    }
}
