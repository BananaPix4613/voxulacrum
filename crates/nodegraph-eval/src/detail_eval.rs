//! The DetailGraph evaluator: runs a foliage placement graph against a chunk's
//! terrain to produce paint maps + scatter instances.
//!
//! Topological whole-graph fill (like [`ColumnEvaluator`](crate::ColumnEvaluator)):
//! intermediate nodes cache candidate points / species assignments; the
//! `PaintDensity` and `ScatterPlace` terminals write into a [`ChunkFoliage`].
//! Output is restricted to columns assigned `target_biome`, so the multi-biome
//! harness can union each present biome's foliage disjointly.

use std::collections::HashMap;

use nodegraph_ir::{
    Graph, NodeId, NodeKind, PoissonDistributionParams, SpeciesPickerParams, SurfaceFilterParams,
    Severity,
};
use voxel_core::{ChunkBuffer, Voxel};

use crate::column::IdColumn;
use crate::context::{mix64, EvalContext};
use crate::error::{EvalError, EvalResult};
use crate::field::CHUNK_DIM;
use crate::foliage::{ChunkFoliage, FoliageInstance, PaintLayer, PaintTexel, ScatterBucket};

/// A candidate placement point in chunk-local space (fractional XZ), annotated
/// with its surface Y once a `SurfaceFilter` (or terminal lookup) resolves it.
#[derive(Copy, Clone, Debug)]
struct Candidate {
    x: f32,
    z: f32,
    surface_y: Option<i32>,
}

/// A cached node output in the detail domain.
#[derive(Clone)]
enum DetailValue {
    Positions(Vec<Candidate>),
    Assignments(Vec<(Candidate, u8)>),
}

/// Evaluates a DetailGraph for one chunk against its terrain.
pub struct DetailEvaluator<'a> {
    graph: &'a Graph,
    ctx: EvalContext,
    terrain: &'a ChunkBuffer<Voxel, 32>,
    /// Per-column biome assignment; `None` = uniform single biome.
    biome_column: Option<&'a IdColumn>,
    /// Only columns assigned this biome receive foliage.
    target_biome: u16,
    cache: HashMap<NodeId, DetailValue>,
}

impl<'a> DetailEvaluator<'a> {
    /// New evaluator over a DetailGraph + the chunk's terrain and biome map.
    pub fn new(
        graph: &'a Graph,
        ctx: EvalContext,
        terrain: &'a ChunkBuffer<Voxel, 32>,
        biome_column: Option<&'a IdColumn>,
        target_biome: u16,
    ) -> Self {
        Self { graph, ctx, terrain, biome_column, target_biome, cache: HashMap::new() }
    }

    /// Validate + evaluate the DetailGraph, returning the chunk's foliage.
    pub fn evaluate(&mut self) -> EvalResult<ChunkFoliage> {
        let errors = self
            .graph
            .validate()
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        if errors > 0 {
            return Err(EvalError::InvalidGraph(errors));
        }
        let order = self.graph.topological_order().map_err(|_| EvalError::Cyclic)?;

        let mut foliage = ChunkFoliage::default();
        for id in order {
            if matches!(self.graph.nodes[id].kind, NodeKind::PaintDensity(_)) {
                foliage.paint.push(self.eval_paint(id));
            } else if matches!(self.graph.nodes[id].kind, NodeKind::ScatterPlace(_)) {
                foliage.scatter.push(self.eval_scatter(id)?);
            } else {
                let value = self.eval_intermediate(id)?;
                self.cache.insert(id, value);
            }
        }
        Ok(foliage)
    }

