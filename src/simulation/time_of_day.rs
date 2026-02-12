use glam::{Mat4, Vec3};

pub struct TimeOfDay {
    /// 0.0 = midnight, 0.5 = noon
    pub time: f32,
    /// Real seconds per full day cycle
    pub day_duration_seconds: f32,
}

impl TimeOfDay {
    pub fn new() -> Self {
        Self {
            time: 0.30, // Start at morning
            day_duration_seconds: 15.0,
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.time += dt / self.day_duration_seconds;
        self.time %= 1.0;
    }

    pub fn sun_direction(&self) -> Vec3 {
        let angle = (self.time - 0.25) * 2.0 * std::f32::consts::PI;
        // Sun rises from +X+Z (southeast), arcs overhead, sets toward -X-Z (northwest)
        let elevation = angle.sin();
        let horizontal = angle.cos();
        Vec3::new(
            horizontal * 0.7,
            elevation,
            horizontal * -0.7,
        ).normalize()
    }

    pub fn sun_color(&self) -> Vec3 {
        let keyframes: &[(f32, Vec3)] = &[
            (0.00, Vec3::new(0.0, 0.0, 0.0)),
            (0.20, Vec3::new(0.0, 0.0, 0.0)),
            (0.25, Vec3::new(1.0, 0.55, 0.25)),
            (0.30, Vec3::new(1.0, 0.85, 0.65)),
            (0.50, Vec3::new(1.0, 0.98, 0.92)),
            (0.60, Vec3::new(1.0, 0.95, 0.82)),
            (0.68, Vec3::new(1.0, 0.85, 0.6)),
            (0.73, Vec3::new(1.0, 0.6, 0.3)),
            (0.78, Vec3::new(0.6, 0.3, 0.15)),
            (0.83, Vec3::new(0.15, 0.08, 0.05)),
            (0.92, Vec3::new(0.0, 0.0, 0.0)),
            (1.00, Vec3::new(0.0, 0.0, 0.0)),
        ];
        // Multiply by smooth horizon fade to avoid hard cutoff
        let raw = Self::interpolate_keyframes(keyframes, self.time);
        let horizon_fade = (self.sun_direction().y * 4.0).clamp(0.0, 1.0);
        raw * horizon_fade
    }

    pub fn ambient_color(&self) -> Vec3 {
        let keyframes: &[(f32, Vec3)] = &[
            (0.00, Vec3::new(0.05, 0.05, 0.12)),
            (0.20, Vec3::new(0.08, 0.08, 0.15)),
            (0.25, Vec3::new(0.25, 0.2, 0.25)),
            (0.30, Vec3::new(0.35, 0.35, 0.4)),
            (0.50, Vec3::new(0.45, 0.45, 0.5)),
            (0.60, Vec3::new(0.42, 0.42, 0.47)),
            (0.68, Vec3::new(0.38, 0.35, 0.4)),
            (0.73, Vec3::new(0.3, 0.22, 0.28)),
            (0.78, Vec3::new(0.18, 0.14, 0.22)),
            (0.83, Vec3::new(0.1, 0.08, 0.16)),
            (0.87, Vec3::new(0.08, 0.08, 0.15)),
            (1.00, Vec3::new(0.05, 0.05, 0.12)),
        ];
        Self::interpolate_keyframes(keyframes, self.time)
    }

    /// When sun is below horizon (y <= 0), contribution should be zero
    pub fn sun_intensity(&self) -> f32 {
        let dir = self.sun_direction();
        dir.y.max(0.0)
    }

    /// Compute orthographic light-space view-projection matrix for shadow mapping.
    /// Covers the entire world (256x128x256) from the sun's perspective.
    pub fn light_space_matrix(&self) -> Mat4 {
        let sun_dir = self.sun_direction();

        // If sun is below horizon, return identity (no shadows at night)
        if sun_dir.y <= 0.01 {
            return Mat4::IDENTITY;
        }

        // World center
        let center = Vec3::new(128.0, 64.0, 128.0);

        // Eye far from scene along sun direction
        let eye = center + sun_dir * 500.0;

        // Choose up vector that isn't parallel to sun direction
        let up = if sun_dir.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };

        let view = Mat4::look_at_rh(eye, center, up);

        // Transform all 8 world AABB corners into light-view space
        // to compute a tight-fitting ortho frustum
        let world_min = Vec3::ZERO;
        let world_max = Vec3::new(256.0, 128.0, 256.0);
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

        let mut min_x = f32::MAX;
        let mut max_x = f32::MIN;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        let mut min_z = f32::MAX;
        let mut max_z = f32::MIN;

        for corner in &corners {
            let p = view.transform_point3(*corner);
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
            min_z = min_z.min(p.z);
            max_z = max_z.max(p.z);
        }

        // Add some padding to avoid edge clipping
        let pad = 10.0;
        min_x -= pad;
        max_x += pad;
        min_y -= pad;
        max_y += pad;
        // For near/far in RH view space, Z is negative (looking down -Z)
        // min_z is the farthest point, max_z is the nearest
        // orthographic_rh expects positive near/far distances
        let near = -max_z - pad;
        let far = -min_z + pad;

        let proj = Mat4::orthographic_rh(
            min_x, max_x,
            min_y, max_y,
            near, far,
        );

        proj * view
    }

    fn interpolate_keyframes(keyframes: &[(f32, Vec3)], t: f32) -> Vec3 {
        // Find surrounding keyframes and lerp
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