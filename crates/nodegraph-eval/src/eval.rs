//! The evaluator: topological whole-chunk fill + pointwise sampling.

use std::sync::Arc;

use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use nodegraph_ir::{Graph, NodeId, NodeKind, Severity};

use crate::cache::{CachedOutput, EvalCache};
use crate::context::EvalContext;
use crate::error::{EvalError, EvalResult};
use crate::field::{ScalarField, Vec3Field, CHUNK_DIM};

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

    /// Configure a fractal Perlin generator from node params + context.
    fn perlin(&self, p: &nodegraph_ir::Perlin2DParams) -> FastNoiseLite {
        let mut noise = FastNoiseLite::with_seed(self.ctx.noise_seed(p.seed));
        noise.set_noise_type(Some(NoiseType::Perlin));
        noise.set_frequency(Some(p.frequency));
        noise.set_fractal_type(Some(FractalType::FBm));
        noise.set_fractal_octaves(Some(p.octaves as i32));
        noise.set_fractal_lacunarity(Some(p.lacunarity));
        noise.set_fractal_gain(Some(p.gain));
        noise
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

    /// Compute one node's output via whole-chunk fill.
    fn fill_node(&self, id: NodeId) -> EvalResult<CachedOutput> {
        let node = self.graph.nodes.get(id).ok_or(EvalError::MissingOutput(id))?;
        match &node.kind {
            NodeKind::Constant(p) => {
                Ok(CachedOutput::Scalar(Arc::new(ScalarField::filled(p.value))))
            }
            NodeKind::Perlin2D(p) => {
                let noise = self.perlin(p);
                let mut field = ScalarField::zeroed();
                // 2D source: compute the X-Z plane once, broadcast across Y.
                for z in 0..CHUNK_DIM {
                    for x in 0..CHUNK_DIM {
                        let w = self.ctx.world_pos(x, 0, z);
                        let v = noise.get_noise_2d(w.x, w.z);
                        for y in 0..CHUNK_DIM {
                            field.set(x, y, z, v);
                        }
                    }
                }
                Ok(CachedOutput::Scalar(Arc::new(field)))
            }
            NodeKind::Add(_) => {
                let a = self.input_scalar(id, 0)?;
                let b = self.input_scalar(id, 1)?;
                Ok(CachedOutput::Scalar(Arc::new(a.zip_with(&b, |x, y| x + y))))
            }
            NodeKind::Threshold(p) => {
                let inp = self.input_scalar(id, 0)?;
                let t = p.threshold;
                Ok(CachedOutput::Scalar(Arc::new(
                    inp.map(|v| if v >= t { 1.0 } else { 0.0 }),
                )))
            }
            NodeKind::WorldPos(_) => {
                let field = Vec3Field::from_fn(|x, y, z| self.ctx.world_pos(x, y, z));
                Ok(CachedOutput::Vec3(Arc::new(field)))
            }
            NodeKind::Output(_) => {
                // Terminal: pass through its single density input for inspection.
                Ok(CachedOutput::Scalar(self.input_scalar(id, 0)?))
            }
        }
    }

    /// Pull-based pointwise sample of a scalar/density node at chunk-local
    /// coords. Recursively samples inputs; used to cross-validate the fill
    /// path. Errors on `WorldPos` (a `Vec3` node).
    pub fn sample_density(&self, id: NodeId, x: usize, y: usize, z: usize) -> EvalResult<f32> {
        let node = self.graph.nodes.get(id).ok_or(EvalError::MissingOutput(id))?;
        let val = match &node.kind {
            NodeKind::Constant(p) => p.value,
            NodeKind::Perlin2D(p) => {
                let w = self.ctx.world_pos(x, y, z);
                self.perlin(p).get_noise_2d(w.x, w.z)
            }
            NodeKind::Add(_) => {
                self.sample_input(id, 0, x, y, z)? + self.sample_input(id, 1, x, y, z)?
            }
            NodeKind::Threshold(p) => {
                let v = self.sample_input(id, 0, x, y, z)?;
                if v >= p.threshold { 1.0 } else { 0.0 }
            }
            NodeKind::Output(_) => self.sample_input(id, 0, x, y, z)?,
            NodeKind::WorldPos(_) => {
                return Err(EvalError::WrongInputType {
                    node: id,
                    expected: "scalar",
                    got: "vec3",
                });
            }
        };
        Ok(val)
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
