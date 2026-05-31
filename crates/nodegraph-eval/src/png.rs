//! Heightmap rendering: grayscale PNG (viewable artifact) and ASCII (snapshot).

use std::path::Path;

use crate::error::EvalResult;
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
