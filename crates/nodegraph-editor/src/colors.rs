use egui::Color32;
use nodegraph_ir::{NodeCategory, PinType};

/// Distinct color per pin type. Used for both pin markers and connection wires.
pub fn pin_color(ty: PinType) -> Color32 {
    match ty {
        PinType::Scalar     => Color32::from_rgb(180, 220, 255),
        PinType::Density    => Color32::from_rgb(120, 200, 120),
        PinType::Material   => Color32::from_rgb(220,  90,  90),
        PinType::Positions  => Color32::from_rgb(230, 200,  70),
        PinType::Assignments=> Color32::from_rgb(230, 160,  70),
        PinType::Curve      => Color32::from_rgb(255, 200, 120),
        PinType::Vec3       => Color32::from_rgb(255, 140, 200),
        PinType::BiomeId    => Color32::from_rgb(180, 130, 255),
        PinType::Terrain    => Color32::from_rgb(230, 230, 230),
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
        NodeCategory::Slope     => Color32::from_rgb(0x70, 0xa0, 0xb0),
        NodeCategory::Biome     => Color32::from_rgb(0xb0, 0x80, 0xff),
        NodeCategory::Output    => Color32::from_rgb(0xe0, 0x4c, 0x4c),
    }
}
