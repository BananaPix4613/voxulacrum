//! The render world and its passes (design §12, "Render world and extract").
//!
//! This crate owns every GPU resource: the passes with their pipelines and
//! shaders, the per-chunk entries, and the render world's schedule
//! (`render_world::build_render_schedule`). It depends on `voxel-core` and the
//! GPU stack and on no world code, so what it draws is only what the app's
//! extract step hands it: `FrameState`, `RenderParams`, the `ChunkUploads`
//! queue, the render requests and egui's output, and the versioned values the
//! passes upload.

pub mod camera_desc;
pub mod debug_lines;
pub mod frustum;
pub mod surface_state;
pub mod pipelines;
pub mod render_targets;
pub mod uniforms;
pub mod terrain_pass;
pub mod upscale_pass;
pub mod detail_paint_pass;
pub mod scatter_pass;
pub mod capsule_pass;
pub mod canopy_pass;
pub mod skeleton_preview_pass;
pub mod water_pass;
pub mod water_scene_pass;
pub mod reflection_pass;
pub mod post_process;
pub mod palette_pass;
pub mod outline_pass;
pub mod cap_pass;
pub mod render_graph;
pub mod shadow_pass;
pub mod main_scene_pass;
pub mod uniform_writer;
pub mod render_context;
pub mod player_pass;
pub mod versioned;
pub mod cloud_textures;
pub mod egui_painter;
pub mod render_world;
pub mod settings;
pub mod render_dims;
pub mod frame_state;
pub mod chunk_meshes;
pub mod uploads;
pub mod resources;
pub mod prepare;
pub mod draw;
