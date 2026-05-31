//! The five Phase 5 colormaps. Each maps `t ∈ [0, 1]` to opaque RGBA.

/// Selectable colormap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colormap {
    /// Linear blue → red.
    BlueRed,
    /// Linear black → white.
    Grayscale,
    /// Geological ramp (water → sand → grass → rock → snow).
    Terrain,
    /// Approximation of the matplotlib "viridis" ramp (11-stop).
    Viridis,
    /// Linear red → black.
    RedBlack,
}

impl Colormap {
    /// All variants, in UI display order.
    pub const ALL: [Self; 5] = [
        Self::BlueRed,
        Self::Grayscale,
        Self::Terrain,
        Self::Viridis,
        Self::RedBlack,
    ];

    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            Self::BlueRed => "Blue-Red",
            Self::Grayscale => "Grayscale",
            Self::Terrain => "Terrain",
            Self::Viridis => "Viridis",
            Self::RedBlack => "Red-Black",
        }
    }

    /// Map a normalized value to opaque RGBA.
    pub fn apply(self, t: f32) -> [u8; 4] {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::BlueRed => lerp([60, 80, 200], [220, 60, 60], t),
            Self::Grayscale => {
                let v = (t * 255.0).round() as u8;
                [v, v, v, 255]
            }
            Self::Terrain => ramp(TERRAIN_STOPS, t),
            Self::Viridis => ramp(VIRIDIS_STOPS, t),
            Self::RedBlack => lerp([220, 40, 40], [0, 0, 0], t),
        }
    }
}

fn lerp(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 4] {
    let m = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]), 255]
}

/// Lookup into a sorted `[(t, rgb)]` stop table with linear interpolation.
fn ramp(stops: &[(f32, [u8; 3])], t: f32) -> [u8; 4] {
    debug_assert!(stops.len() >= 2);
    if t <= stops[0].0 {
        let c = stops[0].1;
        return [c[0], c[1], c[2], 255];
    }
    for w in stops.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let u = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
            return lerp(c0, c1, u);
        }
    }
    let last = stops.last().unwrap().1;
    [last[0], last[1], last[2], 255]
}

/// Six-stop geological ramp: deep water → shallow water → sand → grass → rock → snow.
const TERRAIN_STOPS: &[(f32, [u8; 3])] = &[
    (0.00, [30, 50, 110]),
    (0.30, [60, 110, 180]),
    (0.40, [220, 200, 140]),
    (0.55, [80, 140, 60]),
    (0.80, [120, 95, 70]),
    (1.00, [240, 240, 245]),
];

/// 11-stop hand-picked viridis approximation (matplotlib-like purple→green→yellow).
const VIRIDIS_STOPS: &[(f32, [u8; 3])] = &[
    (0.00, [68, 1, 84]),
    (0.10, [72, 36, 117]),
    (0.20, [68, 65, 141]),
    (0.30, [57, 91, 157]),
    (0.40, [44, 114, 166]),
    (0.50, [33, 145, 168]),
    (0.60, [40, 170, 158]),
    (0.70, [87, 192, 130]),
    (0.80, [164, 207, 80]),
    (0.90, [225, 220, 50]),
    (1.00, [253, 231, 37]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_match_stops() {
        for cmap in Colormap::ALL {
            let lo = cmap.apply(0.0);
            let hi = cmap.apply(1.0);
            assert_eq!(lo[3], 255);
            assert_eq!(hi[3], 255);
            // Endpoints differ for every colormap (sanity).
            assert_ne!(&lo[..3], &hi[..3], "{:?} endpoints collapse", cmap);
        }
    }

    #[test]
    fn out_of_range_clamps() {
        for cmap in Colormap::ALL {
            assert_eq!(cmap.apply(-5.0), cmap.apply(0.0));
            assert_eq!(cmap.apply(5.0), cmap.aply(1.0));
        }
    }

    #[test]
    fn midpoint_between_endpoints_for_two_stop_ramps() {
        // BlueRed is a pure 2-stop ramp; t=0.5 must lie strictly between the endpoints.
        let lo = Colormap::BlueRed.apply(0.0);
        let mid = Colormap::BlueRed.apply(0.5);
        let hi = Colormap::BlueRed.apply(1.0);
        for c in 0..3 {
            let (a, b) = (lo[c].min(hi[c]), lo[c].max(hi[c]));
            assert!(mid[c] >= a && mid[c] <= b);
        }
    }
}
