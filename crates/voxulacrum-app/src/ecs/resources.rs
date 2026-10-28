use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use winit::window::Window;

use crate::rendering::versioned::Versioned;

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct WindowHandle(pub Arc<Window>);

impl Deref for WindowHandle {
    type Target = Window;
    fn deref(&self) -> &Self::Target { &self.0 }
}

// No DerefMut - Arc<Window> does not support &mut Window access.

/// The visibility mask's cells, built on the main side by `visibility_mask_system`.
#[derive(Resource, Default)]
pub struct VisibilityMaskTexels(pub Versioned<Vec<u8>>);

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
