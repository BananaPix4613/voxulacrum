//! The evaluator: topological whole-chunk fill + pointwise sampling.

use std::sync::Arc;

use fastnoise_lite::{FastNoiseLite, FractalType as FnlFractalType, NoiseType};
use nodegraph_ir::{Axis, Graph, FractalType, NoiseParams, NodeId, NodeKind, Severity};
use voxel_core::{ChunkBuffer, MaterialId, ShapeId, Voxel};

use crate::cache::{CachedOutput, EvalCache};
use crate::context::EvalContext;
use crate::error::{EvalError, EvalResult};
use crate::field::{ScalarField, Vec3Field, CHUNK_DIM};
use crate::scatter::ScatterPoint;

/// Evaluates a graph for one chunk. Holds a chunk-scoped CSE cache.
pub struct Evaluator<'g> {
    graph: &'g Graph,
    ctx: EvalContext,
    cache: EvalCache,
}

impl<'g> Evaluator<'g> {
    /// New evaluator over a graph and chunk context.
    pub fn new(graph: &'g Graph, ctx: EvalContext) -> Self {
        Self { graph, ctx, cache: EvalCache::new() }
    }

    /// Borrow the CSE cache (populated by [`Evaluator::evaluate`]).
    pub fn cache(&self) -> &EvalCache {
        &self.cache
    }

    /// Validate, then fill every node's output once in topological order.
    pub fn evaluate(&mut self) -> EvalResult<()> {
        debug_assert!(
            !crate::guard::on_schedule_thread(),
            "graph evaluated on the frame-schedule thread; chunk generation must \
             run on the worker pool, not inline in a schedule system",
        );
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
        for id in order {
            if self.cache.contains(id) {
                continue;
            }
            let out = self.fill_node(id)?;
            self.cache.insert(id, out);
        }
        Ok(())
    }

    /// Build a configured FastNoiseLite for any (params, noise_type) pair.
    fn noise(&self, p: &NoiseParams, noise_type: NoiseType) -> FastNoiseLite {
        let mut n = FastNoiseLite::with_seed(self.ctx.noise_seed(p.seed));
        n.set_noise_type(Some(noise_type));
        n.set_frequency(Some(p.frequency));
        n.set_fractal_type(Some(match p.fractal_type {
            FractalType::FBm => FnlFractalType::FBm,
            FractalType::Ridged => FnlFractalType::Ridged,
            FractalType::PingPong => FnlFractalType::PingPong,
        }));
        n.set_fractal_octaves(Some(p.octaves as i32));
        n.set_fractal_lacunarity(Some(p.lacunarity));
        n.set_fractal_gain(Some(p.gain));
        n
    }

    /// Optional Vec3 override on input pin 0 of a noise source.
    fn position_field(&self, node: NodeId) -> EvalResult<Option<Arc<Vec3Field>>> {
        let Some(edge) = self.graph.edges.iter().find(|e| e.to.node == node && e.to.pin == 0) else {
            return Ok(None);
        };
        let src = self.cache.get(edge.from.node).ok_or(EvalError::MissingOutput(edge.from.node))?;
        match src {
            CachedOutput::Vec3(f) => Ok(Some(f.clone())),
            other => Err(EvalError::WrongInputType {
                node, expected: "vec3", got: other.kind_name(),
            }),
        }
    }

    /// Resolve the scalar field feeding `(node, pin)`. `Scalar → Density`
    /// coercion is a no-op here (both are `f32` fields).
    fn input_scalar(&self, node: NodeId, pin: u16) -> EvalResult<Arc<ScalarField>> {
        let edge = self
            .graph
            .edges
            .iter()
            .find(|e| e.to.node == node && e.to.pin == pin)
            .ok_or(EvalError::MissingInput { node, pin })?;
        let src = self
            .cache
            .get(edge.from.node)
            .ok_or(EvalError::MissingOutput(edge.from.node))?;
        match src {
            CachedOutput::Scalar(f) => Ok(f.clone()),
            other => Err(EvalError::WrongInputType {
                node,
                expected: "scalar/density",
                got: other.kind_name(),
            }),
        }
    }

