pub const MAT_AIR: u16 = 0;
pub const MAT_LIMESTONE: u16 = 1;
pub const MAT_GRANITE: u16 = 2;
pub const MAT_SOIL: u16 = 3;
pub const MAT_CLAY: u16 = 4;
pub const MAT_SAND: u16 = 5;
pub const MAT_GRASS_SOIL: u16 = 6;
pub const MAT_WATER: u16 = 7;
pub const MAT_GRAVEL: u16 = 8;
pub const MAT_SNOW: u16 = 9;
pub const MAT_SANDSTONE: u16 = 10;
pub const MAT_BASALT: u16 = 11;
pub const MAT_DEEP_STONE: u16 = 12;
pub const MAT_BEDROCK: u16 = 13;
pub const MAT_LAVA: u16 = 14;
pub const MAT_ICE: u16 = 15;
pub const MAT_COAL: u16 = 16;
pub const MAT_IRON: u16 = 17;
pub const MAT_COPPER: u16 = 18;
pub const MAT_GOLD: u16 = 19;
pub const MAT_CRYSTAL: u16 = 20;
pub const MAT_MAGMA_GEM: u16 = 21;

pub const MATERIAL_COUNT: usize = 22;

/// Legacy Voxel struct — no longer used for bulk storage (see ChunkStorage).
/// Kept for material constants and as a reference for Phase 3 field restoration.
#[derive(Clone, Copy, Default)]
pub struct Voxel {
    pub material: u16,
    pub density: i8,
    // TODO Phase 3: these fields return as tiered allocation in ChunkStorage
    // pub moisture: u8,
    // pub light_sun: u8,
    // pub light_emit: u8,
    // pub temperature: u8,
    // pub flora_id: u16,
    // pub flora_growth: u8,
    // pub hidden_flags: u8,
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
    // 9: Snow
    MaterialDef {
        name: "Snow",
        color: [0.95, 0.97, 1.0],
        sharpness: 0.1,
        hardness: 0.1,
        permeable: true,
        supports_flora: false,
    },
    // 10: Sandstone
    MaterialDef {
        name: "Sandstone",
        color: [0.82, 0.72, 0.50],
        sharpness: 0.7,
        hardness: 0.7,
        permeable: false,
        supports_flora: false,
    },
    // 11: Basalt
    MaterialDef {
        name: "Basalt",
        color: [0.20, 0.20, 0.22],
        sharpness: 0.95,
        hardness: 0.98,
        permeable: false,
        supports_flora: false,
    },
    // 12: Deep Stone
    MaterialDef {
        name: "Deep Stone",
        color: [0.35, 0.33, 0.38],
        sharpness: 0.9,
        hardness: 0.95,
        permeable: false,
        supports_flora: false,
    },
    // 13: Bedrock
    MaterialDef {
        name: "Bedrock",
        color: [0.15, 0.15, 0.18],
        sharpness: 1.0,
        hardness: 1.0,
        permeable: false,
        supports_flora: false,
    },
    // 14: Lava
    MaterialDef {
        name: "Lava",
        color: [1.0, 0.35, 0.05],
        sharpness: 0.0,
        hardness: 0.0,
        permeable: true,
        supports_flora: false,
    },
    // 15: Ice
    MaterialDef {
        name: "Ice",
        color: [0.70, 0.85, 0.95],
        sharpness: 0.8,
        hardness: 0.6,
        permeable: false,
        supports_flora: false,
    },
    // 16: Coal
    MaterialDef {
        name: "Coal",
        color: [0.12, 0.12, 0.12],
        sharpness: 0.5,
        hardness: 0.6,
        permeable: false,
        supports_flora: false,
    },
    // 17: Iron
    MaterialDef {
        name: "Iron",
        color: [0.55, 0.45, 0.40],
        sharpness: 0.7,
        hardness: 0.85,
        permeable: false,
        supports_flora: false,
    },
    // 18: Copper
    MaterialDef {
        name: "Copper",
        color: [0.60, 0.42, 0.28],
        sharpness: 0.65,
        hardness: 0.75,
        permeable: false,
        supports_flora: false,
    },
    // 19: Gold
    MaterialDef {
        name: "Gold",
        color: [0.85, 0.75, 0.25],
        sharpness: 0.4,
        hardness: 0.5,
        permeable: false,
        supports_flora: false,
    },
    // 20: Crystal
    MaterialDef {
        name: "Crystal",
        color: [0.75, 0.82, 0.95],
        sharpness: 0.95,
        hardness: 0.9,
        permeable: false,
        supports_flora: false,
    },
    // 21: Magma Gem
    MaterialDef {
        name: "Magma Gem",
        color: [0.90, 0.25, 0.10],
        sharpness: 0.9,
        hardness: 0.85,
        permeable: false,
        supports_flora: false,
    },
];