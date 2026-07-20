use glam::{Mat4, Vec3};

use crate::params::{LightingParams, TimeControlParams};

pub struct TimeOfDay {
    pub time: f32,
}

impl TimeOfDay {
    pub fn new() -> Self {
        Self { time: 0.30 }
    }

    pub fn update(&mut self, dt: f32, tc: &TimeControlParams) {
        if tc.paused {
            self.time = tc.manual_time;
        } else {
            self.time += dt * tc.speed_multiplier / tc.day_duration_seconds;
            self.time %= 1.0;
        }
    }

    /// Horizontal direction the sun's daily arc travels along. Currently a
    /// fixed diagonal, but pulled out as its own method (rather than inlined
    /// into sun_direction()'s formula) so light_space_matrix can derive a
    /// shadow-frustum axis that's correct for whatever this direction is - if
    /// this ever varies (e.g. seasonal drift), the shadow math stays correct
    /// without needing to change in lockstep.
    fn sun_horizontal_dir() -> Vec3 {
        Vec3::new(1.0, 0.0, -1.0).normalize()
    }

    pub fn sun_direction(&self) -> Vec3 {
        let angle = (self.time - 0.25) * 2.0 * std::f32::consts::PI;
        let elevation = angle.sin();
        let horizontal = angle.cos();
        (Self::sun_horizontal_dir() * horizontal + Vec3::Y * elevation).normalize()
    }

    pub fn sun_color(&self, lighting: &LightingParams) -> Vec3 {
        let keyframes: Vec<(f32, Vec3)> = lighting.keyframes.iter()
            .map(|kf| (kf.time, Vec3::from(kf.sun_color)))
            .collect();
        let raw = Self::interpolate_keyframes(&keyframes, self.time);
        let horizon_fade = (self.sun_direction().y * 4.0).clamp(0.0, 1.0);
        raw * horizon_fade
    }

    pub fn ambient_color(&self, lighting: &LightingParams) -> Vec3 {
        let keyframes: Vec<(f32, Vec3)> = lighting.keyframes.iter()
            .map(|kf| (kf.time, Vec3::from(kf.ambient_color)))
            .collect();
        Self::interpolate_keyframes(&keyframes, self.time)
    }

    #[allow(dead_code)] // lighting accessor
    pub fn sun_intensity(&self) -> f32 {
        self.sun_direction().y.max(0.0)
    }

    pub fn warm_tint(&self) -> (Vec3, f32) {
        let dawn_strength = Self::bell_curve(self.time, 0.27, 0.04);
        let dusk_strength = Self::bell_curve(self.time, 0.72, 0.05);
        let golden_strength = (dawn_strength + dusk_strength).min(1.0);

        let night_strength = if self.time < 0.22 || self.time > 0.82 {
            let night_t = if self.time < 0.22 {
                1.0 - self.time / 0.22
            } else {
                (self.time - 0.82) / 0.18
            };
            night_t.clamp(0.0, 1.0) * 0.3
        } else {
            0.0
        };

        if golden_strength > night_strength {
            (Vec3::new(1.2, 0.9, 0.7), golden_strength * 0.3)
        } else {
            (Vec3::new(0.7, 0.8, 1.2), night_strength)
        }
    }

    fn bell_curve(t: f32, center: f32, width: f32) -> f32 {
        let d = (t - center) / width;
        (-d * d * 0.5).exp()
    }

