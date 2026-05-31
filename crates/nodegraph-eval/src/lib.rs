//! # nodegraph-eval
//!
//! Evaluates a [`nodegraph_ir::Graph`] for a single chunk, producing dense
//! [`ScalarField`]s (and [`Vec3Field`]s). The [`Evaluator`] topologically
//! orders the graph, fills each node's output once into a chunk-scoped CSE
//! [`EvalCache`], and can also pull-sample any scalar node pointwise.
//!
//! ## Determinism
//!
//! Output is a pure function of `(graph, EvalContext)`. Noise fields seed from
//! `(world_seed, params.seed)` and read **world** coordinates, so they are
//! globally continuous across chunk borders - chunk coordinates never enter a
//! noise seed (that would create seams). See [`EvalContext::noise_seed`].
//!
//! ## Density storage
//!
//! Density is a dense [`ScalarField`] (`f32`, `N³`), not `ChunkBuffer<f32>`:
//! `ChunkBuffer`'s palette compression requires `Eq + Hash`, which `f32` lacks.

#![warn(missing_docs)]

mod cache;
mod context;
mod error;
mod eval;
mod field;
mod png;

pub use cache::{CachedOutput, EvalCache};
pub use context::EvalContext;
pub use error::{EvalError, EvalResult};
pub use eval::Evaluator;
pub use field::{ScalarField, Vec3Field, CHUNK_DIM};
pub use png::{ascii_heightmap, render_graph_to_png, write_heightmap_png};
