use crate::rendering::frustum::Frustum;
use crate::rendering::render_graph::{PassDecl, RenderPassNode, ResourceId, ResourceMap};
use crate::rendering::water_pass::WaterPass;

/// Dedicated water pass (design §7 rendering). Runs after the opaque scene: copies
/// SCENE into SCENE_COPY so the water shader can sample the terrain behind it
/// (refraction + SSR), then draws welded water meshes with manual depth occlusion.
pub struct WaterScenePassNode<'a> {
    pub pipeline: &'a wgpu::RenderPipeline,
    pub global_bind_group: &'a wgpu::BindGroup,
    pub water_pass: &'a WaterPass,
    pub frustum: &'a Frustum,
    pub hide_water: bool,
    // Copy source/dest textures (views live in the ResourceMap).
    pub scene_tex: &'a wgpu::Texture,
    pub scene_copy_tex: &'a wgpu::Texture,
    pub tex_size: wgpu::Extent3d,
}

impl<'a> RenderPassNode for WaterScenePassNode<'a> {
    fn declaration(&self) -> PassDecl {
        PassDecl {
            name: "water",
            reads: &[ResourceId::SCENE_COPY, ResourceId::DEPTH],
            writes: &[ResourceId::SCENE, ResourceId::NORMAL],
        }
    }
    
    fn record(&self, encoder: &mut wgpu::CommandEncoder, resources: &ResourceMap) {
        if self.hide_water || self.water_pass.chunk_meshes.is_empty() {
            return;
        }
        
        // Snapshot the opaque scene for the shader to sample.
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfoBase {
                texture: self.scene_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: self.scene_copy_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            self.tex_size,
        );
        
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("water_pass"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: resources.get(ResourceId::SCENE),
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: resources.get(ResourceId::NORMAL),
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                }),
            ],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        
        pass.set_pipeline(self.pipeline);
        pass.set_bind_group(0, self.global_bind_group, &[]);
        pass.set_bind_group(1, &self.water_pass.bind_group, &[]);
        for (&chunk_pos, mesh) in &self.water_pass.chunk_meshes {
            if self.frustum.is_chunk_visible(chunk_pos) {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
    }
}
