//! 3D voxel viewer for the preview app. Owns its wgpu resources and an
//! orbit camera; rebuilds its mesh only when `Runtime::field_version`
//! changes.

mod camera;
mod mesh;
mod renderer;
mod shape_table;

pub use camera::OrbitCamera;
pub use renderer::{ChunkRenderState, RenderCallback};