    /// Resolve a required Vec3 input - like `input_scalar` but for `Vec3Field`.
    fn input_vec3(&self, node: NodeId, pin: u16) -> EvalResult<Arc<Vec3Field>> {
        let edge = self.graph.edges.iter().find(|e| e.to.node == node && e.to.pin == pin)
            .ok_or(EvalError::MissingInput { node, pin })?;
        let src = self.cache.get(edge.from.node).ok_or(EvalError::MissingOutput(edge.from.node))?;
        match src {
            CachedOutput::Vec3(f) => Ok(f.clone()),
            other => Err(EvalError::WrongInputType {
                node, expected: "vec3", got: other.kind_name(),
            })
        }
    }

    fn input_material(
        &self,
        node: NodeId,
        pin: u16,
    ) -> EvalResult<Arc<ChunkBuffer<MaterialId, 32>>> {
        let edge = self
            .graph
            .edges
            .iter()
            .find(|e| e.to.node == node && e.to.pin == pin)
            .ok_or(EvalError::MissingInput { node, pin })?;
        let src = self.cache.get(edge.from.node).ok_or(EvalError::MissingOutput(edge.from.node))?;
        match src {
            CachedOutput::Material(f) => Ok(f.clone()),
            other => Err(EvalError::WrongInputType {
                node,
                expected: "material",
                got: other.kind_name(),
            }),
        }
    }

    fn input_terrain(&self, node: NodeId, pin: u16) -> EvalResult<Arc<ChunkBuffer<Voxel, 32>>> {
        let edge = self.graph.edges.iter()
            .find(|e| e.to.node == node && e.to.pin == pin)
            .ok_or(EvalError::MissingInput { node, pin })?;
        let src = self.cache.get(edge.from.node).ok_or(EvalError::MissingOutput(edge.from.node))?;
        match src {
            CachedOutput::Terrain(f) => Ok(f.clone()),
            other => Err(EvalError::WrongInputType {
                node, expected: "terrain", got: other.kind_name(),
            }),
        }
    }

    fn input_positions(&self, node: NodeId, pin: u16) -> EvalResult<Arc<Vec<ScatterPoint>>> {
        let edge = self.graph.edges.iter()
            .find(|e| e.to.node == node && e.to.pin == pin)
            .ok_or(EvalError::MissingInput { node, pin })?;
        let src = self.cache.get(edge.from.node).ok_or(EvalError::MissingOutput(edge.from.node))?;
        match src {
            CachedOutput::Positions(p) => Ok(p.clone()),
            other => Err(EvalError::WrongInputType {
                node, expected: "positions", got: other.kind_name(),
            }),
        }
    }