    /// Compute a non-terminal node's cached output.
    fn eval_intermediate(&self, id: NodeId) -> EvalResult<DetailValue> {
        Ok(match &self.graph.nodes[id].kind {
            NodeKind::PoissonDistribution(p) => DetailValue::Positions(self.poisson(p)),
            NodeKind::SurfaceFilter(p) => {
                let pts = self.input_positions(id, 0)?;
                DetailValue::Positions(self.surface_filter(pts, p))
            }
            NodeKind::BiomeContextMask(p) => {
                let pts = self.input_positions(id, 0)?;
                let biome = p.biome;
                DetailValue::Positions(
                    pts.into_iter()
                        .filter(|c| self.column_biome(c.x as usize, c.z as usize) == biome)
                        .collect(),
                )
            }
            NodeKind::SpeciesPicker(p) => {
                let pts = self.input_positions(id, 0)?;
                DetailValue::Assignments(self.species_pick(pts, p))
            }
            // Foliage graphs contain only the detail node kinds.
            _ => return Err(EvalError::WrongGraphDomain { node: id }),
        })
    }

    // --- Intermediate node implementations ---

    /// Seam-safe candidate scatter: a world-cell jittered grid keyed by absolute
    /// cell coords (so placement is continuous across chunk borders). Named
    /// "Poisson" per the node; a jittered grid is the seam-safe approximation
    /// (true Bridson Poisson per-chunk would cut at borders).
    fn poisson(&self, p: &PoissonDistributionParams) -> Vec<Candidate> {
        let cell = p.radius.max(0.5);
        let dim = CHUNK_DIM as f32;
        let base_x = self.ctx.chunk.x * CHUNK_DIM as i32;
        let base_z = self.ctx.chunk.z * CHUNK_DIM as i32;
        let cx0 = (base_x as f32 / cell).floor() as i64 - 1;
        let cx1 = ((base_x as f32 + dim) / cell).ceil() as i64 + 1;
        let cz0 = (base_z as f32 / cell).floor() as i64 - 1;
        let cz1 = ((base_z as f32 + dim) / cell).ceil() as i64 + 1;

        let mut out = Vec::new();
        for cz in cz0..cz1 {
            for cx in cx0..cx1 {
                let seed = self.ctx.world_cell_seed(p.seed, cx, cz);
                let jx = (hash01(seed) - 0.5) * 2.0 * p.jitter;
                let jz = (hash01(seed ^ 0x9E37_79B9) - 0.5) * 2.0 * p.jitter;
                let wx = (cx as f32 + 0.5 + jx) * cell;
                let wz = (cz as f32 + 0.5 + jz) * cell;
                let lx = wx - base_x as f32;
                let lz = wz - base_z as f32;
                if lx >= 0.0 && lx < dim && lz >= 0.0 && lz < dim {
                    out.push(Candidate { x: lx, z: lz, surface_y: None });
                }
            }
        }
        out
    }

    fn surface_filter(&self, pts: Vec<Candidate>, p: &SurfaceFilterParams) -> Vec<Candidate> {
        pts.into_iter()
            .filter_map(|mut c| {
                let (lx, lz) = (c.x as usize, c.z as usize);
                let sy = self.surface_y(lx, lz)?;
                let world_y = (self.ctx.chunk.y * CHUNK_DIM as i32 + sy) as f32;
                if world_y < p.min_height || world_y > p.max_height {
                    return None;
                }
                if !p.materials.is_empty() {
                    let mat = self.terrain.get(lx, sy as usize, lz).material;
                    if !p.materials.contains(&mat) {
                        return None;
                    }
                }
                if self.slope(lx, lz) > p.max_slope {
                    return None;
                }
                c.surface_y = Some(sy);
                Some(c)
            })
            .collect()
    }

    fn species_pick(&self, pts: Vec<Candidate>, p: &SpeciesPickerParams) -> Vec<(Candidate, u8)> {
        let total: f32 = p.weights.iter().copied().sum::<f32>().max(1e-6);
        pts.into_iter()
            .map(|c| {
                let wx = self.ctx.chunk.x * CHUNK_DIM as i32 + c.x as i32;
                let wz = self.ctx.chunk.z * CHUNK_DIM as i32 + c.z as i32;
                let seed = self.ctx.world_cell_seed(p.seed ^ 0x5EED, wx as i64, wz as i64);
                let mut r = hash01(seed) * total;
                let mut species = 0u8;
                for (i, &w) in p.weights.iter().enumerate() {
                    if r < w {
                        species = i as u8;
                        break;
                    }
                    r -= w;
                }
                (c, species)
            })
            .collect()
    }

