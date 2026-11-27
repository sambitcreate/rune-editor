// Single-pass fused scene-referred linear photo develop shader.
// Runs directly on Rune's native objc2-metal backend (macOS) as well as DX12/Vulkan (wgpu).
//
// Color science parity: matches `rune_editor_core::evaluate_pixel_linear` / `CompiledRecipe`
// across WB, Exposure, Brightness, Tones, Contrast, Levels, Dehaze, Tone Curves,
// 3-Way Color Balance, 8-Band HSL, Vibrance/Saturation, 6-Channel B&W, and Filmic tone mapping.

struct DevelopUniforms {
    // [slot_x, slot_y, slot_w, slot_h] in pass target device pixels
    slot_rect: vec4<f32>,
    // [target_w, target_h, image_w, image_h]
    dims: vec4<f32>,
    // [zoom, pan_x, pan_y, rotation_rad]
    camera: vec4<f32>,
    // [exposure, contrast, highlights, shadows]
    tone_a: vec4<f32>,
    // [whites, blacks, temperature, tint]
    tone_b: vec4<f32>,
    // [vibrance, saturation, vignette, grain]
    color_fx: vec4<f32>,
    // [split_enabled (0/1), split_pos (0..1), show_original (0/1), srgb_encode (0/1)]
    flags: vec4<f32>,
    // Capture One 3-Way Color Balance wheels: [hue_x, hue_y, luma, 0.0]
    shadows_wheel: vec4<f32>,
    midtones_wheel: vec4<f32>,
    highlights_wheel: vec4<f32>,
    // 8-Band HSL Color Editor: [hue, saturation, luminance, 0.0]
    hsl: array<vec4<f32>, 8>,
    // Spatial detail: [clarity, sharpen, noise_reduction, structure]
    detail: vec4<f32>,
    // Tone & atmosphere: [brightness, dehaze, levels.out_black, levels.out_white]
    atmosphere: vec4<f32>,
    // Levels input: [in_black, in_mid, in_white, levels_active (0/1)]
    levels: vec4<f32>,
    // 6-Channel B&W mixer: [[enabled, red, yellow, green], [cyan, blue, magenta, 0.0]]
    bw_channels: array<vec4<f32>, 2>,
    // Normalized crop rectangle in oriented-frame coordinates: [x, y, w, h]
    geo_crop: vec4<f32>,
    // Persistent geometry transform: [straighten_rad, rotate_90 (0..3), flip_h (0/1), flip_v (0/1)]
    geo_xform: vec4<f32>,
    // Viewer overlays: [exposure_warning (0/1), focus_mask (0/1), crop_mode (0/1), grid_mode (0..3)]
    overlay_flags: vec4<f32>,
    // Capture One Master Color Balance wheel: [hue_x, hue_y, luma, 0.0]
    master_wheel: vec4<f32>,
    // B&W Split Toning: [split_shadow_hue, split_shadow_sat, split_highlight_hue, split_highlight_sat]
    bw_split: vec4<f32>,
    // Per-channel Levels inputs: [in_black, in_mid, in_white, channel_active (0/1)]
    levels_r_in: vec4<f32>,
    levels_g_in: vec4<f32>,
    levels_b_in: vec4<f32>,
    // Per-channel Levels outputs: [r_out_black, r_out_white, g_out_black, g_out_white]
    levels_rgb_out: vec4<f32>,
    // [b_out_black, b_out_white, film_curve (0..3), master_levels_active (0/1)]
    levels_b_out_curve: vec4<f32>,
    // Extended spatial detail: [sharpen_radius, sharpen_threshold, sharpen_halo, nr_color]
    detail_ext: vec4<f32>,
    // Mode selectors & grain size: [clarity_method (0..3), grain_type (0..5), grain_size (0..1), vignette_method (0..2)]
    style_modes: vec4<f32>,
    // Advanced Color Editor (4 slices x 3 vec4<f32>)
    advanced_color: array<vec4<f32>, 12>,
    // Skin Tone Uniformity (3 vec4<f32>)
    skin_tone: array<vec4<f32>, 3>,
    // Optical Lens Correction: [distortion, chromatic_aberration, purple_fringing, light_falloff]
    lens: vec4<f32>,
    // Perspective Keystone: [keystone_vertical, keystone_horizontal, keystone_aspect, keystone_amount]
    geo_keystone: vec4<f32>,
    // Transient viewport flags: [view_color_range (-1..4), loupe_enabled (0/1), 0.0, 0.0]
    viewport_extra: vec4<f32>,
    // 120Hz GPU Loupe overlay: [loupe_u (0..1), loupe_v (0..1), loupe_zoom (1.25..16), loupe_radius_px]
    loupe_params: vec4<f32>,
    // Phase 4 mask overlay & counts: [mask_overlay_mode (0/1/2), active_layer_index (-1..7), layer_count (0..8), heal_count (0..16)]
    mask_overlay: vec4<f32>,
    // Per-layer meta (8 layers): [enabled (0/1), opacity (0..1), inverted (0/1), has_spatial (0/1)]
    layer_meta: array<vec4<f32>, 8>,
    // Per-layer local adjustments (8 layers x 3 vec4<f32>)
    layer_adj: array<vec4<f32>, 24>,
    // Per-layer live Luma Range params (8 layers): [low, high, low_falloff, high_falloff]
    layer_luma_a: array<vec4<f32>, 8>,
    // Per-layer live Luma Range flags (8 layers): [enabled (0/1), op (0/1/2), inverted (0/1), 0.0]
    layer_luma_b: array<vec4<f32>, 8>,
    // Spot Heal & Clone (16 spots x 2 vec4<f32>: [tx, ty, sx, sy], [radius, feather, opacity, mode])
    heal_spots: array<vec4<f32>, 32>,
    // Film Negative Conversion: [base_r, base_g, base_b, processing_mode (0=Photography, 1=FilmNegative)]
    film_neg_a: vec4<f32>,
    // Film Negative Conversion: [density, contrast, black_clip, white_clip]
    film_neg_b: vec4<f32>,
    // Camera Profile / Film Simulation 3D LUT: [profile_active (0/1), profile_strength (0..1), 0.0, 0.0]
    profile_params: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> u: DevelopUniforms;

@group(0) @binding(1)
var photo_tex: texture_2d<f32>;

@group(0) @binding(2)
var photo_sampler: sampler;

@group(0) @binding(3)
var curve_lut_tex: texture_2d<f32>;

@group(0) @binding(4)
var layer_mask_atlas: texture_2d<f32>;

@group(0) @binding(5)
var profile_lut_3d: texture_3d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) slot_uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Two triangles covering the EffectSite slot rectangle
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let uv = corners[vid];
    let px = u.slot_rect.xy + uv * u.slot_rect.zw;
    let ndc = vec2<f32>(
        (px.x / max(u.dims.x, 1.0)) * 2.0 - 1.0,
        1.0 - (px.y / max(u.dims.y, 1.0)) * 2.0,
    );
    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.slot_uv = uv;
    return out;
}

