//! Hardcoded `MaterialId` → display color table.
//!
//! IDs match `voxulacrum-app/src/world/voxel.rs`'s `MAT_*` constants so a
//! later merge with the engine renderer is direct.

use voxel_core::MaterialId;

/// Linear-RGB color for a material, in `[0, 1]` per channel.
pub fn material_color(id: MaterialId) -> [f32; 3] {
    match id.raw() {
        0  => [0.00, 0.00, 0.00], // AIR — never drawn
        1  => [0.95, 0.90, 0.82], // Limestone
        2  => [0.50, 0.50, 0.53], // Granite
        3  => [0.40, 0.22, 0.10], // Soil
        4  => [0.62, 0.36, 0.20], // Clay
        5  => [0.90, 0.82, 0.55], // Sand
        6  => [0.30, 0.55, 0.18], // Grass Soil
        7  => [0.20, 0.35, 0.60], // Water
        8  => [0.52, 0.49, 0.45], // Gravel
        9  => [0.45, 0.30, 0.15], // Wood
        10 => [0.18, 0.45, 0.16], // Leaves
        _  => [1.00, 0.00, 1.00], // unknown — magenta to flag
    }
}