    // --- Terminal node implementation ---

    fn eval_paint(&self, id: NodeId) -> PaintLayer {
        let p = match &self.graph.nodes[id].kind {
            NodeKind::PaintDensity(p) => p,
            _ => unreachable!("eval_paint on non-PaintDensity"),
        };
        let mut texels = Box::new([PaintTexel::default(); CHUNK_DIM * CHUNK_DIM]);
        for lz in 0..CHUNK_DIM {
            for lx in 0..CHUNK_DIM {
                if self.column_biome(lx, lz) != self.target_biome {
                    continue;
                }
                if self.surface_y(lx, lz).is_none() {
                    continue;
                }
                texels[lx + lz * CHUNK_DIM] = PaintTexel {
                    species: p.species,
                    density: p.density,
                    tint: p.tint,
                    flags: 0,
                };
            }
        }
        // (PaintDensity's optional SurfaceField input is ignored this phase -
        //  uniform density on surfaces; modulation is later refinement.)
        PaintLayer { layer_id: p.layer_id, texels }
    }

    fn eval_scatter(&self, id: NodeId) -> EvalResult<ScatterBucket> {
        let (seed, type_id, prefab_id) = match &self.graph.nodes[id].kind {
            NodeKind::ScatterPlace(p) => (p.seed, p.type_id, p.prefab_id),
            _ => unreachable!("eval_scatter on non-ScatterPlace"),
        };
        let assignments = self.input_assignments(id, 0)?;
        let mut instances = Vec::new();
        let mut seq_by_anchor: HashMap<[u8; 3], u32> = HashMap::new();

        for (c, species) in assignments {
            let (lx, lz) = (c.x as usize, c.z as usize);
            if self.column_biome(lx, lz) != self.target_biome {
                continue;
            }
            let sy = match c.surface_y.or_else(|| self.surface_y(lx, lz)) {
                Some(y) => y,
                None => continue,
            };
            let anchor = [lx as u8, sy as u8, lz as u8];
            let sub_offset = [frac_to_i8(c.x - lx as f32), 0, frac_to_i8(c.z - lz as f32)];

            let wx = self.ctx.chunk.x * CHUNK_DIM as i32 + lx as i32;
            let wy = self.ctx.chunk.y * CHUNK_DIM as i32 + sy;
            let wz = self.ctx.chunk.z * CHUNK_DIM as i32 + lz as i32;

            let seq = seq_by_anchor.entry(anchor).or_insert(0);
            let stable_id = stable_instance_id(self.ctx.world_seed, wx, wy, wz, prefab_id, *seq);
            *seq += 1;

            let rseed = self.ctx.world_cell_seed(seed, wx as i64, wz as i64);
            instances.push(FoliageInstance {
                anchor,
                sub_offset,
                rotation_y: (hash01(rseed) * 256.0) as u8,
                scale_variant: species,
                prefab_id,
                stable_id,
                flags: 0,
            });
        }
        Ok(ScatterBucket { type_id, instances })
    }

    // --- Helpers ---

    /// Topmost solid voxel Y in a column (the surface), or `None` if all air.
    fn surface_y(&self, lx: usize, lz: usize) -> Option<i32> {
        for y in (0..CHUNK_DIM).rev() {
            if self.terrain.get(lx, y, lz) != Voxel::EMPTY {
                return Some(y as i32);
            }
        }
        None
    }

