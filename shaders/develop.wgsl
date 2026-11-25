// Single-pass fused scene-referred linear photo develop shader.
// Runs directly on Rune's native objc2-metal backend (macOS) as well as DX12/Vulkan (wgpu).

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
}

@group(0) @binding(0)
var<uniform> u: DevelopUniforms;

@group(0) @binding(1)
var photo_tex: texture_2d<f32>;

@group(0) @binding(2)
var photo_sampler: sampler;

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

fn apply_develop(src_rgb: vec3<f32>, img_uv: vec2<f32>) -> vec3<f32> {
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

    // 1. White balance + Exposure (linear stops)
    let wb = vec3<f32>(
        exp2(temperature * 0.45 + tint * 0.22),
        exp2(-tint * 0.35),
        exp2(-temperature * 0.45 + tint * 0.22),
    );
    var rgb = max(src_rgb * wb * exp2(exposure), vec3<f32>(0.0));

    // 2. Tonal regions (Shadows / Highlights / Whites / Blacks)
    let luma = max(dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-5);
    let t = clamp(luma / (luma + 0.18), 0.0, 1.0);
    let shadow_w = (1.0 - t) * (1.0 - t);
    let highlight_w = t * t;
    let black_w = shadow_w * shadow_w;
    let white_w = highlight_w * highlight_w;

    let tone_stops = shadows * 1.15 * shadow_w
        + highlights * 1.15 * highlight_w
        + blacks * 0.90 * black_w
        + whites * 0.75 * white_w;
    rgb *= exp2(tone_stops);

    // 3. Contrast around 18% middle gray pivot
    let pivot = 0.18;
    let c_pow = exp2(contrast * 0.85);
    rgb = pivot * pow(max(rgb / pivot, vec3<f32>(0.0)), vec3<f32>(c_pow));

    // 4. Vibrance & Saturation
    let y = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let max_c = max(rgb.r, max(rgb.g, rgb.b));
    let min_c = min(rgb.r, min(rgb.g, rgb.b));
    let est_sat = select(0.0, clamp((max_c - min_c) / max_c, 0.0, 1.0), max_c > 1e-5);
    let sat_scale = max(1.0 + saturation + vibrance * (1.0 - est_sat) * 0.75, 0.0);
    rgb = max(vec3<f32>(y) + (rgb - vec3<f32>(y)) * sat_scale, vec3<f32>(0.0));

    // 5. Optical Vignette
    let centered = (img_uv - vec2<f32>(0.5)) * 2.0;
    let r2 = dot(centered, centered);
    let vig_gain = exp2(vignette * 1.15 * smoothstep(0.18, 1.65, r2));
    rgb *= vig_gain;

    // 6. Subtle Photographic Film Grain (stronger in midtones)
    if grain > 0.001 {
        let n = (hash12(floor(img_uv * u.dims.zw)) - 0.5) * 2.0;
        let mid_mask = 1.0 - abs(2.0 * t - 1.0);
        rgb = max(rgb + vec3<f32>(n * grain * 0.065 * (0.35 + 0.65 * mid_mask)), vec3<f32>(0.0));
    }

    return filmic_tonemap(rgb);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Neutral calibrated darkroom matte surround (~11% sRGB -> ~0.011 linear)
    let matte_linear = vec3<f32>(0.011, 0.0115, 0.0125);

    let slot_w = max(u.slot_rect.z, 1.0);
    let slot_h = max(u.slot_rect.w, 1.0);
    let img_w = max(u.dims.z, 1.0);
    let img_h = max(u.dims.w, 1.0);

    // Compute aspect-preserving fit inside slot with subtle 24px breathing margin at zoom 1.0
    let pad = 28.0;
    let avail_w = max(slot_w - pad * 2.0, 1.0);
    let avail_h = max(slot_h - pad * 2.0, 1.0);
    let fit_scale = min(avail_w / img_w, avail_h / img_h);
    let fitted_w = img_w * fit_scale;
    let fitted_h = img_h * fit_scale;

    let zoom = max(u.camera.x, 0.05);
    let pan_px = u.camera.yz * vec2<f32>(slot_w, slot_h) * 0.5;
    let rot = u.camera.w;
    let cs = cos(-rot);
    let sn = sin(-rot);

    // Position relative to zoomed/panned image center in slot device pixels
    let p_slot = (in.slot_uv - vec2<f32>(0.5)) * vec2<f32>(slot_w, slot_h) - pan_px;
    let p_rot = vec2<f32>(p_slot.x * cs - p_slot.y * sn, p_slot.x * sn + p_slot.y * cs);
    let img_uv = p_rot / (vec2<f32>(fitted_w, fitted_h) * zoom) + vec2<f32>(0.5);

    var out_linear = matte_linear;

    let inside = img_uv.x >= 0.0 && img_uv.x <= 1.0 && img_uv.y >= 0.0 && img_uv.y <= 1.0;
    if inside {
        let src = textureSample(photo_tex, photo_sampler, img_uv).rgb;
        let show_original = u.flags.z > 0.5;
        let split_enabled = u.flags.x > 0.5;
        let split_pos = u.flags.y;

        if show_original || (split_enabled && in.slot_uv.x < split_pos) {
            out_linear = filmic_tonemap(src);
        } else {
            out_linear = apply_develop(src, img_uv);
        }

        // Crisp 1.5px Before/After divider line
        if split_enabled && !show_original {
            let dist_px = abs(in.slot_uv.x - split_pos) * slot_w;
            if dist_px < 1.25 {
                out_linear = vec3<f32>(0.92, 0.94, 0.98);
            }
        }
    } else {
        // Subtle 1px studio frame hairline around the photograph
        let half_ext = vec2<f32>(fitted_w, fitted_h) * zoom * 0.5;
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
