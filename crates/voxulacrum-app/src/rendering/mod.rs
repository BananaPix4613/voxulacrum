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
// `vegetation_pass` retired in Phase 5 4a-2i (replaced by detail_paint_pass).
// File kept on disk, no longer compiled; deletion is a deferred cleanup.
pub mod water_pass;
pub mod post_process;
pub mod palette_pass;
pub mod outline_pass;
pub mod cap_pass;
pub mod render_graph;
pub mod shadow_pass;
pub mod main_scene_pass;
pub mod uniform_writer;
pub mod render_context;
