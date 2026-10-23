use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use winit::window::Window;

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

/// The 64^3 visibility-mask texture (group 0, binding 6). Rewritten each time the
/// player crosses a head cell by `visibility_mask_upload_system`; the bind group
/// holds a static view of it. One `R8Uint` byte per cell: 0 = capped, 1 = revealed.
#[derive(Resource)]
pub struct VisibilityMask(pub wgpu::Texture);

impl Deref for VisibilityMask {
    type Target = wgpu::Texture;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for VisibilityMask {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
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

/// Chunks whose fluid changed since the frame last rebuilt their water meshes.
/// The fluid tick phase and the editor's pour add to it; `water_rebuild_system`
/// drains it once per frame, however many ticks ran. R4's changed-chunk feed
/// replaces it.
#[derive(Resource, Default)]
pub struct FluidChanges {
    pub chunks: std::collections::HashSet<glam::IVec3>,
}