fn luma709(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn sample_photo(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(photo_tex, photo_sampler, uv, 0.0).rgb;
}

// Samples a developed pre-detail source pixel with lateral CA correction and spot healing
// applied (zero extra cost when both CA and heal spots are inactive).
fn sample_clean_source(uv: vec2<f32>) -> vec3<f32> {
    var c = sample_photo_lens(uv);
    if u.mask_overlay.w > 0.5 {
        c = apply_heal_spots(c, uv);
    }
    return c;
}

fn sample_curve_lut(x: f32) -> vec4<f32> {
    let uv_x = (0.5 + clamp(x, 0.0, 1.0) * 255.0) / 256.0;
    return textureSampleLevel(curve_lut_tex, photo_sampler, vec2<f32>(uv_x, 0.5), 0.0);
}

const LUT_SHAPER_MAX_LINEAR: f32 = 64.0;
const LUT_SHAPER_KNEE: f32 = 0.005;
const LUT_SHAPER_LOG_DENOM: f32 = 13.643969;

fn lut_shaper_forward(x: f32) -> f32 {
    let v = max(x, 0.0);
    return clamp(log2(1.0 + v / LUT_SHAPER_KNEE) / LUT_SHAPER_LOG_DENOM, 0.0, 1.0);
}

fn lut_shaper_inverse(s: f32) -> f32 {
    let sc = clamp(s, 0.0, 1.0);
    return max(LUT_SHAPER_KNEE * (exp2(sc * LUT_SHAPER_LOG_DENOM) - 1.0), 0.0);
}

fn apply_profile_lut(rgb: vec3<f32>, strength: f32) -> vec3<f32> {
    let c = max(rgb, vec3<f32>(0.0));
    let s = vec3<f32>(
        lut_shaper_forward(c.r),
        lut_shaper_forward(c.g),
        lut_shaper_forward(c.b),
    );
    let uv = (vec3<f32>(0.5) + s * 32.0) / 33.0;
    let sampled = textureSampleLevel(profile_lut_3d, photo_sampler, uv, 0.0).rgb;
    let hdr_excess = max(c - vec3<f32>(LUT_SHAPER_MAX_LINEAR), vec3<f32>(0.0));
    let mapped = vec3<f32>(
        lut_shaper_inverse(sampled.r),
        lut_shaper_inverse(sampled.g),
        lut_shaper_inverse(sampled.b),
    ) + hdr_excess;
    return max(mix(c, mapped, clamp(strength, 0.0, 1.0)), vec3<f32>(0.0));
}

fn map_level_perceptual(
    p: f32,
    in_black: f32,
    in_mid: f32,
    in_white: f32,
    out_black: f32,
    out_white: f32,
) -> f32 {
    let span = max(in_white - in_black, 0.01);
    let gamma = exp2(-clamp(in_mid, -1.0, 1.0) * 1.5);
    let hdr_keep = clamp((out_white - 0.95) * 20.0, 0.0, 1.0);
    let out_span = out_white - out_black;
    let norm = max((p - in_black) / span, 0.0);
    let base = pow(min(norm, 1.0), gamma);
    let hdr = max(norm - 1.0, 0.0) * hdr_keep;
    return max(out_black + base * out_span + hdr, 0.0);
}

fn apply_levels(rgb: vec3<f32>) -> vec3<f32> {
    if u.levels.w <= 0.5 {
        return rgb;
    }
    let inv_gamma = 1.0 / 2.2;
    let master_active = u.levels_b_out_curve.w > 0.5;
    let r_active = u.levels_r_in.w > 0.5;
    let g_active = u.levels_g_in.w > 0.5;
    let b_active = u.levels_b_in.w > 0.5;

    var out = rgb;
    if master_active || r_active {
        var p = pow(max(rgb.r, 0.0), inv_gamma);
        if master_active {
            p = map_level_perceptual(
                p,
                u.levels.x,
                u.levels.y,
                u.levels.z,
                u.atmosphere.z,
                u.atmosphere.w,
            );
        }
        if r_active {
            p = map_level_perceptual(
                p,
                u.levels_r_in.x,
                u.levels_r_in.y,
                u.levels_r_in.z,
                u.levels_rgb_out.x,
                u.levels_rgb_out.y,
            );
        }
        out.r = pow(max(p, 0.0), 2.2);
    }
    if master_active || g_active {
        var p = pow(max(rgb.g, 0.0), inv_gamma);
        if master_active {
            p = map_level_perceptual(
                p,
                u.levels.x,
                u.levels.y,
                u.levels.z,
                u.atmosphere.z,
                u.atmosphere.w,
            );
        }
        if g_active {
            p = map_level_perceptual(
                p,
                u.levels_g_in.x,
                u.levels_g_in.y,
                u.levels_g_in.z,
                u.levels_rgb_out.z,
                u.levels_rgb_out.w,
            );
        }
        out.g = pow(max(p, 0.0), 2.2);
    }
    if master_active || b_active {
        var p = pow(max(rgb.b, 0.0), inv_gamma);
        if master_active {
            p = map_level_perceptual(
                p,
                u.levels.x,
                u.levels.y,
                u.levels.z,
                u.atmosphere.z,
                u.atmosphere.w,
            );
        }
        if b_active {
            p = map_level_perceptual(
                p,
                u.levels_b_in.x,
                u.levels_b_in.y,
                u.levels_b_in.z,
                u.levels_b_out_curve.x,
                u.levels_b_out_curve.y,
            );
        }
        out.b = pow(max(p, 0.0), 2.2);
    }
    return out;
}

fn apply_dehaze(rgb: vec3<f32>, dehaze: f32) -> vec3<f32> {
    let d = clamp(dehaze, -1.0, 1.0);
    if abs(d) <= 1e-6 {
        return rgb;
    }
    let luma = max(luma709(rgb), 0.0);
    if d > 0.0 {
        let min_c = min(rgb.r, min(rgb.g, rgb.b));
        let veil = min_c * d * 0.45 / (1.0 + luma * 2.0);
        let trans = 1.0 - d * 0.24;
        let dehazed = max(rgb - vec3<f32>(veil), vec3<f32>(0.0)) / trans;
        let pivot = 0.18;
        let c_exp = 1.0 + d * 0.18;
        return pivot * pow(max(dehazed / pivot, vec3<f32>(0.0)), vec3<f32>(c_exp));
    }
    let h = -d;
    let airlight = vec3<f32>(0.42, 0.45, 0.50);
    let w = clamp(h * 0.48 / (1.0 + luma * 1.4), 0.0, 0.85);
    return mix(rgb, airlight, w);
}

fn apply_tone_curves(rgb: vec3<f32>) -> vec3<f32> {
    let inv_gamma = 1.0 / 2.2;
    let p = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(inv_gamma));

    // Master Luma curve (LUT alpha) with HDR headroom preservation
    let lr = sample_curve_lut(min(p.r, 1.0)).a + max(p.r - 1.0, 0.0);
    let lg = sample_curve_lut(min(p.g, 1.0)).a + max(p.g - 1.0, 0.0);
    let lb = sample_curve_lut(min(p.b, 1.0)).a + max(p.b - 1.0, 0.0);

    // Per-channel R / G / B curves with HDR headroom preservation
    let or = sample_curve_lut(min(lr, 1.0)).r + max(lr - 1.0, 0.0);
    let og = sample_curve_lut(min(lg, 1.0)).g + max(lg - 1.0, 0.0);
    let ob = sample_curve_lut(min(lb, 1.0)).b + max(lb - 1.0, 0.0);

    return pow(max(vec3<f32>(or, og, ob), vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn clamp_wheel(w: vec4<f32>) -> vec3<f32> {
    let len = length(w.xy);
    let xy = select(w.xy, w.xy / max(len, 1e-6), len > 1.0);
    return vec3<f32>(xy, clamp(w.z, -1.0, 1.0));
}

fn wheel_tint_rgb(xy: vec2<f32>) -> vec3<f32> {
    let tr = xy.x;
    let tg = -0.5 * xy.x + 0.8660254 * xy.y;
    let tb = -0.5 * xy.x - 0.8660254 * xy.y;
    let raw = vec3<f32>(tr, tg, tb);
    let y = luma709(raw);
    return raw - vec3<f32>(y);
}

fn apply_color_wheels(rgb: vec3<f32>) -> vec3<f32> {
    let sw = clamp_wheel(u.shadows_wheel);
    let mw = clamp_wheel(u.midtones_wheel);
    let hw = clamp_wheel(u.highlights_wheel);
    let gw = clamp_wheel(u.master_wheel);

    let activity = dot(abs(sw), vec3<f32>(1.0))
        + dot(abs(mw), vec3<f32>(1.0))
        + dot(abs(hw), vec3<f32>(1.0))
        + dot(abs(gw), vec3<f32>(1.0));
    if activity <= 1e-6 {
        return rgb;
    }

    let luma = max(luma709(rgb), 1e-5);
    let t = clamp(luma / (luma + 0.18), 0.0, 1.0);
    let w_s = (1.0 - t) * (1.0 - t);
    let w_m = 2.0 * t * (1.0 - t);
    let w_h = t * t;

    let ts = wheel_tint_rgb(sw.xy);
    let tm = wheel_tint_rgb(mw.xy);
    let th = wheel_tint_rgb(hw.xy);
    let tg = wheel_tint_rgb(gw.xy);

    let color_shift = ts * w_s + tm * w_m + th * w_h + tg;
    let luma_shift = sw.z * w_s + mw.z * w_m + hw.z * w_h + gw.z;
    let gain = exp2(color_shift * 0.65 + vec3<f32>(luma_shift * 0.75));
    let shadow_lift = w_s * w_s * 0.025;

    return max(rgb * gain + ts * shadow_lift, vec3<f32>(0.0));
}

fn rgb_to_hsv(rgb: vec3<f32>) -> vec3<f32> {
    let max_c = max(rgb.r, max(rgb.g, rgb.b));
    let min_c = min(rgb.r, min(rgb.g, rgb.b));
    let delta = max_c - min_c;
    let s = select(0.0, clamp(delta / max_c, 0.0, 1.0), max_c > 1e-6);

    var h = 0.0;
    if delta > 1e-6 {
        if max_c == rgb.r {
            var seg = (rgb.g - rgb.b) / delta;
            if seg < 0.0 {
                seg += 6.0;
            }
            h = 60.0 * seg;
        } else if max_c == rgb.g {
            h = 60.0 * (((rgb.b - rgb.r) / delta) + 2.0);
        } else {
            h = 60.0 * (((rgb.r - rgb.g) / delta) + 4.0);
        }
    }
    h = h - 360.0 * floor(h / 360.0);
    return vec3<f32>(h, s, max_c);
}

fn hsv_to_rgb(hsv: vec3<f32>) -> vec3<f32> {
    let h = hsv.x - 360.0 * floor(hsv.x / 360.0);
    let s = clamp(hsv.y, 0.0, 1.0);
    let v = max(hsv.z, 0.0);

    let c = v * s;
    let sector = h / 60.0;
    let mod2 = sector - 2.0 * floor(sector * 0.5);
    let x = c * (1.0 - abs(mod2 - 1.0));
    let m = v - c;

    var rgb1 = vec3<f32>(c, 0.0, x);
    if sector < 1.0 {
        rgb1 = vec3<f32>(c, x, 0.0);
    } else if sector < 2.0 {
        rgb1 = vec3<f32>(x, c, 0.0);
    } else if sector < 3.0 {
        rgb1 = vec3<f32>(0.0, c, x);
    } else if sector < 4.0 {
        rgb1 = vec3<f32>(0.0, x, c);
    } else if sector < 5.0 {
        rgb1 = vec3<f32>(x, 0.0, c);
    }
    return rgb1 + vec3<f32>(m);
}

fn apply_hsl_bands(rgb: vec3<f32>) -> vec3<f32> {
    var any_active = 0.0;
    for (var k: u32 = 0u; k < 8u; k = k + 1u) {
        let b = u.hsl[k];
        any_active += abs(b.x) + abs(b.y) + abs(b.z);
    }
    if any_active <= 1e-6 {
        return rgb;
    }

    let inv_gamma = 1.0 / 2.2;
    let p = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(inv_gamma));
    let hsv = rgb_to_hsv(p);
    let h_deg = hsv.x;
    let s = hsv.y;
    let v = hsv.z;

    let chroma_mask = smoothstep(0.02, 0.14, s);
    if chroma_mask <= 0.0 {
        return rgb;
    }

    var band_hues = array<f32, 8>(0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 280.0, 320.0);
    var i0: u32 = 7u;
    var i1: u32 = 0u;
    var h0: f32 = 320.0;
    var h1: f32 = 360.0;

    for (var i: u32 = 0u; i < 7u; i = i + 1u) {
        if h_deg >= band_hues[i] && h_deg < band_hues[i + 1u] {
            i0 = i;
            i1 = i + 1u;
            h0 = band_hues[i];
            h1 = band_hues[i + 1u];
            break;
        }
    }

    let frac = clamp((h_deg - h0) / (h1 - h0), 0.0, 1.0);
    let w1 = frac * frac * (3.0 - 2.0 * frac);
    let w0 = 1.0 - w1;

    let adj = u.hsl[i0].xyz * w0 + u.hsl[i1].xyz * w1;
    if abs(adj.x) + abs(adj.y) + abs(adj.z) <= 1e-6 {
        return rgb;
    }

    let h_raw = h_deg + adj.x * 30.0 * chroma_mask;
    let h_new = h_raw - 360.0 * floor(h_raw / 360.0);
    let s_new = clamp(s * (1.0 + adj.y * chroma_mask), 0.0, 1.0);
    let v_new = v * exp2(adj.z * 0.65 * chroma_mask);

    let p_out = hsv_to_rgb(vec3<f32>(h_new, s_new, v_new));
    return pow(max(p_out, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn signed_hue_diff_deg(h: f32, target_deg: f32) -> f32 {
    let raw = h - target_deg + 540.0;
    return raw - 360.0 * floor(raw / 360.0) - 180.0;
}

fn advanced_slice_mask(h: f32, s: f32, slice_idx: u32) -> f32 {
    let c0 = u.advanced_color[slice_idx * 3u];
    let c1 = u.advanced_color[slice_idx * 3u + 1u];
    let target_hue = c0.y;
    let target_sat = c0.z;
    let hue_range = c0.w;
    let sat_range = c1.x;
    let smoothness = c1.y;

    let diff_h = abs(signed_hue_diff_deg(h, target_hue));
    let feather_h = max(hue_range * (0.15 + smoothness * 1.15), 2.0);
    let inner_h = hue_range * (1.0 - smoothness * 0.55);
    let outer_h = hue_range + feather_h * 0.5;
    let w_h = 1.0 - smoothstep(inner_h, outer_h, diff_h);

    let diff_s = abs(s - target_sat);
    let inner_s = sat_range * (1.0 - smoothness * 0.55);
    let outer_s = sat_range + max(sat_range * (0.15 + smoothness * 0.85), 0.04);
    let w_s = 1.0 - smoothstep(inner_s, outer_s, diff_s);

    let chroma_guard = select(1.0, smoothstep(0.015, 0.08, s), target_sat > 0.05);
    return clamp(w_h * w_s * chroma_guard, 0.0, 1.0);
}

fn apply_advanced_color(rgb: vec3<f32>) -> vec3<f32> {
    var any_active = false;
    for (var k: u32 = 0u; k < 4u; k = k + 1u) {
        let c0 = u.advanced_color[k * 3u];
        let c1 = u.advanced_color[k * 3u + 1u];
        let c2 = u.advanced_color[k * 3u + 2u];
        if c0.x > 0.5 && (abs(c1.z) > 1e-6 || abs(c1.w) > 1e-6 || abs(c2.x) > 1e-6 || c2.y > 1e-6) {
            any_active = true;
            break;
        }
    }
    if !any_active {
        return rgb;
    }

    let p = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    var hsv = rgb_to_hsv(p);

    for (var k: u32 = 0u; k < 4u; k = k + 1u) {
        let c0 = u.advanced_color[k * 3u];
        let c1 = u.advanced_color[k * 3u + 1u];
        let c2 = u.advanced_color[k * 3u + 2u];
        if c0.x <= 0.5 {
            continue;
        }
        let hue_shift = c1.z;
        let sat_adj = c1.w;
        let light_adj = c2.x;
        let uniformity = c2.y;
        if abs(hue_shift) <= 1e-6 && abs(sat_adj) <= 1e-6 && abs(light_adj) <= 1e-6 && uniformity <= 1e-6 {
            continue;
        }
        let w = advanced_slice_mask(hsv.x, hsv.y, k);
        if w <= 1e-5 {
            continue;
        }
        let diff_h = signed_hue_diff_deg(hsv.x, c0.y);
        let h_raw = hsv.x - diff_h * uniformity * 0.85 * w + hue_shift * 60.0 * w;
        hsv.x = h_raw - 360.0 * floor(h_raw / 360.0);
        let s_u = mix(hsv.y, c0.z, uniformity * 0.75 * w);
        hsv.y = clamp(s_u * (1.0 + sat_adj * w), 0.0, 1.0);
        hsv.z = max(hsv.z * exp2(light_adj * 0.75 * w), 0.0);
    }

    let p_out = hsv_to_rgb(hsv);
    return pow(max(p_out, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn skin_tone_mask(h: f32, s: f32, v: f32) -> f32 {
    let s0 = u.skin_tone[0];
    let s1 = u.skin_tone[1];
    let target_hue = s0.y;
    let target_sat = s0.z;
    let smoothness = s1.x;

    let diff_h = abs(signed_hue_diff_deg(h, target_hue));
    let inner_h = 26.0 * (1.0 - smoothness * 0.55);
    let outer_h = 26.0 + max(26.0 * (0.20 + smoothness * 1.1), 4.0);
    let w_h = 1.0 - smoothstep(inner_h, outer_h, diff_h);

    let diff_s = abs(s - target_sat);
    let inner_s = 0.38 * (1.0 - smoothness * 0.55);
    let outer_s = 0.38 + max(0.38 * (0.20 + smoothness * 0.85), 0.05);
    let w_s = 1.0 - smoothstep(inner_s, outer_s, diff_s);

    let chroma_guard = smoothstep(0.03, 0.12, s);
    let luma_guard = smoothstep(0.03, 0.14, v);
    return clamp(w_h * w_s * chroma_guard * luma_guard, 0.0, 1.0);
}

fn apply_skin_tone(rgb: vec3<f32>) -> vec3<f32> {
    let s0 = u.skin_tone[0];
    let s1 = u.skin_tone[1];
    let s2 = u.skin_tone[2];
    if s0.x <= 0.5 {
        return rgb;
    }
    let u_h = s1.y;
    let u_s = s1.z;
    let u_l = s1.w;
    let a_h = s2.x;
    let a_s = s2.y;
    let a_l = s2.z;
    if u_h <= 1e-6 && u_s <= 1e-6 && u_l <= 1e-6 && abs(a_h) <= 1e-6 && abs(a_s) <= 1e-6 && abs(a_l) <= 1e-6 {
        return rgb;
    }

    let p = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    let hsv = rgb_to_hsv(p);
    let w = skin_tone_mask(hsv.x, hsv.y, hsv.z);
    if w <= 1e-5 {
        return rgb;
    }

    let diff_h = signed_hue_diff_deg(hsv.x, s0.y);
    let h_raw = hsv.x - diff_h * u_h * 0.85 * w + a_h * 35.0 * w;
    let h_new = h_raw - 360.0 * floor(h_raw / 360.0);
    let s_u = mix(hsv.y, s0.z, u_s * 0.80 * w);
    let s_new = clamp(s_u * (1.0 + a_s * 0.65 * w), 0.0, 1.0);
    let v_base = clamp(hsv.z, 0.0, 1.0);
    let v_hdr = max(hsv.z - 1.0, 0.0);
    let v_u = mix(v_base, s0.w, u_l * 0.65 * w) + v_hdr;
    let v_new = max(v_u * exp2(a_l * 0.65 * w), 0.0);

    let p_out = hsv_to_rgb(vec3<f32>(h_new, s_new, v_new));
    return pow(max(p_out, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn apply_bw_mixer(rgb: vec3<f32>) -> vec3<f32> {
    if u.bw_channels[0].x <= 0.5 {
        return rgb;
    }
    let base_y = max(luma709(rgb), 0.0);
    let p = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    let hsv = rgb_to_hsv(p);

    var ch = array<f32, 6>(
        u.bw_channels[0].y,
        u.bw_channels[0].z,
        u.bw_channels[0].w,
        u.bw_channels[1].x,
        u.bw_channels[1].y,
        u.bw_channels[1].z,
    );
    let sector_f = hsv.x / 60.0;
    let i0 = u32(floor(sector_f)) % 6u;
    let i1 = (i0 + 1u) % 6u;
    let frac = clamp(sector_f - floor(sector_f), 0.0, 1.0);
    let w1 = frac * frac * (3.0 - 2.0 * frac);
    let w0 = 1.0 - w1;
    let sens = ch[i0] * w0 + ch[i1] * w1;

    let y_out = max(base_y * exp2(clamp(sens, -1.0, 1.0) * 1.5 * hsv.y), 0.0);
    let sh_sat = clamp(u.bw_split.y, 0.0, 1.0);
    let hi_sat = clamp(u.bw_split.w, 0.0, 1.0);
    if sh_sat <= 1e-5 && hi_sat <= 1e-5 {
        return vec3<f32>(y_out);
    }

    let t = clamp(y_out / (y_out + 0.18), 0.0, 1.0);
    let sh_rgb = hsv_to_rgb(vec3<f32>(u.bw_split.x, 1.0, 1.0));
    let hi_rgb = hsv_to_rgb(vec3<f32>(u.bw_split.z, 1.0, 1.0));
    let tint_s = sh_rgb - vec3<f32>(luma709(sh_rgb));
    let tint_h = hi_rgb - vec3<f32>(luma709(hi_rgb));
    let split_tint = tint_s * (sh_sat * (1.0 - t)) + tint_h * (hi_sat * t);
    return max(vec3<f32>(y_out) * (vec3<f32>(1.0) + split_tint * 0.85), vec3<f32>(0.0));
}

// Fast single-pass spatial Detail (Noise Reduction, Clarity, Structure, Unsharp Mask Sharpening).
// Completely bypassed when all controls are inactive (zero texture fetch overhead).
fn apply_spatial_detail(center_rgb: vec3<f32>, img_uv: vec2<f32>) -> vec3<f32> {
    let clarity = u.detail.x;
    let sharpen = u.detail.y;
    let nr = u.detail.z;
    let structure = u.detail.w;
    let sharpen_radius = clamp(u.detail_ext.x, 0.5, 4.0);
    let sharpen_threshold = clamp(u.detail_ext.y, 0.0, 1.0);
    let sharpen_halo = clamp(u.detail_ext.z, 0.0, 1.0);
    let nr_color = clamp(u.detail_ext.w, 0.0, 1.0);
    let purple_fringing = clamp(u.lens.z, 0.0, 1.0);
    let clarity_method = u32(round(u.style_modes.x));

    if abs(clarity) <= 0.001
        && sharpen <= 0.001
        && nr <= 0.001
        && nr_color <= 0.001
        && abs(structure) <= 0.001
        && purple_fringing <= 0.001
    {
        return center_rgb;
    }

    let texel = vec2<f32>(1.0 / max(u.dims.z, 1.0), 1.0 / max(u.dims.w, 1.0));
    var rgb = max(center_rgb, vec3<f32>(0.0));

    // Tight 4-tap cardinal samples (fetched only when NR, Structure, Purple Fringing, or 1px Sharpening is active)
    var n_up = vec3<f32>(0.0);
    var n_dn = vec3<f32>(0.0);
    var n_lt = vec3<f32>(0.0);
    var n_rt = vec3<f32>(0.0);
    let need_tight_taps = nr > 0.001
        || nr_color > 0.001
        || abs(structure) > 0.001
        || purple_fringing > 0.001
        || (sharpen > 0.001 && abs(sharpen_radius - 1.0) <= 0.01);
    if need_tight_taps {
        n_up = max(sample_clean_source(img_uv + vec2<f32>(0.0, -texel.y)), vec3<f32>(0.0));
        n_dn = max(sample_clean_source(img_uv + vec2<f32>(0.0, texel.y)), vec3<f32>(0.0));
        n_lt = max(sample_clean_source(img_uv + vec2<f32>(-texel.x, 0.0)), vec3<f32>(0.0));
        n_rt = max(sample_clean_source(img_uv + vec2<f32>(texel.x, 0.0)), vec3<f32>(0.0));
    }

    // 1. Edge-aware bilateral Chroma + Luma Noise Reduction
    if nr > 0.001 || nr_color > 0.001 {
        let diag = texel * 1.25;
        let d0 = max(sample_clean_source(img_uv + vec2<f32>(-diag.x, -diag.y)), vec3<f32>(0.0));
        let d1 = max(sample_clean_source(img_uv + vec2<f32>(diag.x, -diag.y)), vec3<f32>(0.0));
        let d2 = max(sample_clean_source(img_uv + vec2<f32>(-diag.x, diag.y)), vec3<f32>(0.0));
        let d3 = max(sample_clean_source(img_uv + vec2<f32>(diag.x, diag.y)), vec3<f32>(0.0));

        let c_p = sqrt(max(luma709(rgb), 0.0));
        let nr_eff = max(nr, nr_color);
        let sigma = 0.04 + nr_eff * 0.14;
        let inv_2s2 = 0.5 / (sigma * sigma);

        var sum_rgb = rgb;
        var sum_w = 1.0;

        var taps = array<vec3<f32>, 8>(n_up, n_dn, n_lt, n_rt, d0, d1, d2, d3);
        for (var i: u32 = 0u; i < 8u; i = i + 1u) {
            let s_rgb = taps[i];
            let diff = sqrt(max(luma709(s_rgb), 0.0)) - c_p;
            let w = exp(-(diff * diff) * inv_2s2);
            sum_rgb += s_rgb * w;
            sum_w += w;
        }
        let denoised = sum_rgb / max(sum_w, 1e-5);

        let y_c = luma709(rgb);
        let y_d = luma709(denoised);
        let chroma_c = rgb - vec3<f32>(y_c);
        let chroma_d = denoised - vec3<f32>(y_d);

        // Smooth chroma strongly while preserving fine luminance detail
        let luma_mix = nr * 0.42;
        let chroma_mix = 1.0 - (1.0 - nr * 0.85) * (1.0 - nr_color * 0.95);
        let y_out = mix(y_c, y_d, luma_mix);
        let chroma_out = mix(chroma_c, chroma_d, chroma_mix);
        rgb = max(vec3<f32>(y_out) + chroma_out, vec3<f32>(0.0));
    }

    // 2. Spatial Clarity (broad-radius local contrast with Natural / Punch / Neutral / Classic methods)
    if abs(clarity) > 0.001 {
        let r1 = texel * 3.0;
        let r2 = texel * 7.0;
        let b1 = sample_clean_source(img_uv + vec2<f32>(r1.x, 0.0))
            + sample_clean_source(img_uv + vec2<f32>(-r1.x, 0.0))
            + sample_clean_source(img_uv + vec2<f32>(0.0, r1.y))
            + sample_clean_source(img_uv + vec2<f32>(0.0, -r1.y));
        let b2 = sample_clean_source(img_uv + vec2<f32>(r2.x, r2.y))
            + sample_clean_source(img_uv + vec2<f32>(-r2.x, r2.y))
            + sample_clean_source(img_uv + vec2<f32>(r2.x, -r2.y))
            + sample_clean_source(img_uv + vec2<f32>(-r2.x, -r2.y));

        let wide_avg = (rgb * 2.0 + b1 + b2 * 0.75) / 9.0;
        let y_c = max(luma709(rgb), 1e-4);
        let y_b = max(luma709(max(wide_avg, vec3<f32>(0.0))), 1e-4);
        let t = clamp(y_c / (y_c + 0.18), 0.0, 1.0);
        let mid_mask = 4.0 * t * (1.0 - t);

        if clarity_method == 1u {
            // Punch: stronger local contrast + subtle midtone saturation lift
            let ratio = clamp(y_c / y_b, 0.25, 4.0);
            let mask = 0.25 + 0.75 * mid_mask;
            let gain = pow(ratio, clarity * 0.88 * mask);
            rgb = max(rgb * gain, vec3<f32>(0.0));
            let y_p = luma709(rgb);
            let sat_boost = max(1.0 + clarity * 0.20 * mid_mask, 0.0);
            rgb = max(vec3<f32>(y_p) + (rgb - vec3<f32>(y_p)) * sat_boost, vec3<f32>(0.0));
        } else if clarity_method == 2u {
            // Neutral: color-neutral local luminance contrast preserving hue & saturation
            let ratio = clamp(y_c / y_b, 0.25, 4.0);
            let gain = pow(ratio, clarity * 0.62 * mid_mask);
            let mul_rgb = max(rgb * gain, vec3<f32>(0.0));
            let add_rgb = max(rgb + vec3<f32>(y_c * (gain - 1.0)), vec3<f32>(0.0));
            rgb = mix(mul_rgb, add_rgb, 0.65);
        } else if clarity_method == 3u {
            // Classic: broader tonal range with controlled ratio clamp
            let ratio = clamp(y_c / y_b, 0.35, 3.0);
            let mask = 0.45 + 0.55 * mid_mask;
            let gain = pow(ratio, clarity * 0.58 * mask);
            rgb = max(rgb * gain, vec3<f32>(0.0));
        } else {
            // Natural (default)
            let ratio = clamp(y_c / y_b, 0.25, 4.0);
            let gain = pow(ratio, clarity * 0.65 * mid_mask);
            rgb = max(rgb * gain, vec3<f32>(0.0));
        }
    }

    // 3. Spatial Structure (fine-radius micro-contrast texture enhancement)
    if abs(structure) > 0.001 {
        let rs = texel * 1.65;
        let s_diag = max(sample_clean_source(img_uv + vec2<f32>(rs.x, rs.y)), vec3<f32>(0.0))
            + max(sample_clean_source(img_uv + vec2<f32>(-rs.x, rs.y)), vec3<f32>(0.0))
            + max(sample_clean_source(img_uv + vec2<f32>(rs.x, -rs.y)), vec3<f32>(0.0))
            + max(sample_clean_source(img_uv + vec2<f32>(-rs.x, -rs.y)), vec3<f32>(0.0));
        let fine_avg = (rgb * 2.0 + (n_up + n_dn + n_lt + n_rt) + s_diag * 0.75) / 9.0;
        let y_c = max(luma709(rgb), 1e-4);
        let y_f = max(luma709(max(fine_avg, vec3<f32>(0.0))), 1e-4);
        let shadow_guard = smoothstep(0.004, 0.05, y_c);
        let ratio = clamp(y_c / y_f, 0.35, 2.8);
        let gain = pow(ratio, clamp(structure, -1.0, 1.0) * 0.72 * shadow_guard);
        rgb = max(rgb * gain, vec3<f32>(0.0));
    }

    // 4. Unsharp Mask Sharpening (Radius, Threshold, Halo Suppression)
    if sharpen > 0.001 {
        var blur_tight = (n_up + n_dn + n_lt + n_rt) * 0.25;
        if abs(sharpen_radius - 1.0) > 0.01 {
            let sr = texel * sharpen_radius;
            let s_up = max(sample_clean_source(img_uv + vec2<f32>(0.0, -sr.y)), vec3<f32>(0.0));
            let s_dn = max(sample_clean_source(img_uv + vec2<f32>(0.0, sr.y)), vec3<f32>(0.0));
            let s_lt = max(sample_clean_source(img_uv + vec2<f32>(-sr.x, 0.0)), vec3<f32>(0.0));
            let s_rt = max(sample_clean_source(img_uv + vec2<f32>(sr.x, 0.0)), vec3<f32>(0.0));
            blur_tight = (s_up + s_dn + s_lt + s_rt) * 0.25;
        }
        let y_c = luma709(rgb);
        let y_blur = luma709(blur_tight);
        let hp = y_c - y_blur;
        let th_lo = 0.002 + sharpen_threshold * 0.048;
        let th_hi = th_lo + 0.010 + sharpen_threshold * 0.030;
        let edge_gate = smoothstep(th_lo, th_hi, abs(hp));
        let rel_clamped = clamp(hp / max(y_c, 0.02), -0.55, 0.55);
        let halo_limit = 0.55 - sharpen_halo * 0.35;
        let halo_damp = 1.0 / (1.0 + sharpen_halo * abs(rel_clamped) * 5.0);
        let rel_hp = clamp(rel_clamped * halo_damp, -halo_limit, halo_limit);
        let sharp_gain = max(1.0 + rel_hp * sharpen * 1.4 * edge_gate, 0.0);
        rgb *= sharp_gain;
    }

    // 5. Axial Purple/Magenta Edge Fringing Suppression
    if purple_fringing > 0.001 {
        let y_c = luma709(rgb);
        let y_up = luma709(n_up);
        let y_dn = luma709(n_dn);
        let y_lt = luma709(n_lt);
        let y_rt = luma709(n_rt);
        let max_n = max(max(y_up, y_dn), max(y_lt, y_rt));
        let min_n = min(min(y_up, y_dn), min(y_lt, y_rt));
        let edge_contrast = smoothstep(0.06, 0.30, max(max_n, y_c) - min(min_n, y_c));
        if edge_contrast > 1e-4 {
            let p = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
            let hsv = rgb_to_hsv(p);
            let diff_p = abs(signed_hue_diff_deg(hsv.x, 295.0));
            let purple_w = (1.0 - smoothstep(22.0, 52.0, diff_p)) * smoothstep(0.18, 0.45, hsv.y);
            let fringe_mask = clamp(purple_w * edge_contrast * purple_fringing, 0.0, 1.0);
            rgb = mix(rgb, vec3<f32>(y_c), fringe_mask * 0.92);
        }
    }

    return rgb;
}

fn filmic_channel(x: f32) -> f32 {
    let v = max(x, 0.0);
    if v <= 0.75 {
        return v;
    }
    let over = v - 0.75;
    return 0.75 + (over / (1.0 + over * 4.0));
}

fn filmic_channel_curve(x: f32, curve_mode: u32) -> f32 {
    let v = max(x, 0.0);
    if curve_mode == 1u {
        // FilmExtraShadow
        let lifted = v * (1.0 + 0.55 / (1.0 + v * 7.0));
        if lifted <= 0.70 {
            return lifted;
        }
        let over = lifted - 0.70;
        return 0.70 + (over / (1.0 + over * (1.0 / 0.30)));
    } else if curve_mode == 2u {
        // FilmHighContrast
        if v <= 0.0 {
            return 0.0;
        }
        let c = 0.18 * pow(v / 0.18, 1.22);
        if c <= 0.72 {
            return c;
        }
        let over = c - 0.72;
        return 0.72 + (over / (1.0 + over * (1.0 / 0.28)));
    } else if curve_mode == 3u {
        // LinearResponse
        return clamp(v, 0.0, 1.0);
    }
    return filmic_channel(v);
}

fn filmic_tonemap(rgb: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        filmic_channel(rgb.r),
        filmic_channel(rgb.g),
        filmic_channel(rgb.b),
    );
}

fn filmic_tonemap_curve(rgb: vec3<f32>, curve_mode: u32) -> vec3<f32> {
    return vec3<f32>(
        filmic_channel_curve(rgb.r, curve_mode),
        filmic_channel_curve(rgb.g, curve_mode),
        filmic_channel_curve(rgb.b, curve_mode),
    );
}

fn linear_to_srgb_channel(x: f32) -> f32 {
    let c = clamp(x, 0.0, 1.0);
    if c <= 0.0031308 {
        return 12.92 * c;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn linear_to_srgb(rgb: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        linear_to_srgb_channel(rgb.r),
        linear_to_srgb_channel(rgb.g),
        linear_to_srgb_channel(rgb.b),
    );
}

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn value_noise_2d(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let w = f * f * (3.0 - 2.0 * f);
    let a = hash12(i);
    let b = hash12(i + vec2<f32>(1.0, 0.0));
    let c = hash12(i + vec2<f32>(0.0, 1.0));
    let d = hash12(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

fn apply_film_grain(
    rgb: vec3<f32>,
    px: vec2<f32>,
    grain: f32,
    grain_type: u32,
    grain_size: f32,
) -> vec3<f32> {
    let sz = clamp(grain_size, 0.0, 1.0);
    let grain_luma = max(luma709(rgb), 1e-5);
    let grain_t = clamp(grain_luma / (grain_luma + 0.18), 0.0, 1.0);
    let mid_mask = 1.0 - abs(2.0 * grain_t - 1.0);
    let base_n = (hash12(px) - 0.5) * 2.0;

    var n = base_n;
    var amp = 0.065;
    var tone_w = 0.35 + 0.65 * mid_mask;

    if grain_type == 1u {
        // SilverRich
        let c_n = (hash12(floor(px / (1.0 + sz * 2.5)) + vec2<f32>(17.0, 31.0)) - 0.5) * 2.0;
        let m = mix(base_n, c_n, 0.35 + sz * 0.50);
        n = sign(m) * pow(max(abs(m), 1e-6), 0.85);
        amp = 0.085;
        tone_w = 0.45 + 0.55 * mid_mask;
    } else if grain_type == 2u {
        // Soft
        let sn = (value_noise_2d(px / (1.2 + sz * 2.8) + vec2<f32>(5.0, 11.0)) - 0.5) * 2.0;
        n = mix(sn, base_n, 0.25);
        amp = 0.060;
        tone_w = 0.30 + 0.70 * mid_mask;
    } else if grain_type == 3u {
        // Cubic
        let cn = (hash12(floor(px / (1.25 + sz * 2.75)) + vec2<f32>(41.0, 73.0)) - 0.5) * 2.0;
        n = mix(base_n, cn, 0.55 + sz * 0.35);
        amp = 0.095;
        tone_w = 0.40 + 0.60 * mid_mask;
    } else if grain_type == 4u {
        // Tabular
        let tn = (value_noise_2d(px / (1.0 + sz * 1.6) + vec2<f32>(23.0, 19.0)) - 0.5) * 2.0;
        n = mix(base_n, tn, 0.30 + sz * 0.35);
        amp = 0.070;
        tone_w = 0.25 + 0.75 * mid_mask;
    } else if grain_type == 5u {
        // Harsh
        let hn = (hash12(floor(px / (1.4 + sz * 3.1)) + vec2<f32>(97.0, 53.0)) - 0.5) * 2.0;
        n = clamp(mix(base_n, hn, 0.60 + sz * 0.35) * 1.35, -1.0, 1.0);
        amp = 0.125;
        tone_w = 0.55 + 0.45 * mid_mask;
    } else {
        // Fine (default)
        let c_n = (hash12(floor(px / (1.0 + sz * 2.5)) + vec2<f32>(17.0, 31.0)) - 0.5) * 2.0;
        n = mix(base_n, c_n, sz * 0.45);
    }

    return max(rgb + vec3<f32>(n * grain * amp * tone_w), vec3<f32>(0.0));
}

fn apply_keystone_uv(ori_uv: vec2<f32>) -> vec2<f32> {
    let amount = clamp(u.geo_keystone.w, 0.0, 1.0);
    let kv = clamp(u.geo_keystone.x, -1.0, 1.0) * amount * 0.45;
    let kh = clamp(u.geo_keystone.y, -1.0, 1.0) * amount * 0.45;
    let ka = clamp(u.geo_keystone.z, -0.5, 1.0);
    if abs(kv) <= 1e-6 && abs(kh) <= 1e-6 && abs(ka) <= 1e-6 {
        return ori_uv;
    }
    let x = ori_uv.x - 0.5;
    let y = ori_uv.y - 0.5;
    let inv_a = 1.0 / (1.0 + ka * 0.5);
    let y_a = y * inv_a;
    let w = max(1.0 + kv * y_a + kh * x, 0.15);
    let w_min = max(1.0 - abs(kv) * 0.5 * inv_a - abs(kh) * 0.5, 0.20);
    let auto_scale = w_min / max(inv_a, 1.0);
    return vec2<f32>(0.5 + (x * auto_scale) / w, 0.5 + (y_a * auto_scale) / w);
}

fn apply_lens_distortion_uv(uv: vec2<f32>, distortion: f32, src_w: f32, src_h: f32) -> vec2<f32> {
    let d = clamp(distortion, -1.0, 1.0);
    if abs(d) <= 1e-6 {
        return uv;
    }
    let min_dim = min(max(src_w, 1.0), max(src_h, 1.0));
    let dx = (uv.x - 0.5) * (max(src_w, 1.0) / min_dim);
    let dy = (uv.y - 0.5) * (max(src_h, 1.0) / min_dim);
    let r2 = dx * dx + dy * dy;
    let k1 = d * 0.24;
    let k2 = d * 0.06;
    let s_fit = 1.0 / (1.0 + max(k1, 0.0) * 0.5 + max(k2, 0.0) * 0.25);
    let scale = (1.0 + k1 * r2 + k2 * r2 * r2) * s_fit;
    return vec2<f32>(
        0.5 + dx * scale * (min_dim / max(src_w, 1.0)),
        0.5 + dy * scale * (min_dim / max(src_h, 1.0)),
    );
}

fn sample_photo_lens(uv: vec2<f32>) -> vec3<f32> {
    let ca_raw = clamp(u.lens.y, -1.0, 1.0);
    if abs(ca_raw) <= 1e-5 {
        return sample_photo(uv);
    }
    let ca = ca_raw * 0.008;
    let d = uv - vec2<f32>(0.5);
    let uv_r = vec2<f32>(0.5) + d * (1.0 + ca);
    let uv_b = vec2<f32>(0.5) + d * (1.0 - ca);
    return vec3<f32>(
        sample_photo(uv_r).r,
        sample_photo(uv).g,
        sample_photo(uv_b).b,
    );
}

// Stage 0 Spot Heal & Clone in normalized source image UV space (zero cost when heal_count == 0).
fn apply_heal_spots(base_rgb: vec3<f32>, img_uv: vec2<f32>) -> vec3<f32> {
    let count = u32(clamp(round(u.mask_overlay.w), 0.0, 16.0));
    if count == 0u {
        return base_rgb;
    }
    let img_w = max(u.dims.z, 1.0);
    let img_h = max(u.dims.w, 1.0);
    let min_dim = min(img_w, img_h);
    let ax = img_w / min_dim;
    let ay = img_h / min_dim;

    var rgb = base_rgb;
    for (var i = 0u; i < 16u; i = i + 1u) {
        if i >= count {
            break;
        }
        let s0 = u.heal_spots[i * 2u];
        let s1 = u.heal_spots[i * 2u + 1u];
        let spot_tgt = s0.xy;
        let spot_src = s0.zw;
        let radius = max(s1.x, 0.002);
        let feather = clamp(s1.y, 0.0, 1.0);
        let opacity = clamp(s1.z, 0.0, 1.0);
        if opacity <= 1e-4 {
            continue;
        }

        let delta = vec2<f32>((img_uv.x - spot_tgt.x) * ax, (img_uv.y - spot_tgt.y) * ay);
        let dist = length(delta);
        if dist < radius {
            let inner = radius * (1.0 - feather);
            var w = 1.0;
            if radius - inner > 1e-6 {
                w = 1.0 - smoothstep(inner, radius, dist);
            }
            w *= opacity;
            let src_uv = clamp(spot_src + (img_uv - spot_tgt), vec2<f32>(0.0), vec2<f32>(1.0));
            let donor = sample_photo_lens(src_uv);
            if s1.w > 0.5 {
                // Clone mode: direct feathered transfer
                rgb = mix(rgb, donor, w);
            } else {
                // Heal mode: match 4-tap perimeter ring luminance around target vs source
                let step_u = (radius * 1.15) / ax;
                let step_v = (radius * 1.15) / ay;
                let t_ring = 0.25 * (
                    luma709(sample_photo_lens(clamp(spot_tgt + vec2<f32>(step_u, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))))
                    + luma709(sample_photo_lens(clamp(spot_tgt - vec2<f32>(step_u, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))))
                    + luma709(sample_photo_lens(clamp(spot_tgt + vec2<f32>(0.0, step_v), vec2<f32>(0.0), vec2<f32>(1.0))))
                    + luma709(sample_photo_lens(clamp(spot_tgt - vec2<f32>(0.0, step_v), vec2<f32>(0.0), vec2<f32>(1.0))))
                );
                let s_ring = 0.25 * (
                    luma709(sample_photo_lens(clamp(spot_src + vec2<f32>(step_u, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))))
                    + luma709(sample_photo_lens(clamp(spot_src - vec2<f32>(step_u, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))))
                    + luma709(sample_photo_lens(clamp(spot_src + vec2<f32>(0.0, step_v), vec2<f32>(0.0), vec2<f32>(1.0))))
                    + luma709(sample_photo_lens(clamp(spot_src - vec2<f32>(0.0, step_v), vec2<f32>(0.0), vec2<f32>(1.0))))
                );
                let ratio = clamp(t_ring / max(s_ring, 1e-4), 0.25, 4.0);
                rgb = mix(rgb, max(donor * ratio, vec3<f32>(0.0)), w);
            }
        }
    }
    return rgb;
}

fn sample_layer_mask_tile(layer_idx: u32, uv: vec2<f32>) -> f32 {
    let col = f32(layer_idx % 4u);
    let row = f32(layer_idx / 4u);
    let local_u = (0.5 + clamp(uv.x, 0.0, 1.0) * 511.0) / 512.0;
    let local_v = (0.5 + clamp(uv.y, 0.0, 1.0) * 511.0) / 512.0;
    let atlas_uv = vec2<f32>((col + local_u) * 0.25, (row + local_v) * 0.5);
    return textureSampleLevel(layer_mask_atlas, photo_sampler, atlas_uv, 0.0).r;
}

fn luma_range_mask(luma_tone: f32, low: f32, high: f32, low_falloff: f32, high_falloff: f32) -> f32 {
    let y = clamp(luma_tone, 0.0, 1.0);
    let lo = clamp(low, 0.0, 1.0);
    let hi = clamp(high, lo, 1.0);
    let lf = clamp(low_falloff, 0.0, 1.0);
    let hf = clamp(high_falloff, 0.0, 1.0);

    var lo_w = select(0.0, 1.0, y >= lo);
    if lf > 1e-5 {
        lo_w = smoothstep(lo - lf, lo, y);
    }
    var hi_w = select(0.0, 1.0, y <= hi);
    if hf > 1e-5 {
        hi_w = 1.0 - smoothstep(hi, hi + hf, y);
    }
    return clamp(lo_w * hi_w, 0.0, 1.0);
}

fn combine_mask_op(base_m: f32, next_m: f32, op_code: f32) -> f32 {
    let op = u32(round(op_code));
    if op == 1u {
        return clamp(base_m - next_m, 0.0, 1.0);
    } else if op == 2u {
        return clamp(base_m * next_m, 0.0, 1.0);
    }
    return max(base_m, next_m);
}

fn eval_layer_mask_raw(layer_idx: u32, img_uv: vec2<f32>, luma_tone: f32) -> f32 {
    let lmeta = u.layer_meta[layer_idx];
    let has_spatial = lmeta.w > 0.5;
    var m = select(0.0, sample_layer_mask_tile(layer_idx, img_uv), has_spatial);

    let lb = u.layer_luma_b[layer_idx];
    if lb.x > 0.5 {
        let la = u.layer_luma_a[layer_idx];
        var lm = luma_range_mask(luma_tone, la.x, la.y, la.z, la.w);
        if lb.z > 0.5 {
            lm = 1.0 - lm;
        }
        if has_spatial {
            m = combine_mask_op(m, lm, lb.y);
        } else {
            let op = u32(round(lb.y));
            if op == 1u {
                m = clamp(1.0 - lm, 0.0, 1.0);
            } else {
                m = lm;
            }
        }
    }
    if lmeta.z > 0.5 {
        m = 1.0 - m;
    }
    return clamp(m, 0.0, 1.0);
}

fn apply_local_adjustments_layer(
    rgb: vec3<f32>,
    layer_idx: u32,
    mask_w: f32,
    local_mid_ratio: f32,
    local_hp_rel: f32,
) -> vec3<f32> {
    let w = clamp(mask_w, 0.0, 1.0);
    if w <= 1e-5 {
        return rgb;
    }
    let a0 = u.layer_adj[layer_idx * 3u];
    let a1 = u.layer_adj[layer_idx * 3u + 1u];
    let a2 = u.layer_adj[layer_idx * 3u + 2u];
    if a2.w <= 0.5 {
        return rgb;
    }

    let l_exp = a0.x;
    let l_con = a0.y;
    let l_bri = a0.z;
    let l_hi = a0.w;
    let l_sh = a1.x;
    let l_temp = a1.y;
    let l_tint = a1.z;
    let l_sat = a1.w;
    let l_clar = a2.x;
    let l_dehaze = a2.y;
    let l_sharp = a2.z;

    var c = max(rgb, vec3<f32>(0.0));

    // 1. Local WB + Exposure
    if abs(l_exp) > 1e-5 || abs(l_temp) > 1e-5 || abs(l_tint) > 1e-5 {
        let exp_g = exp2(l_exp);
        let wb = vec3<f32>(
            exp2(l_temp * 0.45 + l_tint * 0.22) * exp_g,
            exp2(-l_tint * 0.35) * exp_g,
            exp2(-l_temp * 0.45 + l_tint * 0.22) * exp_g,
        );
        c = max(c * wb, vec3<f32>(0.0));
    }

    // 2. Local Shadows / Highlights / Brightness
    if abs(l_sh) > 1e-5 || abs(l_hi) > 1e-5 || abs(l_bri) > 1e-5 {
        let luma = max(luma709(c), 1e-5);
        let t = clamp(luma / (luma + 0.18), 0.0, 1.0);
        let sh_w = (1.0 - t) * (1.0 - t);
        let hi_w = t * t;
        let mid_w = 4.0 * t * (1.0 - t);
        let stops = l_sh * 1.15 * sh_w + l_hi * 1.15 * hi_w + l_bri * 0.95 * mid_w;
        c *= exp2(stops);
    }

    // 3. Local Contrast
    if abs(l_con) > 1e-5 {
        let pivot = 0.18;
        let c_pow = exp2(l_con * 0.85);
        c = pivot * pow(max(c / pivot, vec3<f32>(0.0)), vec3<f32>(c_pow));
    }

    // 4. Local Dehaze
    if abs(l_dehaze) > 1e-5 {
        let dark = max(min(c.r, min(c.g, c.b)), 0.0);
        let veil = clamp(dark / (dark + 0.25), 0.0, 1.0);
        let t_min = clamp(1.0 - l_dehaze * 0.52 * veil, 0.32, 1.65);
        let airlight = 0.42;
        c = max((c - vec3<f32>(airlight * (1.0 - t_min))) / t_min, vec3<f32>(0.0));
        let y_dh = luma709(c);
        let dh_sat = max(1.0 + l_dehaze * 0.16, 0.0);
        c = max(vec3<f32>(y_dh) + (c - vec3<f32>(y_dh)) * dh_sat, vec3<f32>(0.0));
    }

    // 5. Local Clarity & Sharpen
    if abs(l_clar) > 1e-5 {
        let y_c = max(luma709(c), 1e-5);
        let t = clamp(y_c / (y_c + 0.18), 0.0, 1.0);
        let mid_w = clamp(4.0 * t * (1.0 - t), 0.2, 1.0);
        let gain = pow(clamp(local_mid_ratio, 0.45, 2.2), l_clar * 0.65 * mid_w);
        c *= gain;
    }
    if l_sharp > 1e-4 {
        let sharp_gain = max(1.0 + clamp(local_hp_rel, -0.45, 0.45) * l_sharp * 1.25, 0.0);
        c *= sharp_gain;
    }

    // 6. Local Saturation
    if abs(l_sat) > 1e-5 {
        let y_s = luma709(c);
        let sat_scale = max(1.0 + l_sat, 0.0);
        c = max(vec3<f32>(y_s) + (c - vec3<f32>(y_s)) * sat_scale, vec3<f32>(0.0));
    }

    return mix(rgb, c, w);
}

fn oriented_to_source_uv(ori_uv: vec2<f32>, ori_w: f32, ori_h: f32) -> vec2<f32> {
    let theta = u.geo_xform.x;
    var uv = apply_keystone_uv(ori_uv);
    if abs(theta) > 1e-6 {
        let c = cos(theta);
        let s = sin(theta);
        let px = (uv.x - 0.5) * ori_w;
        let py = (uv.y - 0.5) * ori_h;
        let rx = px * c + py * s;
        let ry = -px * s + py * c;
        uv = vec2<f32>(rx / ori_w + 0.5, ry / ori_h + 0.5);
    }

    if u.geo_xform.z > 0.5 {
        uv.x = 1.0 - uv.x;
    }
    if u.geo_xform.w > 0.5 {
        uv.y = 1.0 - uv.y;
    }

    let rot_q = u32(round(u.geo_xform.y)) % 4u;
    if rot_q == 1u {
        return vec2<f32>(uv.y, 1.0 - uv.x);
    } else if rot_q == 2u {
        return vec2<f32>(1.0 - uv.x, 1.0 - uv.y);
    } else if rot_q == 3u {
        return vec2<f32>(1.0 - uv.y, uv.x);
    }
    return uv;
}

fn invert_film_negative(rgb: vec3<f32>) -> vec3<f32> {
    let base = max(u.film_neg_a.xyz, vec3<f32>(0.05));
    let density = clamp(u.film_neg_b.x, -1.0, 1.0);
    let contrast = clamp(u.film_neg_b.y, -1.0, 1.0);
    let black_clip = clamp(u.film_neg_b.z, 0.0, 0.45);
    let white_clip = max(clamp(u.film_neg_b.w, 0.55, 1.0), black_clip + 0.10);
    let span = max(white_clip - black_clip, 0.05);
    let gamma = 2.2 * exp2(contrast * 0.65);
    let density_gain = exp2(density * 1.5);

    let trans = clamp(max(rgb, vec3<f32>(0.0)) / base, vec3<f32>(0.0), vec3<f32>(1.0));
    let t_neg = clamp(pow(trans, vec3<f32>(1.0 / 2.2)), vec3<f32>(0.0), vec3<f32>(1.0));
    let inv = vec3<f32>(1.0) - t_neg;
    let norm = clamp((inv - vec3<f32>(black_clip)) / span, vec3<f32>(0.0), vec3<f32>(1.0));
    return max(pow(norm, vec3<f32>(gamma)) * density_gain, vec3<f32>(0.0));
}

fn apply_develop(
    src_rgb: vec3<f32>,
    img_uv: vec2<f32>,
    vig_uv: vec2<f32>,
    ori_uv: vec2<f32>,
    ori_size: vec2<f32>,
    crop_size: vec2<f32>,
) -> vec3<f32> {
    let exposure = u.tone_a.x;
    let contrast = u.tone_a.y;
    let highlights = u.tone_a.z;
    let shadows = u.tone_a.w;

    let whites = u.tone_b.x;
    let blacks = u.tone_b.y;
    let temperature = u.tone_b.z;
    let tint = u.tone_b.w;

    let vibrance = u.color_fx.x;
    let saturation = u.color_fx.y;
    let vignette = u.color_fx.z;
    let grain = u.color_fx.w;

    let brightness = clamp(u.atmosphere.x, -1.0, 1.0);
    let dehaze = u.atmosphere.y;

    // 0. Optional spatial detail (NR / Clarity / Structure / Sharpen)
    var rgb = apply_spatial_detail(src_rgb, img_uv);

    // 0.5. Optional Film Negative optical inversion (zero cost in Photography mode)
    if u.film_neg_a.w > 0.5 {
        rgb = invert_film_negative(rgb);
    }

    // 0.75. Optional Camera Profile / Film Simulation 33^3 3D LUT (zero cost when inactive)
    if u.profile_params.x > 0.5 && u.profile_params.y > 1e-5 {
        rgb = apply_profile_lut(rgb, u.profile_params.y);
    }

    // 1. White balance + Exposure (linear stops)
    let wb = vec3<f32>(
        exp2(temperature * 0.45 + tint * 0.22),
        exp2(-tint * 0.35),
        exp2(-temperature * 0.45 + tint * 0.22),
    );
    rgb = max(rgb * wb * exp2(exposure), vec3<f32>(0.0));

    // 2. Tonal regions (Shadows / Highlights / Whites / Blacks / Midtone Brightness)
    let luma = max(luma709(rgb), 1e-5);
    let t = clamp(luma / (luma + 0.18), 0.0, 1.0);
    let shadow_w = (1.0 - t) * (1.0 - t);
    let highlight_w = t * t;
    let black_w = shadow_w * shadow_w;
    let white_w = highlight_w * highlight_w;
    let mid_w = 4.0 * t * (1.0 - t);

    let tone_stops = shadows * 1.15 * shadow_w
        + highlights * 1.15 * highlight_w
        + blacks * 0.90 * black_w
        + whites * 0.75 * white_w
        + brightness * 0.95 * mid_w;
    rgb *= exp2(tone_stops);

    // 3. Contrast around 18% middle gray pivot
    let pivot = 0.18;
    let c_pow = exp2(contrast * 0.85);
    rgb = pivot * pow(max(rgb / pivot, vec3<f32>(0.0)), vec3<f32>(c_pow));

    // 4. Levels (Input black/mid/white + Output black/white)
    rgb = apply_levels(rgb);

    // 5. Atmospheric Dehaze
    rgb = apply_dehaze(rgb, dehaze);

    // 6. Tone Curves (256x1 Rgba16Float LUT: Luma + per-channel R/G/B)
    rgb = apply_tone_curves(rgb);

    // 7. Capture One 3-Way Color Balance (Shadows / Midtones / Highlights wheels)
    rgb = apply_color_wheels(rgb);

    // 8. 8-Band HSL Color Editor
    rgb = apply_hsl_bands(rgb);

    // Optional transient `View Selected Color Range` mask sampled right before Advanced/Skin edits
    let view_range = i32(round(u.viewport_extra.x));
    var range_mask = 1.0;
    if view_range >= 0 && view_range <= 4 {
        let p_vr = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
        let hsv_vr = rgb_to_hsv(p_vr);
        if view_range < 4 {
            range_mask = advanced_slice_mask(hsv_vr.x, hsv_vr.y, u32(view_range));
        } else {
            range_mask = skin_tone_mask(hsv_vr.x, hsv_vr.y, hsv_vr.z);
        }
    }

    // 8b. Advanced Color Editor (4 customizable hue/sat slices)
    rgb = apply_advanced_color(rgb);

    // 8c. Skin Tone Uniformity & Amount
    rgb = apply_skin_tone(rgb);

    // 9. Vibrance & Saturation
    let y = luma709(rgb);
    let max_c = max(rgb.r, max(rgb.g, rgb.b));
    let min_c = min(rgb.r, min(rgb.g, rgb.b));
    let est_sat = select(0.0, clamp((max_c - min_c) / max_c, 0.0, 1.0), max_c > 1e-5);
    let sat_scale = max(1.0 + saturation + vibrance * (1.0 - est_sat) * 0.75, 0.0);
    rgb = max(vec3<f32>(y) + (rgb - vec3<f32>(y)) * sat_scale, vec3<f32>(0.0));

    // 10. 6-Channel Black & White Mixer
    rgb = apply_bw_mixer(rgb);

    // 10b. Adjustment Layers (up to 8 layers with baked spatial mask atlas + live Luma Range)
    let layer_count = u32(clamp(round(u.mask_overlay.z), 0.0, 8.0));
    if layer_count > 0u {
        var need_local_detail = false;
        for (var li = 0u; li < 8u; li = li + 1u) {
            if li >= layer_count {
                break;
            }
            let lmeta = u.layer_meta[li];
            let a2 = u.layer_adj[li * 3u + 2u];
            if lmeta.x > 0.5 && lmeta.y > 1e-5 && (abs(a2.x) > 1e-5 || a2.z > 1e-4) {
                need_local_detail = true;
            }
        }
        var local_mid_ratio = 1.0;
        var local_hp_rel = 0.0;
        if need_local_detail {
            let texel = vec2<f32>(1.0 / max(u.dims.z, 1.0), 1.0 / max(u.dims.w, 1.0));
            let y_src = max(luma709(src_rgb), 1e-5);
            let y_tight = 0.25 * (
                luma709(sample_clean_source(img_uv - vec2<f32>(0.0, texel.y)))
                + luma709(sample_clean_source(img_uv + vec2<f32>(0.0, texel.y)))
                + luma709(sample_clean_source(img_uv - vec2<f32>(texel.x, 0.0)))
                + luma709(sample_clean_source(img_uv + vec2<f32>(texel.x, 0.0)))
            );
            let d2 = texel * 2.5;
            let y_mid = 0.25 * (
                luma709(sample_clean_source(img_uv + vec2<f32>(-d2.x, -d2.y)))
                + luma709(sample_clean_source(img_uv + vec2<f32>(d2.x, -d2.y)))
                + luma709(sample_clean_source(img_uv + vec2<f32>(-d2.x, d2.y)))
                + luma709(sample_clean_source(img_uv + vec2<f32>(d2.x, d2.y)))
            );
            local_mid_ratio = (y_src + 0.02) / (y_mid + 0.02);
            local_hp_rel = (y_src - y_tight) / max(y_src, 0.02);
        }

        for (var li = 0u; li < 8u; li = li + 1u) {
            if li >= layer_count {
                break;
            }
            let lmeta = u.layer_meta[li];
            let a2 = u.layer_adj[li * 3u + 2u];
            if lmeta.x > 0.5 && lmeta.y > 1e-5 && a2.w > 0.5 {
                let luma_tone = clamp(sqrt(max(luma709(rgb), 0.0)), 0.0, 1.0);
                let m = eval_layer_mask_raw(li, img_uv, luma_tone) * clamp(lmeta.y, 0.0, 1.0);
                rgb = apply_local_adjustments_layer(rgb, li, m, local_mid_ratio, local_hp_rel);
            }
        }
    }

    // 11a. Optical Radial cos^4 Light Falloff Compensation (anchored to full lens center)
    let light_falloff = clamp(u.lens.w, 0.0, 1.0);
    if light_falloff > 1e-5 {
        let lf_c = (ori_uv - vec2<f32>(0.5)) * 2.0;
        let lf_r2 = dot(lf_c, lf_c);
        rgb *= pow(1.0 + 0.22 * lf_r2, 2.0 * light_falloff);
    }

    // 11b. Crop-Aware Optical Vignette (EllipticOnCrop / CircularOnCrop / Circular)
    if abs(vignette) > 1e-5 {
        let vig_method = u32(round(u.style_modes.w));
        var centered = (vig_uv - vec2<f32>(0.5)) * 2.0;
        if vig_method == 1u {
            let aspect = max(crop_size.x, 1.0) / max(crop_size.y, 1.0);
            let ax = sqrt(aspect);
            let ay = 1.0 / ax;
            centered = vec2<f32>((vig_uv.x - 0.5) * 2.0 * ax, (vig_uv.y - 0.5) * 2.0 * ay);
        } else if vig_method == 2u {
            let aspect = max(ori_size.x, 1.0) / max(ori_size.y, 1.0);
            let ax = sqrt(aspect);
            let ay = 1.0 / ax;
            centered = vec2<f32>((ori_uv.x - 0.5) * 2.0 * ax, (ori_uv.y - 0.5) * 2.0 * ay);
        }
        let r2 = dot(centered, centered);
        let vig_gain = exp2(vignette * 1.15 * smoothstep(0.18, 1.65, r2));
        rgb *= vig_gain;
    }

    // 12. Photographic Film Grain (6 emulsion profiles + Granularity)
    if grain > 0.001 {
        let grain_type = u32(round(u.style_modes.y));
        let grain_size = u.style_modes.z;
        rgb = apply_film_grain(rgb, floor(img_uv * u.dims.zw), grain, grain_type, grain_size);
    }

    if view_range >= 0 && view_range <= 4 {
        let mono_y = max(luma709(rgb), 0.0);
        rgb = mix(vec3<f32>(mono_y), rgb, range_mask);
    }

    let curve_mode = u32(round(u.levels_b_out_curve.z));
    return filmic_tonemap_curve(rgb, curve_mode);
}

fn apply_crop_overlay(
    base_rgb: vec3<f32>,
    ori_uv: vec2<f32>,
    crop: vec4<f32>,
    frame_px_size: vec2<f32>,
    grid_mode: u32,
) -> vec3<f32> {
    let local_u = (ori_uv.x - crop.x) / crop.z;
    let local_v = (ori_uv.y - crop.y) / crop.w;
    let in_crop = local_u >= 0.0 && local_u <= 1.0 && local_v >= 0.0 && local_v <= 1.0;

    if !in_crop {
        return base_rgb * 0.22;
    }

    var rgb = base_rgb;
    let crop_px = crop.zw * frame_px_size;
    let dx_edge = min(local_u, 1.0 - local_u) * crop_px.x;
    let dy_edge = min(local_v, 1.0 - local_v) * crop_px.y;

    // Composition guide grid lines inside crop frame
    if grid_mode == 1u || grid_mode == 2u {
        let g1 = select(0.381966, 1.0 / 3.0, grid_mode == 1u);
        let g2 = 1.0 - g1;
        let dv = min(abs(local_u - g1), abs(local_u - g2)) * crop_px.x;
        let dh = min(abs(local_v - g1), abs(local_v - g2)) * crop_px.y;
        if min(dv, dh) < 0.85 {
            rgb = mix(rgb, vec3<f32>(0.85, 0.87, 0.90), 0.42);
        }
    } else if grid_mode == 3u {
        let diag_len = max(length(crop_px), 1.0);
        let scale = (crop_px.x * crop_px.y) / diag_len;
        let d1 = abs(local_u - local_v) * scale;
        let d2 = abs(local_u + local_v - 1.0) * scale;
        if min(d1, d2) < 0.85 {
            rgb = mix(rgb, vec3<f32>(0.85, 0.87, 0.90), 0.42);
        }
    }

    // Crisp 1.5px crop border + 3.5px corner L-brackets
    let near_corner_x = dx_edge < 18.0;
    let near_corner_y = dy_edge < 18.0;
    if (dx_edge < 3.5 && near_corner_y) || (dy_edge < 3.5 && near_corner_x) {
        rgb = vec3<f32>(0.96, 0.97, 0.99);
    } else if dx_edge < 1.5 || dy_edge < 1.5 {
        rgb = mix(rgb, vec3<f32>(0.92, 0.94, 0.98), 0.88);
    }

    return rgb;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Pure achromatic darkroom matte surround matching OpenCode k.c.page (#121212 sRGB -> ~0.00605 linear)
    let matte_linear = vec3<f32>(0.00605, 0.00605, 0.00605);

    let slot_w = max(u.slot_rect.z, 1.0);
    let slot_h = max(u.slot_rect.w, 1.0);
    let img_w = max(u.dims.z, 1.0);
    let img_h = max(u.dims.w, 1.0);

    // 120Hz GPU Loupe magnification (zero cost when disabled)
    let loupe_active = u.viewport_extra.y > 0.5 && u.loupe_params.w > 1.0;
    let slot_px = in.slot_uv * vec2<f32>(slot_w, slot_h);
    let loupe_center_px = u.loupe_params.xy * vec2<f32>(slot_w, slot_h);
    let loupe_delta = slot_px - loupe_center_px;
    let loupe_dist = length(loupe_delta);
    let loupe_r = u.loupe_params.w;
    let in_loupe = loupe_active && loupe_dist < loupe_r;
    var eval_slot_uv = in.slot_uv;
    if in_loupe {
        let loupe_zoom = max(u.loupe_params.z, 1.25);
        let mag_px = loupe_center_px + loupe_delta / loupe_zoom;
        eval_slot_uv = mag_px / vec2<f32>(slot_w, slot_h);
    }

    let rot_q = u32(round(u.geo_xform.y)) % 4u;
    let swap_axes = (rot_q % 2u) == 1u;
    let ori_w = select(img_w, img_h, swap_axes);
    let ori_h = select(img_h, img_w, swap_axes);

    let cx = clamp(u.geo_crop.x, 0.0, 0.99);
    let cy = clamp(u.geo_crop.y, 0.0, 0.99);
    let crop = vec4<f32>(
        cx,
        cy,
        clamp(u.geo_crop.z, 0.01, max(1.0 - cx, 0.01)),
        clamp(u.geo_crop.w, 0.01, max(1.0 - cy, 0.01)),
    );
    let crop_mode = u.overlay_flags.z > 0.5;
    let frame_w = select(max(ori_w * crop.z, 1.0), ori_w, crop_mode);
    let frame_h = select(max(ori_h * crop.w, 1.0), ori_h, crop_mode);

    // Compute aspect-preserving fit inside slot with subtle 28px breathing margin at zoom 1.0
    let pad = 28.0;
    let avail_w = max(slot_w - pad * 2.0, 1.0);
    let avail_h = max(slot_h - pad * 2.0, 1.0);
    let fit_scale = min(avail_w / frame_w, avail_h / frame_h);
    let fitted_w = frame_w * fit_scale;
    let fitted_h = frame_h * fit_scale;

    let zoom = max(u.camera.x, 0.05);
    let pan_px = u.camera.yz * vec2<f32>(slot_w, slot_h) * 0.5;
    let rot = u.camera.w;
    let cs = cos(-rot);
    let sn = sin(-rot);

    // Position relative to zoomed/panned active frame center in slot device pixels
    let p_slot = (eval_slot_uv - vec2<f32>(0.5)) * vec2<f32>(slot_w, slot_h) - pan_px;
    let p_rot = vec2<f32>(p_slot.x * cs - p_slot.y * sn, p_slot.x * sn + p_slot.y * cs);
    let frame_px_size = vec2<f32>(fitted_w, fitted_h) * zoom;
    let frame_uv = p_rot / frame_px_size + vec2<f32>(0.5);

    var out_linear = matte_linear;

    let in_frame = frame_uv.x >= 0.0 && frame_uv.x <= 1.0 && frame_uv.y >= 0.0 && frame_uv.y <= 1.0;
    if in_frame {
        let ori_uv = select(crop.xy + frame_uv * crop.zw, frame_uv, crop_mode);
        let raw_img_uv = oriented_to_source_uv(ori_uv, ori_w, ori_h);
        let raw_in_source = raw_img_uv.x >= 0.0 && raw_img_uv.x <= 1.0 && raw_img_uv.y >= 0.0 && raw_img_uv.y <= 1.0;
        let img_uv = apply_lens_distortion_uv(raw_img_uv, u.lens.x, img_w, img_h);
        let in_source = img_uv.x >= 0.0 && img_uv.x <= 1.0 && img_uv.y >= 0.0 && img_uv.y <= 1.0;

        let show_original = u.flags.z > 0.5;
        let split_enabled = u.flags.x > 0.5;
        let split_pos = u.flags.y;
        let is_before = show_original || (split_enabled && eval_slot_uv.x < split_pos);

        if (is_before && raw_in_source) || (!is_before && in_source) {
            let vig_uv = (ori_uv - crop.xy) / crop.zw;
            var src = select(sample_photo_lens(img_uv), sample_photo(raw_img_uv), is_before);

            if is_before {
                out_linear = filmic_tonemap(src);
            } else {
                src = apply_heal_spots(src, img_uv);
                out_linear = apply_develop(
                    src,
                    img_uv,
                    vig_uv,
                    ori_uv,
                    vec2<f32>(ori_w, ori_h),
                    vec2<f32>(max(ori_w * crop.z, 1.0), max(ori_h * crop.w, 1.0)),
                );

                // Phase 4 Mask Overlay Display (1 = Red Rubylith, 2 = Grayscale mask)
                let overlay_mode = i32(round(u.mask_overlay.x));
                let active_li = i32(round(u.mask_overlay.y));
                let l_count = i32(round(u.mask_overlay.z));
                if (overlay_mode == 1 || overlay_mode == 2) && active_li >= 0 && active_li < l_count {
                    let luma_tone = clamp(sqrt(max(luma709(out_linear), 0.0)), 0.0, 1.0);
                    let m_vis = eval_layer_mask_raw(u32(active_li), img_uv, luma_tone);
                    if overlay_mode == 1 {
                        out_linear = mix(out_linear, vec3<f32>(0.85, 0.04, 0.04), m_vis * 0.52);
                    } else {
                        let g = pow(clamp(m_vis, 0.0, 1.0), 2.2);
                        out_linear = vec3<f32>(g, g, g);
                    }
                }
            }

            // Exposure & Shadow Clipping Warning overlay (red >= 0.992, blue <= 0.004 in sRGB)
            if u.overlay_flags.x > 0.5 {
                let s = linear_to_srgb(out_linear);
                if max(s.r, max(s.g, s.b)) >= 0.992 {
                    out_linear = mix(out_linear, vec3<f32>(0.88, 0.025, 0.025), 0.78);
                } else if s.r <= 0.004 && s.g <= 0.004 && s.b <= 0.004 {
                    out_linear = mix(out_linear, vec3<f32>(0.03, 0.16, 0.88), 0.78);
                }
            }

            // Focus Mask overlay (fast 4-tap Sobel/Laplacian acutance — zero cost when off)
            if u.overlay_flags.y > 0.5 {
                let texel = vec2<f32>(1.0 / img_w, 1.0 / img_h);
                let l_c = sqrt(max(luma709(src), 0.0));
                let l_l = sqrt(max(luma709(sample_photo(img_uv - vec2<f32>(texel.x, 0.0))), 0.0));
                let l_r = sqrt(max(luma709(sample_photo(img_uv + vec2<f32>(texel.x, 0.0))), 0.0));
                let l_u = sqrt(max(luma709(sample_photo(img_uv - vec2<f32>(0.0, texel.y))), 0.0));
                let l_d = sqrt(max(luma709(sample_photo(img_uv + vec2<f32>(0.0, texel.y))), 0.0));
                let gx = l_r - l_l;
                let gy = l_d - l_u;
                let lap = abs(4.0 * l_c - (l_l + l_r + l_u + l_d));
                let acutance = sqrt(gx * gx + gy * gy) + lap * 0.5;
                let focus_alpha = smoothstep(0.065, 0.16, acutance);
                if focus_alpha > 0.01 {
                    out_linear = mix(out_linear, vec3<f32>(0.05, 0.78, 0.18), focus_alpha * 0.82);
                }
            }

            // Crisp 1.5px Before/After divider line
            if split_enabled && !show_original {
                let dist_px = abs(eval_slot_uv.x - split_pos) * slot_w;
                if dist_px < 1.25 {
                    out_linear = vec3<f32>(0.92, 0.94, 0.98);
                }
            }
        }

        if crop_mode {
            let grid_mode = u32(round(u.overlay_flags.w));
            out_linear = apply_crop_overlay(out_linear, ori_uv, crop, frame_px_size, grid_mode);
        }
    } else {
        // Subtle 1px studio frame hairline around the photograph
        let half_ext = frame_px_size * 0.5;
        let d = abs(p_rot) - half_ext;
        let edge_dist = max(d.x, d.y);
        if edge_dist >= 0.0 && edge_dist < 1.5 {
            out_linear = vec3<f32>(0.035, 0.037, 0.042);
        }
    }

    // 120Hz GPU Loupe chrome: subtle drop shadow, 1.5px neutral rim & precision center crosshair dot
    if loupe_active && loupe_dist <= loupe_r + 3.5 {
        let rim_dist = abs(loupe_dist - loupe_r);
        if loupe_dist > loupe_r + 1.0 {
            let sh_t = clamp((loupe_dist - (loupe_r + 1.0)) / 2.5, 0.0, 1.0);
            out_linear *= mix(0.45, 1.0, sh_t);
        } else if rim_dist <= 1.25 {
            let rim_a = 1.0 - smoothstep(0.65, 1.25, rim_dist);
            out_linear = mix(out_linear, vec3<f32>(0.74, 0.76, 0.80), rim_a * 0.92);
        } else if loupe_dist > loupe_r - 2.25 && loupe_dist < loupe_r - 1.25 {
            out_linear = mix(out_linear, vec3<f32>(0.02, 0.02, 0.025), 0.55);
        } else if loupe_dist < 3.0 {
            if loupe_dist < 1.5 {
                out_linear = mix(out_linear, vec3<f32>(0.90, 0.92, 0.96), 0.88);
            } else {
                out_linear = mix(out_linear, vec3<f32>(0.02, 0.02, 0.025), 0.65);
            }
        }
    }

    if u.flags.w > 0.5 {
        return vec4<f32>(linear_to_srgb(out_linear), 1.0);
    }
    return vec4<f32>(clamp(out_linear, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
