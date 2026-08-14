use wgpu::CommandEncoder;
use wgpu::util::RenderEncoder;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::frustum::Frustum;
use crate::rendering::player_pass::PlayerPass;
use crate::rendering::render_graph::{PassDecl, RenderPassNode, ResourceId, ResourceMap};
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::capsule_pass::CapsulePass;
use crate::world::chunk::LoadedChunk;

/// Planar water reflection pass. Re-renders terrain + foliage + scatter + player
/// into the half-res reflection targets with the mirrored view-projection
/// (ReflectionUniformBindGroup), so the water pass can sample a true geometry
/// reflection. There is no render-time water-plane clip: the water shader clips per
/// fragment by reconstructed height (Step 3), which is what lets this single render
/// serve water at any Y.
pub struct ReflectionPassNode<'a> {
    pub color_view: &'a wgpu::TextureView,
    pub normal_view: &'a wgpu::TextureView,
    pub depth_view: &'a wgpu::TextureView,
    pub sky_color: wgpu::Color,
    pub terrain_pipeline: &'a wgpu::RenderPipeline, // cull-None mirror variant
    pub reflection_bind_group: &'a wgpu::BindGroup,
    pub chunks: &'a [&'a LoadedChunk],
    pub frustum: &'a Frustum,
    pub detail_paint_pass: &'a DetailPaintPass,
    pub detail_paint_pipeline: &'a wgpu::RenderPipeline,
    pub scatter_pass: &'a ScatterPass,
    pub scatter_pipeline: &'a wgpu::RenderPipeline,
    pub capsule_pass: &'a CapsulePass,
    pub capsule_pipeline: &'a wgpu::RenderPipeline,
    pub player_pass: &'a PlayerPass,
    pub hide_foliage: bool,
    pub enabled: bool,
}

impl<'a> RenderPassNode for ReflectionPassNode<'a> {
    fn declaration(&self) -> PassDecl {
        // Reflection targets aren't graph resources. It reads the shadow map (bound
        // via the reflection bind group), so it must run after the shadow pass.
        PassDecl { name: "reflection", reads: &[ResourceId::SHADOW_DEPTH], writes: &[] }
    }

    fn record(&self, encoder: &mut CommandEncoder, _resources: &ResourceMap) {
        if !self.enabled {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("reflection_pass"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: self.color_view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(self.sky_color), store: wgpu::StoreOp::Store },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: self.normal_view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: self.depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // Terrain (mirror-culled variant).
        pass.set_pipeline(self.terrain_pipeline);
        pass.set_bind_group(0, self.reflection_bind_group, &[]);
        for &chunk in self.chunks {
            if let Some(mesh) = &chunk.mesh {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }

        // Tier-1 detail paint.
        if !self.hide_foliage && !self.detail_paint_pass.chunk_paint.is_empty() {
            pass.set_pipeline(self.detail_paint_pipeline);
            pass.set_bind_group(0, self.reflection_bind_group, &[]);
            pass.set_vertex_buffer(0, self.detail_paint_pass.blade_vertex_buffer.slice(..));
            pass.set_index_buffer(self.detail_paint_pass.blade_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (&chunk_pos, chunk) in &self.detail_paint_pass.chunk_paint {
                if self.frustum.is_chunk_visible(chunk_pos) {
                    pass.set_bind_group(1, &chunk.bind_group, &[]);
                    pass.draw_indexed(
                        0..self.detail_paint_pass.blade_index_count,
                        0,
                        0..crate::rendering::detail_paint_pass::INSTANCES_PER_CHUNK,
                    );
                }
            }
        }

        // Tier-2/3 scatter.
        if !self.hide_foliage && !self.scatter_pass.chunk_scatter.is_empty() {
            pass.set_pipeline(self.scatter_pipeline);
            pass.set_bind_group(0, self.reflection_bind_group, &[]);
            for (prefab_idx, mesh) in self.scatter_pass.prefab_meshes.iter().enumerate() {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for (&chunk_pos, sc) in &self.scatter_pass.chunk_scatter {
                    if !self.frustum.is_chunk_visible(chunk_pos) {
                        continue;
                    }
                    if let Some(Some(insts)) = sc.by_prefab.get(prefab_idx) {
                        pass.set_vertex_buffer(1, insts.instance_buffer.slice(..));
                        pass.draw_indexed(0..mesh.index_count, 0, 0..insts.instance_count);
                    }
                }
            }
        }

        // Wood impostors. The same pipeline as the main pass - what differs is
        // the bind group, which carries the mirrored view basis, so the solve
        // reconstructs the reflected view without a second shader.
        // 
        // Not gated on `hide_foliage`, matching the main pass: a trunk is a
        // solid occluder rather than decoration.
        if !self.capsule_pass.chunks.is_empty() {
            pass.set_pipeline(self.capsule_pipeline);
            pass.set_bind_group(0, self.reflection_bind_group, &[]);
            for (&chunk_pos, caps) in &self.capsule_pass.chunks {
                if !self.frustum.is_chunk_visible(chunk_pos) {
                    continue;
                }
                pass.set_vertex_buffer(0, caps.instance_buffer.slice(..));
                pass.draw(0..6, 0..caps.instance_count);
            }
        }
        
        // Player avatar (cull-None pipeline, reused as-is).
        if self.player_pass.visible && self.player_pass.body_index_count > 0 {
            pass.set_pipeline(&self.player_pass.pipeline);
            pass.set_bind_group(0, self.reflection_bind_group, &[]);
            pass.set_vertex_buffer(0, self.player_pass.vertex_buffer.slice(..));
            pass.set_index_buffer(self.player_pass.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.player_pass.body_index_count, 0, 0..1);
        }
    }
}
