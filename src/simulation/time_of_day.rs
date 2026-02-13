use glam::{Mat4, Vec3};

use crate::world::generation::{WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

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
            day_duration_seconds: 300.0,
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.time += dt / self.day_duration_seconds;
        self.time %= 1.0;
    }

    pub fn sun_direction(&self) -> Vec3 {
        let angle = (self.time - 0.25) * 2.0 * std::f32::consts::PI;
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
            (0.87, Vec3::new(0.0, 0.0, 0.0)),
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

    pub fn sun_intensity(&self) -> f32 {
        let dir = self.sun_direction();
        dir.y.max(0.0)
    }

    /// Compute warm tint for post-processing based on time of day.
    /// Returns (tint_color, strength).
    pub fn warm_tint(&self) -> (Vec3, f32) {
        let dawn_strength = Self::bell_curve(self.time, 0.27, 0.04);
        let dusk_strength = Self::bell_curve(self.time, 0.72, 0.05);
        let golden_strength= (dawn_strength + dusk_strength).min(1.0);

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

    /// Compute orthographic light-space view-projection matrix for shadow mapping.
    pub fn light_space_matrix(&self) -> Mat4 {
        let sun_dir = self.sun_direction();

        if sun_dir.y <= 0.01 {
            return Mat4::IDENTITY;
        }

        let center = Vec3::new(WORLD_SIZE_X * 0.5, WORLD_SIZE_Y * 0.5, WORLD_SIZE_Z * 0.5);
        let eye = center + sun_dir * 500.0;

        let up = if sun_dir.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };

        let view = Mat4::look_at_rh(eye, center, up);

        let world_min = Vec3::ZERO;
        let world_max = Vec3::new(WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z);
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

        let pad = 10.0;
        min_x -= pad;
        max_x += pad;
        min_y -= pad;
        max_y += pad;
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