    fn fill_noise_2d(&self, p: &NoiseParams, id: NodeId, noise_type: NoiseType) -> EvalResult<Arc<ScalarField>> {
        let n = self.noise(p, noise_type);
        let pos = self.position_field(id)?;
        let mut field = ScalarField::zeroed();
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let (sx, sz) = match &pos {
                    Some(pf) => { let v = pf.get(x, 0, z); (v.x, v.z) }
                    None => { let w = self.ctx.world_pos(x, 0, z); (w.x, w.z) }
                };
                let v = n.get_noise_2d(sx, sz);
                for y in 0..CHUNK_DIM { field.set(x, y, z, v); }
            }
        }
        Ok(Arc::new(field))
    }

    fn fill_noise_3d(&self, p: &NoiseParams, id: NodeId, noise_type: NoiseType) -> EvalResult<Arc<ScalarField>> {
        let n = self.noise(p, noise_type);
        let pos = self.position_field(id)?;
        let mut field = ScalarField::zeroed();
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    let w = match &pos {
                        Some(pf) => pf.get(x, y, z),
                        None => self.ctx.world_pos(x, y, z),
                    };
                    field.set(x, y, z, n.get_noise_3d(w.x, w.y, w.z));
                }
            }
        }
        Ok(Arc::new(field))
    }

    /// Compute one node's output via whole-chunk fill.
    fn fill_node(&self, id: NodeId) -> EvalResult<CachedOutput> {
        let node = self.graph.nodes.get(id).ok_or(EvalError::MissingOutput(id))?;
        Ok(match &node.kind {
            // --- Sources ---
            NodeKind::Constant(p)   => CachedOutput::Scalar(Arc::new(ScalarField::filled(p.value))),
            NodeKind::Perlin2D(p)   => CachedOutput::Scalar(self.fill_noise_2d(p, id, NoiseType::Perlin)?),
            NodeKind::Perlin3D(p)   => CachedOutput::Scalar(self.fill_noise_3d(p, id, NoiseType::Perlin)?),
            NodeKind::Simplex2D(p)  => CachedOutput::Scalar(self.fill_noise_2d(p, id, NoiseType::OpenSimplex2)?),
            NodeKind::Simplex3D(p)  => CachedOutput::Scalar(self.fill_noise_3d(p, id, NoiseType::OpenSimplex2)?),
            NodeKind::WorldPos(_)   => CachedOutput::Vec3(Arc::new(
                Vec3Field::from_fn(|x, y, z| self.ctx.world_pos(x, y, z))
            )),
            NodeKind::WorldAxis(p) => {
                let axis = p.axis;
                let mut field = ScalarField::zeroed();
                for z in 0..CHUNK_DIM {
                    for y in 0..CHUNK_DIM {
                        for x in 0..CHUNK_DIM {
                            let w = self.ctx.world_pos(x, y, z);
                            let v = match axis {
                                Axis::X => w.x,
                                Axis::Y => w.y,
                                Axis::Z => w.z,
                            };
                            field.set(x, y, z, v);
                        }
                    }
                }
                CachedOutput::Scalar(Arc::new(field))
            },
            // --- Math (all elementwise on density fields) ---
            NodeKind::Add(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, |x, y| x + y)))
            }
            NodeKind::Multiply(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, |x, y| x * y)))
            }
            NodeKind::Subtract(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, |x, y| x - y)))
            }
            NodeKind::Min(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, f32::min)))
            }
            NodeKind::Max(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, f32::max)))
            }
            NodeKind::Clamp(p) => {
                let inp = self.input_scalar(id, 0)?;
                let (lo, hi) = (p.min, p.max);
                CachedOutput::Scalar(Arc::new(inp.map(|v| v.clamp(lo, hi))))
            }
            NodeKind::Lerp(_) => {
                let (a, b, t) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?, self.input_scalar(id, 2)?);
                // Three-way zip: build by index since ScalarField only has zip_with(other, f).
                let mut out = ScalarField::zeroed();
                for i in 0..ScalarField::VOLUME {
                    let (av, bv, tv) = (a.data()[i], b.data()[i], t.data()[i]);
                    let (x, y, z) = (i % CHUNK_DIM, (i / CHUNK_DIM) % CHUNK_DIM, i / (CHUNK_DIM * CHUNK_DIM));
                    out.set(x, y, z, av + (bv - av) * tv);
                }
                CachedOutput::Scalar(Arc::new(out))
            }
            NodeKind::Remap(p) => {
                let inp = self.input_scalar(id, 0)?;
                let src_span = (p.src_hi - p.src_lo).abs().max(1e-9);
                let scale = (p.dst_hi - p.dst_lo) / src_span;
                let (src_lo, dst_lo) = (p.src_lo, p.dst_lo);
                CachedOutput::Scalar(Arc::new(inp.map(|v| dst_lo + (v - src_lo) * scale)))
            }
            // --- Curves ---
            NodeKind::Threshold(p) => {
                let inp = self.input_scalar(id, 0)?;
                let t = p.threshold;
                CachedOutput::Scalar(Arc::new(inp.map(|v| if v >= t { 1.0 } else { 0.0 })))
            }
            NodeKind::CurveMapper(p) => {
                let inp = self.input_scalar(id, 0)?;
                let stops = p.stops.clone(); // captured by move into the closure
                CachedOutput::Scalar(Arc::new(inp.map(move |v| sample_curve(&stops, v))))
            }
            // --- Domain ---
            NodeKind::DomainWarp(p) => {
                let pos = self.input_vec3(id, 0)?;
                // Two decorrelated 2D noises for X and Z displacements.
                let amp = p.amplitude;
                let noise_x = self.noise(
                    &NoiseParams { seed: p.seed, frequency: p.frequency,
                        octaves: 3, lacunarity: 2.0, gain: 0.5,
                        fractal_type: FractalType::FBm },
                    NoiseType::OpenSimplex2);
                let noise_z = self.noise(
                    &NoiseParams { seed: p.seed ^ 0xDEAD_BEEF, frequency: p.frequency,
                        octaves: 3, lacunarity: 2.0, gain: 0.5,
                        fractal_type: FractalType::FBm },
                    NoiseType::OpenSimplex2);
                let warped = Vec3Field::from_fn(|x, y, z| {
                    let p0 = pos.get(x, y, z);
                    let dx = noise_x.get_noise_2d(p0.x, p0.z) * amp;
                    let dz = noise_z.get_noise_2d(p0.x, p0.z) * amp;
                    glam::Vec3::new(p0.x + dx, p0.y, p0.z + dz)
                });
                CachedOutput::Vec3(Arc::new(warped))
            }
            // --- Density combinators ---
            NodeKind::Union(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, f32::max)))
            }
            NodeKind::Intersect(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, f32::min)))
            }
            NodeKind::DensitySubtract(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, |x, y| x.max(-y))))
            }
            NodeKind::Mix(_) => {
                let (a, b, f) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?, self.input_scalar(id, 2)?);
                let mut out = ScalarField::zeroed();
                for i in 0..ScalarField::VOLUME {
                    let (av, bv, fv) = (a.data()[i], b.data()[i], f.data()[i].clamp(0.0, 1.0));
                    let (x, y, z) = (i % CHUNK_DIM, (i / CHUNK_DIM) % CHUNK_DIM, i / (CHUNK_DIM * CHUNK_DIM));
                    out.set(x, y, z, av + (bv - av) * fv);
                }
                CachedOutput::Scalar(Arc::new(out))
            }
            NodeKind::Mask(_) => {
                let (signal, mask) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(signal.zip_with(&mask, |s, m| s * m.clamp(0.0, 1.0))))
            }
            // --- Material providers ---
            NodeKind::ConstantMaterial(p) => {
                CachedOutput::Material(Arc::new(ChunkBuffer::uniform(p.material)))
            }
            NodeKind::Layer(p) => {
                let density = self.input_scalar(id, 0)?;
                let bands = p.bands.clone();
                let fill = p.fill;
                let mut out: ChunkBuffer<MaterialId, 32> = ChunkBuffer::uniform(MaterialId::AIR);

                // For each XZ column, find the topmost solid Y, then write
                // materials downward.
                for z in 0..CHUNK_DIM {
                    for x in 0..CHUNK_DIM {
                        // Find surface (topmost solid).
                        let mut surface: Option<usize> = None;
                        for y in (0..CHUNK_DIM).rev() {
                            if density.get(x, y, z) > 0.0 {
                                surface = Some(y);
                                break;
                            }
                        }
                        for y in 0..CHUNK_DIM {
                            let mat = match surface {
                                None => MaterialId::AIR, // empty column
                                Some(sy) if y > sy => MaterialId::AIR, // above surface
                                Some(sy) => {
                                    let depth = sy - y;
                                    let mut accum: u32 = 0;
                                    let mut chosen = fill;
                                    for &(material, thickness) in &bands {
                                        if (depth as u32) < accum + thickness {
                                            chosen = material;
                                            break;
                                        }
                                        accum += thickness;
                                    }
                                    chosen
                                }
                            };
                            out.set(x, y, z, mat);
                        }
                    }
                }
                out.try_collapse();
                CachedOutput::Material(Arc::new(out))
            }
            NodeKind::Queue(_) => {
                let primary = self.input_material(id, 0)?;
                let fallback = self.input_material(id, 1)?;
                let mut out: ChunkBuffer<MaterialId, 32> = ChunkBuffer::uniform(MaterialId::AIR);
                for z in 0..CHUNK_DIM {
                    for y in 0..CHUNK_DIM {
                        for x in 0..CHUNK_DIM {
                            let p = primary.get(x, y, z);
                            let m = if p != MaterialId::AIR { p } else { fallback.get(x, y, z) };
                            out.set(x, y, z, m);
                        }
                    }
                }
                out.try_collapse();
                CachedOutput::Material(Arc::new(out))
            }
            // --- Terrain pipeline ---
            NodeKind::BuildTerrain(_) => {
                let density = self.input_scalar(id, 0)?;
                let material = self.input_material(id, 1)?;
                let mut out: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
                for z in 0..CHUNK_DIM {
                    for y in 0..CHUNK_DIM {
                        for x in 0..CHUNK_DIM {
                            if density.get(x, y, z) > 0.0 {
                                let m = material.get(x, y, z);
                                out.set(x, y, z, Voxel {
                                    shape: ShapeId::Cube,
                                    material: m,
                                    flags: 0,
                                });
                            }
                        }
                    }
                }
                out.try_collapse();
                CachedOutput::Terrain(Arc::new(out))
            }
            // --- Positions / Scanners / Props ---
            NodeKind::JitteredGrid(p) => {
                CachedOutput::Positions(Arc::new(crate::scatter::jittered_grid(&self.ctx, p)))
            }
            NodeKind::PoissonDisk(p) => {
                CachedOutput::Positions(Arc::new(crate::scatter::poisson_disk(&self.ctx, p)))
            }
            NodeKind::FindFlat(p) => {
                let terrain = self.input_terrain(id, 0)?;
                let points = self.input_positions(id, 1)?;
                CachedOutput::Positions(Arc::new(crate::scan::find_flat(&terrain, &points, p)))
            }
            NodeKind::PlaceTree(p) => {
                let terrain = self.input_terrain(id, 0)?;
                let points = self.input_positions(id, 1)?;
                CachedOutput::Terrain(Arc::new(crate::place::place_trees(&terrain, &points, p)))
            }
            NodeKind::PlacePrefab(p) => {
                let terrain = self.input_terrain(id, 0)?;
                let points = self.input_positions(id, 1)?;
                let template = p.template.as_ref()
                    .ok_or(EvalError::UnresolvedPrefab { node: id })?;
                CachedOutput::Terrain(Arc::new(crate::place::place_prefabs(&terrain, &points, template)))
            }
            // --- Terrain terminal ---
            NodeKind::TerrainOutput(_) => {
                CachedOutput::Terrain(self.input_terrain(id, 0)?)
            }
            // --- Terminal ---
            NodeKind::Output(_) => CachedOutput::Scalar(self.input_scalar(id, 0)?),
        })
    }

    /// Pull-based pointwise sample of a scalar/density node at chunk-local
    /// coords. Recursively samples inputs; used to cross-validate the fill
    /// path. Errors on `WorldPos` (a `Vec3` node).
    pub fn sample_density(&self, id: NodeId, x: usize, y: usize, z: usize) -> EvalResult<f32> {
        let node = self.graph.nodes.get(id).ok_or(EvalError::MissingOutput(id))?;
        Ok(match &node.kind {
            NodeKind::Constant(p) => p.value,
            NodeKind::Perlin2D(p)  => { let w = self.ctx.world_pos(x, y, z); self.noise(p, NoiseType::Perlin).get_noise_2d(w.x, w.z) }
            NodeKind::Perlin3D(p)  => { let w = self.ctx.world_pos(x, y, z); self.noise(p, NoiseType::Perlin).get_noise_3d(w.x, w.y, w.z) }
            NodeKind::Simplex2D(p) => { let w = self.ctx.world_pos(x, y, z); self.noise(p, NoiseType::OpenSimplex2).get_noise_2d(w.x, w.z) }
            NodeKind::Simplex3D(p) => { let w = self.ctx.world_pos(x, y, z); self.noise(p, NoiseType::OpenSimplex2).get_noise_3d(w.x, w.y, w.z) }
            NodeKind::WorldAxis(p) => {
                let w = self.ctx.world_pos(x, y, z);
                match p.axis { Axis::X => w.x, Axis::Y => w.y, Axis::Z => w.z }
            }
            NodeKind::Add(_)       => self.sample_input(id, 0, x, y, z)? + self.sample_input(id, 1, x, y, z)?,
            NodeKind::Multiply(_)  => self.sample_input(id, 0, x, y, z)? * self.sample_input(id, 1, x, y, z)?,
            NodeKind::Subtract(_)  => self.sample_input(id, 0, x, y, z)? - self.sample_input(id, 1, x, y, z)?,
            NodeKind::Min(_)       => self.sample_input(id, 0, x, y, z)?.min(self.sample_input(id, 1, x, y, z)?),
            NodeKind::Max(_)       => self.sample_input(id, 0, x, y, z)?.max(self.sample_input(id, 1, x, y, z)?),
            NodeKind::Clamp(p)     => self.sample_input(id, 0, x, y, z)?.clamp(p.min, p.max),
            NodeKind::Lerp(_)      => {
                let (a, b, t) = (self.sample_input(id, 0, x, y, z)?, self.sample_input(id, 1, x, y, z)?, self.sample_input(id, 2, x, y, z)?);
                a + (b - a) * t
            }
            NodeKind::Remap(p)     => {
                let v = self.sample_input(id, 0, x, y, z)?;
                let span = (p.src_hi - p.src_lo).abs().max(1e-9);
                p.dst_lo + (v - p.src_lo) * (p.dst_hi - p.dst_lo) / span
            }
            NodeKind::Threshold(p) => if self.sample_input(id, 0, x, y, z)? >= p.threshold { 1.0 } else { 0.0 },
            NodeKind::CurveMapper(p) => sample_curve(&p.stops, self.sample_input(id, 0, x, y, z)?),
            NodeKind::Union(_)            => self.sample_input(id, 0, x, y, z)?.max(self.sample_input(id, 1, x, y, z)?),
            NodeKind::Intersect(_)        => self.sample_input(id, 0, x, y, z)?.min(self.sample_input(id, 1, x, y, z)?),
            NodeKind::DensitySubtract(_)  => self.sample_input(id, 0, x, y, z)?.max(-self.sample_input(id, 1, x, y, z)?),
            NodeKind::Mix(_) => {
                let (a, b, f) = (self.sample_input(id, 0, x, y, z)?, self.sample_input(id, 1, x, y, z)?, self.sample_input(id, 2, x, y, z)?.clamp(0.0, 1.0));
                a + (b - a) * f
            }
            NodeKind::Mask(_) => self.sample_input(id, 0, x, y, z)? * self.sample_input(id, 1, x, y, z)?.clamp(0.0, 1.0),
            NodeKind::Output(_) => self.sample_input(id, 0, x, y, z)?,
            NodeKind::WorldPos(_) | NodeKind::DomainWarp(_) => {
                return Err(EvalError::WrongInputType { node: id, expected: "scalar", got: "vec3" });
            }
            NodeKind::ConstantMaterial(_)
            | NodeKind::Layer(_)
            | NodeKind::Queue(_)
            | NodeKind::BuildTerrain(_)
            | NodeKind::TerrainOutput(_)
            | NodeKind::JitteredGrid(_)
            | NodeKind::PoissonDisk(_)
            | NodeKind::FindFlat(_)
            | NodeKind::PlaceTree(_)
            | NodeKind::PlacePrefab(_) => {
                return Err(EvalError::WrongInputType {
                    node: id, expected: "scalar", got: "material/terrain/positions",
                });
            }
        })
    }

    fn sample_input(
        &self,
        node: NodeId,
        pin: u16,
        x: usize,
        y: usize,
        z: usize,
    ) -> EvalResult<f32> {
        let edge = self
            .graph
            .edges
            .iter()
            .find(|e| e.to.node == node && e.to.pin == pin)
            .ok_or(EvalError::MissingInput { node, pin })?;
        self.sample_density(edge.from.node, x, y, z)
    }
}

/// Piecewise-linear curve sample. Stops should be sorted ascending by `x`;
/// out-of-range inputs clamp to the endpoint values.
fn sample_curve(stops: &[(f32, f32)], v: f32) -> f32 {
    if stops.is_empty() { return v; }
    if v <= stops[0].0 { return stops[0].1; }
    for w in stops.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if v <= x1 {
            let span = (x1 - x0).abs().max(1e-9);
            let t = ((v - x0) / span).clamp(0.0, 1.0);
            return y0 + (y1 - y0) * t;
        }
    }
    stops.last().unwrap().1
}
