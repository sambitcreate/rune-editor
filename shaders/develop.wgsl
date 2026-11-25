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
}

@group(0) @binding(0)
var<uniform> u: DevelopUniforms;

@group(0) @binding(1)
var photo_tex: texture_2d<f32>;

@group(0) @binding(2)
var photo_sampler: sampler;

@group(0) @binding(3)
var curve_lut_tex: texture_2d<f32>;

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

fn sample_curve_lut(x: f32) -> vec4<f32> {
    let uv_x = (0.5 + clamp(x, 0.0, 1.0) * 255.0) / 256.0;
    return textureSampleLevel(curve_lut_tex, photo_sampler, vec2<f32>(uv_x, 0.5), 0.0);
}

fn apply_levels(rgb: vec3<f32>) -> vec3<f32> {
    if u.levels.w <= 0.5 {
        return rgb;
    }
    let inv_gamma = 1.0 / 2.2;
    let in_black = u.levels.x;
    let in_mid = clamp(u.levels.y, -1.0, 1.0);
    let in_white = u.levels.z;
    let out_black = u.atmosphere.z;
    let out_white = u.atmosphere.w;

    let span = max(in_white - in_black, 0.01);
    let gamma = exp2(-in_mid * 1.5);
    let hdr_keep = clamp((out_white - 0.95) * 20.0, 0.0, 1.0);
    let out_span = out_white - out_black;

    let p = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(inv_gamma));
    let norm = max((p - vec3<f32>(in_black)) / span, vec3<f32>(0.0));
    let base = pow(min(norm, vec3<f32>(1.0)), vec3<f32>(gamma));
    let hdr = max(norm - vec3<f32>(1.0), vec3<f32>(0.0)) * hdr_keep;
    let mapped = vec3<f32>(out_black) + base * out_span + hdr;
    return pow(max(mapped, vec3<f32>(0.0)), vec3<f32>(2.2));
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
    let xy = select(w.xy, w.xy / len, len > 1.0);
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

    let activity = dot(abs(sw), vec3<f32>(1.0))
        + dot(abs(mw), vec3<f32>(1.0))
        + dot(abs(hw), vec3<f32>(1.0));
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

    let color_shift = ts * w_s + tm * w_m + th * w_h;
    let luma_shift = sw.z * w_s + mw.z * w_m + hw.z * w_h;
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
    return vec3<f32>(y_out);
}

