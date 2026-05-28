use std::path::Path;
use serde::Deserialize;
use crate::params::PaletteParams;
use crate::rendering::palette_pass::{PaletteUniforms, MAX_PALETTE_COLORS};

#[derive(Deserialize)]
pub struct PaletteFile {
    pub name: String,
    pub colors: Vec<String>,
}

pub struct Palette {
    pub name: String,
    pub colors_srgb: Vec<[f32; 3]>,
    pub colors_lab: Vec<[f32; 3]>,
}

fn parse_hex_color(hex: &str) -> Option<[f32; 3]> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

fn srgb_channel_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_rgb_to_xyz(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    [
        0.4124564 * r + 0.3575761 * g + 0.1804375 * b,
        0.2126729 * r + 0.7151522 * g + 0.0721750 * b,
        0.0193339 * r + 0.1191920 * g + 0.9503041 * b,
    ]
}

fn lab_f(t: f32) -> f32 {
    let delta: f32 = 6.0 / 29.0;
    if t > delta * delta * delta {
        t.powf(1.0 / 3.0)
    } else {
        t / (3.0 * delta * delta) + 4.0 / 29.0
    }
}

fn xyz_to_lab(xyz: [f32; 3]) -> [f32; 3] {
    let xn = 0.95047_f32;
    let yn = 1.00000_f32;
    let zn = 1.08883_f32;

    let fx = lab_f(xyz[0] / xn);
    let fy = lab_f(xyz[1] / yn);
    let fz = lab_f(xyz[2] / zn);

    let l = 116.0 * fy - 16.0;
    let a = 500.0 * (fx - fy);
    let b = 200.0 * (fy - fz);
    [l, a, b]
}

pub fn srgb_to_lab(srgb: [f32; 3]) -> [f32; 3] {
    let linear = [
        srgb_channel_to_linear(srgb[0]),
        srgb_channel_to_linear(srgb[1]),
        srgb_channel_to_linear(srgb[2]),
    ];
    let xyz = linear_rgb_to_xyz(linear);
    xyz_to_lab(xyz)
}

pub fn load_palette(path: &Path) -> Result<Palette, Box<dyn std::error::Error>> {
    let json = std::fs::read_to_string(path)?;
    let file: PaletteFile = serde_json::from_str(&json)?;

    let mut colors_srgb = Vec::new();
    let mut colors_lab = Vec::new();

    for hex in &file.colors {
        if let Some(srgb) = parse_hex_color(hex) {
            colors_srgb.push(srgb);
            colors_lab.push(srgb_to_lab(srgb));
        } else {
            log::warn!("Skipping invalid hex color: {}", hex);
        }
    }

    if colors_srgb.len() > MAX_PALETTE_COLORS {
        colors_srgb.truncate(MAX_PALETTE_COLORS);
        colors_lab.truncate(MAX_PALETTE_COLORS);
        log::warn!("Palette truncated to {} colors", MAX_PALETTE_COLORS);
    }

    Ok(Palette {
        name: file.name,
        colors_srgb,
        colors_lab,
    })
}

pub fn list_palettes(dir: &Path) -> Vec<String> {
    let mut palettes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "json") {
                if let Some(stem) = path.file_stem() {
                    palettes.push(stem.to_string_lossy().into_owned());
                }
            }
        }
    }
    palettes.sort();
    palettes
}

pub fn palette_to_uniforms(palette: &Palette, params: &crate::params::PaletteParams) -> PaletteUniforms {
    let mut uniforms = PaletteUniforms::default();
    let count = palette.colors_srgb.len().min(MAX_PALETTE_COLORS);

    for i in 0..count {
        let srgb = palette.colors_srgb[i];
        uniforms.colors_srgb[i] = [srgb[0], srgb[1], srgb[2], 1.0];

        let lab = palette.colors_lab[i];
        uniforms.colors_lab[i] = [lab[0], lab[1], lab[2], 0.0];
    }

    uniforms.count = count as u32;
    uniforms.enabled = if params.enabled { 1 } else { 0 };
    uniforms.mode = params.mode;
    uniforms.l_levels = params.l_levels;
    uniforms.ab_levels = params.ab_levels;
    uniforms.l_gamma = params.l_gamma;
    uniforms
}

pub fn stepping_uniforms(params: &crate::params::PaletteParams) -> PaletteUniforms {
    let mut uniforms = PaletteUniforms::default();
    uniforms.enabled = if params.enabled { 1 } else { 0 };
    uniforms.mode = params.mode;
    uniforms.l_levels = params.l_levels;
    uniforms.ab_levels = params.ab_levels;
    uniforms.l_gamma = params.l_gamma;
    uniforms
}