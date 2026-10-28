//! Areas of interest (design §12, "Residency"): what an observer needs
//! resident, as residency reads it.
//!
//! An area is plain data in chunk coordinates. A camera or controller computes
//! it from its own view; residency only tests chunks against it, so it holds no
//! projection and no camera setting.

use glam::IVec3;

/// What one observer needs resident: the chunk columns its footprints cover,
/// over a range of chunk layers.
///
/// Two footprints give residency its hysteresis. A chunk inside `load` loads,
/// and one outside `retain` unloads; one between the two does neither, so an
/// observer moving along the edge of its area does not load and unload the same
/// chunks over and over. `retain` is expected to contain `load`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaOfInterest {
    /// The chunk column the footprints are placed around, as `(x, z)`. Load
    /// priority is horizontal distance from it.
    pub center: (i32, i32),
    /// The chunk layers the area covers, as `(start, end)` with `end` excluded.
    pub chunk_y: (i32, i32),
    /// The columns that load.
    pub load: Footprint,
    /// The columns that stay loaded once they are.
    pub retain: Footprint,
}

impl AreaOfInterest {
    /// Whether the chunk at `pos` is inside the load footprint and the layers.
    pub fn loads(&self, pos: IVec3) -> bool {
        self.covers_layer(pos.y)
            && self.load.contains(pos.x - self.center.0, pos.z - self.center.1)
    }

    /// Whether the chunk at `pos` is inside the retain footprint and the layers.
    pub fn retains(&self, pos: IVec3) -> bool {
        self.covers_layer(pos.y)
            && self.retain.contains(pos.x - self.center.0, pos.z - self.center.1)
    }

    /// Squared horizontal distance, in chunks, from the center column to the
    /// column of `pos`.
    pub fn distance_sq(&self, pos: IVec3) -> i32 {
        let dx = pos.x - self.center.0;
        let dz = pos.z - self.center.1;
        dx * dx + dz * dz
    }

    fn covers_layer(&self, y: i32) -> bool {
        y >= self.chunk_y.0 && y < self.chunk_y.1
    }
}

/// A set of chunk columns, placed around an area's center column. Offsets from
/// the center are in chunks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Footprint {
    /// The columns within `radius` of the center.
    Disc {
        /// Radius in chunks.
        radius: f32,
    },
    /// The columns inside a rectangle turned `yaw` about the vertical. A
    /// column's offset is measured along two horizontal axes, `right` along
    /// `(sin yaw, -cos yaw)` and `up` along `(-cos yaw, -sin yaw)` in `(x, z)`.
    /// The column is inside when its `right` measure falls in `right` and the
    /// stretch it covers along `up`, from its `up` measure plus `span.0` to its
    /// `up` measure plus `span.1`, overlaps `up`.
    Rect {
        /// Rotation about the vertical, in radians.
        yaw: f32,
        /// The range along the right axis, as `(min, max)`.
        right: (f32, f32),
        /// The range along the up axis, as `(min, max)`.
        up: (f32, f32),
        /// How far a column reaches along the up axis from where it stands, as
        /// `(min, max)`. A view that shows raised geometry displaced along that
        /// axis sees a column as a stretch rather than a point; `(0.0, 0.0)`
        /// for a plain rectangle.
        span: (f32, f32),
    },
}

impl Footprint {
    /// Whether the column at offset `(dx, dz)` from the center is inside.
    pub fn contains(&self, dx: i32, dz: i32) -> bool {
        let (dx, dz) = (dx as f32, dz as f32);
        match *self {
            Self::Disc { radius } => dx * dx + dz * dz <= radius * radius,
            Self::Rect { yaw, right, up, span } => {
                let (sin, cos) = (yaw.sin(), yaw.cos());
                let along_right = dx * sin - dz * cos;
                let along_up = -dx * cos - dz * sin;
                right.0 <= along_right
                    && along_right <= right.1
                    && along_up + span.0 <= up.1
                    && along_up + span.1 >= up.0
            }
        }
    }

    /// How far a scan for the footprint's columns has to reach: the smallest
    /// whole number of chunks that holds every column inside on both axes.
    pub fn reach(&self) -> i32 {
        match *self {
            Self::Disc { radius } => radius.ceil() as i32,
            Self::Rect { yaw, right, up, span } => {
                let r = right.0.abs().max(right.1.abs());
                let u = (up.0 - span.1).abs().max((up.1 - span.0).abs());
                let (sin, cos) = (yaw.sin().abs(), yaw.cos().abs());
                (r * sin + u * cos).max(r * cos + u * sin).ceil() as i32
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disc_holds_the_columns_within_its_radius() {
        let disc = Footprint::Disc { radius: 2.0 };
        assert!(disc.contains(0, 0));
        assert!(disc.contains(2, 0));
        assert!(disc.contains(-1, 1));
        assert!(!disc.contains(2, 1));
        assert_eq!(disc.reach(), 2);
    }

    /// At yaw 0 the right axis runs toward -z and the up axis toward -x.
    #[test]
    fn an_unturned_rect_measures_toward_minus_z_and_minus_x() {
        let rect = Footprint::Rect {
            yaw: 0.0,
            right: (0.0, 1.0),
            up: (-2.0, 0.0),
            span: (0.0, 0.0),
        };
        assert!(rect.contains(0, 0));
        assert!(rect.contains(0, -1));
        assert!(rect.contains(2, -1));
        assert!(!rect.contains(0, 1));
        assert!(!rect.contains(-1, 0));
        assert_eq!(rect.reach(), 2);
    }

    /// A column that reaches along the up axis is inside while any of its
    /// stretch overlaps the range, and the scan reaches as far as it can be.
    #[test]
    fn a_span_widens_a_rect_along_its_up_axis() {
        let rect = Footprint::Rect {
            yaw: 0.0,
            right: (-1.0, 1.0),
            up: (-1.0, 1.0),
            span: (-3.0, 0.0),
        };
        assert!(rect.contains(-3, 0), "stands at up 3 and reaches down to 0");
        assert!(!rect.contains(-5, 0), "reaches only down to 2");
        assert!(rect.contains(1, 0), "stands at up -1, the range's edge");
        assert!(!rect.contains(2, 0), "stands below the range");
        assert_eq!(rect.reach(), 4);
    }

    #[test]
    fn an_area_loads_inside_its_layers_and_retains_further_out() {
        let area = AreaOfInterest {
            center: (10, -4),
            chunk_y: (0, 2),
            load: Footprint::Disc { radius: 1.0 },
            retain: Footprint::Disc { radius: 3.0 },
        };
        assert!(area.loads(IVec3::new(11, 1, -4)));
        assert!(!area.loads(IVec3::new(11, 2, -4)), "above the layers");
        assert!(!area.loads(IVec3::new(12, 0, -4)));
        assert!(area.retains(IVec3::new(12, 0, -4)));
        assert!(!area.retains(IVec3::new(14, 0, -4)));
        assert_eq!(area.distance_sq(IVec3::new(12, 5, -2)), 8);
    }
}
