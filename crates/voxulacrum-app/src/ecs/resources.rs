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

#[derive(Resource)]
pub struct ShadowUniformBuffer(pub wgpu::Buffer);

impl Deref for ShadowUniformBuffer {
    type Target = wgpu::Buffer;
    fn deref(&self) -> &Self::Target { &self.0 }
}

impl DerefMut for ShadowUniformBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

// ---------------------------------------------------------------------------
// GPU bind groups
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct GlobalUniformBindGroup(pub wgpu::BindGroup);

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