    pub fn light_space_matrix(&self, camera_target: Vec3, shadow_radius: f32) -> Mat4 {
        let sun_dir = self.sun_direction();
        if sun_dir.y <= 0.01 { return Mat4::IDENTITY; }

        // Angle used to derive the shadow frustum's geometry - elevation is
        // floored below (see MIN_SHADOW_SUN_ELEVATION) but otherwise this
        // stays the true, continuous per-frame angle. sun_color/ambient/
        // diffuse elsewhere keep using the true sun_direction() too, so the
        // visible sun/lighting is unaffected by the floor.
        let angle = (self.time - 0.25) * 2.0 * std::f32::consts::PI;

        // Floor the elevation used to build the shadow frustum (not sun_dir
        // itself). At near-horizontal sun angles, the occluder that should
        // shadow a receiver can sit arbitrarily far away - required shadow
        // reach scales with height_difference / tan(elevation), which diverges
        // as elevation -> 0 - but this frustum only covers a fixed
        // shadow_radius around the camera. At very low true elevation, the
        // ridge or wall that should be blocking a valley floor can fall
        // entirely outside that window and never make it into the shadow map,
        // so the valley reads as lit straight through solid terrain. Flooring
        // the elevation used here keeps the required occluder reach bounded
        // and inside shadow_radius; by the time the true sun is this low,
        // horizon_fade has already cut direct light substantially, so the
        // shadow direction lagging slightly behind the true sun position
        // isn't visually apparent.
        const MIN_SHADOW_SUN_ELEVATION: f32 = 0.2;
        let raw_elevation = angle.sin();
        let horizontal_sign = angle.cos().signum();
        let shadow_elevation = raw_elevation.max(MIN_SHADOW_SUN_ELEVATION);
        let shadow_horizontal = horizontal_sign * (1.0 - shadow_elevation * shadow_elevation).max(0.0).sqrt();
        let shadow_sun_dir = (Self::sun_horizontal_dir() * shadow_horizontal
            + Vec3::Y * shadow_elevation).normalize();

        let eye = camera_target + shadow_sun_dir * 500.0;

        // sun_direction()'s daily arc lies in one fixed vertical plane (spanned
        // by sun_horizontal_dir() and Y - only elevation changes within a
        // single day). plane_right is derived (not hardcoded) as whatever's
        // perpendicular to *that* plane, so it's always exactly 90 degrees
        // from `forward` regardless of elevation - no singularity, even at
        // solar noon when the sun passes overhead - and it stays correct
        // automatically if sun_horizontal_dir() ever changes (e.g. seasonal
        // drift), without the shadow math needing to change in lockstep. A
        // hardcoded axis (e.g. always world X or Z) would only stay exactly
        // perpendicular to the orbit if the sun's azimuth were permanently
        // locked to a matching world axis, which isn't the case here.
        //
        // We want plane_right to end up as the *computed* right axis of the shadow
        // view (locking the shadow map's horizontal texel axis to world space), not
        // just handed to look_at_rh as its `up` hint. Passing it as `up` directly
        // does the opposite: look_at_rh computes right = cross(forward, up), and
        // since plane_right is already perpendicular to forward, that cross product
        // stays in-plane and rotates with elevation instead - the shadow frustum's
        // orientation sweeps through the whole day, which is what was rotating the
        // shadow map's texel grid under the (axis-aligned, so rotation-sensitive)
        // voxel terrain and reading as oscillating/wavy shadow edges.
        //
        // Feeding cross(plane_right, forward) as the `up` hint instead makes
        // look_at_rh's computed right axis come out as plane_right (constant), and
        // leaves only the vertical axis to rotate with elevation.
        let forward = -shadow_sun_dir;
        let plane_right = Self::sun_horizontal_dir().cross(Vec3::Y).normalize();
        let up_hint = plane_right.cross(forward);
        let view = Mat4::look_at_rh(eye, camera_target, up_hint);

        let half = Vec3::splat(shadow_radius);
        let world_min = camera_target - half;
        let world_max = camera_target + half;
        let corners = [
            Vec3::new(world_min.x, world_min.y, world_min.z),
            Vec3::new(world_max.x, world_min.y, world_min.z),
            Vec3::new(world_min.x, world_max.y, world_min.z),
            Vec3::new(world_max.x, world_max.y, world_min.z),
            Vec3::new(world_min.x, world_min.y, world_max.z),
            Vec3::new(world_max.x, world_min.y, world_max.z),
            Vec3::new(world_min.x, world_max.y, world_max.z),
            Vec3::new(world_max.x, world_max.y, world_max.z),
        ];

        let mut min_x = f32::MAX; let mut max_x = f32::MIN;
        let mut min_y = f32::MAX; let mut max_y = f32::MIN;
        let mut min_z = f32::MAX; let mut max_z = f32::MIN;

        for corner in &corners {
            let p = view.transform_point3(*corner);
            min_x = min_x.min(p.x); max_x = max_x.max(p.x);
            min_y = min_y.min(p.y); max_y = max_y.max(p.y);
            min_z = min_z.min(p.z); max_z = max_z.max(p.z);
        }

        let pad = 10.0;
        let proj = Mat4::orthographic_rh(
            min_x - pad, max_x + pad,
            min_y - pad, max_y + pad,
            -max_z - pad, -min_z + pad,
        );
        let vp = proj * view;
        
        // Texel-snap: anchor the world origin to a shadow-map texel so shadow edges
        // don't shimmer as the camera-following light frustum slides.
        let half_size = 0.5 * crate::SHADOW_MAP_SIZE as f32;
        let o = vp.project_point3(Vec3::ZERO);
        let dx = (o.x * half_size).round() / half_size - o.x;
        let dy = (o.y * half_size).round() / half_size - o.y;
        let mut snapped = vp;
        snapped.w_axis.x += dx;
        snapped.w_axis.y += dy;
        snapped
    }

    fn interpolate_keyframes(keyframes: &[(f32, Vec3)], t: f32) -> Vec3 {
        for i in 0..keyframes.len() - 1 {
            let (t0, v0) = keyframes[i];
            let (t1, v1) = keyframes[i + 1];
            if t >= t0 && t <= t1 {
                let f = (t - t0) / (t1 - t0);
                return v0.lerp(v1, f);
            }
        }
        keyframes.last().unwrap().1
    }
}