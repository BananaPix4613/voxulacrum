//! Native library kernels: the byte-identical home of the computations that
//! authored library assets bind to by name.
//!
//! A `.library.json` asset declares a typed boundary plus a `kernel` name; the
//! engine resolves that name to a [`LibraryKernel`] and, where the library's
//! output is consumed, invokes the correspondingly-named kernel function (or,
//! for `BiomeBorderFade`, [`biome_border_fade`](crate::biome_border_fade)).
//! Because a kernel *is* native code - shared with the node that already
//! implemented it, where one exists - a library port carries no numeric drift.

use fastnoise_lite::{FastNoiseLite, FractalType as FnlFractalType, NoiseType};
use glam::Vec3;
use voxel_core::MaterialId;

/// A native library kernel, identified by the `kernel` name of an authored
/// library asset. The computation is the correspondingly-named free function in
/// this module (or [`biome_border_fade`](crate::biome_border_fade) for
/// `BiomeBorderFade`); a consumer matches on the kernel and calls it with the
/// arguments that kernel expects.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum LibraryKernel {
    /// Own-biome fade weight from border distance + fade radius.
    BiomeBorderFade,
    /// 3D cave-carve density: Worley cells unioned with ridged tunnels
    /// ([`standard_cave_noise`]).
    StandardCaveNoise,
    /// Depth-conditional surface material cake ([`surface_layering`]).
    SurfaceLayering,
    /// Exposure-varied surface material - a cap material on top-facing surfaces
    /// ([`exposure_layering`]).
    ExposureLayering,
    /// Bridson Poisson-disk point placement
    /// ([`poisson_placement`](crate::poisson_placement)).
    PoissonPlacement,
}

impl LibraryKernel {
    /// Resolve a kernel from an asset's `kernel` field, or `None` if unknown.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "biome_border_fade" => Some(Self::BiomeBorderFade),
            "standard_cave_noise" => Some(Self::StandardCaveNoise),
            "surface_layering" => Some(Self::SurfaceLayering),
            "exposure_layering" => Some(Self::ExposureLayering),
            "poisson_placement" => Some(Self::PoissonPlacement),
            _ => None,
        }
    }

    /// The stable kernel name (matches the asset's `kernel` field).
    pub fn name(self) -> &'static str {
        match self {
            Self::BiomeBorderFade => "biome_border_fade",
            Self::StandardCaveNoise => "standard_cave_noise",
            Self::SurfaceLayering => "surface_layering",
            Self::ExposureLayering => "exposure_layering",
            Self::PoissonPlacement => "poisson_placement",
        }
    }
}

/// `StandardCaveNoise` kernel: a 3D cave-carve density at `pos` seeded from
/// `seed` - a Worley (cellular) network of rooms unioned with ridged tunnels,
/// per design doc §4. Higher values carve more open space when subtracted from
/// terrain density. Deterministic in `(pos, seed)`. Nothing consumes it yet
/// (caves are a later phase); it ships ready for use.
pub fn standard_cave_noise(pos: Vec3, seed: i32) -> f32 {
    let cells = cave_cellular(seed).get_noise_3d(pos.x, pos.y, pos.z);
    let tunnels = cave_ridged(seed ^ 0x5CA1_E5CA).get_noise_3d(pos.x, pos.y, pos.z);
    cells.max(tunnels)
}

/// 3D Worley/cellular noise for cave rooms.
fn cave_cellular(seed: i32) -> FastNoiseLite {
    let mut n = FastNoiseLite::with_seed(seed);
    n.set_noise_type(Some(NoiseType::Cellular));
    n.set_frequency(Some(0.03));
    n
}

/// 3D ridged fractal noise for cave tunnels.
fn cave_ridged(seed: i32) -> FastNoiseLite {
    let mut n = FastNoiseLite::with_seed(seed);
    n.set_noise_type(Some(NoiseType::Perlin));
    n.set_fractal_type(Some(FnlFractalType::Ridged));
    n.set_frequency(Some(0.02));
    n.set_fractal_octaves(Some(3));
    n
}

/// `SurfaceLayering` kernel: the material `depth` cells below a column's surface,
/// given a top-down `(material, thickness)` `bands` cake and the `fill` material
/// below the last band. Depth `0` is the surface cell. Shared by the `Layer`
/// node and the SurfaceLayering library so the two never drift.
pub fn surface_layering(depth: u32, bands: &[(MaterialId, u32)], fill: MaterialId) -> MaterialId
{
    let mut accum: u32 = 0;
    for &(material, thickness) in bands {
        if depth < accum + thickness {
            return material;
        }
        accum += thickness;
    }
    fill
}

/// `ExposureLayering` kernel: swap a surface's `base` material for a `cap`
/// material where the surface is sufficiently exposed (top-facing) - snow caps,
/// exposed rock, moss lines, etc. `exposure` is `0..=1` (1 = fully top-facing);
/// the swap happens at or above `threshold`. Deterministic; nothing consumes it
/// yet, so it ships ready for use.
pub fn exposure_layering(
    base: MaterialId,
    cap: MaterialId,
    exposure: f32,
    threshold: f32,
) -> MaterialId {
    if exposure >= threshold {
        cap
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_names_round_trip() {
        for k in [
            LibraryKernel::BiomeBorderFade,
            LibraryKernel::StandardCaveNoise,
            LibraryKernel::SurfaceLayering,
            LibraryKernel::ExposureLayering,
            LibraryKernel::PoissonPlacement,
        ] {
            assert_eq!(LibraryKernel::from_name(k.name()), Some(k));
        }
        assert_eq!(LibraryKernel::from_name("nope"), None);
    }

    #[test]
    fn cave_noise_is_deterministic_and_finite() {
        let p = Vec3::new(3.0, 4.0, 5.0);
        assert_eq!(standard_cave_noise(p, 7), standard_cave_noise(p, 7));
        assert!(standard_cave_noise(p, 7).is_finite());
    }

    #[test]
    fn cave_noise_seed_changes_output() {
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert!(standard_cave_noise(p, 1) != standard_cave_noise(p, 2));
    }

    #[test]
    fn surface_layering_walks_the_cake() {
        const GRASS: MaterialId = MaterialId(6);
        const SOIL: MaterialId = MaterialId(3);
        const STONE: MaterialId = MaterialId(1);
        let bands = [(GRASS, 1), (SOIL, 3)];
        assert_eq!(surface_layering(0, &bands, STONE), GRASS); // surface cell
        assert_eq!(surface_layering(1, &bands, STONE), SOIL);  // first soil
        assert_eq!(surface_layering(3, &bands, STONE), SOIL);  // last soil
        assert_eq!(surface_layering(4, &bands, STONE), STONE); // fill below
    }
    
    #[test]
    fn exposure_layering_swaps_above_threshold() {
        const ROCK: MaterialId = MaterialId(2);
        const SNOW: MaterialId = MaterialId(5);
        assert_eq!(exposure_layering(ROCK, SNOW, 0.9, 0.7), SNOW); // exposed -> cap
        assert_eq!(exposure_layering(ROCK, SNOW, 0.7, 0.7), SNOW); // at threshold
        assert_eq!(exposure_layering(ROCK, SNOW, 0.5, 0.7), ROCK); // sheltered -> base
    }
}
