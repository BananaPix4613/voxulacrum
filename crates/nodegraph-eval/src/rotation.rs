//! Rotations for Tier-1 code, in scalar arithmetic.
//! 
//! glam's `Quat` is SIMD-backed, and its SSE2 and NEON backends sum a dot
//! product in different orders, so its `normalize` and `slerp` can differ in the
//! last bit between x86_64 and aarch64. Generation must give the same bits on
//! every machine (engine-design.md §12, "Determinism"), so it rotates with this
//! instead: a unit quaternion with every operation written out in a fixed order
//! and its trigonometry from `libm`. This crate's `clippy.toml` bans `Quat`.

use std::ops::Mul;

use glam::Vec3;

/// A rotation, as a unit quaternion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rotation {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

impl Rotation {
    /// `angle` radians about `axis`, which must be unit length.
    pub(crate) fn about(axis: Vec3, angle: f32) -> Self {
        let (s, c) = libm::sincosf(angle * 0.5);
        Self { x: axis.x * s, y: axis.y * s, z: axis.z * s, w: c }
    }
    
    /// Rescale to unit length, undoing the drift a chain of products builds up.
    pub(crate) fn normalize(self) -> Self {
        let len = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        let inv = 1.0 / len;
        Self { x: self.x * inv, y: self.y * inv, z: self.z * inv, w: self.w * inv }
    }
}

/// `a * b` applies `b` first, then `a`, as glam's `Quat` does.
impl Mul for Rotation {
    type Output = Self;
    
    fn mul(self, r: Self) -> Self {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        Self {
            x: w * r.x + x * r.w + y * r.z - z * r.y,
            y: w * r.y - x * r.z + y * r.w + z * r.x,
            z: w * r.z + x * r.y - y * r.x + z * r.w,
            w: w * r.w - x * r.x - y * r.y - z * r.z,
        }
    }
}

impl Mul<Vec3> for Rotation {
    type Output = Vec3;
    
    fn mul(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let t = u.cross(v) * 2.0;
        v + t * self.w + u.cross(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUARTER: f32 = std::f32::consts::FRAC_PI_2;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-6
    }

    #[test]
    fn a_quarter_turn_about_y_takes_x_to_negative_z() {
        // Right-handed, matching glam, so the skeleton's frames mean what they
        // meant before.
        let r = Rotation::about(Vec3::Y, QUARTER);
        assert!(close(r * Vec3::X, Vec3::NEG_Z), "{}", r * Vec3::X);
        assert!(close(r * Vec3::Y, Vec3::Y), "the axis must not move");
    }

    #[test]
    fn a_product_applies_its_right_operand_first() {
        let a = Rotation::about(Vec3::Y, QUARTER);
        let b = Rotation::about(Vec3::X, QUARTER);
        let v = Vec3::new(0.3, -1.2, 2.0);
        assert!(close((a * b) * v, a * (b * v)));
        assert!(!close((a * b) * v, (b * a) * v), "vacuous: the two orders agree");
    }

    #[test]
    fn normalize_restores_unit_length_and_keeps_direction() {
        let n = Rotation { x: 0.0, y: 3.0, z: 0.0, w: 4.0 }.normalize();
        assert!((n.y - 0.6).abs() < 1e-6 && (n.w - 0.8).abs() < 1e-6, "{n:?}");
        assert!(n.x == 0.0 && n.z == 0.0, "{n:?}");
    }
}
