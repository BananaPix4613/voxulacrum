//! The evaluator: topological whole-chunk fill + pointwise sampling.

use std::collections::HashMap;
use std::sync::Arc;

use fastnoise_lite::{FastNoiseLite, FractalType as FnlFractalType, NoiseType};
use glam::Vec3;
use nodegraph_ir::{
    Axis, DomainWarpParams, EdgeIndex, Graph, FractalType, LibraryGraphId, NoiseParams, NodeId,
    NodeKind, Severity,
};
use voxel_core::{ChunkBuffer, MaterialId, ShapeId, Voxel};

use crate::scalar_params::{BiomeParams, WorldParams};
use crate::column_eval::{graph_ref_output_name, UpstreamGraphs};
use crate::library_kernel::{standard_cave_noise, LibraryKernel};
use crate::cache::{CachedOutput, EvalCache};
use crate::context::EvalContext;
use crate::error::{EvalError, EvalResult};
use crate::field::{ScalarField, Vec3Field, CHUNK_DIM};
use crate::scatter::ScatterPoint;

/// Build a configured `FastNoiseLite` from a resolved seed + noise params.
/// Shared by the voxel evaluator and the per-column evaluator so both derive
/// identical fields from the same parameters.
pub(crate) fn configured_noise(
    seed: i32,
    p: &NoiseParams,
    noise_type: NoiseType,
) -> FastNoiseLite {
    let mut n = FastNoiseLite::with_seed(seed);
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

/// Evaluates a graph for one chunk. Holds a chunk-scoped CSE cache.
pub struct Evaluator<'g> {
    graph: &'g Graph,
    /// `(node, pin) -> source` over `graph.edges`, built once. The pointwise
    /// samplers resolve an input per sample, so scanning the edge list here
    /// makes a probe quadratic in graph size.
    edges: EdgeIndex,
    ctx: EvalContext,
    cache: EvalCache,
    /// The active biome's scalar parameters, read by `BiomeParam` nodes. `None`
    /// falls every `BiomeParam` back to its node default.
    biome_params: Option<&'g BiomeParams>,
    /// The world's scalar parameters, read by `WorldParam` nodes. `None` falls
    /// every `WorldParam` back to its node default.
    world_params: Option<&'g WorldParams>,
    /// Native kernels by library id, for `LibraryRef` nodes. `None` makes every
    /// `LibraryRef` an `UnresolvedLibraryRef` error - which is what it was
    /// unconditionally before this existed.
    kernels: Option<&'g HashMap<LibraryGraphId, LibraryKernel>>,
    /// Cross-graph resolution context for `GraphRef` reads. `None` keeps the
    /// behavior this evaluator had before: a `GraphRef` is an
    /// `UnresolvedGraphRef` error.
    upstream: Option<&'g UpstreamGraphs<'g>>,
    /// River networks resolved for this chunk, by node. Resolving costs nine
    /// potential samples per cell, and the pointwise sampler is called once per
    /// voxel of a vertical span, so this must not be recomputed per sample.
    /// Interior mutability because `sample_density` takes `&self`.
    rivers: std::cell::RefCell<Vec<(NodeId, Arc<crate::river::RiverNetwork>)>>,
}

impl<'g> Evaluator<'g> {
    /// The river network for a node, resolved once per evaluator.
    fn river_net(&self, id: NodeId, p: &nodegraph_ir::RiverParams) -> Arc<crate::river::RiverNetwork> {
        if let Some((_, net)) = self.rivers.borrow().iter().find(|(n, _)| *n == id) {
            return Arc::clone(net);
        }
        let net = Arc::new(crate::river::RiverNetwork::resolve(self.ctx, p));
        self.rivers.borrow_mut().push((id, Arc::clone(&net)));
        net
    }

    /// New evaluator over a graph and chunk context.
    pub fn new(graph: &'g Graph, ctx: EvalContext) -> Self {
        Self { graph, edges: EdgeIndex::build(graph), ctx, cache: EvalCache::new(), biome_params: None, world_params: None, kernels: None, upstream: None, rivers: std::cell::RefCell::new(Vec::new()) }
    }

    /// Attach the native kernel bindings, so `LibraryRef` nodes evaluate.
    pub fn with_library_kernels(
        mut self,
        kernels: &'g HashMap<LibraryGraphId, LibraryKernel>,
    ) -> Self {
        self.kernels = Some(kernels);
        self
    }
    
