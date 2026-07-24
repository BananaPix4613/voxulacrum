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
/// draw time, replacing the baked per-vertex color. Re-uploaded each frame from
/// `MaterialParams` so live color edits show without a re-mesh.
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

/// Fixed-timestep clock for the fluid simulation. Accumulates frame time and
/// yields a bounded number of fixed-rate ticks, so the sim runs at a steady rate
/// independent of framerate and a stall can't spiral into a tick storm.
#[derive(Resource)]
pub struct FluidClock {
    accumulator: f32,
    tick_dt: f32,
    max_ticks_per_frame: u32,
}

impl FluidClock {
    /// 10 Hz, at most 4 catch-up ticks per frame.
    pub fn new() -> Self {
        Self { accumulator: 0.0, tick_dt: 0.1, max_ticks_per_frame: 4 }
    }

    /// Add a frame's `dt` and return how many fixed ticks to run this frame.
    pub fn ticks_for(&mut self, dt: f32) -> u32 {
        self.accumulator += dt.min(0.25); // clamp a huge dt (e.g. after a stall)
        let mut n = 0;
        while self.accumulator >= self.tick_dt && n < self.max_ticks_per_frame {
            self.accumulator -= self.tick_dt;
            n += 1;
        }
        if self.accumulator > self.tick_dt {
            self.accumulator = 0.0; // drop backlog beyond the cap to avoid perma-lag
        }
        n
    }
}

/// Wall-clock-driven tick timer for the cloud/climate simulation (Substep
/// 3c). Unlike `FluidClock`'s fixed-rate catch-up scheme, this just tracks
/// real elapsed time between frames - the climate grids tick once per frame,
/// the same rate as rendering.
#[derive(Resource)]
pub struct ClimateClock {
    pub last_tick: std::time::Instant,
    pub elapsed: f32,
}

impl ClimateClock {
    pub fn new() -> Self {
        Self { last_tick: std::time::Instant::now(), elapsed: 0.0 }
    }
}

#[cfg(test)]
mod fluid_clock_tests {
    use super::FluidClock;

    #[test]
    fn accumulates_to_fixed_ticks() {
        let mut c = FluidClock::new(); // 0.1s per tick
        assert_eq!(c.ticks_for(0.05), 0); // 0.05 accumulated
        assert_eq!(c.ticks_for(0.06), 1); // 0.11 -> 1 tick, 0.01 left
        assert_eq!(c.ticks_for(0.30), 2); // dt clamped to 0.25 -> 0.26 -> 2 ticks
    }
}
