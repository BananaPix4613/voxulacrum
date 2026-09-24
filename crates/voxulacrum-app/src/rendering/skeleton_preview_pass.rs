//! Solid rendering of the tree skeleton debug preview.
//!
//! The preview is not a chunk, so it cannot ride `CapsulePass` or `CanopyPass` -
//! those are keyed by chunk coordinate and rebuilt from chunk content, which
//! would drop the preview the moment its chunk remeshed. This holds the same two
//! instance kinds in one place with a different lifetime: rebuilt when a panel
//! input moves, cleared when the preview is.
//!
//! It draws through the *same* pipelines the world uses, which is the whole
//! point - a preview rendered by a second code path is a preview of that path.

use bevy_ecs::prelude::Resource;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::{CapsuleInstanceGpu, LobeInstanceGpu};
use crate::rendering::render_context::RenderContext;

#[derive(Resource, Default)]
pub struct SkeletonPreviewPass {
    pub capsule_buffer: Option<wgpu::Buffer>,
    pub capsule_count: u32,
    pub lobe_buffer: Option<wgpu::Buffer>,
    pub lobe_count: u32,
}

impl SkeletonPreviewPass {
    /// Replace both instance streams. Buffers are recreated rather than mapped
    /// and written: this runs only when a panel input changes, and the counts
    /// change with it.
    pub fn set(
        &mut self,
        ctx: &RenderContext,
        capsules: &[CapsuleInstanceGpu],
        lobes: &[LobeInstanceGpu],
    ) {
        self.capsule_count = capsules.len() as u32;
        self.capsule_buffer = (!capsules.is_empty()).then(|| {
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("skeleton_preview_capsules"),
                contents: bytemuck::cast_slice(capsules),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        self.lobe_count = lobes.len() as u32;
        self.lobe_buffer = (!lobes.is_empty()).then(|| {
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("skeleton_preview_lobes"),
                contents: bytemuck::cast_slice(lobes),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
    }

    pub fn clear(&mut self) {
        self.capsule_buffer = None;
        self.capsule_count = 0;
        self.lobe_buffer = None;
        self.lobe_count = 0;
    }
}