    /// Max surface-Y step to the 4 in-chunk neighbors (in voxels).
    fn slope(&self, lx: usize, lz: usize) -> f32 {
        let h = match self.surface_y(lx, lz) {
            Some(h) => h,
            None => return f32::MAX,
        };
        let mut max_diff = 0;
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = lx as i32 + dx;
            let nz = lz as i32 + dz;
            if nx >= 0 && nx < CHUNK_DIM as i32 && nz >= 0 && nz < CHUNK_DIM as i32 {
                if let Some(nh) = self.surface_y(nx as usize, nz as usize) {
                    max_diff = max_diff.max((h - nh).abs());
                }
            }
        }
        max_diff as f32
    }

    fn column_biome(&self, lx: usize, lz: usize) -> u16 {
        self.biome_column.map_or(0, |b| b.get(lx, lz))
    }

    fn input_positions(&self, node: NodeId, pin: u16) -> EvalResult<Vec<Candidate>> {
        match self.input_value(node, pin)? {
            DetailValue::Positions(v) => Ok(v.clone()),
            DetailValue::Assignments(_) => Err(EvalError::WrongInputType {
                node,
                expected: "positions",
                got: "assignments",
            }),
        }
    }

    fn input_assignments(&self, node: NodeId, pin: u16) -> EvalResult<Vec<(Candidate, u8)>> {
        match self.input_value(node, pin)? {
            DetailValue::Assignments(v) => Ok(v.clone()),
            DetailValue::Positions(_) => Err(EvalError::WrongInputType {
                node,
                expected: "assignments",
                got: "positions",
            }),
        }
    }

    fn input_value(&self, node: NodeId, pin: u16) -> EvalResult<&DetailValue> {
        let edge = self
            .graph
            .edges
            .iter()
            .find(|e| e.to.node == node && e.to.pin == pin)
            .ok_or(EvalError::MissingInput { node, pin })?;
        self.cache
            .get(&edge.from.node)
            .ok_or(EvalError::MissingOutput(edge.from.node))
    }
}

