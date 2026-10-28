//! The pixel-art lattice a frame is drawn on, sized from the surface, the
//! camera's zoom and the world pixel density.

/// The pixel-art lattice for one frame. `k` fixes the world->texel mapping
/// (integer texels per voxel - the image is always painted at this density);
/// `s` is how big one texel appears on screen (continuous, non-integer fine).
/// Zoom only ever changes `s` (and, in octaves, `k`) - never the lattice phase.
#[derive(Clone, Copy)]
pub struct RenderDims {
    /// Texels per voxel: integer, stepped in octaves.
    pub k: u32,
    /// Native pixels per texel: continuous screen magnification.
    pub s: f32,
    /// Visible content in texels (= surface / s, fractional).
    pub view_w: f32,
    pub view_h: f32,
    /// Texture allocation in texels (bucketed so zoom doesn't reallocate).
    pub alloc_w: u32,
    pub alloc_h: u32,
}

pub fn compute_render_dimensions(
    surface_w: u32,
    surface_h: u32,
    zoom: f32,
    world_pixel_density: f32
) -> RenderDims {
    let base_k = world_pixel_density.round().max(1.0) as u32;
    let mut k = base_k;
    let mut s = surface_h as f32 / (2.0 * zoom * k as f32);
    // Octave ladder: partial minify (s < 1 downsamples the texel image).
    while s < 0.85 && k > 1 {
        k /= 2;
        s *= 2.0;
    }
    let view_w = surface_w as f32 / s;
    let view_h = surface_h as f32 / s;
    // Bucket allocation to 64-texel steps (+2 guard) so continuous zoom only
    // reallocates at bucket boundaries, not every tick.
    let alloc_w = (view_w.ceil() as u32 + 2).div_ceil(64) * 64;
    let alloc_h = (view_h.ceil() as u32 + 2).div_ceil(64) * 64;
    RenderDims { k, s, view_w, view_h, alloc_w, alloc_h }
}
