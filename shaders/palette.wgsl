struct PaletteUniforms {
    colors_srgb: array<vec4<f32>, 32>,
    colors_lab: array<vec4<f32>, 32>,
    count: u32,
    enabled: u32,
    mode: u32,
    l_levels: u32,
    ab_levels: u32,
    l_gamma: f32,
    _pad2: u32,
    _pad3: u32,
};

@group(0) @binding(0)
var<uniform> palette: PaletteUniforms;

@group(0) @binding(1)
var source_texture: texture_2d<f32>;

@group(0) @binding(2)
var source_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index >> 1u) * 4 - 1);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// ===== sRGB -> Lab (forward) =====

fn srgb_to_linear(c: f32) -> f32 {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn linear_rgb_to_xyz(rgb: vec3<f32>) -> vec3<f32> {
    let x = 0.4124564 * rgb.x + 0.3575761 * rgb.y + 0.1804375 * rgb.z;
    let y = 0.2126729 * rgb.x + 0.7151522 * rgb.y + 0.0721750 * rgb.z;
    let z = 0.0193339 * rgb.x + 0.1191920 * rgb.y + 0.9503041 * rgb.z;
    return vec3<f32>(x, y, z);
}

fn lab_f(t: f32) -> f32 {
    let delta = 6.0 / 29.0;
    if (t > delta * delta * delta) {
        return pow(t, 1.0 / 3.0);
    }
    return t / (3.0 * delta * delta) + 4.0 / 29.0;
}

fn xyz_to_lab(xyz: vec3<f32>) -> vec3<f32> {
    let xn = 0.95047;
    let yn = 1.00000;
    let zn = 1.08883;

    let fx = lab_f(xyz.x / xn);
    let fy = lab_f(xyz.y / yn);
    let fz = lab_f(xyz.z / zn);

    let L = 116.0 * fy - 16.0;
    let a = 500.0 * (fx - fy);
    let b = 200.0 * (fy - fz);

    return vec3<f32>(L, a, b);
}

fn srgb_to_lab(srgb: vec3<f32>) -> vec3<f32> {
    let linear_form = vec3<f32>(
        srgb_to_linear(srgb.x),
        srgb_to_linear(srgb.y),
        srgb_to_linear(srgb.z),
    );
    let xyz = linear_rgb_to_xyz(linear_form);
    return xyz_to_lab(xyz);
}

// ===== Lab -> sRGB (inverse) =====

fn lab_f_inv(f_val: f32) -> f32 {
    let delta = 6.0 / 29.0;
    if (f_val > delta) {
        return f_val * f_val * f_val;
    }
    return 3.0 * delta * delta * (f_val - 4.0 / 29.0);
}

fn lab_to_xyz(lab: vec3<f32>) -> vec3<f32> {
    let xn = 0.95047;
    let yn = 1.00000;
    let zn = 1.08883;

    let fy = (lab.x + 16.0) / 116.0;
    let fx = lab.y / 500.0 + fy;
    let fz = fy - lab.z / 200.0;

    return vec3<f32>(
        xn * lab_f_inv(fx),
        yn * lab_f_inv(fy),
        zn * lab_f_inv(fz),
    );
}

fn xyz_to_linear_rgb(xyz: vec3<f32>) -> vec3<f32> {
    let r =  3.2404542 * xyz.x - 1.5371385 * xyz.y - 0.4985314 * xyz.z;
    let g = -0.9692660 * xyz.x + 1.8760108 * xyz.y + 0.0415560 * xyz.z;
    let b =  0.0556434 * xyz.x - 0.2040259 * xyz.y + 1.0572252 * xyz.z;
    return vec3<f32>(r, g, b);
}

fn linear_to_srgb(c: f32) -> f32 {
    if (c <= 0.0031308) {
        return 12.92 * c;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn lab_to_srgb(lab: vec3<f32>) -> vec3<f32> {
    let xyz = lab_to_xyz(lab);
    let linear_val = xyz_to_linear_rgb(xyz);
    return vec3<f32>(
        linear_to_srgb(clamp(linear_val.x, 0.0, 1.0)),
        linear_to_srgb(clamp(linear_val.y, 0.0, 1.0)),
        linear_to_srgb(clamp(linear_val.z, 0.0, 1.0)),
    );
}

// ===== sRGB -> Oklab (forward) =====

fn srgb_to_oklab(srgb: vec3<f32>) -> vec3<f32> {
    let r_lin = srgb_to_linear(srgb.x);
    let g_lin = srgb_to_linear(srgb.y);
    let b_lin = srgb_to_linear(srgb.z);

    // Linear RGB -> LMS (Ottosson M1 matrix)
    let l = 0.4122214708 * r_lin + 0.5363325363 * g_lin + 0.0514459929 * b_lin;
    let m = 0.2119034982 * r_lin + 0.6806995451 * g_lin + 0.1073969566 * b_lin;
    let s = 0.0883024619 * r_lin + 0.2817188376 * g_lin + 0.6299787005 * b_lin;

    // Cube root (safe: all M1 coefficients are positive, so LMS >= 0 for sRGB)
    let l_ = pow(l, 1.0 / 3.0);
    let m_ = pow(m, 1.0 / 3.0);
    let s_ = pow(s, 1.0 / 3.0);

    // Cube-root LMS -> Oklab (Ottosson M2 matrix)
    return vec3<f32>(
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    );
}

// ===== Oklab -> sRGB (inverse) =====

fn oklab_to_srgb(oklab: vec3<f32>) -> vec3<f32> {
    let L = oklab.x;
    let a = oklab.y;
    let b = oklab.z;

    // Oklab -> cube-root LMS (M2 inverse)
    let l_ = L + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = L - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = L - 0.0894841775 * a - 1.2914855480 * b;

    // Cube (inverse of cube root)
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    // LMS -> Linear RGB (M1 inverse)
    let r_lin =  4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
    let g_lin = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
    let b_lin = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;

    // Clamp to [0,1] then apply sRGB gamma
    return vec3<f32>(
        linear_to_srgb(clamp(r_lin, 0.0, 1.0)),
        linear_to_srgb(clamp(g_lin, 0.0, 1.0)),
        linear_to_srgb(clamp(b_lin, 0.0, 1.0)),
    );
}

// ===== Quantization helpers =====

// Color stepping: quantize in OkLCh (polar Oklab) for perceptually uniform steps
fn quantize_stepping(color: vec3<f32>, l_levels: f32, ab_levels: f32) -> vec3<f32> {
    let oklab = srgb_to_oklab(color);

    // Quantize L with gamma warp to bias more steps toward dark values.
    // pow(L, l_gamma) compresses the dark end, giving it more quantization steps.
    // l_gamma = 1.0 is uniform (no warp); l_gamma = 0.5 doubles dark-range resolution.
    let l_clamped  = clamp(oklab.x, 0.0, 1.0);
    let l_warped   = pow(l_clamped, palette.l_gamma);
    let l_q_warped = round(l_warped * (l_levels - 1.0)) / (l_levels - 1.0);
    let l_q        = pow(l_q_warped, 1.0 / palette.l_gamma);

    // Convert a,b to polar (chroma, hue) to preserve hue identity
    let C = sqrt(oklab.y * oklab.y + oklab.z * oklab.z);
    let h = atan2(oklab.z, oklab.y);

    // Quantize chroma in [0, 0.35] (covers max sRGB chroma ~0.323)
    let c_max = 0.35;
    let c_q = clamp(round(C / c_max * (ab_levels - 1.0)) / (ab_levels - 1.0) * c_max, 0.0, c_max);

    // Quantize hue uniformly around the circle
    let PI = 3.14159265;
    let h_norm = (h + PI) / (2.0 * PI);
    let h_q_idx = round(h_norm * ab_levels) % ab_levels;
    let h_q = h_q_idx / ab_levels * 2.0 * PI - PI;

    // Convert back to rectangular a,b
    let a_q = c_q * cos(h_q);
    let b_q = c_q * sin(h_q);

    return clamp(oklab_to_srgb(vec3<f32>(l_q, a_q, b_q)), vec3<f32>(0.0), vec3<f32>(1.0));
}

// ===== Fragment shader =====

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(source_texture, source_sampler, in.uv).rgb;

    if (palette.enabled == 0u) {
        return vec4<f32>(color, 1.0);
    }

    // Mode 0: Palette lookup (existing behavior)
    if (palette.mode == 0u) {
        if (palette.count == 0u) {
            return vec4<f32>(color, 1.0);
        }

        let pixel_lab = srgb_to_lab(color);

        var best_dist = 1e20;
        var best_idx = 0u;

        for (var i = 0u; i < palette.count; i = i + 1u) {
            let pal_lab = palette.colors_lab[i].xyz;
            let diff = pixel_lab - pal_lab;
            let dist = dot(diff, diff);
            if (dist < best_dist) {
                best_dist = dist;
                best_idx = i;
            }
        }

        return vec4<f32>(palette.colors_srgb[best_idx].xyz, 1.0);
    }

    // Mode 1: Color stepping
    let result = quantize_stepping(
        color,
        f32(palette.l_levels),
        f32(palette.ab_levels),
    );
    return vec4<f32>(result, 1.0);
}