    /// Attach the active biome's parameter sidecar, read by `BiomeParam` nodes.
    pub fn with_biome_params(mut self, params: &'g BiomeParams) -> Self {
        self.biome_params = Some(params);
        self
    }

    /// Attach the world's parameter sidecar, read by `WorldParam` nodes.
    pub fn with_world_params(mut self, params: &'g WorldParams) -> Self {
        self.world_params = Some(params);
        self
    }

    /// Attach a cross-graph resolution context, so `GraphRef` nodes read the
    /// referenced graph's named boundary outputs instead of erroring.
    pub fn with_upstream(mut self, upstream: &'g UpstreamGraphs<'g>) -> Self {
        self.upstream = Some(upstream);
        self
    }

    /// The upstream graph a `GraphRef` node reads, and the boundary-output name
    /// it exposes on output `pin`.
    fn graph_ref_read(
        &self,
        id: NodeId,
        pin: u16,
    ) -> EvalResult<(&'g UpstreamGraphs<'g>, nodegraph_ir::GraphRefTarget, String)> {
        let Some(NodeKind::GraphRef(gr)) = self.graph.nodes.get(id).map(|n| &n.kind) else {
            return Err(EvalError::UnresolvedGraphRef { node: id });
        };
        let up = self.upstream.ok_or(EvalError::UnresolvedGraphRef { node: id })?;
        Ok((up, gr.target, graph_ref_output_name(self.graph, id, pin)?))
    }

    /// Borrow the CSE cache (populated by [`Evaluator::evaluate`]).
    pub fn cache(&self) -> &EvalCache {
        &self.cache
    }
    
