//! The camera description (design §12, "Render world and extract"; roadmap
//! P20): what core code and rendering know about the camera this frame. A
//! camera module fills it each frame, and core systems read its core fields,
//! never a projection type or a camera's settings.

use glam::{Mat4, Vec3};

use super::frustum::Frustum;

/// One view's camera description.
#[derive(Clone, Copy)]
pub struct CameraDesc {
    /// World space to view space.
    pub view: Mat4,
    /// View space to clip space, mapped to the window.
    #[allow(dead_code)] // read once a second camera exists (V8)
    pub projection: Mat4,
    /// `projection * view`: window pixels to world rays, as picking uses it.
    pub view_proj: Mat4,
    /// What culling keeps chunks inside. Built from what is drawn, which a
    /// camera may refine from `view_proj`, and held still while culling is
    /// frozen.
    pub frustum: Frustum,
    /// The camera's right, up and forward directions in world space, unit
    /// length. Anything that reconstructs a view ray reads these.
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
    /// What the observer behind this camera needs resident.
    #[allow(dead_code)] // read once residency comes from observers (R6)
    pub area_of_interest: AreaOfInterest,
    /// The isometric camera module's own values. Only that module should read
    /// them; the uniform writer still does, which V8 removes.
    pub iso: IsoCameraBlock,
}

/// What an observer needs resident (design §12, "Residency"), in world units.
#[allow(dead_code)] // read once residency comes from observers (R6)
#[derive(Clone, Copy)]
pub struct AreaOfInterest {
    pub center: Vec3,
    pub horizontal_radius: f32,
    pub vertical_range: (f32, f32),
}

/// The isometric camera's block of the description.
#[derive(Clone, Copy)]
pub struct IsoCameraBlock {
    /// The view-projection the scene is drawn with: its projection spans the
    /// allocated texel lattice and is nudged so integer world coordinates land
    /// on texel centers. Equal to `view_proj` with snapping off.
    pub snapped_view_proj: Mat4,
    /// What that snap left over, in window pixels, which the upscale pass
    /// shifts by to keep motion smooth.
    pub snap_offset: [f32; 2],
    /// Yaw about world Y, in radians: the smoothed rotation that is drawn.
    #[allow(dead_code)] // read once the isometric camera is a module (V8)
    pub rotation: f32,
    /// Pitch below the horizon, in radians.
    #[allow(dead_code)] // read once the isometric camera is a module (v8)
    pub pitch: f32,
}

impl Default for CameraDesc {
    /// A placeholder for the frame before the camera first runs.
    fn default() -> Self {
        Self {
            view: Mat4::IDENTITY,
            projection: Mat4::IDENTITY,
            view_proj: Mat4::IDENTITY,
            frustum: Frustum::from_view_projection(Mat4::IDENTITY),
            right: Vec3::X,
            up: Vec3::Y,
            forward: Vec3::NEG_Y,
            area_of_interest: AreaOfInterest {
                center: Vec3::ZERO,
                horizontal_radius: 0.0,
                vertical_range: (0.0, 0.0),
            },
            iso: IsoCameraBlock {
                snapped_view_proj: Mat4::ZERO,
                snap_offset: [0.0; 2],
                rotation: 0.0,
                pitch: 0.0,
            },
        }
    }
}
