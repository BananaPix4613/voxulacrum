//! Heightmap rendering: grayscale PNG (viewable artifact) and ASCII (snapshot).

use nodegraph_ir::{Graph, NodeKind};
use std::path::Path;

use crate::context::EvalContext;
use crate::error::{EvalError, EvalResult};
use crate::eval::Evaluator;
use crate::field::{ScalarField, CHUNK_DIM};

/// Render a Y-slice of a field to a grayscale PNG, normalizing values across
/// the given `(lo, hi)` range. Pixel `(x, z)` <- field `(x, y, z)`.
pub fn write_heightmap_png(
    field: &ScalarField,
    y: usize,
    range: (f32, f32),
    path: &Path,
) -> EvalResult<()> {
    let (lo, hi) = range;
    let span = (hi - lo).max(1e-6);
    let mut img = image::GrayImage::new(CHUNK_DIM as u32, CHUNK_DIM as u32);
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let n = ((field.get(x, y, z) - lo) / span).clamp(0.0, 1.0);
            img.put_pixel(x as u32, z as u32, image::Luma([(n * 255.0).round() as u8]));
        }
    }
    img.save(path)?;
    Ok(())
}

/// One-shot: validate + evaluate the graph for `ctx`, locate its `Output`
/// node, and dump a Y-slice of its density to a PNG at `path`. Returns
/// `EvalError::InvalidGraph(0)` (count 0) as a sentinel when no `Output`
/// node is present.
pub fn render_graph_to_png(
    graph: &Graph,
    ctx: EvalContext,
    y: usize,
    range: (f32, f32),
    path: &std::path::Path,
) -> EvalResult<()> {
    let out_id = graph
        .nodes
        .iter()
        .find(|(_, n)| matches!(n.kind, NodeKind::Output(_)))
        .map(|(id, _)| id)
        .ok_or(EvalError::InvalidGraph(0))?;
    let mut eval = Evaluator::new(graph, ctx);
    eval.evaluate()?;
    let field = eval
        .cache()
        .get(out_id)
        .and_then(|o| o.as_scalar())
        .ok_or(EvalError::MissingOutput(out_id))?;
    write_heightmap_png(field, y, range, path)
}

/// Render a Y-slice as an ASCII heightmap over a fixed `(lo, hi)` range using a
/// 10-level ramp. Quantization makes the output robust to sub-bucket float
/// noise - this is what the `insta` snapshot asserts on.
pub fn ascii_heightmap(field: &ScalarField, y: usize, range: (f32, f32)) -> String {
    const RAMP: &[u8] = b" .:-=+*#%@";
    let (lo, hi) = range;
    let span = (hi - lo).max(1e-6);
    let mut s = String::with_capacity((CHUNK_DIM + 1) * CHUNK_DIM);
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let n = ((field.get(x, y, z) - lo) / span).clamp(0.0, 1.0);
            let idx = ((n * (RAMP.len() - 1) as f32).round() as usize).min(RAMP.len() - 1);
            s.push(RAMP[idx] as char);
        }
        s.push('\n');
    }
    s
}