    /// Consume the evaluator, returning its populated CSE cache.
    pub fn into_cache(self) -> EvalCache {
        self.cache
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

    /// Build a configured FastNoiseLite for any (params, noise_type) pair,
    /// seeded seam-safely (no chunk coords) via [`EvalContext::noise_seed`].
    fn noise(&self, p: &NoiseParams, noise_type: NoiseType) -> FastNoiseLite {
        configured_noise(self.ctx.noise_seed(p.seed), p, noise_type)
    }

    /// Optional Vec3 override on input pin 0 of a noise source.
    fn position_field(&self, node: NodeId) -> EvalResult<Option<Arc<Vec3Field>>> {
        let Some(from) = self.edges.source(node, 0) else {
            return Ok(None);
        };
        let src = self.cache.get(from.node).ok_or(EvalError::MissingOutput(from.node))?;
        match src {
            CachedOutput::Vec3(f) => Ok(Some(f.clone())),
            other => Err(EvalError::WrongInputType {
                node, expected: "vec3", got: other.kind_name(),
            }),
        }
    }

    /// The two decorrelated noise fields a `DomainWarp` displaces by.
    ///
    /// Shared by the fill and pointwise paths so they cannot drift - the same
    /// discipline the `Layer` node and the `SurfaceLayering` kernel follow. A
    /// warp that displaced differently in the two paths would show up as a seam
    /// only at chunk-Y boundaries, which is the hardest kind to attribute.
    fn warp_noises(&self, p: &DomainWarpParams) -> (FastNoiseLite, FastNoiseLite) {
        let params = |seed: u32| NoiseParams {
            seed,
            frequency: p.frequency,
            octaves: 3,
            lacunarity: 2.0,
            gain: 0.5,
            fractal_type: FractalType::FBm,
        };
        (
            self.noise(&params(p.seed), NoiseType::OpenSimplex2),
            self.noise(&params(p.seed ^ 0xDEAD_BEEF), NoiseType::OpenSimplex2),
        )
    }

    /// The kernel bound to `library`, or an unresolved-reference error.
    fn kernel_for(&self, node: NodeId, library: LibraryGraphId) -> EvalResult<LibraryKernel> {
        self.kernels
            .and_then(|k| k.get(&library).copied())
            .ok_or(EvalError::UnresolvedLibraryRef { node })
    }

    /// The cached output feeding `(node, pin)`. Every typed `input_*` resolver
    /// differs only in which variant it accepts, so they share this.
    fn input(&self, node: NodeId, pin: u16) -> EvalResult<&CachedOutput> {
        let from = self.edges.source(node, pin).ok_or(EvalError::MissingInput { node, pin })?;
        self.cache.get(from.node).ok_or(EvalError::MissingOutput(from.node))
    }

    /// Resolve the scalar field feeding `(node, pin)`. `Scalar → Density`
    /// coercion is a no-op here (both are `f32` fields).
    fn input_scalar(&self, node: NodeId, pin: u16) -> EvalResult<Arc<ScalarField>> {
        match self.input(node, pin)? {
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
        match self.input(node, pin)? {
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
        match self.input(node, pin)? {
            CachedOutput::Material(f) => Ok(f.clone()),
            other => Err(EvalError::WrongInputType {
                node,
                expected: "material",
                got: other.kind_name(),
            }),
        }
    }

    fn input_terrain(&self, node: NodeId, pin: u16) -> EvalResult<Arc<ChunkBuffer<Voxel, 32>>> {
        match self.input(node, pin)? {
            CachedOutput::Terrain(f) => Ok(f.clone()),
            other => Err(EvalError::WrongInputType {
                node, expected: "terrain", got: other.kind_name(),
            }),
        }
    }

    fn input_positions(&self, node: NodeId, pin: u16) -> EvalResult<Arc<Vec<ScatterPoint>>> {
        match self.input(node, pin)? {
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
            NodeKind::YBand(p) => {
                let mut field = ScalarField::zeroed();
                for z in 0..CHUNK_DIM {
                    for y in 0..CHUNK_DIM {
                        for x in 0..CHUNK_DIM {
                            let wy = self.ctx.world_pos(x, y, z).y;
                            let v = if wy >= p.min && wy < p.max { 1.0 } else { 0.0 };
                            field.set(x, y, z, v);
                        }
                    }
                }
                CachedOutput::Scalar(Arc::new(field))
            }
            NodeKind::BiomeParam(p) => {
                let value = self
                    .biome_params
                    .and_then(|bp| bp.get(&p.name))
                    .unwrap_or(p.default);
                CachedOutput::Scalar(Arc::new(ScalarField::filled(value)))
            }
            NodeKind::WorldParam(p) => {
                let value = self
                    .world_params
                    .and_then(|wp| wp.get(&p.name))
                    .unwrap_or(p.default);
                CachedOutput::Scalar(Arc::new(ScalarField::filled(value)))
            }
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
            NodeKind::Abs(_) => {
                let inp = self.input_scalar(id, 0)?;
                CachedOutput::Scalar(Arc::new(inp.map(f32::abs)))
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
                let amp = p.amplitude;
                let (noise_x, noise_z) = self.warp_noises(p);
                let warped = Vec3Field::from_fn(|x, y, z| {
                    let p0 = pos.get(x, y, z);
                    let dx = noise_x.get_noise_2d(p0.x, p0.z) * amp;
                    let dz = noise_z.get_noise_2d(p0.x, p0.z) * amp;
                    Vec3::new(p0.x + dx, p0.y, p0.z + dz)
                });
                CachedOutput::Vec3(Arc::new(warped))
            }
            // --- Density combinators ---
            // Identity at run time: a `SurfaceField` reaching the voxel domain
            // is already cached as a column-broadcast field. The node exists to
            // make the domain change explicit in the graph rather than a
            // coercion, and it is where a distinct column-typed cache entry
            // would convert if one is ever added.
            NodeKind::SurfaceToDensity(_) => {
                CachedOutput::Scalar(self.input_scalar(id, 0)?)
            }
            NodeKind::Union(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, f32::max)))
            }
            NodeKind::Intersect(_) => {
                let (a, b) = (self.input_scalar(id, 0)?, self.input_scalar(id, 1)?);
                CachedOutput::Scalar(Arc::new(a.zip_with(&b, f32::min)))
            }
            NodeKind::River(p) => {
                let terrain = self.input_scalar(id, 0)?;
                let net = self.river_net(id, p);
                let mut out = ScalarField::zeroed();
                for i in 0..ScalarField::VOLUME {
                    let (x, y, z) = (i % CHUNK_DIM, (i / CHUNK_DIM) % CHUNK_DIM, i / (CHUNK_DIM * CHUNK_DIM));
                    let w = self.ctx.world_pos(x, y, z);
                    out.set(x, y, z, net.carve(p, w.x, w.y, w.z, terrain.data()[i]));
                }
                CachedOutput::Scalar(Arc::new(out))
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
                                    crate::library_kernel::surface_layering(
                                        (sy - y) as u32, &bands, fill,
                                    )
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
            NodeKind::PlaceBlueprint(p) => {
                let terrain = self.input_terrain(id, 0)?;
                let points = self.input_positions(id, 1)?;
                let blueprint = p.resolved.as_ref()
                    .ok_or(EvalError::UnresolvedBlueprint { node: id })?;
                CachedOutput::Terrain(Arc::new(crate::place::place_blueprints(&terrain, &points, blueprint)))
            }
            // Declaration-only: it has no output pins, so nothing can ask for a value.
            NodeKind::PlaceStructure(_) => {
                return Err(EvalError::WrongInputType {
                    node: id,
                    expected: "terrain",
                    got: "declaration",
                });
            }
            // --- Terrain terminal ---
            NodeKind::TerrainOutput(_) => {
                CachedOutput::Terrain(self.input_terrain(id, 0)?)
            }
            NodeKind::DensityOutput(_) => CachedOutput::BiomeLayer {
                density: self.input_scalar(id, 0)?,
                material: self.input_material(id, 1)?,
            },
            // --- References (resolved outside the single-graph evaluator) ---
            NodeKind::LibraryRef(p) => match self.kernel_for(id, p.library)? {
                LibraryKernel::StandardCaveNoise => {
                    let pos = self.input_vec3(id, 0)?;
                    // Seeded from the library id, not a node-local seed: the
                    // library's boundary declares no `seed` input, so two
                    // references to the same library are the same field - which
                    // is the correct reading of "the same library".
                    // **Trigger:** the first graph wanting two independently
                    // seeded cave fields forces a `seed` port onto the boundary.
                    let seed = self.ctx.noise_seed(p.library.0);
                    let mut out = ScalarField::zeroed();
                    for z in 0..CHUNK_DIM {
                        for y in 0..CHUNK_DIM {
                            for x in 0..CHUNK_DIM {
                                out.set(x, y, z, standard_cave_noise(pos.get(x, y, z), seed));
                            }
                        }
                    }
                    CachedOutput::Scalar(Arc::new(out))
                }
                // The other kernels produce weights, materials or point sets and
                // are consumed where those are consumed, not in a density chain.
                _ => return Err(EvalError::WrongGraphDomain { node: id }),
            },
            // A `GraphRef` in the voxel domain reads a per-column value - the
            // upstream graphs expose `SurfaceField` boundary outputs - so it
            // broadcasts down each column. Cached as `Scalar` because that is
            // what a 3D field is here; the pin stays `SurfaceField`, so only a
            // node that accepts one can consume it.
            //
            // Output pin 0: the fill path caches one value per node, and a
            // `GraphRef` exposing several outputs needs one entry per pin to do
            // better. **Trigger:** the first biome graph wiring two channels
            // from the same `GraphRef` forces per-pin cache entries.
            NodeKind::GraphRef(_) => {
                let (up, target, name) = self.graph_ref_read(id, 0)?;
                let column = up.surface_field(target, &name, id)?;
                let mut field = ScalarField::zeroed();
                for z in 0..CHUNK_DIM {
                    for x in 0..CHUNK_DIM {
                        let v = column.get(x, z);
                        for y in 0..CHUNK_DIM {
                            field.set(x, y, z, v);
                        }
                    }
                }
                CachedOutput::Scalar(Arc::new(field))
            }
            // FluidOutput terminal: cache its `mask` input as its value so the
            // graph-wide evaluate() succeeds; the world evaluator harvests the
            // pond per column from the mask field + the level input.
            NodeKind::FluidOutput(_) => CachedOutput::Scalar(self.input_scalar(id, 1)?),
            // --- Per-column nodes belong to the World/Zone graphs ---
            NodeKind::SurfaceNoise(_) | NodeKind::WorldOutput(_) | NodeKind::ZoneOutput(_)
            | NodeKind::GraphOutput(_)
            | NodeKind::PoissonDistribution(_) | NodeKind::SurfaceFilter(_)
            | NodeKind::BiomeContextMask(_) | NodeKind::SpeciesPicker(_)
            | NodeKind::PaintDensity(_) | NodeKind::ScatterPlace(_) => {
                return Err(EvalError::WrongGraphDomain { node: id });
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
            NodeKind::BiomeParam(p) => {
                self.biome_params.and_then(|bp| bp.get(&p.name)).unwrap_or(p.default)
            }
            NodeKind::WorldParam(p) => {
                self.world_params.and_then(|wp| wp.get(&p.name)).unwrap_or(p.default)
            }
            NodeKind::Perlin2D(p)  => { let w = self.ctx.world_pos(x, y, z); self.noise(p, NoiseType::Perlin).get_noise_2d(w.x, w.z) }
            NodeKind::Perlin3D(p)  => { let w = self.ctx.world_pos(x, y, z); self.noise(p, NoiseType::Perlin).get_noise_3d(w.x, w.y, w.z) }
            NodeKind::Simplex2D(p) => { let w = self.ctx.world_pos(x, y, z); self.noise(p, NoiseType::OpenSimplex2).get_noise_2d(w.x, w.z) }
            NodeKind::Simplex3D(p) => { let w = self.ctx.world_pos(x, y, z); self.noise(p, NoiseType::OpenSimplex2).get_noise_3d(w.x, w.y, w.z) }
            NodeKind::WorldAxis(p) => {
                let w = self.ctx.world_pos(x, y, z);
                match p.axis { Axis::X => w.x, Axis::Y => w.y, Axis::Z => w.z }
            }
            NodeKind::YBand(p) => {
                let wy = self.ctx.world_pos(x, y, z).y;
                if wy >= p.min && wy < p.max { 1.0 } else { 0.0 }
            }
            NodeKind::Add(_)       => self.sample_input(id, 0, x, y, z)? + self.sample_input(id, 1, x, y, z)?,
            NodeKind::Multiply(_)  => self.sample_input(id, 0, x, y, z)? * self.sample_input(id, 1, x, y, z)?,
            NodeKind::Subtract(_)  => self.sample_input(id, 0, x, y, z)? - self.sample_input(id, 1, x, y, z)?,
            NodeKind::Min(_)       => self.sample_input(id, 0, x, y, z)?.min(self.sample_input(id, 1, x, y, z)?),
            NodeKind::Max(_)       => self.sample_input(id, 0, x, y, z)?.max(self.sample_input(id, 1, x, y, z)?),
            NodeKind::Clamp(p)     => self.sample_input(id, 0, x, y, z)?.clamp(p.min, p.max),
            NodeKind::Abs(_)       => self.sample_input(id, 0, x, y, z)?.abs(),
            NodeKind::SurfaceToDensity(_) => self.sample_input(id, 0, x, y, z)?,
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
            NodeKind::River(p) => {
                let d = self.sample_input(id, 0, x, y, z)?;
                let w = self.ctx.world_pos(x, y, z);
                self.river_net(id, p).carve(p, w.x, w.y, w.z, d)
            }
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
            NodeKind::FluidOutput(_)
            | NodeKind::SurfaceNoise(_) | NodeKind::WorldOutput(_) | NodeKind::ZoneOutput(_)
            | NodeKind::GraphOutput(_)
            | NodeKind::PoissonDistribution(_) | NodeKind::SurfaceFilter(_)
            | NodeKind::BiomeContextMask(_) | NodeKind::SpeciesPicker(_)
            | NodeKind::PaintDensity(_) | NodeKind::ScatterPlace(_) => {
                return Err(EvalError::WrongGraphDomain { node: id });
            }
            NodeKind::ConstantMaterial(_)
            | NodeKind::Layer(_)
            | NodeKind::Queue(_)
            | NodeKind::BuildTerrain(_)
            | NodeKind::TerrainOutput(_)
            | NodeKind::DensityOutput(_)
            | NodeKind::JitteredGrid(_)
            | NodeKind::PoissonDisk(_)
            | NodeKind::FindFlat(_)
            | NodeKind::PlaceTree(_)
            | NodeKind::PlaceBlueprint(_)
            | NodeKind::PlaceStructure(_) => {
                return Err(EvalError::WrongInputType {
                    node: id, expected: "scalar", got: "material/terrain/positions",
                });
            }
            NodeKind::LibraryRef(p) => match self.kernel_for(id, p.library)? {
                LibraryKernel::StandardCaveNoise => {
                    let pos = self.sample_vec3(id, 0, x, y, z)?;
                    standard_cave_noise(pos, self.ctx.noise_seed(p.library.0))
                }
                _ => return Err(EvalError::WrongGraphDomain { node: id }),
            },
            // Chunk-independent by construction: the upstream resamples at the
            // world column, which is what lets two chunks agree about a column
            // on their shared border.
            NodeKind::GraphRef(_) => {
                let (up, target, name) = self.graph_ref_read(id, 0)?;
                let dim = CHUNK_DIM as i32;
                let wx = self.ctx.chunk.x * dim + x as i32;
                let wz = self.ctx.chunk.z * dim + z as i32;
                up.surface_sample(target, &name, wx, wz, id)?
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
        let from = self.edges.source(node, pin).ok_or(EvalError::MissingInput { node, pin })?;
        self.sample_density(from.node, x, y, z)
    }

    /// Pointwise sample of a `Vec3`-producing chain feeding `(node, pin)`.
    ///
    /// The pointwise counterpart of `input_vec3`, and the reason the chunk-Y
    /// seam continuation can follow a density chain through a library: that
    /// continuation samples *above* the chunk window, where no filled field
    /// exists.
    fn sample_vec3(&self, node: NodeId, pin: u16, x: usize, y: usize, z: usize) -> EvalResult<Vec3> {
        let from = self.edges.source(node, pin).ok_or(EvalError::MissingInput { node, pin })?;
        self.sample_position(from.node, x, y, z)
    }

    fn sample_position(&self, id: NodeId, x: usize, y: usize, z: usize) -> EvalResult<Vec3> {
        let node = self.graph.nodes.get(id).ok_or(EvalError::MissingOutput(id))?;
        Ok(match &node.kind {
            NodeKind::WorldPos(_) => self.ctx.world_pos(x, y, z),
            NodeKind::DomainWarp(p) => {
                let p0 = self.sample_vec3(id, 0, x, y, z)?;
                let (noise_x, noise_z) = self.warp_noises(p);
                Vec3::new(
                    p0.x + noise_x.get_noise_2d(p0.x, p0.z) * p.amplitude,
                    p0.y,
                    p0.z + noise_z.get_noise_2d(p0.x, p0.z) * p.amplitude,
                )
            }
            _ => {
                return Err(EvalError::WrongInputType {
                    node: id,
                    expected: "vec3",
                    got: "scalar",
                })
            }
        })
    }
}

/// Piecewise-linear curve sample. Stops should be sorted ascending by `x`;
/// out-of-range inputs clamp to the endpoint values.
pub(crate) fn sample_curve(stops: &[(f32, f32)], v: f32) -> f32 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use nodegraph_ir::BiomeParamParams;
    
    fn scalar0(out: Option<&CachedOutput>) -> f32 {
        match out {
            Some(CachedOutput::Scalar(f)) => f.get(0, 0, 0),
            _ => panic!("expected scalar output"),
        }
    }
    
    #[test]
    fn biome_param_reads_sidecar_with_default_fallback() {
        let mut g = Graph::new();
        let bp = g.add_node(NodeKind::BiomeParam(BiomeParamParams {
            name: "scale".into(),
            default: 2.0,
        }));
        let ctx = EvalContext::new(0, IVec3::ZERO);
        
        // No sidecar -> node default.
        let mut e = Evaluator::new(&g, ctx);
        e.evaluate().unwrap();
        assert_eq!(scalar0(e.cache().get(bp)), 2.0, "no sidecar -> node default");
        
        // Sidecar with the param -> that value.
        let mut params = BiomeParams::new();
        params.set("scale", 5.5);
        let mut e2 = Evaluator::new(&g, ctx).with_biome_params(&params);
        e2.evaluate().unwrap();
        assert_eq!(scalar0(e2.cache().get(bp)), 5.5, "sidecar value");
        
        // Sidecar missing the param -> node default.
        let mut other = BiomeParams::new();
        other.set("other", 9.0);
        let mut e3 = Evaluator::new(&g, ctx).with_biome_params(&other);
        e3.evaluate().unwrap();
        assert_eq!(scalar0(e3.cache().get(bp)), 2.0, "missing param -> node default");
    }

    #[test]
    fn density_output_caches_biome_layer() {
        use nodegraph_ir::{ConstantMaterialParams, ConstantParams, DensityOutputParams, PinRef};
        let mut g = Graph::new();
        let d = g.add_node(NodeKind::Constant(ConstantParams { value: 0.75 }));
        let m = g.add_node(NodeKind::ConstantMaterial(ConstantMaterialParams::default()));
        let out = g.add_node(NodeKind::DensityOutput(DensityOutputParams::default()));
        g.connect(PinRef::new(d, 0), PinRef::new(out, 0)).unwrap(); // Scalar -> Density coercion
        g.connect(PinRef::new(m, 0), PinRef::new(out, 1)).unwrap();

        let mut e = Evaluator::new(&g, EvalContext::new(0, IVec3::ZERO));
        e.evaluate().unwrap();
        let (density, material) = e.cache().get(out).unwrap().as_biome_layer().expect("biome layer");
        assert_eq!(density.get(0, 0, 0), 0.75);
        assert_eq!(material.get(0, 0, 0), ConstantMaterialParams::default().material);
    }

    /// World: `SurfaceNoise -> GraphOutput("climate")`, evaluated for `ctx`.
    fn climate_upstream(ctx: EvalContext) -> (Graph, crate::ColumnCache) {
        use nodegraph_ir::{GraphKind, GraphOutputParams, PinRef};
        let mut w = Graph::of_kind(GraphKind::World);
        let n = w.add_node(NodeKind::SurfaceNoise(NoiseParams::default()));
        let o = w.add_node(NodeKind::GraphOutput(GraphOutputParams { name: "climate".into() }));
        w.connect(PinRef::new(n, 0), PinRef::new(o, 0)).unwrap();
        w.derive_output_boundary();
        let mut we = crate::ColumnEvaluator::new(&w, ctx);
        we.evaluate().unwrap();
        let cache = we.into_cache();
        (w, cache)
    }

    /// A biome graph that is nothing but a `GraphRef(World)`.
    fn graph_ref_biome(world: &Graph) -> (Graph, NodeId) {
        use nodegraph_ir::{GraphRefParams, GraphRefTarget};
        let mut b = Graph::new();
        let gr = b.add_node(NodeKind::GraphRef(GraphRefParams {
            target: GraphRefTarget::World,
            ..Default::default()
        }));
        let wb = world.boundary.clone();
        b.resolve_graph_refs(|t| (t == GraphRefTarget::World).then(|| wb.clone()));
        (b, gr)
    }

    #[test]
    fn graph_ref_fills_a_column_broadcast_and_samples_the_same_values() {
        use nodegraph_ir::GraphRefTarget;
        let ctx = EvalContext::new(9, IVec3::new(2, -1, -3));
        let (world, world_cache) = climate_upstream(ctx);
        let (biome, gr) = graph_ref_biome(&world);

        let mut up = crate::UpstreamGraphs::new();
        up.insert(GraphRefTarget::World, &world, ctx, &world_cache, None);

        let mut e = Evaluator::new(&biome, ctx).with_upstream(&up);
        e.evaluate().unwrap();
        let filled = match e.cache().get(gr) {
            Some(CachedOutput::Scalar(f)) => f.clone(),
            _ => panic!("GraphRef must fill as a scalar field"),
        };

        // The upstream value is per column, so it must be constant down Y, and
        // the pointwise path must agree with the fill everywhere.
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let at0 = filled.get(x, 0, z);
                for y in 0..CHUNK_DIM {
                    assert_eq!(filled.get(x, y, z), at0, "column ({x}, {z}) varies in y");
                    assert_eq!(
                        e.sample_density(gr, x, y, z).unwrap(),
                        at0,
                        "pointwise disagreed with the fill at ({x}, {y}, {z})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_same_world_column_reads_the_same_at_every_chunk_y() {
        use nodegraph_ir::GraphRefTarget;
        // A biome graph is evaluated once per vertical chunk, and the same world
        // column appears in all of them. A cross-graph read that varied with
        // chunk Y would show up as a horizontal seam at every chunk boundary -
        // the hardest kind to attribute, and one this stack has produced before.
        let sample_at = |chunk_y: i32| {
            let ctx = EvalContext::new(9, IVec3::new(2, chunk_y, -3));
            let (world, world_cache) = climate_upstream(ctx);
            let (biome, gr) = graph_ref_biome(&world);
            let mut up = crate::UpstreamGraphs::new();
            up.insert(GraphRefTarget::World, &world, ctx, &world_cache, None);
            let mut e = Evaluator::new(&biome, ctx).with_upstream(&up);
            e.evaluate().unwrap();
            let filled = match e.cache().get(gr) {
                Some(CachedOutput::Scalar(f)) => f.get(5, 0, 6),
                _ => panic!("expected scalar"),
            };
            (filled, e.sample_density(gr, 5, 3, 6).unwrap())
        };
        let base = sample_at(0);
        for chunk_y in [-2, -1, 1, 4] {
            assert_eq!(sample_at(chunk_y), base, "chunk y {chunk_y} disagreed");
        }
    }

    #[test]
    fn graph_ref_without_an_upstream_is_unresolved() {
        let ctx = EvalContext::new(9, IVec3::ZERO);
        let (world, _) = climate_upstream(ctx);
        let (biome, gr) = graph_ref_biome(&world);

        let mut e = Evaluator::new(&biome, ctx);
        assert!(
            matches!(e.evaluate(), Err(EvalError::UnresolvedGraphRef { .. })),
            "no upstream must stay an error rather than silently reading zero"
        );
        assert!(matches!(
            Evaluator::new(&biome, ctx).sample_density(gr, 0, 0, 0),
            Err(EvalError::UnresolvedGraphRef { .. })
        ));
    }

    #[test]
    fn a_climate_channel_drives_a_density_chain_through_surface_to_density() {
        use nodegraph_ir::{
            AbsParams, GraphRefTarget, OutputParams, PinRef, SurfaceToDensityParams,
        };
        // The end-to-end shape B2 and B3 exist for: World climate -> GraphRef ->
        // SurfaceToDensity -> Abs -> Output, all inside a biome graph.
        let ctx = EvalContext::new(9, IVec3::new(-4, 2, 5));
        let (world, world_cache) = climate_upstream(ctx);
        let (mut biome, gr) = graph_ref_biome(&world);
        let s2d = biome.add_node(NodeKind::SurfaceToDensity(SurfaceToDensityParams::default()));
        let abs = biome.add_node(NodeKind::Abs(AbsParams::default()));
        let out = biome.add_node(NodeKind::Output(OutputParams::default()));
        biome.connect(PinRef::new(gr, 0), PinRef::new(s2d, 0)).unwrap();
        biome.connect(PinRef::new(s2d, 0), PinRef::new(abs, 0)).unwrap();
        biome.connect(PinRef::new(abs, 0), PinRef::new(out, 0)).unwrap();
        assert!(!biome.has_errors(), "the chain must type-check without coercion");

        let mut up = crate::UpstreamGraphs::new();
        up.insert(GraphRefTarget::World, &world, ctx, &world_cache, None);
        let mut e = Evaluator::new(&biome, ctx).with_upstream(&up);
        e.evaluate().unwrap();

        let climate = match e.cache().get(gr) {
            Some(CachedOutput::Scalar(f)) => f.clone(),
            _ => panic!("expected scalar"),
        };
        let density = match e.cache().get(out) {
            Some(CachedOutput::Scalar(f)) => f.clone(),
            _ => panic!("expected scalar"),
        };
        let mut nonzero = 0;
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let expect = climate.get(x, 0, z).abs();
                assert_eq!(density.get(x, 7, z), expect, "fill at ({x}, {z})");
                assert_eq!(
                    e.sample_density(out, x, 7, z).unwrap(),
                    expect,
                    "pointwise at ({x}, {z})"
                );
                if expect != 0.0 {
                    nonzero += 1;
                }
            }
        }
        // Guard against the whole assertion passing on an all-zero field.
        assert!(nonzero > 0, "climate was uniformly zero; the test proved nothing");
    }

    #[test]
    fn surface_field_still_does_not_coerce_into_a_density_pin() {
        use nodegraph_ir::{OutputParams, PinRef};
        // The bridge must be the node, not a silent conversion - `Output` takes
        // a density and must refuse a `SurfaceField` outright.
        let ctx = EvalContext::new(9, IVec3::ZERO);
        let (world, _) = climate_upstream(ctx);
        let (mut biome, gr) = graph_ref_biome(&world);
        let out = biome.add_node(NodeKind::Output(OutputParams::default()));
        assert!(
            biome.connect(PinRef::new(gr, 0), PinRef::new(out, 0)).is_err(),
            "SurfaceField -> Density must not connect without SurfaceToDensity"
        );
    }
}