// Fast single-pass spatial Detail (Noise Reduction, Clarity, Structure, Unsharp Mask Sharpening).
// Completely bypassed when all four controls are inactive (zero texture fetch overhead).
fn apply_spatial_detail(center_rgb: vec3<f32>, img_uv: vec2<f32>) -> vec3<f32> {
    let clarity = u.detail.x;
    let sharpen = u.detail.y;
    let nr = u.detail.z;
    let structure = u.detail.w;

    if abs(clarity) <= 0.001 && sharpen <= 0.001 && nr <= 0.001 && abs(structure) <= 0.001 {
        return center_rgb;
    }

    let texel = vec2<f32>(1.0 / max(u.dims.z, 1.0), 1.0 / max(u.dims.w, 1.0));
    var rgb = max(center_rgb, vec3<f32>(0.0));

    // Tight 4-tap cardinal samples (fetched only when NR, Structure, or Sharpening is active)
    var n_up = vec3<f32>(0.0);
    var n_dn = vec3<f32>(0.0);
    var n_lt = vec3<f32>(0.0);
    var n_rt = vec3<f32>(0.0);
    if nr > 0.001 || abs(structure) > 0.001 || sharpen > 0.001 {
        n_up = max(sample_photo(img_uv + vec2<f32>(0.0, -texel.y)), vec3<f32>(0.0));
        n_dn = max(sample_photo(img_uv + vec2<f32>(0.0, texel.y)), vec3<f32>(0.0));
        n_lt = max(sample_photo(img_uv + vec2<f32>(-texel.x, 0.0)), vec3<f32>(0.0));
        n_rt = max(sample_photo(img_uv + vec2<f32>(texel.x, 0.0)), vec3<f32>(0.0));
    }

    // 1. Edge-aware bilateral Chroma + Luma Noise Reduction
    if nr > 0.001 {
        let diag = texel * 1.25;
        let d0 = max(sample_photo(img_uv + vec2<f32>(-diag.x, -diag.y)), vec3<f32>(0.0));
        let d1 = max(sample_photo(img_uv + vec2<f32>(diag.x, -diag.y)), vec3<f32>(0.0));
        let d2 = max(sample_photo(img_uv + vec2<f32>(-diag.x, diag.y)), vec3<f32>(0.0));
        let d3 = max(sample_photo(img_uv + vec2<f32>(diag.x, diag.y)), vec3<f32>(0.0));

        let c_p = sqrt(max(luma709(rgb), 0.0));
        let sigma = 0.04 + nr * 0.14;
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
        let y_out = mix(y_c, y_d, nr * 0.42);
        let chroma_out = mix(chroma_c, chroma_d, nr * 0.85);
        rgb = max(vec3<f32>(y_out) + chroma_out, vec3<f32>(0.0));
    }

    // 2. Spatial Clarity (broad-radius midtone local contrast)
    if abs(clarity) > 0.001 {
        let r1 = texel * 3.0;
        let r2 = texel * 7.0;
        let b1 = sample_photo(img_uv + vec2<f32>(r1.x, 0.0))
            + sample_photo(img_uv + vec2<f32>(-r1.x, 0.0))
            + sample_photo(img_uv + vec2<f32>(0.0, r1.y))
            + sample_photo(img_uv + vec2<f32>(0.0, -r1.y));
        let b2 = sample_photo(img_uv + vec2<f32>(r2.x, r2.y))
            + sample_photo(img_uv + vec2<f32>(-r2.x, r2.y))
            + sample_photo(img_uv + vec2<f32>(r2.x, -r2.y))
            + sample_photo(img_uv + vec2<f32>(-r2.x, -r2.y));

        let wide_avg = (rgb * 2.0 + b1 + b2 * 0.75) / 9.0;
        let y_c = max(luma709(rgb), 1e-4);
        let y_b = max(luma709(max(wide_avg, vec3<f32>(0.0))), 1e-4);
        let t = clamp(y_c / (y_c + 0.18), 0.0, 1.0);
        let mid_mask = 4.0 * t * (1.0 - t);
        let ratio = clamp(y_c / y_b, 0.25, 4.0);
        let gain = pow(ratio, clarity * 0.65 * mid_mask);
        rgb = max(rgb * gain, vec3<f32>(0.0));
    }

    // 3. Spatial Structure (fine-radius micro-contrast texture enhancement)
    if abs(structure) > 0.001 {
        let rs = texel * 1.65;
        let s_diag = max(sample_photo(img_uv + vec2<f32>(rs.x, rs.y)), vec3<f32>(0.0))
            + max(sample_photo(img_uv + vec2<f32>(-rs.x, rs.y)), vec3<f32>(0.0))
            + max(sample_photo(img_uv + vec2<f32>(rs.x, -rs.y)), vec3<f32>(0.0))
            + max(sample_photo(img_uv + vec2<f32>(-rs.x, -rs.y)), vec3<f32>(0.0));
        let fine_avg = (rgb * 2.0 + (n_up + n_dn + n_lt + n_rt) + s_diag * 0.75) / 9.0;
        let y_c = max(luma709(rgb), 1e-4);
        let y_f = max(luma709(max(fine_avg, vec3<f32>(0.0))), 1e-4);
        let shadow_guard = smoothstep(0.004, 0.05, y_c);
        let ratio = clamp(y_c / y_f, 0.35, 2.8);
        let gain = pow(ratio, clamp(structure, -1.0, 1.0) * 0.72 * shadow_guard);
        rgb = max(rgb * gain, vec3<f32>(0.0));
    }

    // 4. Unsharp Mask Sharpening (edge-thresholded high-pass luminance detail)
    if sharpen > 0.001 {
        let blur_tight = (n_up + n_dn + n_lt + n_rt) * 0.25;
        let y_c = luma709(rgb);
        let y_blur = luma709(blur_tight);
        let hp = y_c - y_blur;
        let edge_gate = smoothstep(0.002, 0.012, abs(hp));
        let rel_hp = clamp(hp / max(y_c, 0.02), -0.55, 0.55);
        let sharp_gain = max(1.0 + rel_hp * sharpen * 1.4 * edge_gate, 0.0);
        rgb *= sharp_gain;
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

fn filmic_tonemap(rgb: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        filmic_channel(rgb.r),
        filmic_channel(rgb.g),
        filmic_channel(rgb.b),
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

fn oriented_to_source_uv(ori_uv: vec2<f32>, ori_w: f32, ori_h: f32) -> vec2<f32> {
    let theta = u.geo_xform.x;
    var uv = ori_uv;
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

fn apply_develop(src_rgb: vec3<f32>, img_uv: vec2<f32>, vig_uv: vec2<f32>) -> vec3<f32> {
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

    // 9. Vibrance & Saturation
    let y = luma709(rgb);
    let max_c = max(rgb.r, max(rgb.g, rgb.b));
    let min_c = min(rgb.r, min(rgb.g, rgb.b));
    let est_sat = select(0.0, clamp((max_c - min_c) / max_c, 0.0, 1.0), max_c > 1e-5);
    let sat_scale = max(1.0 + saturation + vibrance * (1.0 - est_sat) * 0.75, 0.0);
    rgb = max(vec3<f32>(y) + (rgb - vec3<f32>(y)) * sat_scale, vec3<f32>(0.0));

    // 10. 6-Channel Black & White Mixer
    rgb = apply_bw_mixer(rgb);

    // 11. Crop-Aware Optical Vignette
    let centered = (vig_uv - vec2<f32>(0.5)) * 2.0;
    let r2 = dot(centered, centered);
    let vig_gain = exp2(vignette * 1.15 * smoothstep(0.18, 1.65, r2));
    rgb *= vig_gain;

    // 12. Subtle Photographic Film Grain (stronger in midtones)
    if grain > 0.001 {
        let n = (hash12(floor(img_uv * u.dims.zw)) - 0.5) * 2.0;
        let mid_mask = 1.0 - abs(2.0 * t - 1.0);
        rgb = max(rgb + vec3<f32>(n * grain * 0.065 * (0.35 + 0.65 * mid_mask)), vec3<f32>(0.0));
    }

    return filmic_tonemap(rgb);
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

    let rot_q = u32(round(u.geo_xform.y)) % 4u;
    let swap_axes = (rot_q % 2u) == 1u;
    let ori_w = select(img_w, img_h, swap_axes);
    let ori_h = select(img_h, img_w, swap_axes);

    let cx = clamp(u.geo_crop.x, 0.0, 0.99);
    let cy = clamp(u.geo_crop.y, 0.0, 0.99);
    let crop = vec4<f32>(
        cx,
        cy,
        clamp(u.geo_crop.z, 0.01, 1.0 - cx),
        clamp(u.geo_crop.w, 0.01, 1.0 - cy),
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
    let p_slot = (in.slot_uv - vec2<f32>(0.5)) * vec2<f32>(slot_w, slot_h) - pan_px;
    let p_rot = vec2<f32>(p_slot.x * cs - p_slot.y * sn, p_slot.x * sn + p_slot.y * cs);
    let frame_px_size = vec2<f32>(fitted_w, fitted_h) * zoom;
    let frame_uv = p_rot / frame_px_size + vec2<f32>(0.5);

    var out_linear = matte_linear;

    let in_frame = frame_uv.x >= 0.0 && frame_uv.x <= 1.0 && frame_uv.y >= 0.0 && frame_uv.y <= 1.0;
    if in_frame {
        let ori_uv = select(crop.xy + frame_uv * crop.zw, frame_uv, crop_mode);
        let img_uv = oriented_to_source_uv(ori_uv, ori_w, ori_h);
        let in_source = img_uv.x >= 0.0 && img_uv.x <= 1.0 && img_uv.y >= 0.0 && img_uv.y <= 1.0;

        if in_source {
            let src = sample_photo(img_uv);
            let show_original = u.flags.z > 0.5;
            let split_enabled = u.flags.x > 0.5;
            let split_pos = u.flags.y;
            let vig_uv = (ori_uv - crop.xy) / crop.zw;

            if show_original || (split_enabled && in.slot_uv.x < split_pos) {
                out_linear = filmic_tonemap(src);
            } else {
                out_linear = apply_develop(src, img_uv, vig_uv);
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
                let dist_px = abs(in.slot_uv.x - split_pos) * slot_w;
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

    if u.flags.w > 0.5 {
        return vec4<f32>(linear_to_srgb(out_linear), 1.0);
    }
    return vec4<f32>(clamp(out_linear, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
