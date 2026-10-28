//! The render world's GPU resources that belong to no single pass: the uniform
//! buffers and bind groups, the shadow depth view, the material color table and
//! the visibility mask, plus the requests the main side sends it.

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;

use bevy_ecs::prelude::Resource;

use crate::rendering::versioned::{Uploaded, Versioned};

// ---------------------------------------------------------------------------
// GPU uniform buffers
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct GlobalUniformBuffer(pub wgpu::Buffer);

impl Deref for GlobalUniformBuffer {
    type Target = wgpu::Buffer;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for GlobalUniformBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

/// GPU storage buffer holding the per-material color table (group 0, binding 5).
/// The terrain fragment shader indexes it by `material_id` to compose color at
/// draw time, replacing the baked per-vertex color. Written once at startup from
/// the material registry; kept as a resource so a reload of `materials.ron` has
/// the buffer to rewrite.
#[derive(Resource)]
pub struct MaterialColorBuffer(pub wgpu::Buffer);

impl Deref for MaterialColorBuffer {
    type Target = wgpu::Buffer;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for MaterialColorBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

#[derive(Resource)]
pub struct ShadowUniformBuffer(pub wgpu::Buffer);

impl Deref for ShadowUniformBuffer {
    type Target = wgpu::Buffer;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for ShadowUniformBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

/// Second global uniform buffer holding the *mirrored* view-projection for the
/// planar water reflection pass. Same `GlobalUniforms` layout as the primary; only
/// `view_proj` differs (reflected across the reference water plane).
#[derive(Resource)]
pub struct ReflectionUniformBuffer(pub wgpu::Buffer);

impl Deref for ReflectionUniformBuffer {
    type Target = wgpu::Buffer;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl DerefMut for ReflectionUniformBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

// ---------------------------------------------------------------------------
// GPU bind groups
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct GlobalUniformBindGroup(pub wgpu::BindGroup);

/// Global bind group for the reflection pass: identical textures to the primary
/// [`GlobalUniformBindGroup`], bound to [`ReflectionUniformBuffer`] so the
/// re-rendered scene uses the mirrored view-projection.
#[derive(Resource)]
pub struct ReflectionUniformBindGroup(pub wgpu::BindGroup);

impl Deref for ReflectionUniformBindGroup {
    type Target = wgpu::BindGroup;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl DerefMut for ReflectionUniformBindGroup {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

impl Deref for GlobalUniformBindGroup {
    type Target = wgpu::BindGroup;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for GlobalUniformBindGroup {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

#[derive(Resource)]
pub struct ShadowBindGroup(pub wgpu::BindGroup);

impl Deref for ShadowBindGroup {
    type Target = wgpu::BindGroup;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for ShadowBindGroup {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

// ---------------------------------------------------------------------------
// GPU texture views
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct ShadowDepthView(pub wgpu::TextureView);

impl Deref for ShadowDepthView {
    type Target = wgpu::TextureView;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for ShadowDepthView {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

/// The 64^3 visibility-mask texture (group 0, binding 6). Rewritten from
/// `VisibilityMaskTexels` each time the player crosses a head cell; the bind
/// group holds a static view of it. One `R8Uint` byte per cell: 0 = capped, 1 = revealed.
#[derive(Resource)]
pub struct VisibilityMask {
    pub texture: wgpu::Texture,
    uploaded: Uploaded,
}

impl VisibilityMask {
    pub fn new(texture: wgpu::Texture) -> Self {
        Self { texture, uploaded: Uploaded::default() }
    }

    /// Copy the texels into the texture when they moved since the last call.
    pub fn upload(&mut self, queue: &wgpu::Queue, source: &Versioned<Vec<u8>>) {
        const N: u32 = 64;
        let Some(bytes) = self.uploaded.changed(source) else { return };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(N),
                rows_per_image: Some(N),
            },
            wgpu::Extent3d { width: N, height: N, depth_or_array_layers: N },
        );
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// What the main side asks of the renderer beyond per-frame data: a new surface
/// size from the window, and the shader files that changed on disk. Handed to
/// the render world at Extract and applied there before the frame draws.
#[derive(Resource, Default)]
pub struct RenderRequests {
    /// The window's latest size, when it changed since the last frame.
    pub surface_size: Option<(u32, u32)>,
    /// Shader files that changed on disk, oldest first.
    pub shader_changes: Vec<PathBuf>,
}