/// Top 24 bits of a seed as `[0, 1)`.
fn hash01(seed: u64) -> f32 {
    ((seed >> 40) as u32 & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// Fractional `[0, 1)` voxel position to a signed sub-voxel byte.
fn frac_to_i8(frac: f32) -> i8 {
    ((frac * 256.0) - 128.0).clamp(-128.0, 127.0) as i8
}

/// Stable instance id (design doc §9): deterministic from generation context,
/// not persisted. `seq` disambiguates instances sharing an anchor cell.
fn stable_instance_id(world_seed: u64, wx: i32, wy: i32, wz: i32, prefab_id: u32, seq: u32) -> u64 {
    let mut h = world_seed;
    h = mix64(h ^ (wx as i64 as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
    h = mix64(h ^ (wy as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    h = mix64(h ^ (wz as i64 as u64).wrapping_mul(0xABC9_8388_FB8F_AC03));
    h = mix64(h ^ (prefab_id as u64).wrapping_mul(0xFF51_AFD7_ED55_8CCD));
    h = mix64(h ^ (seq as u64).wrapping_mul(0xC4CE_B9FE_1A85_EC53));
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use nodegraph_ir::{
        Graph, GraphKind, NodeKind, PaintDensityParams, PinRef, ScatterPlaceParams,
    };
    use voxel_core::MaterialId;

    /// Bottom-half-solid terrain: surface at local y=15 in every column.
    fn flat_terrain() -> ChunkBuffer<Voxel, 32> {
        let mut b = ChunkBuffer::uniform(Voxel::EMPTY);
        for z in 0..32 {
            for y in 0..16 {
                for x in 0..32 {
                    b.set(x, y, z, Voxel::cube(MaterialId(1)));
                }
            }
        }
        b
    }

    fn scatter_graph() -> Graph {
        let mut g = Graph::of_kind(GraphKind::Detail);
        let pois = g.add_node(NodeKind::PoissonDistribution(PoissonDistributionParams {
            seed: 1,
            radius: 8.0,
            jitter: 0.5,
        }));
        let pick = g.add_node(NodeKind::SpeciesPicker(SpeciesPickerParams::default()));
        let place = g.add_node(NodeKind::ScatterPlace(ScatterPlaceParams {
            seed: 2,
            type_id: 0,
            prefab_id: 7,
        }));
        g.connect(PinRef::new(pois, 0), PinRef::new(pick, 0)).unwrap();
        g.connect(PinRef::new(pick, 0), PinRef::new(place, 0)).unwrap();
        g
    }

    /// Chunk-local XZ anchors emitted by `scatter_graph` for one chunk.
    fn anchor_xz(terrain: &ChunkBuffer<Voxel, 32>, g: &Graph, chunk: IVec3) -> Vec<(u8, u8)> {
        let mut e = DetailEvaluator::new(g, EvalContext::new(42, chunk), terrain, None, 0);
        let f = e.evaluate().unwrap();
        let mut v: Vec<(u8, u8)> = f
            .scatter
            .iter()
            .flat_map(|b| b.instances.iter())
            .map(|i| (i.anchor[0], i.anchor[2]))
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn empty_detail_graph_produces_empty_foliage() {
        let terrain = flat_terrain();
        let g = Graph::of_kind(GraphKind::Detail);
        let mut e = DetailEvaluator::new(&g, EvalContext::new(0, IVec3::ZERO), &terrain, None, 0);
        assert!(e.evaluate().unwrap().is_empty());
    }

    #[test]
    fn scatter_is_deterministic_and_nonempty() {
        let terrain = flat_terrain();
        let g = scatter_graph();
        let run = || {
            let mut e = DetailEvaluator::new(
                &g,
                EvalContext::new(42, IVec3::new(1, 0, 2)),
                &terrain,
                None,
                0,
            );
            e.evaluate().unwrap()
        };
        let (a, b) = (run(), run());
        assert_eq!(a, b, "foliage generation must be deterministic");
        assert!(!a.scatter.is_empty());
        assert!(!a.scatter[0].instances.is_empty());
        // Anchors sit on the surface (y=15) with valid sub-offsets.
        assert!(a.scatter[0].instances.iter().all(|i| i.anchor[1] == 15));
    }

    #[test]
    fn paint_covers_surface_columns() {
        let terrain = flat_terrain();
        let mut g = Graph::of_kind(GraphKind::Detail);
        g.add_node(NodeKind::PaintDensity(PaintDensityParams {
            layer_id: 0,
            species: 1,
            density: 200,
            tint: 0,
        }));
        let mut e = DetailEvaluator::new(&g, EvalContext::new(0, IVec3::ZERO), &terrain, None, 0);
        let f = e.evaluate().unwrap();
        assert_eq!(f.paint.len(), 1);
        assert!(f.paint[0].texels.iter().all(|t| t.density == 200 && t.species == 1));
    }

    #[test]
    fn scatter_layout_is_world_absolute_not_per_chunk() {
        // `PoissonDistribution` - the node the shipped meadow detail graph
        // actually uses - seeds each candidate from its *world-absolute* cell
        // (`world_cell_seed`), which is what makes placement continuous across
        // chunk borders. If the seed ever became chunk-relative, every chunk
        // would scan cell indices starting from the same place and produce an
        // identical local layout: a visible tiling artifact, and a silent
        // relocation of every derived `StableInstanceId`.
        let terrain = flat_terrain();
        let g = scatter_graph();
        let a = anchor_xz(&terrain, &g, IVec3::new(0, 0, 0));
        let b = anchor_xz(&terrain, &g, IVec3::new(1, 0, 0));
        assert!(!a.is_empty() && !b.is_empty(), "both chunks must scatter something");
        assert_ne!(a, b, "adjacent chunks must not share a local scatter layout");
    }

    #[test]
    fn scatter_layout_is_independent_of_chunk_y() {
        // The candidate grid is a world-XZ cell grid with no Y term, so two
        // vertically stacked chunks must propose the identical XZ candidates -
        // what differs between them is which candidates survive the surface
        // filter, not where the candidates are. A chunk.y term leaking into the
        // cell seed would break the XZ continuity this node exists to provide.
        // (Contrast `PoissonDisk` in `scatter.rs`, whose per-chunk Bridson walk
        // *is* seeded per chunk including Y - a different node with different
        // guarantees.)
        let terrain = flat_terrain();
        let g = scatter_graph();
        let low = anchor_xz(&terrain, &g, IVec3::new(0, 0, 0));
        let high = anchor_xz(&terrain, &g, IVec3::new(0, 1, 0));
        assert!(!low.is_empty(), "chunk must scatter something");
        assert_eq!(low, high, "XZ candidates must not depend on chunk Y");
    }
}