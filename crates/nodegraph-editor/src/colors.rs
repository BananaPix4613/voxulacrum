use egui::Color32;
use egui_snarl::ui::PinShape;
use nodegraph_ir::{NodeCategory, PinType};

/// Distinct color per pin type. Used for both pin markers and connection wires.
pub fn pin_color(ty: PinType) -> Color32 {
    match ty {
        PinType::Scalar         => Color32::from_rgb(180, 220, 255),
        PinType::Density        => Color32::from_rgb(120, 200, 120),
        PinType::SurfaceField   => Color32::from_rgb(110, 205, 200),
        PinType::Material       => Color32::from_rgb(220,  90,  90),
        PinType::FluidProvider  => Color32::from_rgb( 80, 160, 235),
        PinType::Positions      => Color32::from_rgb(230, 200,  70),
        PinType::Assignments    => Color32::from_rgb(230, 160,  70),
        PinType::Curve          => Color32::from_rgb(255, 200, 120),
        PinType::Vec3           => Color32::from_rgb(255, 140, 200),
        PinType::BiomeId        => Color32::from_rgb(180, 130, 255),
        PinType::ZoneId         => Color32::from_rgb(150, 110, 215),
        PinType::Terrain        => Color32::from_rgb(230, 230, 230),
        PinType::PaintOutput    => Color32::from_rgb(100, 190, 100),
        PinType::ScatterOutput  => Color32::from_rgb(160, 200,  80),
        PinType::PlacementMask  => Color32::from_rgb(130, 175, 160),
        PinType::SpeciesWeights => Color32::from_rgb(215, 175, 110),
    }
}

/// Fill color for a node header, derived from its category.
pub fn category_fill(cat: NodeCategory) -> Color32 {
    match cat {
        NodeCategory::Source    => Color32::from_rgb(0x4c, 0x9a, 0xff),
        NodeCategory::Math      => Color32::from_rgb(0x9c, 0x7c, 0xff),
        NodeCategory::Curves    => Color32::from_rgb(0xff, 0xb3, 0x4c),
        NodeCategory::Domain    => Color32::from_rgb(0xff, 0x7c, 0xc4),
        NodeCategory::Density   => Color32::from_rgb(0x6c, 0xc0, 0x6c),
        NodeCategory::Material  => Color32::from_rgb(0xe0, 0x4c, 0x4c),
        NodeCategory::Positions => Color32::from_rgb(0xe0, 0xc0, 0x4c),
        NodeCategory::Scanners  => Color32::from_rgb(0xb0, 0xb0, 0xc0),
        NodeCategory::Props     => Color32::from_rgb(0xc8, 0x90, 0x60),
        NodeCategory::Biome     => Color32::from_rgb(0xb0, 0x80, 0xff),
        NodeCategory::Library   => Color32::from_rgb(0xb0, 0x80, 0xff),
        NodeCategory::Foliage   => Color32::from_rgb(0x6c, 0xb0, 0x4c),
        NodeCategory::Output    => Color32::from_rgb(0xe0, 0x4c, 0x4c),
    }
}

/// Black or white header title text, whichever reads better against the
/// category band color. Uses a perceptual-luminance threshold so bright
/// bands (e.g. Curves, Positions) get dark text and dark bands get light
/// text.
pub fn header_text_color(cat: NodeCategory) -> Color32 {
    let c = category_fill(cat);
    let luma = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if luma > 140.0 {
        Color32::from_gray(20)
    } else {
        Color32::from_gray(235)
    }
}

/// Pin marker shape by type *family*. Shape gives at-a-glance grouping;
/// [`pin_color`] disambiguates the exact type within a family. Snarl offers
/// four built-in shapes, so the nine pin types fold into four families.
pub fn pin_shape(ty: PinType) -> PinShape {
    match ty {
        // Continuous scalar fields & transfer functions.
        PinType::Scalar | PinType::Density | PinType::Curve | PinType::SurfaceField => {
            PinShape::Circle
        }
        // Discrete provider / category / id data.
        PinType::Material
        | PinType::FluidProvider
        | PinType::BiomeId
        | PinType::ZoneId
        | PinType::Assignments
        | PinType::PlacementMask
        | PinType::SpeciesWeights => PinShape::Square,
        // Spatial / positional data.
        PinType::Positions | PinType::Vec3 => PinShape::Triangle,
        // Terminal payloads (terrain + foliage writers).
        PinType::Terrain | PinType::PaintOutput | PinType::ScatterOutput => PinShape::Star,
    }
}
