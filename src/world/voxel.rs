pub const MAT_AIR: u16 = 0;
pub const MAT_LIMESTONE: u16 = 1;
pub const MAT_GRANITE: u16 = 2;
pub const MAT_SOIL: u16 = 3;
pub const MAT_CLAY: u16 = 4;
pub const MAT_SAND: u16 = 5;
pub const MAT_GRASS_SOIL: u16 = 6;
pub const MAT_WATER: u16 = 7;
pub const MAT_GRAVEL: u16 = 8;

pub const MATERIAL_COUNT: usize = 9;

/// Legacy Voxel struct — bulk storage lives in ChunkStorage.
#[derive(Clone, Copy, Default)]
pub struct Voxel {
    pub material: u16,
}

pub struct MaterialDef {
    pub name: &'static str,
    pub color: [f32; 3],
    pub sharpness: f32,
    pub hardness: f32,
    pub permeable: bool,
    pub supports_flora: bool,
}

pub static MATERIAL_TABLE: [MaterialDef; MATERIAL_COUNT] = [
    // 0: Air
    MaterialDef {
        name: "Air",
        color: [0.0, 0.0, 0.0],
        sharpness: 0.0,
        hardness: 0.0,
        permeable: true,
        supports_flora: false,
    },
    // 1: Limestone - warmer, more distinct off-white
    MaterialDef {
        name: "Limestone",
        color: [0.95, 0.90, 0.82],
        sharpness: 0.9,
        hardness: 0.9,
        permeable: false,
        supports_flora: false,
    },
    // 2: Granite - slightly cool blue-grey
    MaterialDef {
        name: "Granite",
        color: [0.50, 0.50, 0.53],
        sharpness: 0.85,
        hardness: 0.95,
        permeable: false,
        supports_flora: false,
    },
    // 3: Soil - richer brown
    MaterialDef {
        name: "Soil",
        color: [0.40, 0.22, 0.10],
        sharpness: 0.3,
        hardness: 0.2,
        permeable: false,
        supports_flora: true,
    },
    // 4: Clay
    MaterialDef {
        name: "Clay",
        color: [0.62, 0.36, 0.20],
        sharpness: 0.4,
        hardness: 0.4,
        permeable: false,
        supports_flora: false,
    },
    // 5: Sand - brighter warm yellow
    MaterialDef {
        name: "Sand",
        color: [0.90, 0.82, 0.55],
        sharpness: 0.15,
        hardness: 0.1,
        permeable: true,
        supports_flora: false,
    },
    // 6: Grass-covered soil - more vivid green
    MaterialDef {
        name: "Grass Soil",
        color: [0.30, 0.55, 0.18],
        sharpness: 0.35,
        hardness: 0.2,
        permeable: false,
        supports_flora: true,
    },
    // 7: Water
    MaterialDef {
        name: "Water",
        color: [0.2, 0.35, 0.6],
        sharpness: 0.0,
        hardness: 0.0,
        permeable: true,
        supports_flora: false,
    },
    // 8: Gravel
    MaterialDef {
        name: "Gravel",
        color: [0.52, 0.49, 0.45],
        sharpness: 0.6,
        hardness: 0.5,
        permeable: true,
        supports_flora: false,
    },
];