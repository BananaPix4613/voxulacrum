use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use winit::window::Window;

use crate::rendering::versioned::{Uploaded, Versioned};

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct WindowHandle(pub Arc<Window>);

impl Deref for WindowHandle {
    type Target = Window;
    fn deref(&self) -> &Self::Target { &self.0 }
}

// No DerefMut — Arc<Window> does not support &mut Window access.

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

/// The visibility mask's cells, built on the main side by `visibility_mask_system`.
#[derive(Resource, Default)]
pub struct VisibilityMaskTexels(pub Versioned<Vec<u8>>);

/// What the main side asks of the renderer beyond per-frame data: a new surface
/// size from the window, and the shader files that changed on disk. Applied in
/// UniformWrite.
#[derive(Resource, Default)]
pub struct RenderRequests {
    /// The window's latest size, when it changed since the last frame.
    pub surface_size: Option<(u32, u32)>,
    /// Shader files that changed on disk, oldest first.
    pub shader_changes: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct ShaderDir(pub PathBuf);

impl Deref for ShaderDir {
    type Target = PathBuf;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for ShaderDir {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

use crate::palette::Palette;

#[derive(Resource, Default)]
pub struct LoadedPalette(pub Option<Palette>);

impl Deref for LoadedPalette {
    type Target = Option<Palette>;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for LoadedPalette {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

// ---------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct VoxelWorld(pub crate::world::World);

impl Deref for VoxelWorld {
    type Target = crate::world::World;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for VoxelWorld {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}
